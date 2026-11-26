#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

fn integrations(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
    command
        .arg("integrations")
        .args(args)
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env(paneflow_home::HOME_ENV, home.join(".paneflow-dev"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("PANEFLOW_SOCKET_PATH")
        .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE");
    for key in paneflow_host::env::PANE_CONTEXT_ENV {
        command.env_remove(key);
    }
    command.output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn installing_codex_puts_the_conductor_skill_where_codex_and_gemini_read_skills() {
    let home = tempfile::tempdir().unwrap();
    let skill = home
        .path()
        .join(".agents")
        .join("skills")
        .join("paneflow-conductor")
        .join("SKILL.md");

    let installed = integrations(home.path(), &["install", "codex"]);
    assert!(installed.status.success(), "{}", stderr(&installed));
    let text = std::fs::read_to_string(&skill).expect("the Codex copy of the skill");
    let frontmatter = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .expect("frontmatter")
        .0;
    assert!(
        frontmatter.contains(&format!(
            "paneflow-version: \"{}\"",
            env!("CARGO_PKG_VERSION")
        )),
        "{frontmatter}"
    );
    assert!(text.contains("untrusted_terminal_output"), "{text}");
    assert!(text.contains("paneflow write"), "{text}");

    std::fs::write(&skill, "my own notes\n").unwrap();
    let refreshed = integrations(home.path(), &["install", "codex"]);
    assert!(refreshed.status.success(), "{}", stderr(&refreshed));
    assert!(
        stderr(&refreshed).contains("kept your modified paneflow-conductor skill"),
        "{}",
        stderr(&refreshed)
    );
    let removed = integrations(home.path(), &["remove", "codex"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), "my own notes\n");

    std::fs::remove_file(&skill).unwrap();
    let reinstalled = integrations(home.path(), &["install", "codex"]);
    assert!(reinstalled.status.success(), "{}", stderr(&reinstalled));
    assert!(skill.is_file());
    let removed = integrations(home.path(), &["remove", "codex"]);
    assert!(removed.status.success(), "{}", stderr(&removed));
    assert!(
        !skill.exists(),
        "an unmodified copy is removed with the integration"
    );
}
