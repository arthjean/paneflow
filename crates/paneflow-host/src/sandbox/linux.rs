use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use super::{ExecArgs, SandboxPlan, Wrapped};

const BWRAP: &str = "bwrap";

const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;
const LANDLOCK_ACCESS_FS_RESOLVE_UNIX: u64 = 1 << 16;
const LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;

pub const LANDLOCK_SCOPE_ABI: i64 = 6;
pub const LANDLOCK_RESOLVE_UNIX_ABI: i64 = 9;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

pub fn landlock_abi() -> i64 {
    let abi = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<RulesetAttr>(),
            0_usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    abi.max(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LandlockScoping {
    pub abi: i64,
    pub abstract_unix: bool,
    pub pathname_unix: bool,
}

impl LandlockScoping {
    pub fn for_abi(abi: i64, pathname_wanted: bool) -> Self {
        Self {
            abi,
            abstract_unix: abi >= LANDLOCK_SCOPE_ABI,
            pathname_unix: pathname_wanted && abi >= LANDLOCK_RESOLVE_UNIX_ABI,
        }
    }

    pub fn describe(self) -> String {
        match (self.abstract_unix, self.pathname_unix) {
            (true, true) => format!(
                "Landlock ABI {} scopes abstract and pathname unix sockets",
                self.abi
            ),
            (true, false) if self.abi >= LANDLOCK_RESOLVE_UNIX_ABI => format!(
                "Landlock ABI {} scopes abstract unix sockets; the profile leaves pathname sockets to the mount namespace so the agent's own sandbox can run",
                self.abi
            ),
            (true, false) => format!(
                "Landlock ABI {} scopes abstract unix sockets; scoping pathname sockets needs ABI {LANDLOCK_RESOLVE_UNIX_ABI}",
                self.abi
            ),
            (false, _) if self.abi == 0 => format!(
                "Landlock is unavailable; scoping abstract unix sockets needs ABI {LANDLOCK_SCOPE_ABI}"
            ),
            (false, _) => format!(
                "Landlock ABI {} is too old; scoping abstract unix sockets needs ABI {LANDLOCK_SCOPE_ABI}",
                self.abi
            ),
        }
    }
}

pub fn describe(pathname_wanted: bool) -> String {
    format!(
        "bubblewrap; {}",
        LandlockScoping::for_abi(landlock_abi(), pathname_wanted).describe()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub hidden: Vec<PathBuf>,
    pub reexposed: Vec<(PathBuf, Entry)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Bind,
    Symlink(PathBuf),
}

fn is_paneflow_entry(name: &std::ffi::OsStr) -> bool {
    name.as_bytes().starts_with(b"paneflow")
}

pub const KEPT_ENTRY_PREFIXES: [&str; 2] = ["wayland-", "pipewire-"];

pub const KEPT_ENTRIES: [&str; 2] = ["pulse", "gnupg"];

fn is_kept_entry(name: &std::ffi::OsStr) -> bool {
    let name = name.as_bytes();
    KEPT_ENTRIES.iter().any(|kept| name == kept.as_bytes())
        || KEPT_ENTRY_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix.as_bytes()))
}

pub fn ssh_agent_from_process() -> Option<PathBuf> {
    std::env::var_os("SSH_AUTH_SOCK")
        .map(PathBuf::from)
        .filter(|socket| socket.is_absolute())
}

fn comes_back(plan: &SandboxPlan, entry: &Path, name: &std::ffi::OsStr) -> bool {
    if is_paneflow_entry(name) {
        return false;
    }
    is_kept_entry(name)
        || plan.cwd.starts_with(entry)
        || plan.home.starts_with(entry)
        || plan
            .ssh_agent
            .as_ref()
            .is_some_and(|socket| socket.starts_with(entry))
}

pub fn layout(plan: &SandboxPlan) -> Layout {
    let mut hidden: Vec<PathBuf> = Vec::new();
    for endpoint in &plan.shared_endpoints {
        let Some(parent) = endpoint.parent() else {
            continue;
        };
        let hide = parent.to_path_buf();
        if hide.parent().is_none() || hide.starts_with(&plan.pane_dir) {
            continue;
        }
        if !hidden.contains(&hide) {
            hidden.push(hide);
        }
    }
    let all = hidden.clone();
    hidden.retain(|dir| {
        !all.iter()
            .any(|other| other != dir && dir.starts_with(other))
    });
    let mut reexposed = Vec::new();
    for dir in &hidden {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut names: Vec<_> = entries.filter_map(Result::ok).collect();
        names.sort_by_key(std::fs::DirEntry::file_name);
        for entry in names {
            let name = entry.file_name();
            let path = entry.path();
            if !comes_back(plan, &path, &name) {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                if let Ok(target) = std::fs::read_link(&path) {
                    reexposed.push((path, Entry::Symlink(target)));
                }
            } else {
                reexposed.push((path, Entry::Bind));
            }
        }
    }
    Layout { hidden, reexposed }
}

fn strictly_under(path: &Path, base: &Path) -> bool {
    path != base && path.starts_with(base)
}

pub fn bwrap_args(plan: &SandboxPlan, helper: &Path, shell: &str, args: &[String]) -> Vec<String> {
    let layout = layout(plan);
    let text = |path: &Path| path.display().to_string();
    let mut out: Vec<String> = [
        "--die-with-parent",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-cgroup",
        "--bind",
        "/",
        "/",
        "--dev-bind",
        "/dev",
        "/dev",
        "--proc",
        "/proc",
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect();
    for dir in &layout.hidden {
        out.extend(["--tmpfs".into(), text(dir)]);
    }
    for (path, entry) in &layout.reexposed {
        match entry {
            Entry::Bind => out.extend(["--bind".into(), text(path), text(path)]),
            Entry::Symlink(target) => out.extend(["--symlink".into(), text(target), text(path)]),
        }
    }
    out.extend(["--ro-bind".into(), text(&plan.home), text(&plan.home)]);
    let panes = paneflow_home::panes_dir_in(&plan.home);
    out.extend(["--tmpfs".into(), text(&panes)]);
    out.extend(["--bind".into(), text(&plan.pane_dir), text(&plan.pane_dir)]);
    out.extend([
        "--bind".into(),
        text(&plan.session_data_dir),
        text(&plan.session_data_dir),
    ]);
    if strictly_under(&plan.cwd, &plan.home) && !plan.cwd.starts_with(&panes) {
        out.extend(["--bind".into(), text(&plan.cwd), text(&plan.cwd)]);
    }
    out.extend(["--chdir".into(), text(&plan.cwd), "--".into(), text(helper)]);
    let mut unix_allow = vec![plan.pane_dir.clone()];
    unix_allow.extend(layout.hidden.iter().cloned());
    if let Some(agent) = plan
        .ssh_agent
        .as_deref()
        .and_then(Path::parent)
        .filter(|dir| dir.parent().is_some())
    {
        unix_allow.push(agent.to_path_buf());
    }
    let exec = ExecArgs {
        scope_pathname_unix: plan.scope_pathname_unix,
        unix_allow,
        session: Some(plan.session.clone()),
        command: std::iter::once(OsString::from(shell))
            .chain(args.iter().map(OsString::from))
            .collect(),
    };
    out.extend(exec.to_args());
    out
}

pub fn wrap(plan: &SandboxPlan, shell: &str, args: &[String]) -> Result<Wrapped, String> {
    wrap_with(
        plan,
        which::which(BWRAP).map_err(|error| error.to_string()),
        shell,
        args,
    )
}

fn wrap_with(
    plan: &SandboxPlan,
    bwrap: Result<PathBuf, String>,
    shell: &str,
    args: &[String],
) -> Result<Wrapped, String> {
    let bwrap = bwrap.map_err(|error| {
        format!("the sandbox needs bwrap (bubblewrap), which cannot be found: {error}")
    })?;
    let helper = plan.helper.clone()?;
    preflight(&bwrap)?;
    std::fs::create_dir_all(&plan.session_data_dir).map_err(|error| {
        format!(
            "cannot prepare {} for the sandbox: {error}",
            plan.session_data_dir.display()
        )
    })?;
    Ok(Wrapped {
        program: bwrap.display().to_string(),
        args: bwrap_args(plan, &helper, shell, args),
        env: Vec::new(),
    })
}

fn preflight(bwrap: &Path) -> Result<(), String> {
    let probe = which::which("true").map_err(|error| {
        format!("the sandbox check needs the `true` command, which cannot be found: {error}")
    })?;
    let output = std::process::Command::new(bwrap)
        .args([
            "--die-with-parent",
            "--unshare-user",
            "--unshare-pid",
            "--unshare-cgroup",
            "--bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
        ])
        .arg(&probe)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("bwrap cannot run: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(format!(
        "bwrap refused to create the sandbox ({}): {}",
        output.status,
        if detail.is_empty() {
            "no error was printed"
        } else {
            detail.as_str()
        }
    ))
}

fn open_path(path: &Path) -> Option<libc::c_int> {
    let raw = CString::new(path.as_os_str().as_bytes()).ok()?;
    let fd = unsafe { libc::open(raw.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    (fd >= 0).then_some(fd)
}

pub fn restrict_self(scoping: LandlockScoping, unix_allow: &[PathBuf]) -> Result<(), String> {
    if !scoping.abstract_unix {
        return Ok(());
    }
    let attr = RulesetAttr {
        handled_access_fs: if scoping.pathname_unix {
            LANDLOCK_ACCESS_FS_RESOLVE_UNIX
        } else {
            0
        },
        handled_access_net: 0,
        scoped: LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET,
    };
    let ruleset = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            &attr as *const RulesetAttr,
            std::mem::size_of::<RulesetAttr>(),
            0_u32,
        )
    };
    if ruleset < 0 {
        return Err(format!(
            "landlock_create_ruleset failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let ruleset = libc::c_int::try_from(ruleset).map_err(|_| "the ruleset fd is out of range")?;
    let outcome = (|| {
        if scoping.pathname_unix {
            for path in unix_allow {
                let Some(parent) = open_path(path) else {
                    continue;
                };
                let rule = PathBeneathAttr {
                    allowed_access: LANDLOCK_ACCESS_FS_RESOLVE_UNIX,
                    parent_fd: parent,
                };
                let added = unsafe {
                    libc::syscall(
                        SYS_LANDLOCK_ADD_RULE,
                        ruleset,
                        LANDLOCK_RULE_PATH_BENEATH,
                        &rule as *const PathBeneathAttr,
                        0_u32,
                    )
                };
                let error = std::io::Error::last_os_error();
                unsafe { libc::close(parent) };
                if added < 0 {
                    return Err(format!(
                        "landlock_add_rule for {} failed: {error}",
                        path.display()
                    ));
                }
            }
        }
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(format!(
                "cannot set no_new_privs: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset, 0_u32) } < 0 {
            return Err(format!(
                "landlock_restrict_self failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    })();
    unsafe { libc::close(ruleset) };
    outcome
}

pub fn exec(args: ExecArgs) -> i32 {
    let scoping = LandlockScoping::for_abi(landlock_abi(), args.scope_pathname_unix);
    if let Err(error) = restrict_self(scoping, &args.unix_allow) {
        eprintln!("paneflow sandbox: {error}; the pane does not start without it");
        return 126;
    }
    let Some((program, rest)) = args.command.split_first() else {
        return 2;
    };
    let error =
        std::os::unix::process::CommandExt::exec(std::process::Command::new(program).args(rest));
    eprintln!(
        "paneflow sandbox: cannot start {}: {error}",
        Path::new(program).display()
    );
    127
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(root: &Path) -> SandboxPlan {
        let home = root.join("home");
        let runtime = root.join("run");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("paneflow-host-1.sock"), b"").unwrap();
        std::fs::write(runtime.join("bus"), b"").unwrap();
        std::fs::create_dir_all(runtime.join("systemd")).unwrap();
        std::fs::write(runtime.join("docker.sock"), b"").unwrap();
        std::fs::write(runtime.join("wayland-0"), b"").unwrap();
        std::os::unix::fs::symlink("wayland-0", runtime.join("wayland-1")).unwrap();
        std::fs::create_dir_all(runtime.join("keyring")).unwrap();
        std::fs::write(runtime.join("keyring").join("ssh"), b"").unwrap();
        std::fs::create_dir_all(runtime.join("work")).unwrap();
        std::fs::create_dir_all(runtime.join("paneflow-dev")).unwrap();
        SandboxPlan {
            helper: Ok(PathBuf::from("/opt/paneflow-host")),
            ssh_agent: None,
            ..SandboxPlan::new(
                &home,
                "s-1",
                &home.join("worktrees").join("w"),
                [
                    runtime.join("paneflow-host-1.sock"),
                    runtime.join("paneflow-dev").join("paneflow-dev.sock"),
                ],
            )
        }
    }

    #[test]
    fn the_runtime_directory_is_hidden_and_only_the_listed_entries_come_back() {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("run");
        let plan = plan(root.path());
        let layout = layout(&plan);
        assert_eq!(layout.hidden, vec![run.clone()]);
        assert_eq!(
            layout.reexposed,
            vec![
                (run.join("wayland-0"), Entry::Bind),
                (
                    run.join("wayland-1"),
                    Entry::Symlink(PathBuf::from("wayland-0"))
                ),
            ],
            "the session bus, the systemd sockets and a container daemon socket stay out"
        );
    }

    #[test]
    fn the_ssh_agent_and_a_working_directory_under_a_hidden_directory_come_back() {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("run");
        let plan = SandboxPlan {
            ssh_agent: Some(run.join("keyring").join("ssh")),
            cwd: run.join("work").join("repo"),
            ..plan(root.path())
        };
        let names: Vec<PathBuf> = layout(&plan)
            .reexposed
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert_eq!(
            names,
            vec![
                run.join("keyring"),
                run.join("wayland-0"),
                run.join("wayland-1"),
                run.join("work"),
            ]
        );
    }

    #[test]
    fn the_home_is_read_only_except_the_pane_its_data_and_a_worktree_cwd() {
        let root = tempfile::tempdir().unwrap();
        let plan = plan(root.path());
        let args = bwrap_args(
            &plan,
            Path::new("/opt/paneflow-host"),
            "/bin/sh",
            &["-l".into()],
        );
        let joined = args.join(" ");
        let home = plan.home.display().to_string();
        for expected in [
            "--die-with-parent --unshare-user --unshare-pid --unshare-cgroup".to_string(),
            format!("--ro-bind {home} {home}"),
            format!("--tmpfs {home}/panes"),
            format!("--bind {0} {0}", plan.pane_dir.display()),
            format!("--bind {0} {0}", plan.session_data_dir.display()),
            format!("--bind {0} {0}", plan.cwd.display()),
            format!(
                "-- /opt/paneflow-host sandbox-exec --unix-allow {}",
                plan.pane_dir.display()
            ),
            "--session s-1 -- /bin/sh -l".to_string(),
        ] {
            assert!(
                joined.contains(&expected),
                "missing {expected:?} in {joined}"
            );
        }
        let ro = args.iter().position(|arg| arg == "--ro-bind").unwrap();
        let pane = args
            .iter()
            .position(|arg| *arg == plan.pane_dir.display().to_string())
            .unwrap();
        assert!(
            ro < pane,
            "the pane directory must be bound over the read-only home"
        );
    }

    #[test]
    fn a_missing_or_refusing_bwrap_names_bwrap_and_its_error() {
        let root = tempfile::tempdir().unwrap();
        let plan = plan(root.path());
        let missing = wrap_with(&plan, Err("not found".into()), "/bin/sh", &[]).unwrap_err();
        assert!(missing.contains("bwrap (bubblewrap)"), "{missing}");
        let refusing = root.path().join("bwrap");
        std::fs::write(
            &refusing,
            "#!/bin/sh\necho 'bwrap: setting up uid map: Permission denied' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &refusing,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let refused = wrap_with(&plan, Ok(refusing), "/bin/sh", &[]).unwrap_err();
        assert!(
            refused.contains("bwrap refused to create the sandbox"),
            "{refused}"
        );
        assert!(
            refused.contains("setting up uid map: Permission denied"),
            "{refused}"
        );
    }

    #[test]
    fn landlock_scoping_follows_the_abi() {
        assert_eq!(
            LandlockScoping::for_abi(5, true),
            LandlockScoping {
                abi: 5,
                abstract_unix: false,
                pathname_unix: false
            }
        );
        assert!(LandlockScoping::for_abi(6, false).abstract_unix);
        assert!(!LandlockScoping::for_abi(8, true).pathname_unix);
        assert!(LandlockScoping::for_abi(9, true).pathname_unix);
        assert!(!LandlockScoping::for_abi(11, false).pathname_unix);
        assert!(
            LandlockScoping::for_abi(0, false)
                .describe()
                .contains("unavailable")
        );
        assert!(
            LandlockScoping::for_abi(7, true)
                .describe()
                .contains("needs ABI 9")
        );
        assert!(
            LandlockScoping::for_abi(5, false)
                .describe()
                .contains("needs ABI 6")
        );
        assert!(
            LandlockScoping::for_abi(11, false)
                .describe()
                .contains("agent's own sandbox")
        );
    }
}
