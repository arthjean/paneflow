use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, RevertToSelf, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, ImpersonateNamedPipeClient};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetProcessTimes, OpenProcess, OpenProcessToken,
    OpenThreadToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW,
};

use crate::{Membership, PaneLeader, PeerIdentity, PeerRead, PeerSource};

const MAX_ANCESTORS: usize = 1024;

pub(crate) fn creation_time(process: HANDLE) -> Option<u64> {
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
    (ok != 0)
        .then(|| (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

fn image_path(process: HANDLE) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32 * 1024];
    let mut length = buffer.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &mut length,
        )
    };
    (ok != 0).then(|| PathBuf::from(OsString::from_wide(&buffer[..length as usize])))
}

fn open_process(pid: u32, access: u32) -> io::Result<OwnedHandle> {
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn token_user(token: HANDLE) -> Result<Vec<u64>, String> {
    let mut needed = 0u32;
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(format!(
            "GetTokenInformation(TokenUser) failed: {}",
            io::Error::last_os_error()
        ));
    }
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(format!(
            "GetTokenInformation(TokenUser) failed: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(buffer)
}

fn user_sid(token_user: &[u64]) -> *mut core::ffi::c_void {
    unsafe { (*token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
}

fn client_token_user(pipe: HANDLE) -> Result<Vec<u64>, String> {
    if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
        return Err(format!(
            "ImpersonateNamedPipeClient failed: {}",
            io::Error::last_os_error()
        ));
    }
    let mut token: HANDLE = std::ptr::null_mut();
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) };
    let open_error = io::Error::last_os_error();
    if unsafe { RevertToSelf() } == 0 {
        if !token.is_null() {
            unsafe { CloseHandle(token) };
        }
        return Err(format!(
            "RevertToSelf failed after impersonating the client: {}",
            io::Error::last_os_error()
        ));
    }
    if opened == 0 {
        return Err(format!("OpenThreadToken failed: {open_error}"));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    token_user(token.as_raw_handle())
}

fn server_token_user() -> Result<Vec<u64>, String> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(format!(
            "OpenProcessToken failed: {}",
            io::Error::last_os_error()
        ));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    token_user(token.as_raw_handle())
}

fn same_user_as_server(pipe: HANDLE) -> Result<(), String> {
    let client = client_token_user(pipe)?;
    let server = server_token_user()?;
    if unsafe { EqualSid(user_sid(&client), user_sid(&server)) } == 0 {
        return Err("the pipe client's token belongs to another user than the server".into());
    }
    Ok(())
}

pub(crate) fn read(pipe: BorrowedHandle<'_>) -> PeerRead {
    let raw = pipe.as_raw_handle();
    let mut pid = 0u32;
    if unsafe { GetNamedPipeClientProcessId(raw, &mut pid) } == 0 {
        return PeerRead::unverified(format!(
            "GetNamedPipeClientProcessId failed: {}",
            io::Error::last_os_error()
        ));
    }
    let process = match open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE) {
        Ok(process) => process,
        Err(error) => return PeerRead::unverified(format!("OpenProcess({pid}) failed: {error}")),
    };
    let Some(started) = creation_time(process.as_raw_handle()) else {
        return PeerRead::unverified(format!(
            "GetProcessTimes({pid}) failed: {}",
            io::Error::last_os_error()
        ));
    };
    if let Err(reason) = same_user_as_server(raw) {
        return PeerRead::unverified(reason);
    }
    PeerRead::Identified(PeerIdentity {
        pid,
        started_at: started,
        executable: image_path(process.as_raw_handle()),
        source: PeerSource::PipeClient,
        _anchor: Some(process),
    })
}

#[derive(Debug)]
pub struct PaneJob(OwnedHandle);

impl PaneJob {
    pub fn contain(pid: u32) -> io::Result<Self> {
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        let limited = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if limited == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = open_process(pid, PROCESS_SET_QUOTA | PROCESS_TERMINATE)?;
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(job))
    }

    fn holds(&self, process: HANDLE) -> io::Result<bool> {
        let mut member = 0;
        if unsafe { IsProcessInJob(process, self.0.as_raw_handle(), &mut member) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(member != 0)
    }
}

fn parents() -> io::Result<HashMap<u32, u32>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut parents = HashMap::new();
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Ok(parents)
}

fn leader_at(leaders: &[PaneLeader], pid: u32, started_at: u64) -> Option<&PaneLeader> {
    leaders
        .iter()
        .find(|leader| leader.pid == pid && leader.started_at == started_at)
}

pub(crate) fn classify(pid: u32, started_at: u64, leaders: &[PaneLeader]) -> Membership {
    let process = match open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(process) => process,
        Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) => {
            return Membership::Stale;
        }
        Err(error) => {
            return Membership::Unverified {
                reason: format!("OpenProcess({pid}) failed: {error}"),
            };
        }
    };
    if creation_time(process.as_raw_handle()) != Some(started_at) {
        return Membership::Stale;
    }
    for leader in leaders {
        let Some(job) = leader.job.as_deref() else {
            continue;
        };
        match job.holds(process.as_raw_handle()) {
            Ok(true) => return Membership::pane(leader),
            Ok(false) => {}
            Err(error) => {
                return Membership::Unverified {
                    reason: format!("IsProcessInJob({pid}) failed: {error}"),
                };
            }
        }
    }
    let unjobbed: Vec<PaneLeader> = leaders
        .iter()
        .filter(|leader| leader.job.is_none())
        .cloned()
        .collect();
    if unjobbed.is_empty() {
        return Membership::NotInPane;
    }
    let parents = match parents() {
        Ok(parents) => parents,
        Err(error) => {
            return Membership::Unverified {
                reason: format!("the process snapshot failed: {error}"),
            };
        }
    };
    let mut current = (pid, started_at);
    for _ in 0..MAX_ANCESTORS {
        if let Some(leader) = leader_at(&unjobbed, current.0, current.1) {
            return Membership::pane(leader);
        }
        let Some(&parent) = parents.get(&current.0) else {
            return Membership::NotInPane;
        };
        if parent == 0 || parent == current.0 {
            return Membership::NotInPane;
        }
        match crate::process_start_time(parent) {
            Some(parent_started) if parent_started <= current.1 => {
                current = (parent, parent_started);
            }
            _ => return Membership::NotInPane,
        }
    }
    Membership::Unverified {
        reason: format!("the parent chain of {pid} is longer than {MAX_ANCESTORS} processes"),
    }
}
