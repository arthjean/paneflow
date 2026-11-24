use std::path::Path;

use paneflow_agent_config::runtime_catalog::{self, Runtime};
use paneflow_config::schema::SessionId;
use paneflow_peer::{Ancestor, LeaderStamp, Lineage, LineageReader};

const WRAPPERS: &[&str] = &[
    "node", "nodejs", "bun", "deno", "npx", "bunx", "pnpx", "uv", "uvx", "env", "python",
    "python3", "pythonw",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerVerdict {
    Agent {
        label: Option<String>,
        evidence: String,
    },
    User,
}

impl CallerVerdict {
    pub fn is_agent(&self) -> bool {
        matches!(self, Self::Agent { .. })
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Agent { label, .. } => label.as_deref(),
            Self::User => None,
        }
    }

    fn unproven(evidence: impl Into<String>) -> Self {
        Self::Agent {
            label: None,
            evidence: evidence.into(),
        }
    }
}

pub trait ProcessReader {
    fn argv(&self, pid: u32) -> Option<Vec<String>>;
    fn started_at(&self, pid: u32) -> Option<u64>;
}

pub struct LiveProcesses;

impl ProcessReader for LiveProcesses {
    fn argv(&self, pid: u32) -> Option<Vec<String>> {
        paneflow_host::process::process_argv(pid)
    }

    fn started_at(&self, pid: u32) -> Option<u64> {
        paneflow_peer::process_start_time(pid)
    }
}

fn stem(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs", ".cjs"]
        .iter()
        .find_map(|extension| lower.strip_suffix(extension))
        .map_or_else(|| lower.clone(), str::to_string)
}

fn runtime_named(name: &str) -> Option<&'static Runtime> {
    let name = stem(name);
    runtime_catalog::runtime_by_process_alias(&name)
        .or_else(|| runtime_catalog::runtime_by_command_alias(&name))
}

fn runtime_in_wrapper_argv(argv: &[String]) -> Option<&'static Runtime> {
    let wrapper = stem(argv.first()?);
    if !WRAPPERS.contains(&wrapper.as_str()) && !wrapper.starts_with("python") {
        return None;
    }
    let script = argv.iter().skip(1).find(|arg| !arg.starts_with('-'))?;
    runtime_named(script).or_else(|| runtime_catalog::runtime_by_script_path(script))
}

pub fn runtime_of(ancestor: &Ancestor, argv: Option<&[String]>) -> Option<&'static Runtime> {
    let named = [
        ancestor.name.as_deref(),
        ancestor
            .executable
            .as_deref()
            .and_then(Path::to_str)
            .filter(|path| !path.is_empty()),
        argv.and_then(<[String]>::first).map(String::as_str),
    ];
    named
        .into_iter()
        .flatten()
        .find_map(runtime_named)
        .or_else(|| argv.and_then(runtime_in_wrapper_argv))
}

pub fn judge(lineage: Lineage, processes: &impl ProcessReader) -> CallerVerdict {
    let chain = match lineage {
        Lineage::ToLeader(chain) => chain,
        Lineage::Broken { reason } => return CallerVerdict::unproven(reason),
    };
    for ancestor in &chain {
        let argv = processes.argv(ancestor.pid);
        if processes.started_at(ancestor.pid) != Some(ancestor.started_at) {
            return CallerVerdict::unproven(format!(
                "process {} ended while its parents were read",
                ancestor.pid
            ));
        }
        if let Some(runtime) = runtime_of(ancestor, argv.as_deref()) {
            return CallerVerdict::Agent {
                label: Some(runtime.label.to_string()),
                evidence: format!("process {} runs {}", ancestor.pid, runtime.label),
            };
        }
    }
    CallerVerdict::User
}

pub fn leader_of_session(home: &Path, session: &str) -> Result<LeaderStamp, String> {
    let session = SessionId::parse(session)
        .map_err(|error| format!("the pane's session id is malformed: {error}"))?;
    let path = paneflow_host::manifest::manifest_path(home, &session);
    let manifest = paneflow_host::manifest::read_manifest(&path)
        .map_err(|error| format!("the pane's session manifest cannot be read: {error}"))?;
    let process = manifest
        .process
        .ok_or_else(|| "the pane's session manifest records no shell".to_string())?;
    let started_at = process
        .started_at
        .ok_or_else(|| "the pane's session manifest records no shell start time".to_string())?;
    Ok(LeaderStamp {
        pid: process.pid,
        started_at,
    })
}

pub fn classify_caller(
    lineage: &impl LineageReader,
    processes: &impl ProcessReader,
    caller: (u32, u64),
    leader: Result<LeaderStamp, String>,
) -> CallerVerdict {
    match leader {
        Ok(leader) => judge(lineage.lineage(caller.0, caller.1, leader), processes),
        Err(reason) => CallerVerdict::unproven(reason),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;

    #[derive(Default)]
    struct Processes {
        argv: HashMap<u32, Vec<String>>,
        started: HashMap<u32, u64>,
    }

    impl ProcessReader for Processes {
        fn argv(&self, pid: u32) -> Option<Vec<String>> {
            self.argv.get(&pid).cloned()
        }

        fn started_at(&self, pid: u32) -> Option<u64> {
            self.started.get(&pid).copied()
        }
    }

    struct Fixed(Lineage);

    impl LineageReader for Fixed {
        fn lineage(&self, _: u32, _: u64, _: LeaderStamp) -> Lineage {
            self.0.clone()
        }
    }

    fn process(pid: u32, name: &str, executable: Option<&str>) -> Ancestor {
        Ancestor {
            pid,
            started_at: u64::from(pid),
            name: Some(name.into()),
            executable: executable.map(PathBuf::from),
        }
    }

    fn reader(chain: &[Ancestor], argv: &[(u32, &[&str])]) -> Processes {
        Processes {
            argv: argv
                .iter()
                .map(|(pid, args)| (*pid, args.iter().map(|arg| (*arg).to_string()).collect()))
                .collect(),
            started: chain
                .iter()
                .map(|ancestor| (ancestor.pid, ancestor.started_at))
                .collect(),
        }
    }

    const SHELL: LeaderStamp = LeaderStamp {
        pid: 100,
        started_at: 100,
    };

    fn verdict(chain: Vec<Ancestor>, argv: &[(u32, &[&str])]) -> CallerVerdict {
        let processes = reader(&chain, argv);
        classify_caller(
            &Fixed(Lineage::ToLeader(chain)),
            &processes,
            (400, 400),
            Ok(SHELL),
        )
    }

    #[test]
    fn a_command_started_by_a_native_or_a_node_agent_is_the_agent() {
        let native = verdict(
            vec![
                process(400, "paneflow", Some("/usr/bin/paneflow")),
                process(300, "bash", Some("/usr/bin/bash")),
                process(
                    200,
                    "claude",
                    Some("/home/u/.local/share/claude/versions/2.1.330"),
                ),
                process(100, "zsh", Some("/usr/bin/zsh")),
            ],
            &[],
        );
        assert_eq!(native.label(), Some("Claude Code"));
        let node = verdict(
            vec![
                process(400, "paneflow", None),
                process(200, "node", Some("/usr/bin/node")),
                process(100, "zsh", None),
            ],
            &[(
                200,
                &[
                    "node",
                    "--no-warnings",
                    "/usr/lib/node_modules/@openai/codex/bin/codex.js",
                ],
            )],
        );
        assert_eq!(node.label(), Some("Codex"));
        let windows = verdict(
            vec![
                process(400, "paneflow.exe", Some("C:\\Paneflow\\paneflow.exe")),
                process(
                    200,
                    "claude.exe",
                    Some("C:\\Users\\u\\.local\\bin\\claude.exe"),
                ),
                process(100, "pwsh.exe", None),
            ],
            &[],
        );
        assert_eq!(windows.label(), Some("Claude Code"));
    }

    #[test]
    fn a_command_the_user_typed_in_the_pane_shell_is_the_user() {
        assert_eq!(
            verdict(
                vec![
                    process(400, "paneflow", Some("/usr/bin/paneflow")),
                    process(100, "zsh", Some("/usr/bin/zsh")),
                ],
                &[(400, &["paneflow", "do", "claude.lua"]), (100, &["-zsh"])],
            ),
            CallerVerdict::User
        );
        assert_eq!(
            verdict(
                vec![
                    process(400, "paneflow", None),
                    process(300, "node", None),
                    process(100, "zsh", None)
                ],
                &[(300, &["node", "build.js"])],
            ),
            CallerVerdict::User
        );
    }

    #[test]
    fn an_unproven_chain_or_shell_counts_as_an_agent() {
        let broken = classify_caller(
            &Fixed(Lineage::Broken {
                reason: "cannot read the parent 200".into(),
            }),
            &Processes::default(),
            (400, 400),
            Ok(SHELL),
        );
        assert!(broken.is_agent());
        assert_eq!(broken.label(), None);
        let no_shell = classify_caller(
            &Fixed(Lineage::ToLeader(Vec::new())),
            &Processes::default(),
            (400, 400),
            Err("the pane's session manifest cannot be read".into()),
        );
        assert!(no_shell.is_agent());
        let chain = vec![process(400, "paneflow", None), process(100, "zsh", None)];
        let mut processes = reader(&chain, &[]);
        processes.started.insert(400, 7);
        assert!(
            classify_caller(
                &Fixed(Lineage::ToLeader(chain)),
                &processes,
                (400, 400),
                Ok(SHELL)
            )
            .is_agent(),
            "a process that ended while it was read is an agent"
        );
    }
}
