#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result,
    clippy::panic
)]

pub mod base;
pub mod layouts;
pub mod linked;
pub mod submodules;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use paneflow_git::porcelain::{self, Record, CLEAN_STATUS_ARGS, FILES_STATUS_ARGS};
use paneflow_git::{snapshot, Options, Submodules};

use crate::support;

const PROBE_CONFIG: &[&str] = &[
    "core.fsmonitor=false",
    "safe.bareRepository=explicit",
    "diff.external=",
    "diff.autoRefreshIndex=false",
    "log.showSignature=false",
];

const PROBE_ENV: &[(&str, &str)] = &[
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("LC_ALL", "C"),
    ("LANGUAGE", "C"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Files,
    Clean,
}

impl Probe {
    pub fn name(self) -> &'static str {
        match self {
            Probe::Files => "files",
            Probe::Clean => "is_clean",
        }
    }

    pub fn cli_args(self) -> &'static [&'static str] {
        match self {
            Probe::Files => FILES_STATUS_ARGS,
            Probe::Clean => CLEAN_STATUS_ARGS,
        }
    }

    pub fn options(self) -> Options {
        match self {
            Probe::Files => Options::default(),
            Probe::Clean => Options {
                submodules: Submodules::CheckAll,
                ..Options::default()
            },
        }
    }
}

#[derive(Debug)]
pub struct Skip(pub String);

pub type Built = Result<PathBuf, Skip>;

pub struct Fixture {
    pub name: &'static str,
    pub build: fn(&Path) -> Built,
    pub must_include: &'static [(&'static str, &'static str)],
}

pub struct Report {
    pub compared: Vec<&'static str>,
    pub skipped: Vec<(&'static str, String)>,
    pub both_failed: Vec<(&'static str, String)>,
    pub failures: Vec<String>,
}

impl Report {
    pub fn assert_parity(&self, may_skip: &[&str]) {
        self.assert_parity_with_failures(may_skip, &[]);
    }

    pub fn assert_parity_with_failures(&self, may_skip: &[&str], must_fail: &[&str]) {
        for (name, reason) in &self.skipped {
            eprintln!("oracle: SKIPPED fixture {name}: {reason}");
        }
        let mut failures = self.failures.clone();
        for (name, reason) in &self.skipped {
            if !may_skip.contains(name) {
                failures.push(format!(
                    "fixture {name} was skipped but is not allowed to skip: {reason}"
                ));
            }
        }
        for (name, reason) in &self.both_failed {
            if !must_fail.contains(name) {
                failures.push(format!("fixture {name}: both engines failed: {reason}"));
            }
        }
        for name in must_fail {
            let skipped = self.skipped.iter().any(|(skipped, _)| skipped == name);
            let failed = self.both_failed.iter().any(|(failed, _)| failed == name);
            if !skipped && !failed {
                failures.push(format!(
                    "fixture {name}: both engines were expected to fail, but they did not"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "the git CLI and gix disagree ({} compared, {} skipped):\n{}",
            self.compared.len(),
            self.skipped.len(),
            failures.join("\n")
        );
    }
}

pub fn hooks_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        tempfile::Builder::new()
            .prefix("paneflow-oracle-hooks")
            .tempdir()
            .expect("hooks dir")
            .keep()
    })
}

pub fn cli_output(cwd: &Path, args: &[&str]) -> std::process::Output {
    cli_command(cwd, args)
        .output()
        .unwrap_or_else(|error| panic!("git status in {}: {error}", cwd.display()))
}

pub fn cli_command(cwd: &Path, args: &[&str]) -> Command {
    let mut probe_args: Vec<String> = Vec::new();
    for setting in PROBE_CONFIG {
        probe_args.push("-c".into());
        probe_args.push((*setting).into());
    }
    probe_args.push("-c".into());
    probe_args.push(format!(
        "core.hooksPath={}",
        support::forward_slashes(hooks_dir())
    ));
    probe_args.extend(args.iter().map(|arg| (*arg).to_string()));
    let probe_args: Vec<&str> = probe_args.iter().map(String::as_str).collect();
    let mut command = support::command(cwd, &probe_args);
    command.envs(PROBE_ENV.iter().copied());
    command
}

pub fn cli_records(cwd: &Path, probe: Probe) -> Result<Vec<Record>, String> {
    let out = cli_output(cwd, probe.cli_args());
    if !out.status.success() {
        return Err(format!(
            "git exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_z(&out.stdout))
}

pub fn gix_records(cwd: &Path, probe: Probe) -> Result<Vec<Record>, String> {
    snapshot(cwd, &probe.options())
        .map(|snapshot| porcelain::records(&snapshot))
        .map_err(|error| error.to_string())
}

pub fn parse_z(stdout: &[u8]) -> Vec<Record> {
    let mut records = Vec::new();
    let mut fields = stdout.split(|byte| *byte == 0);
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        assert!(
            field.len() >= 4 && field[2] == b' ',
            "malformed porcelain record {:?}",
            String::from_utf8_lossy(field)
        );
        let code = [field[0], field[1]];
        if matches!(code[0], b'R' | b'C') {
            fields.next();
        }
        records.push(Record {
            code,
            path: field[3..].to_vec(),
        });
    }
    records
}

pub fn display_codes(codes: &[[u8; 2]]) -> String {
    let codes: Vec<String> = codes
        .iter()
        .map(|code| format!("{:?}", String::from_utf8_lossy(code)))
        .collect();
    format!("[{}]", codes.join(", "))
}

type CodesByEngine = (Vec<[u8; 2]>, Vec<[u8; 2]>);

pub fn divergences(fixture: &str, cli: &[Record], gix: &[Record]) -> Vec<String> {
    let mut by_path: BTreeMap<Vec<u8>, CodesByEngine> = BTreeMap::new();
    for record in cli {
        by_path
            .entry(record.path.clone())
            .or_default()
            .0
            .push(record.code);
    }
    for record in gix {
        by_path
            .entry(record.path.clone())
            .or_default()
            .1
            .push(record.code);
    }
    by_path
        .into_iter()
        .filter_map(|(path, (mut cli, mut gix))| {
            cli.sort_unstable();
            gix.sort_unstable();
            (cli != gix).then(|| {
                format!(
                    "fixture {fixture}: path {:?}: cli {} gix {}",
                    String::from_utf8_lossy(&path),
                    display_codes(&cli),
                    display_codes(&gix)
                )
            })
        })
        .collect()
}

pub fn missing_expectations(
    fixture: &str,
    cli: &[Record],
    must_include: &[(&str, &str)],
) -> Vec<String> {
    must_include
        .iter()
        .filter(|(code, path)| {
            !cli.iter()
                .any(|record| record.code == code.as_bytes() && record.path == path.as_bytes())
        })
        .map(|(code, path)| {
            format!("fixture {fixture}: the CLI does not report {code:?} {path:?}, so the fixture does not build what it claims")
        })
        .collect()
}

pub fn run(fixtures: &[Fixture], probe: Probe) -> Report {
    support::isolate_process();
    let tmp = tempfile::Builder::new()
        .prefix("paneflow-oracle")
        .tempdir()
        .expect("oracle tempdir");
    let mut report = Report {
        compared: Vec::new(),
        skipped: Vec::new(),
        both_failed: Vec::new(),
        failures: Vec::new(),
    };
    for fixture in fixtures {
        let root = tmp.path().join(fixture.name).join("repo");
        let cwd = match (fixture.build)(&root) {
            Ok(cwd) => cwd,
            Err(Skip(reason)) => {
                report.skipped.push((fixture.name, reason));
                continue;
            }
        };
        let label = format!("{} ({} probe)", fixture.name, probe.name());
        match (cli_records(&cwd, probe), gix_records(&cwd, probe)) {
            (Ok(cli), Ok(gix)) => {
                report.failures.extend(missing_expectations(
                    fixture.name,
                    &cli,
                    fixture.must_include,
                ));
                report.failures.extend(divergences(&label, &cli, &gix));
            }
            (Err(cli), Err(gix)) => report
                .both_failed
                .push((fixture.name, format!("cli: {cli}; gix: {gix}"))),
            (Err(cli), Ok(gix)) => report.failures.push(format!(
                "fixture {label}: the CLI failed ({cli}) while gix answered {} records",
                gix.len()
            )),
            (Ok(cli), Err(gix)) => report.failures.push(format!(
                "fixture {label}: gix failed ({gix}) while the CLI answered {} records",
                cli.len()
            )),
        }
        report.compared.push(fixture.name);
    }
    report
}

pub fn git_with_stdin(cwd: &Path, args: &[&str], input: &[u8]) -> String {
    let mut child = support::command(cwd, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("git {args:?} in {}: {error}", cwd.display()));
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input)
        .expect("write stdin");
    let out = child.wait_with_output().expect("git output");
    assert!(
        out.status.success(),
        "git {args:?} in {} exited with {}: {}",
        cwd.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn hash_blob(cwd: &Path, content: &[u8]) -> String {
    git_with_stdin(cwd, &["hash-object", "-w", "--stdin"], content)
        .trim()
        .to_string()
}

pub fn symlink_file(target: &str, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
}
