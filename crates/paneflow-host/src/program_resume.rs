use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use paneflow_peer::LineageReader as _;
use paneflow_program_resume::{
    Action, Admission, CwdSource, IgnoreReason, Outcome, ProcessStamp, RecordCwd, RecordStore,
    Report, SandboxStamp, Token,
};

use crate::owner_watch::ExitSignal;

pub const TOKEN_ENV: &str = "OSC7577_TOKEN";
pub const MAX_STORE_BYTES: usize = 24 * 1024;
pub const OWNER_WATCH_STACK_BYTES: usize = 128 * 1024;
const TOKEN_ENTROPY_BYTES: usize = 32;
const OWNER_POLL: Duration = Duration::from_secs(1);
#[cfg(target_os = "linux")]
const DELETED_CWD_SUFFIX: &str = " (deleted)";

#[derive(Clone)]
pub struct PaneToken(Token);

impl PaneToken {
    pub fn generate() -> Option<Self> {
        let mut entropy = [0u8; TOKEN_ENTROPY_BYTES];
        if let Err(error) = getrandom::fill(&mut entropy) {
            log::warn!(
                "paneflow-host: the pane gets no {TOKEN_ENV}: the random source failed ({error})"
            );
            return None;
        }
        Token::new(URL_SAFE_NO_PAD.encode(entropy)).ok().map(Self)
    }

    pub fn for_shell(shell: &str) -> Option<Self> {
        if !advertised_on_this_platform() || is_wsl_shell(shell) {
            return None;
        }
        Self::generate()
    }

    pub fn token(&self) -> &Token {
        &self.0
    }

    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl PartialEq for PaneToken {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose() == other.0.expose()
    }
}

impl Eq for PaneToken {}

impl fmt::Debug for PaneToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PaneToken(<redacted>)")
    }
}

pub fn advertised_on_this_platform() -> bool {
    !cfg!(windows)
}

pub fn is_wsl_shell(shell: &str) -> bool {
    shell.rsplit(['/', '\\']).next().is_some_and(|name| {
        name.eq_ignore_ascii_case("wsl.exe") || name.eq_ignore_ascii_case("wsl")
    })
}

pub fn is_token_env_key(key: &str) -> bool {
    key.eq_ignore_ascii_case(TOKEN_ENV)
}

#[derive(Debug, Clone)]
pub struct PaneOrigin {
    pub leader: ProcessStamp,
    pub process_session: Option<u32>,
    pub spawn_cwd: String,
    pub sandbox: Option<SandboxStamp>,
    #[cfg(windows)]
    pub job: Option<std::sync::Arc<paneflow_peer::PaneJob>>,
}

impl PartialEq for PaneOrigin {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(windows)]
        let same_job = match (&self.job, &other.job) {
            (Some(left), Some(right)) => std::sync::Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        };
        #[cfg(not(windows))]
        let same_job = true;
        same_job
            && self.leader == other.leader
            && self.process_session == other.process_session
            && self.spawn_cwd == other.spawn_cwd
            && self.sandbox == other.sandbox
    }
}

impl Eq for PaneOrigin {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportSequence {
    pub content: Vec<u8>,
    pub truncated: bool,
    pub origin: PaneOrigin,
}

pub fn is_report_content(content: &[u8]) -> bool {
    paneflow_program_resume::sequence_body(content).is_some()
}

pub fn classify(sequence: &ReportSequence, token: &PaneToken, store: &RecordStore) -> Outcome {
    let Some(body) = paneflow_program_resume::sequence_body(&sequence.content) else {
        return Outcome::Ignored(IgnoreReason::NoMac);
    };
    if sequence.truncated {
        return Outcome::Ignored(IgnoreReason::TooLong);
    }
    paneflow_program_resume::classify(
        body,
        token.token(),
        crate::manifest::now_ms() / 1000,
        store.replay_guard(),
    )
}

pub fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Accepted(report) => match report.action {
            Action::Argv(ref argv) => format!(
                "accepted a command of {} element(s) from pid {}",
                argv.len(),
                report.pid
            ),
            Action::Clear => format!("accepted a clear from pid {}", report.pid),
        },
        Outcome::Rejected { owner, reason } => match owner {
            Some(pid) => format!("rejected a report of pid {pid} ({reason:?})"),
            None => format!("rejected a report without a readable pid ({reason:?})"),
        },
        Outcome::Ignored(reason) => format!("ignored a report ({reason:?})"),
    }
}

pub fn admit(report: &Report, origin: &PaneOrigin) -> Option<Admission> {
    admit_pid(report.pid, origin)
}

pub fn admit_pid(pid: u32, origin: &PaneOrigin) -> Option<Admission> {
    let report = Report {
        pid,
        time: 0,
        action: Action::Clear,
        cwd: None,
    };
    let started_at = paneflow_peer::process_start_time(report.pid)?;
    if !runs_as_this_user(report.pid) {
        log::debug!(
            "paneflow-host: OSC 7577 owner {} runs as another user",
            report.pid
        );
        return None;
    }
    if !belongs_to_pane(report.pid, started_at, origin) {
        log::debug!(
            "paneflow-host: OSC 7577 owner {} is not a process of the pane",
            report.pid
        );
        return None;
    }
    if paneflow_peer::process_start_time(report.pid) != Some(started_at) {
        return None;
    }
    let first_process = report.pid == origin.leader.pid && started_at == origin.leader.started_at;
    let ancestors = if first_process {
        Vec::new()
    } else {
        ancestors_of(report.pid, started_at, origin.leader)
    };
    let fallback_cwd = match owner_cwd(report.pid) {
        Some(path) => RecordCwd {
            path,
            source: CwdSource::Owner,
        },
        None => RecordCwd {
            path: origin.spawn_cwd.clone(),
            source: CwdSource::Spawn,
        },
    };
    Some(Admission {
        started_at,
        ancestors,
        first_process,
        fallback_cwd,
        sandbox: origin.sandbox.clone(),
    })
}

pub fn sandbox_stamp(plan: &crate::sandbox::SandboxPlan) -> SandboxStamp {
    SandboxStamp {
        root: plan.cwd.display().to_string(),
        unix_scope: plan.scope_pathname_unix,
    }
}

pub fn accepted_owner(outcome: &Outcome, admission: Option<&Admission>) -> Option<ProcessStamp> {
    match (outcome, admission) {
        (Outcome::Accepted(report), Some(admission))
            if matches!(report.action, Action::Argv(_)) =>
        {
            Some(ProcessStamp {
                pid: report.pid,
                started_at: admission.started_at,
            })
        }
        _ => None,
    }
}

pub fn store_bytes_in_manifest(store: &RecordStore) -> usize {
    serde_json::to_vec_pretty(&serde_json::json!({ "program_resume": store }))
        .map_or(usize::MAX, |bytes| bytes.len())
}

pub fn fit_store_budget(store: &mut RecordStore) {
    while store_bytes_in_manifest(store) > MAX_STORE_BYTES {
        let Some(oldest) = store
            .records()
            .iter()
            .min_by_key(|record| record.revision)
            .map(|record| record.id)
        else {
            return;
        };
        if !store.discard(oldest) {
            return;
        }
    }
}

fn ancestors_of(pid: u32, started_at: u64, leader: ProcessStamp) -> Vec<ProcessStamp> {
    let stamp = paneflow_peer::LeaderStamp {
        pid: leader.pid,
        started_at: leader.started_at,
    };
    match paneflow_peer::LiveLineage.lineage(pid, started_at, stamp) {
        paneflow_peer::Lineage::ToLeader(chain) => chain
            .into_iter()
            .skip(1)
            .map(|ancestor| ProcessStamp {
                pid: ancestor.pid,
                started_at: ancestor.started_at,
            })
            .collect(),
        paneflow_peer::Lineage::Broken { reason } => {
            log::debug!("paneflow-host: the parents of OSC 7577 owner {pid} are unknown: {reason}");
            Vec::new()
        }
    }
}

#[cfg(unix)]
fn belongs_to_pane(pid: u32, _started_at: u64, origin: &PaneOrigin) -> bool {
    let pane_session = origin.process_session.unwrap_or(origin.leader.pid);
    paneflow_peer::process_session(pid) == Some(pane_session)
}

#[cfg(windows)]
fn belongs_to_pane(pid: u32, started_at: u64, origin: &PaneOrigin) -> bool {
    let Some(job) = origin.job.clone() else {
        return false;
    };
    let leader = paneflow_peer::PaneLeader {
        session: String::new(),
        workspace: None,
        pid: origin.leader.pid,
        started_at: origin.leader.started_at,
        process_session: None,
        job: Some(job),
    };
    matches!(
        paneflow_peer::classify(pid, started_at, &[leader]),
        paneflow_peer::Membership::Pane { .. }
    )
}

#[cfg(not(any(unix, windows)))]
fn belongs_to_pane(_pid: u32, _started_at: u64, _origin: &PaneOrigin) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn runs_as_this_user(pid: u32) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(format!("/proc/{pid}"))
        .is_ok_and(|metadata| metadata.uid() == unsafe { libc::geteuid() })
}

#[cfg(target_os = "macos")]
fn runs_as_this_user(pid: u32) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let Ok(pid) = libc::c_int::try_from(pid) else {
        return false;
    };
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<libc::c_void>(),
            size,
        )
    };
    written == size && unsafe { info.assume_init() }.pbi_uid == unsafe { libc::geteuid() }
}

#[cfg(windows)]
fn runs_as_this_user(_pid: u32) -> bool {
    true
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn runs_as_this_user(_pid: u32) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn owner_cwd(pid: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    let path = path.to_str()?;
    (path.starts_with('/') && !path.ends_with(DELETED_CWD_SUFFIX)).then(|| path.to_owned())
}

#[cfg(target_os = "macos")]
fn owner_cwd(pid: u32) -> Option<String> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast::<libc::c_void>(),
            size,
        )
    };
    if written != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let raw = &info.pvi_cdir.vip_path;
    let bytes: Vec<u8> = raw
        .iter()
        .flatten()
        .map(|byte| *byte as u8)
        .take_while(|byte| *byte != 0)
        .collect();
    let path = String::from_utf8(bytes).ok()?;
    path.starts_with('/').then_some(path)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn owner_cwd(_pid: u32) -> Option<String> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitObservation {
    Notified,
    AlreadyGone,
    Polled,
}

pub enum OwnerExit {
    Watch(ExitSignal, ProcessStamp),
    Gone,
    Poll(ProcessStamp),
}

impl OwnerExit {
    pub fn arm(owner: ProcessStamp) -> Self {
        let still_owner = || paneflow_peer::process_start_time(owner.pid) == Some(owner.started_at);
        match ExitSignal::open(owner.pid) {
            Ok(signal) if still_owner() => Self::Watch(signal, owner),
            Ok(_) => Self::Gone,
            Err(_) if !still_owner() => Self::Gone,
            Err(error) => {
                log::warn!(
                    "paneflow-host: OSC 7577 owner {} has no exit notification ({error}); its identity is polled",
                    owner.pid
                );
                Self::Poll(owner)
            }
        }
    }

    pub fn is_watched(&self) -> bool {
        matches!(self, Self::Watch(..))
    }

    pub fn wait(self) -> ExitObservation {
        match self {
            Self::Gone => ExitObservation::AlreadyGone,
            Self::Watch(signal, owner) => match signal.wait() {
                Ok(()) => ExitObservation::Notified,
                Err(error) => {
                    log::warn!(
                        "paneflow-host: waiting on OSC 7577 owner {} failed ({error}); its identity is polled",
                        owner.pid
                    );
                    poll_until_gone(owner)
                }
            },
            Self::Poll(owner) => poll_until_gone(owner),
        }
    }
}

fn poll_until_gone(owner: ProcessStamp) -> ExitObservation {
    while paneflow_peer::process_start_time(owner.pid) == Some(owner.started_at) {
        std::thread::sleep(OWNER_POLL);
    }
    ExitObservation::Polled
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_program_resume::{Outcome, Report};

    fn own_origin() -> PaneOrigin {
        let pid = std::process::id();
        PaneOrigin {
            leader: ProcessStamp {
                pid,
                started_at: paneflow_peer::process_start_time(pid).unwrap(),
            },
            #[cfg(unix)]
            process_session: paneflow_peer::process_session(pid),
            #[cfg(not(unix))]
            process_session: None,
            spawn_cwd: "/spawn".into(),
            sandbox: None,
            #[cfg(windows)]
            job: None,
        }
    }

    fn report_of(pid: u32, argv: &[&str]) -> Report {
        Report {
            pid,
            time: crate::manifest::now_ms() / 1000,
            action: Action::Argv(argv.iter().map(|element| (*element).to_owned()).collect()),
            cwd: None,
        }
    }

    #[test]
    fn a_token_is_43_url_safe_characters_and_never_repeats() {
        let first = PaneToken::generate().unwrap();
        let second = PaneToken::generate().unwrap();
        assert_eq!(first.expose().len(), 43);
        assert!(
            first
                .expose()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        );
        assert_ne!(first, second);
        assert!(!format!("{first:?}").contains(first.expose()));
    }

    #[test]
    fn a_wsl_shell_never_gets_a_token_whatever_its_path_or_casing() {
        for shell in [
            "wsl",
            "wsl.exe",
            "WSL.EXE",
            r"C:\Windows\System32\wsl.exe",
            "/mnt/c/Windows/System32/wsl.exe",
        ] {
            assert!(is_wsl_shell(shell), "{shell}");
            assert!(PaneToken::for_shell(shell).is_none(), "{shell}");
        }
        assert!(!is_wsl_shell("/bin/bash"));
        assert!(!is_wsl_shell("pwsh.exe"));
    }

    #[test]
    fn the_token_is_advertised_everywhere_but_windows_until_conpty_is_proven() {
        assert_eq!(advertised_on_this_platform(), !cfg!(windows));
        assert_eq!(
            PaneToken::for_shell("/bin/sh").is_some(),
            advertised_on_this_platform()
        );
    }

    #[test]
    fn the_token_variable_is_recognized_whatever_its_casing() {
        assert!(is_token_env_key("OSC7577_TOKEN"));
        assert!(is_token_env_key("osc7577_token"));
        assert!(!is_token_env_key("OSC7501_TOKEN"));
    }

    #[cfg(unix)]
    #[test]
    fn the_first_process_of_the_pane_is_admitted_as_first_process_with_its_os_directory() {
        let origin = own_origin();
        let admission = admit(&report_of(origin.leader.pid, &["nvim"]), &origin).unwrap();
        assert!(admission.first_process);
        assert_eq!(admission.started_at, origin.leader.started_at);
        assert!(admission.ancestors.is_empty());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            assert_eq!(admission.fallback_cwd.source, CwdSource::Owner);
            assert_eq!(
                std::path::Path::new(&admission.fallback_cwd.path),
                std::env::current_dir().unwrap()
            );
        }
    }

    #[test]
    fn a_pid_that_does_not_exist_is_never_admitted() {
        let mut child = short_lived_child();
        let pid = child.id();
        child.wait().unwrap();
        assert!(admit(&report_of(pid, &["nvim"]), &own_origin()).is_none());
    }

    #[test]
    fn only_an_accepted_command_names_an_owner_to_watch() {
        let report = report_of(7, &["nvim"]);
        let admission = Admission {
            started_at: 9,
            ancestors: Vec::new(),
            first_process: false,
            fallback_cwd: RecordCwd {
                path: "/".into(),
                source: CwdSource::Spawn,
            },
            sandbox: None,
        };
        assert_eq!(
            accepted_owner(&Outcome::Accepted(report.clone()), Some(&admission)),
            Some(ProcessStamp {
                pid: 7,
                started_at: 9
            })
        );
        assert_eq!(accepted_owner(&Outcome::Accepted(report), None), None);
        let clear = Report {
            action: Action::Clear,
            ..report_of(7, &["nvim"])
        };
        assert_eq!(
            accepted_owner(&Outcome::Accepted(clear), Some(&admission)),
            None
        );
        assert_eq!(
            accepted_owner(&Outcome::Ignored(IgnoreReason::MacMismatch), None),
            None
        );
    }

    #[test]
    fn a_store_over_its_byte_budget_evicts_its_least_recently_updated_records() {
        let mut store = RecordStore::new();
        let admission = |pid: u32| Admission {
            started_at: u64::from(pid),
            ancestors: Vec::new(),
            first_process: false,
            fallback_cwd: RecordCwd {
                path: "/".into(),
                source: CwdSource::Spawn,
            },
            sandbox: None,
        };
        let long = "x".repeat(3000);
        for pid in 1..=12 {
            let report = report_of(pid, &["nvim", &long, &long]);
            let _ = store.apply(Outcome::Accepted(report), |_| Some(admission(pid)), 1_000);
        }
        fit_store_budget(&mut store);
        let bytes = store_bytes_in_manifest(&store);
        assert!(bytes <= MAX_STORE_BYTES, "{bytes}");
        assert!(!store.is_empty());
        let owners: Vec<u32> = store
            .records()
            .iter()
            .map(|record| record.owner.pid)
            .collect();
        assert!(owners.contains(&12));
        assert!(!owners.contains(&1));
    }

    fn short_lived_child() -> std::process::Child {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "exit 0"]);
            command
        };
        #[cfg(unix)]
        let mut command = std::process::Command::new("true");
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn an_owner_that_exited_before_its_watch_is_armed_is_gone_at_once() {
        let mut child = short_lived_child();
        let pid = child.id();
        let started_at = paneflow_peer::process_start_time(pid).unwrap_or(1);
        child.wait().unwrap();
        let exit = OwnerExit::arm(ProcessStamp { pid, started_at });
        assert!(!exit.is_watched());
        assert_eq!(exit.wait(), ExitObservation::AlreadyGone);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_live_owner_is_watched_through_the_kernel_and_reported_when_it_exits() {
        let mut child = std::process::Command::new("sleep")
            .arg("1")
            .spawn()
            .unwrap();
        let owner = ProcessStamp {
            pid: child.id(),
            started_at: paneflow_peer::process_start_time(child.id()).unwrap(),
        };
        let exit = OwnerExit::arm(owner);
        assert!(exit.is_watched());
        let waiter = std::thread::spawn(move || exit.wait());
        child.wait().unwrap();
        assert_eq!(waiter.join().unwrap(), ExitObservation::Notified);
    }
}
