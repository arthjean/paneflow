#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SHIM: &str = env!("CARGO_BIN_EXE_paneflow-shim");
const TOOL: &str = "copilot";
const SHIM_DEPTH_CAP: usize = 8;
const LAUNCHER_BRAKE: usize = 12;
const LAUNCHER_BRAKE_EXIT: i32 = 99;

#[cfg(windows)]
const SCRIPT_NAME: &str = "copilot.cmd";
#[cfg(unix)]
const SCRIPT_NAME: &str = "copilot";

#[cfg(windows)]
fn launcher_script(appended_argument: &str) -> String {
    format!(
        "@echo off\r\n\
         echo run>>\"%FAKE_LAUNCHER_LOG%\"\r\n\
         if defined FAKE_LAUNCHER_DEPTH (set /a FAKE_LAUNCHER_DEPTH+=1) else (set FAKE_LAUNCHER_DEPTH=1)\r\n\
         if %FAKE_LAUNCHER_DEPTH% GEQ {LAUNCHER_BRAKE} exit /b {LAUNCHER_BRAKE_EXIT}\r\n\
         copilot %* {appended_argument}\r\n\
         exit /b %ERRORLEVEL%\r\n"
    )
}

#[cfg(unix)]
fn launcher_script(appended_argument: &str) -> String {
    format!(
        "#!/bin/sh\n\
         echo run >> \"$FAKE_LAUNCHER_LOG\"\n\
         FAKE_LAUNCHER_DEPTH=$(( ${{FAKE_LAUNCHER_DEPTH:-0}} + 1 ))\n\
         export FAKE_LAUNCHER_DEPTH\n\
         [ \"$FAKE_LAUNCHER_DEPTH\" -ge {LAUNCHER_BRAKE} ] && exit {LAUNCHER_BRAKE_EXIT}\n\
         exec copilot \"$@\" {appended_argument}\n"
    )
}

#[cfg(windows)]
const REAL_TOOL_SCRIPT: &str = "@echo off\r\n\
if \"%~1\"==\"outer\" goto nest\r\n\
echo real-copilot %*\r\n\
exit /b 0\r\n\
:nest\r\n\
copilot inner\r\n\
exit /b %ERRORLEVEL%\r\n";

#[cfg(unix)]
const REAL_TOOL_SCRIPT: &str = "#!/bin/sh\n\
if [ \"$1\" = outer ]; then exec copilot inner; fi\n\
echo real-copilot \"$@\"\n";

struct Fixture {
    shim_dir: tempfile::TempDir,
    launcher_dir: tempfile::TempDir,
    real_dir: tempfile::TempDir,
    scratch: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            shim_dir: tempfile::TempDir::new().unwrap(),
            launcher_dir: tempfile::TempDir::new().unwrap(),
            real_dir: tempfile::TempDir::new().unwrap(),
            scratch: tempfile::TempDir::new().unwrap(),
        };
        std::fs::copy(SHIM, fixture.shim()).unwrap();
        fixture
    }

    fn shim(&self) -> PathBuf {
        self.shim_dir
            .path()
            .join(format!("{TOOL}{}", std::env::consts::EXE_SUFFIX))
    }

    fn with_launcher(self, appended_argument: &str) -> Self {
        write_script(
            self.launcher_dir.path(),
            SCRIPT_NAME,
            &launcher_script(appended_argument),
        );
        self
    }

    fn with_real_tool(self) -> Self {
        write_script(self.real_dir.path(), SCRIPT_NAME, REAL_TOOL_SCRIPT);
        self
    }

    fn launcher(&self) -> PathBuf {
        self.launcher_dir.path().join(SCRIPT_NAME)
    }

    fn log(&self) -> PathBuf {
        self.scratch.path().join("launcher.log")
    }

    fn launcher_runs(&self) -> usize {
        std::fs::read_to_string(self.log())
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn run(&self, argument: &str) -> Output {
        self.run_in(argument, None)
    }

    fn hook_log(&self) -> PathBuf {
        self.scratch.path().join("hook.log")
    }

    fn run_in(&self, argument: &str, pane_session: Option<&str>) -> Output {
        let path = std::env::join_paths([
            self.shim_dir.path(),
            self.launcher_dir.path(),
            self.real_dir.path(),
        ])
        .unwrap();
        let mut command = Command::new(self.shim());
        command
            .arg(argument)
            .current_dir(self.scratch.path())
            .env("PATH", path)
            .env("FAKE_LAUNCHER_LOG", self.log())
            .env("FAKE_HOOK_LOG", self.hook_log())
            .env_remove("FAKE_LAUNCHER_DEPTH")
            .env_remove("PANEFLOW_SHIM_TARGET")
            .env_remove("PANEFLOW_SHIM_CHAIN")
            .env_remove("PANEFLOW_SHIM_CHAIN_TARGETS")
            .env_remove("PANEFLOW_SESSION_ID");
        if let Some(pane_session) = pane_session {
            command.env("PANEFLOW_SESSION_ID", pane_session);
        }
        command.output().unwrap()
    }
}

fn write_script(dir: &Path, name: &str, script: &str) {
    let path = dir.join(name);
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn a_launcher_that_resolves_the_tool_back_to_the_shim_ends_with_127() {
    let fixture = Fixture::new().with_launcher("");

    let output = fixture.run("--version");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let runs = fixture.launcher_runs();
    assert_eq!(
        output.status.code(),
        Some(127),
        "{runs} launcher runs: {stderr}"
    );
    assert_eq!(runs, 1, "{stderr}");
    assert!(
        stderr.contains(&fixture.launcher().display().to_string()),
        "{stderr}"
    );
}

#[test]
fn the_real_tool_behind_a_launcher_that_loops_back_is_the_one_that_runs() {
    let fixture = Fixture::new().with_launcher("").with_real_tool();

    let output = fixture.run("--version");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "real-copilot --version",
        "{stderr}"
    );
    assert_eq!(fixture.launcher_runs(), 1, "{stderr}");
}

#[test]
fn a_launcher_that_rewrites_the_arguments_is_stopped_by_the_depth_cap() {
    let fixture = Fixture::new().with_launcher("again");

    let output = fixture.run("--version");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let runs = fixture.launcher_runs();
    assert_eq!(
        output.status.code(),
        Some(127),
        "{runs} launcher runs: {stderr}"
    );
    assert_eq!(runs, SHIM_DEPTH_CAP, "{stderr}");
    assert!(stderr.contains("nested"), "{stderr}");
}

#[test]
fn a_tool_that_runs_itself_with_other_arguments_is_not_a_loop() {
    let fixture = Fixture::new().with_real_tool();

    let output = fixture.run("outer");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "real-copilot inner"
    );
}

#[test]
fn a_refusal_captured_from_a_pane_carries_no_escape_sequence() {
    let fixture = Fixture::new().with_launcher("");

    let output = fixture.run_in("--version", Some("1fa27fad-d319-43a0-a3fa-59f2d710b5b2"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(127), "{stderr}");
    assert!(!output.stderr.contains(&0x1b), "{stderr:?}");
    assert!(!output.stdout.contains(&0x1b), "{output:?}");
}

#[cfg(unix)]
#[test]
fn only_the_outermost_shim_reports_the_agent_lifecycle() {
    let fixture = Fixture::new().with_real_tool();
    write_script(
        fixture.shim_dir.path(),
        "paneflow-ai-hook",
        "#!/bin/sh\necho \"$1\" >> \"$FAKE_HOOK_LOG\"\n",
    );

    let output = fixture.run("outer");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "real-copilot inner"
    );
    let reported = std::fs::read_to_string(fixture.hook_log()).unwrap();
    for event in ["Exit", "SessionEnd"] {
        assert_eq!(
            reported.lines().filter(|line| *line == event).count(),
            1,
            "{event}: {reported}"
        );
    }
}
