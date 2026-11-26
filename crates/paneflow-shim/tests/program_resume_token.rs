#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

const SHIM: &str = env!("CARGO_BIN_EXE_paneflow-shim");
const TOKEN: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";

struct Fixture {
    shim_dir: tempfile::TempDir,
    real_dir: tempfile::TempDir,
}

impl Fixture {
    fn new(tools: &[&str]) -> Self {
        let fixture = Self {
            shim_dir: tempfile::TempDir::new().unwrap(),
            real_dir: tempfile::TempDir::new().unwrap(),
        };
        for tool in tools {
            std::fs::copy(SHIM, fixture.shim_dir.path().join(tool)).unwrap();
        }
        fixture
    }

    fn real(self, tool: &str, script: &str) -> Self {
        write_script(self.real_dir.path(), tool, script);
        self
    }

    fn run(&self, tool: &str) -> String {
        let path = std::env::join_paths([self.shim_dir.path(), self.real_dir.path()]).unwrap();
        let output = Command::new(self.shim_dir.path().join(tool))
            .current_dir(self.real_dir.path())
            .env("PATH", path)
            .env("OSC7577_TOKEN", TOKEN)
            .env_remove("PANEFLOW_SHIM_TARGET")
            .env_remove("PANEFLOW_SHIM_CHAIN")
            .env_remove("PANEFLOW_SHIM_CHAIN_TARGETS")
            .env_remove("PANEFLOW_SESSION_ID")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }
}

fn write_script(dir: &Path, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn token_dump(tool: &str) -> String {
    format!("#!/bin/sh\nprintf '{tool}<%s>\\n' \"${{OSC7577_TOKEN-unset}}\"\n")
}

#[test]
fn pi_reports_natively_and_keeps_the_token() {
    let fixture = Fixture::new(&["pi"]).real("pi", &token_dump("pi"));
    assert_eq!(fixture.run("pi").trim(), format!("pi<{TOKEN}>"));
}

#[test]
fn claude_does_not_report_natively_and_loses_the_token() {
    let fixture = Fixture::new(&["claude"]).real("claude", &token_dump("claude"));
    assert_eq!(fixture.run("claude").trim(), "claude<unset>");
}

#[test]
fn a_nested_shim_never_adds_back_the_token_an_outer_shim_stripped() {
    let fixture = Fixture::new(&["claude", "pi"])
        .real(
            "claude",
            "#!/bin/sh\nprintf 'claude<%s>\\n' \"${OSC7577_TOKEN-unset}\"\nexec pi\n",
        )
        .real("pi", &token_dump("pi"));
    let lines: Vec<String> = fixture.run("claude").lines().map(str::to_owned).collect();
    assert_eq!(lines, ["claude<unset>", "pi<unset>"]);
}
