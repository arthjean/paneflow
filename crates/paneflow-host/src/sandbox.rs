use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub const SANDBOX_EXEC_SUBCOMMAND: &str = "sandbox-exec";

pub const HOST_EXECUTABLE_STEM: &str = "paneflow-host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPlan {
    pub home: PathBuf,
    pub session: String,
    pub pane_sid: String,
    pub pane_dir: PathBuf,
    pub session_data_dir: PathBuf,
    pub shared_endpoints: Vec<PathBuf>,
    pub cwd: PathBuf,
    pub helper: Result<PathBuf, String>,
    pub scope_pathname_unix: bool,
    pub ssh_agent: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapped {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl SandboxPlan {
    pub fn new(
        home: &Path,
        session: &str,
        cwd: &Path,
        shared_endpoints: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        let mut endpoints: Vec<PathBuf> = Vec::new();
        for endpoint in shared_endpoints {
            if endpoint.is_absolute() && !endpoints.contains(&endpoint) {
                endpoints.push(endpoint);
            }
        }
        Self {
            home: home.to_path_buf(),
            session: session.to_string(),
            pane_sid: paneflow_home::pane_sandbox_sid(session),
            pane_dir: paneflow_home::pane_dir_in(home, session),
            session_data_dir: paneflow_home::host_session_data_dir_in(home, session),
            shared_endpoints: endpoints,
            cwd: cwd.to_path_buf(),
            helper: host_executable(),
            scope_pathname_unix: false,
            ssh_agent: ssh_agent_socket(),
        }
    }

    pub fn wrap(&self, shell: &str, args: &[String]) -> Result<Wrapped, String> {
        #[cfg(target_os = "linux")]
        {
            linux::wrap(self, shell, args)
        }
        #[cfg(target_os = "macos")]
        {
            macos::wrap(self, shell, args)
        }
        #[cfg(windows)]
        {
            windows::wrap(self, shell, args)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = (shell, args);
            Err("Sandbox 'paneflow' is not available on this platform".into())
        }
    }

    pub fn with_pathname_unix_scope(mut self, scope: bool) -> Self {
        self.scope_pathname_unix = scope;
        self
    }

    pub fn describe(&self) -> String {
        #[cfg(target_os = "linux")]
        {
            linux::describe(self.scope_pathname_unix)
        }
        #[cfg(not(target_os = "linux"))]
        {
            String::from(platform_mechanism())
        }
    }
}

#[cfg(windows)]
pub fn current_user_sid() -> Result<String, String> {
    windows::current_user_sid()
}

pub fn landlock_abi() -> i64 {
    #[cfg(target_os = "linux")]
    {
        linux::landlock_abi()
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

fn ssh_agent_socket() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        linux::ssh_agent_from_process()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub const fn platform_mechanism() -> &'static str {
    if cfg!(target_os = "linux") {
        "bubblewrap"
    } else if cfg!(target_os = "macos") {
        "Seatbelt"
    } else if cfg!(windows) {
        "a write-restricted token"
    } else {
        "none"
    }
}

pub fn host_executable() -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("cannot locate the paneflow-host executable: {error}"))?;
    if current
        .file_stem()
        .is_some_and(|stem| stem == HOST_EXECUTABLE_STEM)
    {
        return Ok(current);
    }
    let name = format!("{HOST_EXECUTABLE_STEM}{}", std::env::consts::EXE_SUFFIX);
    let sibling = current
        .ancestors()
        .skip(1)
        .take(2)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file());
    sibling.ok_or_else(|| {
        format!(
            "the sandbox helper is the paneflow-host executable, and none sits beside {}",
            current.display()
        )
    })
}

pub fn shared_endpoints_for(
    home: &Path,
    host_endpoint: &Path,
    window: Option<&str>,
) -> Vec<PathBuf> {
    let mut endpoints = vec![host_endpoint.to_path_buf()];
    endpoints.extend(paneflow_home::serve_endpoint_beside_host_endpoint(
        host_endpoint,
    ));
    endpoints.push(paneflow_home::serve_endpoint_path(home));
    endpoints.push(paneflow_home::ipc_endpoint_path(home));
    if let Some(endpoint) =
        paneflow_home::ipc_endpoint_in(&paneflow_home::EndpointEnv::from_process())
    {
        endpoints.push(endpoint.path);
    }
    endpoints.extend(window.map(PathBuf::from).filter(|path| path.is_absolute()));
    endpoints
}

pub fn exec_main(args: Vec<std::ffi::OsString>) -> i32 {
    let parsed = match ExecArgs::parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("paneflow-host {SANDBOX_EXEC_SUBCOMMAND}: {message}");
            return 2;
        }
    };
    #[cfg(target_os = "linux")]
    {
        linux::exec(parsed)
    }
    #[cfg(windows)]
    {
        windows::exec(parsed)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = parsed;
        eprintln!("paneflow-host {SANDBOX_EXEC_SUBCOMMAND}: not used on this platform");
        2
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecArgs {
    pub scope_pathname_unix: bool,
    pub unix_allow: Vec<PathBuf>,
    pub session: Option<String>,
    pub command: Vec<std::ffi::OsString>,
}

impl ExecArgs {
    pub fn parse(args: Vec<std::ffi::OsString>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--unix-allow") => parsed.unix_allow.push(PathBuf::from(
                    args.next().ok_or("--unix-allow needs a path")?,
                )),
                Some("--scope-pathname-unix") => parsed.scope_pathname_unix = true,
                Some("--session") => {
                    let session = args
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .ok_or("--session needs a session id")?;
                    parsed.session = Some(session);
                }
                Some("--") => {
                    parsed.command = args.collect();
                    break;
                }
                _ => return Err(format!("unexpected argument {arg:?}")),
            }
        }
        if parsed.command.is_empty() {
            return Err("no command follows --".into());
        }
        Ok(parsed)
    }

    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![SANDBOX_EXEC_SUBCOMMAND.to_string()];
        if self.scope_pathname_unix {
            args.push("--scope-pathname-unix".into());
        }
        for path in &self.unix_allow {
            args.push("--unix-allow".into());
            args.push(path.display().to_string());
        }
        if let Some(session) = &self.session {
            args.push("--session".into());
            args.push(session.clone());
        }
        args.push("--".into());
        args.extend(
            self.command
                .iter()
                .map(|part| part.to_string_lossy().into_owned()),
        );
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exec_arguments_survive_a_round_trip() {
        let args = ExecArgs {
            scope_pathname_unix: true,
            unix_allow: vec![PathBuf::from("/a/b")],
            session: Some("s-1".into()),
            command: vec!["/bin/sh".into(), "-l".into()],
        };
        let rendered: Vec<std::ffi::OsString> =
            args.to_args().into_iter().skip(1).map(Into::into).collect();
        assert_eq!(ExecArgs::parse(rendered).unwrap(), args);
    }

    #[test]
    fn the_exec_arguments_need_a_command() {
        assert!(ExecArgs::parse(vec!["--".into()]).is_err());
        assert!(ExecArgs::parse(vec!["--bogus".into()]).is_err());
    }

    #[test]
    fn a_plan_keeps_each_shared_endpoint_once_and_only_absolute_ones() {
        let home = Path::new(if cfg!(windows) { r"C:\h" } else { "/h" });
        let shared = PathBuf::from(if cfg!(windows) {
            r"\\.\pipe\x"
        } else {
            "/run/x.sock"
        });
        let plan = SandboxPlan::new(
            home,
            "s-1",
            home,
            [
                shared.clone(),
                shared.clone(),
                PathBuf::from("relative.sock"),
            ],
        );
        assert_eq!(plan.shared_endpoints, vec![shared]);
        assert_eq!(plan.pane_dir, paneflow_home::pane_dir_in(home, "s-1"));
    }
}
