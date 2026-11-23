use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{BOOL, CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW,
    NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, CreateRestrictedToken, DACL_SECURITY_INFORMATION, DISABLE_MAX_PRIVILEGE,
    PSECURITY_DESCRIPTOR, PSID, SID_AND_ATTRIBUTES, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY, WRITE_RESTRICTED,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken,
    PROCESS_INFORMATION, STARTUPINFOW, WaitForSingleObject,
};

use super::{ExecArgs, SandboxPlan, Wrapped};

const JOB_WAIT: Duration = Duration::from_secs(5);

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

struct LocalSid(PSID);

impl LocalSid {
    fn parse(sid: &str) -> Result<Self, String> {
        let text = wide(std::ffi::OsStr::new(sid));
        let mut parsed: PSID = std::ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut parsed) } == 0 {
            return Err(format!(
                "cannot build the sandbox SID {sid}: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(parsed))
    }
}

impl Drop for LocalSid {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn last_error(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

pub fn grant_sandbox_access(dir: &Path) -> Result<(), String> {
    let sid = LocalSid::parse(paneflow_home::SANDBOX_SHARED_SID)?;
    let name = wide(dir.as_os_str());
    let mut current: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let read = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut current,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if read != 0 {
        return Err(format!(
            "cannot read the access list of {}: {}",
            dir.display(),
            std::io::Error::from_raw_os_error(read as i32)
        ));
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_GENERIC_READ
            | FILE_GENERIC_WRITE
            | FILE_GENERIC_EXECUTE
            | DELETE,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: sid.0.cast(),
        },
    };
    let mut updated: *mut ACL = std::ptr::null_mut();
    let merged = unsafe { SetEntriesInAclW(1, &entry, current, &mut updated) };
    let outcome = if merged != 0 {
        Err(format!(
            "cannot extend the access list of {}: {}",
            dir.display(),
            std::io::Error::from_raw_os_error(merged as i32)
        ))
    } else {
        let written = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                updated,
                std::ptr::null(),
            )
        };
        if written == 0 {
            Ok(())
        } else {
            Err(format!(
                "cannot let the sandbox write {}: {}",
                dir.display(),
                std::io::Error::from_raw_os_error(written as i32)
            ))
        }
    };
    unsafe {
        LocalFree(updated.cast());
        LocalFree(descriptor);
    }
    outcome
}

pub fn wrap(plan: &SandboxPlan, shell: &str, args: &[String]) -> Result<Wrapped, String> {
    let helper = plan.helper.clone()?;
    grant_sandbox_access(&plan.cwd)?;
    if let Some(temp) = std::env::var_os("TEMP")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
    {
        grant_sandbox_access(&temp)?;
    }
    std::fs::create_dir_all(&plan.session_data_dir).map_err(|error| {
        format!(
            "cannot prepare {} for the sandbox: {error}",
            plan.session_data_dir.display()
        )
    })?;
    grant_sandbox_access(&plan.session_data_dir)?;
    let exec = ExecArgs {
        scope_pathname_unix: false,
        unix_allow: Vec::new(),
        session: Some(plan.session.clone()),
        command: std::iter::once(OsString::from(shell))
            .chain(args.iter().map(OsString::from))
            .collect(),
    };
    Ok(Wrapped {
        program: helper.display().to_string(),
        args: exec.to_args(),
    })
}

fn quote_argument(argument: &std::ffi::OsStr, line: &mut Vec<u16>) {
    let units: Vec<u16> = argument.encode_wide().collect();
    let plain = !units.is_empty()
        && !units
            .iter()
            .any(|unit| matches!(*unit, 0x20 | 0x09 | 0x0a | 0x0b | 0x22));
    if plain {
        line.extend(units);
        return;
    }
    line.push(u16::from(b'"'));
    let mut backslashes = 0_usize;
    for unit in units {
        if unit == u16::from(b'\\') {
            backslashes += 1;
            continue;
        }
        let repeat = if unit == u16::from(b'"') {
            backslashes * 2 + 1
        } else {
            backslashes
        };
        line.extend(std::iter::repeat_n(u16::from(b'\\'), repeat));
        backslashes = 0;
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    line.push(u16::from(b'"'));
}

pub fn command_line(command: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    for (index, argument) in command.iter().enumerate() {
        if index > 0 {
            line.push(u16::from(b' '));
        }
        quote_argument(argument, &mut line);
    }
    line.push(0);
    line
}

unsafe extern "system" fn leave_console_events_to_the_shell(_event: u32) -> BOOL {
    1
}

fn wait_until_contained() -> Result<(), String> {
    let started = Instant::now();
    loop {
        let mut contained: BOOL = 0;
        if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut contained) } == 0
        {
            return Err(last_error("cannot read the pane job"));
        }
        if contained != 0 {
            return Ok(());
        }
        if started.elapsed() >= JOB_WAIT {
            return Err("the pane job never contained the sandbox helper".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn restricted_token(session: &str) -> Result<OwnedHandle, String> {
    let mut own: HANDLE = std::ptr::null_mut();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_QUERY,
            &mut own,
        )
    } == 0
    {
        return Err(last_error("cannot open the process token"));
    }
    let own = OwnedHandle(own);
    let pane = LocalSid::parse(&paneflow_home::pane_sandbox_sid(session))?;
    let shared = LocalSid::parse(paneflow_home::SANDBOX_SHARED_SID)?;
    let restricting = [
        SID_AND_ATTRIBUTES {
            Sid: pane.0,
            Attributes: 0,
        },
        SID_AND_ATTRIBUTES {
            Sid: shared.0,
            Attributes: 0,
        },
    ];
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe {
        CreateRestrictedToken(
            own.0,
            DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            restricting.len() as u32,
            restricting.as_ptr(),
            &mut token,
        )
    } == 0
    {
        return Err(last_error("cannot create the write-restricted token"));
    }
    Ok(OwnedHandle(token))
}

fn run(args: &ExecArgs) -> Result<u32, String> {
    let session = args
        .session
        .as_deref()
        .ok_or("the sandbox helper needs --session")?;
    wait_until_contained()?;
    let token = restricted_token(session)?;
    if unsafe { SetConsoleCtrlHandler(Some(leave_console_events_to_the_shell), 1) } == 0 {
        return Err(last_error("cannot leave console events to the shell"));
    }
    let mut line = command_line(&args.command);
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessAsUserW(
            token.0,
            std::ptr::null(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            0,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(last_error(&format!(
            "cannot start {} with the restricted token",
            Path::new(&args.command[0]).display()
        )));
    }
    let child = OwnedHandle(process.hProcess);
    drop(OwnedHandle(process.hThread));
    unsafe { WaitForSingleObject(child.0, INFINITE) };
    let mut code = 0_u32;
    if unsafe { GetExitCodeProcess(child.0, &mut code) } == 0 {
        return Err(last_error("cannot read the shell's exit code"));
    }
    Ok(code)
}

pub fn exec(args: ExecArgs) -> i32 {
    match run(&args) {
        Ok(code) => code as i32,
        Err(error) => {
            eprintln!("paneflow sandbox: {error}; the pane does not start without it");
            126
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(parts: &[&str]) -> String {
        let parts: Vec<OsString> = parts.iter().map(OsString::from).collect();
        let line = command_line(&parts);
        String::from_utf16(&line[..line.len() - 1]).unwrap()
    }

    #[test]
    fn a_command_line_quotes_only_what_needs_it() {
        assert_eq!(rendered(&["pwsh.exe", "-NoLogo"]), "pwsh.exe -NoLogo");
        assert_eq!(
            rendered(&[r"C:\Program Files\x.exe", ""]),
            r#""C:\Program Files\x.exe" """#
        );
        assert_eq!(rendered(&[r#"a"b"#]), r#""a\"b""#);
        assert_eq!(rendered(&[r"a b\"]), r#""a b\\""#);
    }
}
