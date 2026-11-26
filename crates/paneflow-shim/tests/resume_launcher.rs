#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use paneflow_resume_handoff::{CwdSource, Handoff};

const SHIM: &str = env!("CARGO_BIN_EXE_paneflow-shim");
const ATTEMPT: &str = "0123456789abcdef0123456789abcdef";
const DEADLINE: Duration = Duration::from_secs(10);

struct Bench {
    root: tempfile::TempDir,
    launcher: PathBuf,
}

impl Bench {
    fn new() -> Self {
        let root = tempfile::TempDir::new().unwrap();
        let bin = root.path().join("helpers");
        std::fs::create_dir_all(&bin).unwrap();
        let launcher = bin.join("paneflow-resume");
        std::fs::copy(SHIM, &launcher).unwrap();
        Self { root, launcher }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn with_reporting_hook(&self) -> PathBuf {
        let log = self.path("reports.log");
        write_script(
            &self.launcher.with_file_name("paneflow-ai-hook"),
            &format!("printf '%s\\n' \"$*\" >> '{}'\n", log.display()),
        );
        log
    }

    fn handoff(&self, argv: &[&str], cwd: &Path, source: CwdSource) -> PathBuf {
        let path = self.path("resume.handoff");
        let text = paneflow_resume_handoff::encode(&Handoff {
            attempt: ATTEMPT.to_owned(),
            cwd: cwd.display().to_string(),
            cwd_source: source,
            argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        });
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn command(&self, handoff: &Path, path_var: &str) -> Command {
        let mut command = Command::new(&self.launcher);
        command.arg(handoff).env("PATH", path_var);
        command
    }

    fn run(&self, handoff: &Path, path_var: &str) -> Output {
        self.command(handoff, path_var).output().unwrap()
    }
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !done() {
        assert!(Instant::now() < deadline, "the outcome never came");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_handoff_is_deleted_before_the_program_starts() {
    let bench = Bench::new();
    let seen = bench.path("seen");
    let handoff_path = bench.path("resume.handoff");
    let program = bench.path("program");
    write_script(
        &program,
        &format!(
            "if [ -e '{}' ]; then echo present > '{}'; else echo absent > '{}'; fi\n",
            handoff_path.display(),
            seen.display(),
            seen.display()
        ),
    );
    let program_text = program.display().to_string();
    let handoff = bench.handoff(&[&program_text], bench.root.path(), CwdSource::Report);
    let output = bench.run(&handoff, "/usr/bin:/bin");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "absent\n");
    assert!(!handoff.exists());
}

#[test]
fn the_program_keeps_the_launcher_pid_and_receives_argv0_as_written() {
    let bench = Bench::new();
    let sleep = ["/usr/bin/sleep", "/bin/sleep"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .unwrap();
    let dir = Path::new(sleep).parent().unwrap().display().to_string();
    let handoff = bench.handoff(&["sleep", "30"], bench.root.path(), CwdSource::Spawn);
    let mut launcher = Command::new(&bench.launcher);
    launcher.arg(&handoff).env("PATH", &dir);
    let mut child = launcher.spawn().unwrap();
    let pid = child.id();
    let cmdline = || std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    wait_for(|| cmdline().starts_with(b"sleep\0"));
    assert_eq!(cmdline(), b"sleep\x0030\0");
    assert_eq!(
        std::fs::canonicalize(format!("/proc/{pid}/exe")).unwrap(),
        std::fs::canonicalize(sleep).unwrap()
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn a_bare_program_is_found_on_the_launchers_path_so_an_agent_shim_first_on_path_runs() {
    let bench = Bench::new();
    let shims = bench.path("shims");
    let real = bench.path("real");
    std::fs::create_dir_all(&shims).unwrap();
    std::fs::create_dir_all(&real).unwrap();
    std::fs::copy(SHIM, shims.join("claude")).unwrap();
    let marker = bench.path("marker");
    write_script(
        &real.join("claude"),
        &format!(
            "printf '%s|%s\\n' \"$PANEFLOW_SHIM_TARGET\" \"$*\" > '{}'\n",
            marker.display()
        ),
    );
    let handoff = bench.handoff(
        &["claude", "--resume", "abc"],
        bench.root.path(),
        CwdSource::Spawn,
    );
    let path_var = format!("{}:{}:/usr/bin:/bin", shims.display(), real.display());
    let output = bench.run(&handoff, &path_var);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        format!("{}|--resume abc\n", real.join("claude").display())
    );
}

#[test]
fn an_absolute_program_starts_without_a_path_lookup() {
    let bench = Bench::new();
    let marker = bench.path("marker");
    let program = bench.path("absolute-program");
    write_script(&program, &format!("echo ran > '{}'\n", marker.display()));
    let program_text = program.display().to_string();
    let handoff = bench.handoff(&[&program_text], bench.root.path(), CwdSource::Spawn);
    let output = bench.run(&handoff, "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "ran\n");
}

#[test]
fn a_program_that_is_not_found_exits_127_names_itself_and_reports_the_failure() {
    let bench = Bench::new();
    let reports = bench.with_reporting_hook();
    let handoff = bench.handoff(
        &["paneflow-no-such-program"],
        bench.root.path(),
        CwdSource::Spawn,
    );
    let empty = bench.path("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let output = bench.run(&handoff, &empty.display().to_string());
    assert_eq!(output.status.code(), Some(127));
    assert_eq!(
        stderr(&output),
        "paneflow-resume: paneflow-no-such-program: not found\n"
    );
    assert_eq!(
        std::fs::read_to_string(reports).unwrap(),
        format!("resume-attempt {ATTEMPT} failed not_found\n")
    );
}

#[test]
fn a_started_program_is_reported_with_the_launcher_pid_before_it_runs() {
    let bench = Bench::new();
    let reports = bench.with_reporting_hook();
    let marker = bench.path("marker");
    let program = bench.path("program");
    write_script(&program, &format!("echo \"$$\" > '{}'\n", marker.display()));
    let program_text = program.display().to_string();
    let handoff = bench.handoff(&[&program_text], bench.root.path(), CwdSource::Spawn);
    let output = bench.run(&handoff, "/usr/bin:/bin");
    assert!(output.status.success(), "{}", stderr(&output));
    let pid = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        std::fs::read_to_string(reports).unwrap(),
        format!("resume-attempt {ATTEMPT} started {pid}")
    );
}

#[test]
fn a_missing_report_directory_blocks_the_resume_and_another_source_stays_in_place() {
    let bench = Bench::new();
    let reports = bench.with_reporting_hook();
    let marker = bench.path("marker");
    let program = bench.path("program");
    write_script(&program, &format!("pwd > '{}'\n", marker.display()));
    let program_text = program.display().to_string();
    let gone = bench.path("deleted-directory");
    let handoff = bench.handoff(&[&program_text], &gone, CwdSource::Report);
    let output = bench.run(&handoff, "/usr/bin:/bin");
    assert_eq!(output.status.code(), Some(126));
    assert!(!marker.exists());
    assert_eq!(
        std::fs::read_to_string(&reports).unwrap(),
        format!("resume-attempt {ATTEMPT} failed cwd_missing\n")
    );

    let handoff = bench.handoff(&[&program_text], &gone, CwdSource::Owner);
    let output = bench
        .command(&handoff, "/usr/bin:/bin")
        .current_dir(bench.root.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        Path::new(std::fs::read_to_string(&marker).unwrap().trim_end()),
        std::fs::canonicalize(bench.root.path()).unwrap()
    );
}

#[test]
fn a_handoff_with_another_mode_or_behind_a_link_is_refused_with_126() {
    let bench = Bench::new();
    let marker = bench.path("marker");
    let program = bench.path("program");
    write_script(&program, &format!("echo ran > '{}'\n", marker.display()));
    let program_text = program.display().to_string();
    let handoff = bench.handoff(&[&program_text], bench.root.path(), CwdSource::Spawn);
    std::fs::set_permissions(&handoff, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = bench.run(&handoff, "/usr/bin:/bin");
    assert_eq!(output.status.code(), Some(126));
    assert!(stderr(&output).contains("mode 0600"), "{}", stderr(&output));
    assert!(handoff.exists());

    std::fs::set_permissions(&handoff, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = bench.path("link.handoff");
    std::os::unix::fs::symlink(&handoff, &link).unwrap();
    let output = bench.run(&link, "/usr/bin:/bin");
    assert_eq!(output.status.code(), Some(126));
    assert!(handoff.exists());
    assert!(!marker.exists());
}

#[test]
fn a_missing_or_malformed_handoff_exits_126_without_starting_anything() {
    let bench = Bench::new();
    let reports = bench.with_reporting_hook();
    let output = bench.run(&bench.path("never-written.handoff"), "/usr/bin:/bin");
    assert_eq!(output.status.code(), Some(126));
    let output = Command::new(&bench.launcher).output().unwrap();
    assert_eq!(output.status.code(), Some(126));

    let malformed = bench.path("malformed.handoff");
    std::fs::write(
        &malformed,
        format!("paneflow-resume 1\nattempt {ATTEMPT}\ncwd Lw== spawn\nenv UEFUSD0v\n"),
    )
    .unwrap();
    std::fs::set_permissions(&malformed, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = bench.run(&malformed, "/usr/bin:/bin");
    assert_eq!(output.status.code(), Some(126));
    assert!(!malformed.exists());
    assert_eq!(
        std::fs::read_to_string(reports).unwrap(),
        format!("resume-attempt {ATTEMPT} failed handoff_invalid\n")
    );
}
