use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{BOOL, CloseHandle, GENERIC_ALL, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, AclSizeInformation,
    AddAccessAllowedAce, CONTAINER_INHERIT_ACE, CreateRestrictedToken, DACL_SECURITY_INFORMATION,
    DISABLE_MAX_PRIVILEGE, EqualSid, GetAce, GetAclInformation, GetLengthSid, GetTokenInformation,
    INHERIT_ONLY_ACE, InitializeAcl, OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID,
    SID_AND_ATTRIBUTES, SUB_CONTAINERS_AND_OBJECTS_INHERIT, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DEFAULT_DACL, TOKEN_DUPLICATE, TOKEN_GROUPS,
    TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER, TokenDefaultDacl, TokenLogonSid, TokenUser,
    WRITE_RESTRICTED,
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

const SANDBOX_FILE_ACCESS: u32 =
    FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

const INHERITED_BY_FILES_AND_DIRECTORIES: u32 = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;

const ACCESS_ALLOWED_ACE_TYPE: u32 = 0;

const LOCAL_SYSTEM_SID: &str = "S-1-5-18";

const EVERYONE_SID: &str = "S-1-1-0";

const PANE_TEMP_DIR_NAME: &str = "tmp";

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

fn grants_sandbox_access(acl: *const ACL, sid: PSID) -> bool {
    if acl.is_null() {
        return true;
    }
    let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        GetAclInformation(
            acl,
            (&raw mut info).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return false;
    }
    (0..info.AceCount).any(|index| {
        let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
        if unsafe { GetAce(acl, index, &mut ace) } == 0 || ace.is_null() {
            return false;
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        let flags = u32::from(header.AceFlags);
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || flags & INHERIT_ONLY_ACE != 0
            || flags & INHERITED_BY_FILES_AND_DIRECTORIES != INHERITED_BY_FILES_AND_DIRECTORIES
        {
            return false;
        }
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        let mask = unsafe { (*allowed).Mask };
        let ace_sid = unsafe { (&raw mut (*allowed).SidStart).cast::<core::ffi::c_void>() };
        mask & SANDBOX_FILE_ACCESS == SANDBOX_FILE_ACCESS && unsafe { EqualSid(ace_sid, sid) } != 0
    })
}

pub fn grant_sandbox_access(dir: &Path, sid: &str) -> Result<bool, String> {
    let sid = LocalSid::parse(sid)?;
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
    if grants_sandbox_access(current, sid.0) {
        unsafe { LocalFree(descriptor) };
        return Ok(false);
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: SANDBOX_FILE_ACCESS,
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
            Ok(true)
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
    restricted_token(&plan.pane_sid)?;
    grant_sandbox_access(&plan.cwd, paneflow_home::SANDBOX_SHARED_SID)?;
    let temp = plan.session_data_dir.join(PANE_TEMP_DIR_NAME);
    std::fs::create_dir_all(&temp)
        .map_err(|error| format!("cannot prepare {} for the sandbox: {error}", temp.display()))?;
    grant_sandbox_access(&plan.session_data_dir, &plan.pane_sid)?;
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
        env: ["TEMP", "TMP"]
            .into_iter()
            .map(|key| (key.to_string(), temp.display().to_string()))
            .collect(),
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

struct TokenInformation(Vec<u64>);

impl TokenInformation {
    fn read(token: HANDLE, class: TOKEN_INFORMATION_CLASS, what: &str) -> Result<Self, String> {
        let mut needed = 0_u32;
        unsafe { GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(last_error(&format!(
                "cannot read {what} of the process token"
            )));
        }
        let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
        if unsafe {
            GetTokenInformation(
                token,
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        } == 0
        {
            return Err(last_error(&format!(
                "cannot read {what} of the process token"
            )));
        }
        Ok(Self(buffer))
    }

    fn user(token: HANDLE) -> Result<Self, String> {
        Self::read(token, TokenUser, "the user")
    }

    fn logon(token: HANDLE) -> Result<Self, String> {
        let logon = Self::read(token, TokenLogonSid, "the logon session")?;
        if unsafe { (*logon.0.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount } == 0 {
            return Err("the process token belongs to no logon session".into());
        }
        Ok(logon)
    }

    fn user_sid(&self) -> PSID {
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn first_group_sid(&self) -> PSID {
        unsafe { (*self.0.as_ptr().cast::<TOKEN_GROUPS>()).Groups[0].Sid }
    }
}

fn open_own_token(access: u32) -> Result<OwnedHandle, String> {
    let mut own: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut own) } == 0 {
        return Err(last_error("cannot open the process token"));
    }
    Ok(OwnedHandle(own))
}

pub fn current_user_sid() -> Result<String, String> {
    let own = open_own_token(TOKEN_QUERY)?;
    let user = TokenInformation::user(own.0)?;
    let mut text: *mut u16 = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(user.user_sid(), &mut text) } == 0 {
        return Err(last_error("cannot render the user SID"));
    }
    let length = (0..)
        .take_while(|&offset| unsafe { *text.add(offset) } != 0)
        .count();
    let rendered = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(rendered)
}

fn pane_default_dacl(user: PSID, pane: PSID) -> Result<Vec<u64>, String> {
    let system = LocalSid::parse(LOCAL_SYSTEM_SID)?;
    let sids = [user, system.0, pane];
    let size = std::mem::size_of::<ACL>()
        + sids
            .iter()
            .map(|sid| {
                std::mem::size_of::<ACCESS_ALLOWED_ACE>() - std::mem::size_of::<u32>()
                    + unsafe { GetLengthSid(*sid) } as usize
            })
            .sum::<usize>();
    let mut acl = vec![0_u64; size.div_ceil(8)];
    let pointer = acl.as_mut_ptr().cast::<ACL>();
    if unsafe { InitializeAcl(pointer, (acl.len() * 8) as u32, ACL_REVISION) } == 0 {
        return Err(last_error(
            "cannot build the default access list of the sandbox",
        ));
    }
    for sid in sids {
        if unsafe { AddAccessAllowedAce(pointer, ACL_REVISION, GENERIC_ALL, sid) } == 0 {
            return Err(last_error(
                "cannot build the default access list of the sandbox",
            ));
        }
    }
    Ok(acl)
}

fn restricted_token(pane_sid: &str) -> Result<OwnedHandle, String> {
    let own = open_own_token(
        TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
    )?;
    let pane = LocalSid::parse(pane_sid)?;
    let shared = LocalSid::parse(paneflow_home::SANDBOX_SHARED_SID)?;
    let user = TokenInformation::user(own.0)?;
    let logon = TokenInformation::logon(own.0)?;
    let everyone = LocalSid::parse(EVERYONE_SID)?;
    let restricting =
        [pane.0, shared.0, logon.first_group_sid(), everyone.0].map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid,
            Attributes: 0,
        });
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
    let token = OwnedHandle(token);
    let mut dacl = pane_default_dacl(user.user_sid(), pane.0)?;
    let default = TOKEN_DEFAULT_DACL {
        DefaultDacl: dacl.as_mut_ptr().cast(),
    };
    if unsafe {
        SetTokenInformation(
            token.0,
            TokenDefaultDacl,
            (&raw const default).cast(),
            std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32,
        )
    } == 0
    {
        return Err(last_error(
            "cannot set the default access list of the sandbox",
        ));
    }
    Ok(token)
}

fn run(args: &ExecArgs) -> Result<u32, String> {
    let session = args
        .session
        .as_deref()
        .ok_or("the sandbox helper needs --session")?;
    wait_until_contained()?;
    let token = restricted_token(&paneflow_home::pane_sandbox_sid(session))?;
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

    #[test]
    fn a_directory_that_already_grants_the_sandbox_is_left_as_it_is() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let shared = paneflow_home::SANDBOX_SHARED_SID;
        assert_eq!(grant_sandbox_access(root.path(), shared), Ok(true));
        assert_eq!(grant_sandbox_access(root.path(), shared), Ok(false));
        assert_eq!(
            grant_sandbox_access(&nested, shared),
            Ok(false),
            "a directory inheriting the grant needs no rewrite of its tree"
        );
    }

    #[test]
    fn a_token_with_an_invalid_pane_sid_is_refused_with_the_os_error() {
        let refused = restricted_token("S-1-not-a-sid").err().unwrap();
        assert!(refused.contains("S-1-not-a-sid"), "{refused}");
        assert!(
            refused.contains(&std::io::Error::from_raw_os_error(1337).to_string()),
            "{refused}"
        );
    }

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
