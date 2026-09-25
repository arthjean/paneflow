use paneflow_agent_config::runtime_catalog::{self, Runtime};
use serde::{Deserialize, Serialize};

use crate::process::ProcessIdentity;

const MAX_OBSERVED_ARGV_ITEMS: usize = 8;
const MAX_OBSERVED_ARGV_BYTES: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeObservation {
    pub id: String,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_started_at: Option<u64>,
    #[serde(default)]
    pub process_group: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub process_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
}

impl RuntimeObservation {
    pub fn identity(&self) -> String {
        format!(
            "{}:{}:{}",
            self.id,
            self.pid,
            self.pid_started_at.unwrap_or_default()
        )
    }

    pub fn runtime(&self) -> Option<&'static Runtime> {
        runtime_catalog::runtime_by_id(&self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForegroundProcess {
    pid: u32,
    parent_pid: u32,
    process_group: u32,
    started_at: Option<u64>,
    name: String,
    argv: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForegroundJob {
    process_group: u32,
    processes: Vec<ForegroundProcess>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchStrength {
    Wrapper,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeMatch {
    runtime_id: &'static str,
    evidence_argv_len: usize,
    strength: MatchStrength,
}

pub fn observe_foreground_runtime(
    session_leader: ProcessIdentity,
    foreground_process_group: Option<i32>,
) -> Option<RuntimeObservation> {
    if session_leader.pid <= 1 || !session_leader.is_provably_live() {
        return None;
    }
    let job = platform::foreground_job(session_leader.pid, foreground_process_group)?;
    if !session_leader.is_provably_live() {
        return None;
    }
    identify_runtime_in_job(&job)
}

fn identify_runtime_in_job(job: &ForegroundJob) -> Option<RuntimeObservation> {
    if let Some(leader) = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group)
        && let Some(runtime_match) = match_process(leader)
    {
        return Some(observation(leader, runtime_match));
    }

    let mut best: Option<(MatchStrength, usize, u32, &ForegroundProcess, RuntimeMatch)> = None;
    for process in &job.processes {
        let Some(runtime_match) = match_process(process) else {
            continue;
        };
        let depth =
            ancestry_depth(process.pid, job.process_group, &job.processes).unwrap_or(usize::MAX);
        let candidate = (
            runtime_match.strength,
            usize::MAX.saturating_sub(depth),
            u32::MAX.saturating_sub(process.pid),
            process,
            runtime_match,
        );
        if best.as_ref().is_none_or(|current| {
            (candidate.0, candidate.1, candidate.2) > (current.0, current.1, current.2)
        }) {
            best = Some(candidate);
        }
    }

    best.map(|(_, _, _, process, runtime_match)| observation(process, runtime_match))
}

fn observation(process: &ForegroundProcess, runtime_match: RuntimeMatch) -> RuntimeObservation {
    let argv = (runtime_match.strength == MatchStrength::Direct)
        .then(|| bounded_evidence_argv(process.argv.as_deref(), runtime_match.evidence_argv_len))
        .flatten();
    RuntimeObservation {
        id: runtime_match.runtime_id.to_string(),
        pid: process.pid,
        pid_started_at: process.started_at,
        process_group: process.process_group,
        process_name: normalized_executable_name(&process.name),
        argv,
    }
}

fn ancestry_depth(pid: u32, ancestor: u32, processes: &[ForegroundProcess]) -> Option<usize> {
    let mut current = pid;
    for depth in 0..=processes.len() {
        if current == ancestor {
            return Some(depth);
        }
        let process = processes
            .iter()
            .find(|candidate| candidate.pid == current)?;
        if process.parent_pid == 0 || process.parent_pid == current {
            return None;
        }
        current = process.parent_pid;
    }
    None
}

fn match_process(process: &ForegroundProcess) -> Option<RuntimeMatch> {
    if let Some(runtime_id) = runtime_id_for_executable(&process.name) {
        return Some(RuntimeMatch {
            runtime_id,
            evidence_argv_len: process.argv.as_ref().map_or(0, |_| 1),
            strength: MatchStrength::Direct,
        });
    }
    let argv = process.argv.as_deref()?;
    if let Some(runtime_id) = argv.first().and_then(|arg| runtime_id_for_executable(arg)) {
        return Some(RuntimeMatch {
            runtime_id,
            evidence_argv_len: 1,
            strength: MatchStrength::Direct,
        });
    }
    let (runtime_id, evidence_argv_len) = runtime_from_wrapper_argv(argv)?;
    Some(RuntimeMatch {
        runtime_id,
        evidence_argv_len,
        strength: MatchStrength::Wrapper,
    })
}

fn supported(runtime: &'static Runtime) -> Option<&'static str> {
    runtime.supports_current_platform().then_some(runtime.id)
}

fn runtime_id_for_executable(executable: &str) -> Option<&'static str> {
    let executable = normalized_executable_name(executable);
    runtime_catalog::runtime_by_process_alias(&executable).and_then(supported)
}

fn runtime_id_from_script_path(path: &str) -> Option<&'static str> {
    if let Some(runtime_id) = runtime_id_for_executable(path) {
        return Some(runtime_id);
    }
    runtime_catalog::runtime_by_script_path(path).and_then(supported)
}

fn runtime_from_wrapper_argv(argv: &[String]) -> Option<(&'static str, usize)> {
    let wrapper = normalized_executable_name(argv.first()?);
    match wrapper.as_str() {
        "node" | "bun" => runtime_from_script_argv(argv, &["-e", "--eval", "-p", "--print"]),
        name if is_python_runtime(name) => runtime_from_script_argv(argv, &["-c", "-m"]),
        "sh" | "bash" | "zsh" | "fish" => runtime_from_shell_argv(argv),
        "env" | "command" => runtime_from_prefix_argv(argv),
        "npx" | "bunx" => runtime_from_package_runner_argv(argv),
        _ => None,
    }
}

fn runtime_from_script_argv(
    argv: &[String],
    ambiguous_execution_flags: &[&str],
) -> Option<(&'static str, usize)> {
    let mut index = 1;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            index += 1;
            return argv
                .get(index)
                .and_then(|path| runtime_id_from_script_path(path))
                .map(|runtime_id| (runtime_id, index + 1));
        }
        if ambiguous_execution_flags.iter().any(|flag| {
            arg == *flag
                || (flag.starts_with("--") && arg.starts_with(&format!("{flag}=")))
                || (!flag.starts_with("--") && arg.starts_with(flag) && arg.len() > flag.len())
        }) {
            return None;
        }
        if arg.starts_with('-') {
            if runtime_option_takes_value(arg) {
                index += 1;
            }
            index += 1;
            continue;
        }
        return runtime_id_from_script_path(arg).map(|runtime_id| (runtime_id, index + 1));
    }
    None
}

fn runtime_from_shell_argv(argv: &[String]) -> Option<(&'static str, usize)> {
    let command_index =
        argv.iter().enumerate().skip(1).find_map(|(index, arg)| {
            matches!(arg.as_str(), "-c" | "--command").then_some(index + 1)
        })?;
    let command = argv.get(command_index)?;
    let token = first_shell_token(command)?;
    runtime_id_for_executable(token).map(|runtime_id| (runtime_id, command_index + 1))
}

fn runtime_from_prefix_argv(argv: &[String]) -> Option<(&'static str, usize)> {
    for (index, arg) in argv.iter().enumerate().skip(1) {
        if arg == "--" || arg.starts_with('-') || is_environment_assignment(arg) {
            continue;
        }
        return runtime_id_for_executable(arg).map(|runtime_id| (runtime_id, index + 1));
    }
    None
}

fn runtime_from_package_runner_argv(argv: &[String]) -> Option<(&'static str, usize)> {
    for (index, arg) in argv.iter().enumerate().skip(1) {
        if arg == "--" || arg.starts_with('-') {
            continue;
        }
        return runtime_id_from_script_path(arg).map(|runtime_id| (runtime_id, index + 1));
    }
    None
}

pub(crate) fn normalized_executable_name(executable: &str) -> String {
    let basename = executable
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(executable)
        .trim_matches(|character| matches!(character, '\'' | '"'))
        .trim_start_matches('-')
        .to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if let Some(stripped) = basename.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    basename
}

fn is_python_runtime(name: &str) -> bool {
    name == "python"
        || name.strip_prefix("python").is_some_and(|version| {
            !version.is_empty()
                && version
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        })
}

fn runtime_option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "-r" | "--require"
            | "--loader"
            | "--import"
            | "--experimental-loader"
            | "--inspect-port"
            | "-W"
            | "-X"
            | "-S"
            | "-L"
            | "-o"
    )
}

fn first_shell_token(command: &str) -> Option<&str> {
    let command = command.trim_start();
    let first = command.chars().next()?;
    if matches!(first, '\'' | '"') {
        let start = first.len_utf8();
        let end = command[start..]
            .find(first)
            .map(|offset| start + offset)
            .unwrap_or(command.len());
        return command.get(start..end).filter(|token| !token.is_empty());
    }
    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    command.get(..end).filter(|token| !token.is_empty())
}

fn is_environment_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn bounded_evidence_argv(argv: Option<&[String]>, evidence_len: usize) -> Option<Vec<String>> {
    let argv = argv?;
    let mut remaining = MAX_OBSERVED_ARGV_BYTES;
    let mut bounded = Vec::new();
    for arg in argv.iter().take(evidence_len.min(MAX_OBSERVED_ARGV_ITEMS)) {
        if remaining == 0 {
            break;
        }
        let mut end = arg.len().min(remaining);
        while !arg.is_char_boundary(end) {
            end -= 1;
        }
        bounded.push(arg[..end].to_string());
        remaining = remaining.saturating_sub(end);
    }
    (!bounded.is_empty()).then_some(bounded)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::collections::{HashSet, VecDeque};

    use super::{ForegroundJob, ForegroundProcess};

    struct ProcessStat {
        parent_pid: u32,
        process_group: u32,
        session: u32,
        name: String,
    }

    pub(super) fn foreground_job(
        session_leader_pid: u32,
        foreground_process_group: Option<i32>,
    ) -> Option<ForegroundJob> {
        let process_group = u32::try_from(foreground_process_group?)
            .ok()
            .filter(|group| *group > 1)?;
        let mut processes = Vec::new();
        for pid in process_tree_pids([session_leader_pid, process_group]) {
            let Some(stat) = process_stat(pid) else {
                continue;
            };
            if stat.process_group != process_group || stat.session != session_leader_pid {
                continue;
            }
            let started_at = crate::process::process_start_time(pid);
            if started_at.is_none() {
                continue;
            }
            let argv = crate::process::process_argv(pid);
            if crate::process::process_start_time(pid) != started_at {
                continue;
            }
            processes.push(ForegroundProcess {
                pid,
                parent_pid: stat.parent_pid,
                process_group: stat.process_group,
                started_at,
                name: stat.name,
                argv,
            });
        }
        (!processes.is_empty()).then_some(ForegroundJob {
            process_group,
            processes,
        })
    }

    fn process_tree_pids(roots: impl IntoIterator<Item = u32>) -> Vec<u32> {
        let mut pending = VecDeque::new();
        let mut visited = HashSet::new();
        for pid in roots {
            if pid > 1 && visited.insert(pid) {
                pending.push_back(pid);
            }
        }
        let mut pids = Vec::new();
        while let Some(pid) = pending.pop_front() {
            pids.push(pid);
            for task_id in process_task_ids(pid) {
                for child_pid in process_task_children(pid, task_id) {
                    if child_pid > 1 && visited.insert(child_pid) {
                        pending.push_back(child_pid);
                    }
                }
            }
        }
        pids
    }

    fn process_task_ids(pid: u32) -> Vec<u32> {
        std::fs::read_dir(format!("/proc/{pid}/task"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
            .collect()
    }

    fn process_task_children(pid: u32, task_id: u32) -> Vec<u32> {
        std::fs::read_to_string(format!("/proc/{pid}/task/{task_id}/children"))
            .ok()
            .into_iter()
            .flat_map(|children| {
                children
                    .split_whitespace()
                    .filter_map(|child| child.parse().ok())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn process_stat(pid: u32) -> Option<ProcessStat> {
        parse_process_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    fn parse_process_stat(stat: &str) -> Option<ProcessStat> {
        let open = stat.find('(')?;
        let close = stat.rfind(')')?;
        let name = stat.get(open + 1..close)?.to_string();
        let fields = stat
            .get(close + 2..)?
            .split_whitespace()
            .collect::<Vec<_>>();
        Some(ProcessStat {
            parent_pid: fields.get(1)?.parse().ok()?,
            process_group: fields.get(2)?.parse().ok()?,
            session: fields.get(3)?.parse().ok()?,
            name,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_stat_parser_survives_a_name_with_spaces_and_parentheses() {
            let parsed =
                parse_process_stat("123 (name with ) paren) S 7 456 789 0 456").expect("stat");
            assert_eq!(parsed.name, "name with ) paren");
            assert_eq!(parsed.parent_pid, 7);
            assert_eq!(parsed.process_group, 456);
            assert_eq!(parsed.session, 789);
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{ForegroundJob, ForegroundProcess};

    const PROC_PGRP_ONLY: u32 = 2;

    pub(super) fn foreground_job(
        session_leader_pid: u32,
        foreground_process_group: Option<i32>,
    ) -> Option<ForegroundJob> {
        let process_group = u32::try_from(foreground_process_group?)
            .ok()
            .filter(|group| *group > 1)?;
        let mut processes = Vec::new();
        for pid in process_group_pids(process_group) {
            let Some(info) = process_bsdinfo(pid) else {
                continue;
            };
            if info.pbi_pgid != process_group
                || unsafe { libc::getsid(pid as libc::pid_t) } != session_leader_pid as libc::pid_t
            {
                continue;
            }
            let started_at = crate::process::process_start_time(pid);
            if started_at.is_none() {
                continue;
            }
            let Some(name) = process_name(&info) else {
                continue;
            };
            let argv = crate::process::process_argv(pid);
            if crate::process::process_start_time(pid) != started_at {
                continue;
            }
            processes.push(ForegroundProcess {
                pid,
                parent_pid: info.pbi_ppid,
                process_group: info.pbi_pgid,
                started_at,
                name,
                argv,
            });
        }
        (!processes.is_empty()).then_some(ForegroundJob {
            process_group,
            processes,
        })
    }

    fn process_group_pids(process_group: u32) -> Vec<u32> {
        let mut capacity = 16usize;
        for _ in 0..8 {
            let mut pids = vec![0 as libc::pid_t; capacity];
            let buffer_bytes = pids.len() * std::mem::size_of::<libc::pid_t>();
            let returned_bytes = unsafe {
                libc::proc_listpids(
                    PROC_PGRP_ONLY,
                    process_group,
                    pids.as_mut_ptr().cast::<libc::c_void>(),
                    buffer_bytes as libc::c_int,
                )
            };
            if returned_bytes <= 0 {
                return Vec::new();
            }
            let returned_bytes = returned_bytes as usize;
            if returned_bytes < buffer_bytes {
                pids.truncate(returned_bytes / std::mem::size_of::<libc::pid_t>());
                return pids
                    .into_iter()
                    .filter_map(|pid| u32::try_from(pid).ok())
                    .filter(|pid| *pid > 1)
                    .collect();
            }
            capacity = capacity.saturating_mul(2);
        }
        Vec::new()
    }

    fn process_bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let returned = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast::<libc::c_void>(),
                size,
            )
        };
        (returned == size).then(|| unsafe { info.assume_init() })
    }

    fn process_name(info: &libc::proc_bsdinfo) -> Option<String> {
        let end = info
            .pbi_comm
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(info.pbi_comm.len());
        (end > 0).then(|| {
            let bytes = info.pbi_comm[..end]
                .iter()
                .map(|byte| *byte as u8)
                .collect::<Vec<_>>();
            String::from_utf8_lossy(&bytes).into_owned()
        })
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::{HashSet, VecDeque};

    use super::{ForegroundJob, ForegroundProcess};

    const MAX_OBSERVED_PROCESSES: usize = 64;

    pub(super) fn foreground_job(
        session_leader_pid: u32,
        _foreground_process_group: Option<i32>,
    ) -> Option<ForegroundJob> {
        let entries = crate::process::windows_process_entries_named().ok()?;
        let mut processes = Vec::new();
        for entry in descendants(session_leader_pid, &entries) {
            let started_at = crate::process::process_start_time(entry.pid);
            if started_at.is_none() {
                continue;
            }
            let argv = crate::process::process_argv(entry.pid);
            if crate::process::process_start_time(entry.pid) != started_at {
                continue;
            }
            processes.push(ForegroundProcess {
                pid: entry.pid,
                parent_pid: entry.parent_pid,
                process_group: session_leader_pid,
                started_at,
                name: entry.name,
                argv,
            });
        }
        (!processes.is_empty()).then_some(ForegroundJob {
            process_group: session_leader_pid,
            processes,
        })
    }

    fn descendants(
        root_pid: u32,
        entries: &[crate::process::WindowsProcessEntry],
    ) -> Vec<crate::process::WindowsProcessEntry> {
        let mut collected = Vec::new();
        let mut visited = HashSet::new();
        let mut pending = VecDeque::from([root_pid]);
        visited.insert(root_pid);
        while let Some(pid) = pending.pop_front() {
            if collected.len() >= MAX_OBSERVED_PROCESSES {
                break;
            }
            if let Some(entry) = entries.iter().find(|entry| entry.pid == pid) {
                collected.push(entry.clone());
            }
            for entry in entries.iter().filter(|entry| entry.parent_pid == pid) {
                if visited.insert(entry.pid) {
                    pending.push_back(entry.pid);
                }
            }
        }
        collected
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::ForegroundJob;

    pub(super) fn foreground_job(
        _session_leader_pid: u32,
        _foreground_process_group: Option<i32>,
    ) -> Option<ForegroundJob> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent_pid: u32, name: &str, argv: &[&str]) -> ForegroundProcess {
        ForegroundProcess {
            pid,
            parent_pid,
            process_group: 100,
            started_at: Some(1_000 + u64::from(pid)),
            name: name.to_string(),
            argv: Some(argv.iter().map(|arg| (*arg).to_string()).collect()),
        }
    }

    fn job(processes: Vec<ForegroundProcess>) -> ForegroundJob {
        ForegroundJob {
            process_group: 100,
            processes,
        }
    }

    const CLAUDE: &str = "com.anthropic.claude-code";
    const CODEX: &str = "com.openai.codex";

    #[test]
    fn the_group_leader_wins_over_a_deeper_direct_match() {
        let observed = identify_runtime_in_job(&job(vec![
            process(100, 1, "claude", &["claude"]),
            process(140, 100, "codex", &["codex"]),
        ]))
        .expect("the leader is a runtime");
        assert_eq!(observed.id, CLAUDE);
        assert_eq!(observed.pid, 100);
    }

    #[test]
    fn without_a_leader_match_strength_then_depth_then_pid_decide() {
        let observed = identify_runtime_in_job(&job(vec![
            process(100, 1, "bash", &["bash"]),
            process(
                120,
                100,
                "node",
                &["node", "/opt/node_modules/@openai/codex/bin/codex.js"],
            ),
            process(130, 100, "claude", &["claude", "--resume"]),
        ]))
        .expect("a direct match beats a wrapper match");
        assert_eq!(observed.id, CLAUDE);
        assert_eq!(observed.pid, 130);

        let shallow = identify_runtime_in_job(&job(vec![
            process(100, 1, "bash", &["bash"]),
            process(160, 100, "codex", &["codex"]),
            process(150, 160, "codex", &["codex"]),
        ]))
        .expect("the shallower match wins");
        assert_eq!(shallow.pid, 160);

        let lowest = identify_runtime_in_job(&job(vec![
            process(100, 1, "bash", &["bash"]),
            process(170, 100, "codex", &["codex"]),
            process(160, 100, "codex", &["codex"]),
        ]))
        .expect("the lowest pid breaks the tie");
        assert_eq!(lowest.pid, 160);
    }

    #[test]
    fn an_npm_install_under_node_is_a_wrapper_match_without_argv_evidence() {
        let observed = identify_runtime_in_job(&job(vec![
            process(100, 1, "cmd", &["cmd.exe", "/c", "claude.cmd"]),
            process(
                180,
                100,
                "node.exe",
                &[
                    r"C:\Program Files\nodejs\node.exe",
                    r"C:\Users\a\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js",
                    "--dangerously-skip-permissions",
                ],
            ),
        ]))
        .expect("the npm install is recognized through its script path");
        assert_eq!(observed.id, CLAUDE);
        assert_eq!(observed.pid, 180);
        assert_eq!(observed.process_name, "node");
        assert_eq!(observed.argv, None);
    }

    #[test]
    fn every_declared_wrapper_reaches_the_script_signature() {
        for argv in [
            vec!["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"],
            vec!["bun", "/usr/lib/node_modules/@openai/codex/bin/codex.js"],
            vec!["python3", "/opt/@openai/codex/main.py"],
            vec!["env", "CODEX_HOME=/tmp", "codex"],
            vec!["npx", "@openai/codex"],
            vec!["sh", "-c", "codex --search"],
        ] {
            let argv: Vec<String> = argv.into_iter().map(str::to_string).collect();
            assert_eq!(
                runtime_from_wrapper_argv(&argv).map(|(id, _)| id),
                Some(CODEX),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn an_ambiguous_interpreter_flag_never_names_a_runtime() {
        for argv in [
            vec!["node", "-e", "require('@openai/codex')"],
            vec!["node", "--eval=@openai/codex"],
            vec!["python3", "-c", "import codex"],
            vec!["python3", "-m", "codex"],
            vec!["node", "/opt/tools/unrelated.js"],
        ] {
            let argv: Vec<String> = argv.into_iter().map(str::to_string).collect();
            assert_eq!(runtime_from_wrapper_argv(&argv), None, "{argv:?}");
        }
    }

    #[test]
    fn direct_argv_evidence_stops_at_the_matching_cell_and_stays_bounded() {
        let observed = identify_runtime_in_job(&job(vec![process(
            100,
            1,
            "claude",
            &["claude", "--print", "a very long user prompt"],
        )]))
        .expect("a direct match");
        assert_eq!(observed.argv, Some(vec!["claude".to_string()]));

        let long = vec!["x".repeat(4_000)];
        let bounded = bounded_evidence_argv(Some(&long), 4).expect("bounded evidence");
        assert_eq!(bounded.len(), 1);
        assert_eq!(bounded[0].len(), MAX_OBSERVED_ARGV_BYTES);

        let many: Vec<String> = (0..20).map(|index| format!("arg{index}")).collect();
        let capped = bounded_evidence_argv(Some(&many), 20).expect("bounded evidence");
        assert_eq!(capped.len(), MAX_OBSERVED_ARGV_ITEMS);
    }

    #[test]
    fn a_generic_interpreter_never_becomes_the_observed_runtime() {
        assert_eq!(
            identify_runtime_in_job(&job(vec![
                process(100, 1, "bash", &["bash"]),
                process(110, 100, "node", &["node", "server.js"]),
                process(120, 100, "fx", &["fx", "payload.json"]),
            ])),
            None
        );
    }

    #[test]
    fn a_recycled_session_leader_pid_yields_no_observation() {
        let leader = ProcessIdentity {
            pid: std::process::id(),
            started_at: Some(u64::MAX),
        };
        assert_eq!(observe_foreground_runtime(leader, Some(1_234)), None);
        let unverifiable = ProcessIdentity {
            pid: std::process::id(),
            started_at: None,
        };
        assert_eq!(observe_foreground_runtime(unverifiable, Some(1_234)), None);
    }

    #[test]
    fn the_identity_string_carries_the_runtime_and_its_process() {
        let observed = RuntimeObservation {
            id: CLAUDE.to_string(),
            pid: 10,
            pid_started_at: Some(5),
            process_group: 10,
            process_name: "claude".to_string(),
            argv: None,
        };
        assert_eq!(observed.identity(), "com.anthropic.claude-code:10:5");
        assert_eq!(
            observed.runtime().map(|runtime| runtime.slug),
            Some("claude-code")
        );
    }

    #[test]
    fn the_platform_layer_enumerates_the_live_foreground_job() {
        #[cfg(windows)]
        let job = platform::foreground_job(std::process::id(), None);
        #[cfg(unix)]
        let job = {
            let session = unsafe { libc::getsid(0) };
            let group = unsafe { libc::getpgrp() };
            platform::foreground_job(session as u32, Some(group))
        };
        #[cfg(not(any(windows, unix)))]
        let job: Option<ForegroundJob> = platform::foreground_job(std::process::id(), None);

        #[cfg(any(windows, unix))]
        {
            let job = job.expect("the test process sits in an enumerable foreground job");
            assert!(
                job.processes
                    .iter()
                    .any(|process| process.pid == std::process::id()),
                "the scan sees the live process it was asked about"
            );
            assert!(
                job.processes
                    .iter()
                    .all(|process| process.started_at.is_some()),
                "every enumerated process carries a kernel start time"
            );
        }
        #[cfg(not(any(windows, unix)))]
        assert!(job.is_none());
    }

    #[test]
    fn executable_names_normalize_across_platform_suffixes() {
        for raw in [
            r"C:\Program Files\claude\claude.exe",
            "/usr/local/bin/claude",
            "-claude",
            "\"claude.cmd\"",
        ] {
            assert_eq!(normalized_executable_name(raw), "claude", "{raw}");
        }
    }
}
