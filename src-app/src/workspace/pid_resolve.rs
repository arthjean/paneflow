use std::collections::HashMap;

const MAX_DEPTH: usize = 32;

pub fn resolve_with(
    pid: u32,
    candidates: &HashMap<u32, u64>,
    parent_of: impl Fn(u32) -> Option<u32>,
) -> Option<u64> {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if let Some(&sid) = candidates.get(&current) {
            return Some(sid);
        }
        match parent_of(current) {
            Some(parent) if parent > 1 && parent != current => current = parent,
            _ => return None,
        }
    }
    None
}

pub fn resolve_surface_for_pid(pid: u32, candidates: &HashMap<u32, u64>) -> Option<u64> {
    resolve_with(pid, candidates, parent_of)
}

#[cfg(target_os = "linux")]
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat_ppid(&stat)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat_ppid(stat: &str) -> Option<u32> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn parent_of(pid: u32) -> Option<u32> {
    use libproc::libproc::bsd_info::BSDInfo;
    use libproc::libproc::proc_pid::pidinfo;
    pidinfo::<BSDInfo>(pid as i32, 0)
        .ok()
        .map(|info| info.pbi_ppid)
}

#[cfg(windows)]
fn parent_of(pid: u32) -> Option<u32> {
    use std::mem;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    if pid == 0 {
        return None;
    }

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut parent = None;
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snap, &mut entry) } != 0 {
        loop {
            if entry.th32ProcessID == pid {
                parent = Some(entry.th32ParentProcessID);
                break;
            }
            if unsafe { Process32NextW(snap, &mut entry) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snap) };
    parent.filter(|p| *p > 0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn parent_of(_pid: u32) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(edges: &[(u32, u32)]) -> impl Fn(u32) -> Option<u32> + '_ {
        move |pid| edges.iter().find(|(c, _)| *c == pid).map(|(_, p)| *p)
    }

    #[test]
    fn direct_child_resolves_without_walking() {
        let mut candidates = HashMap::new();
        candidates.insert(100, 7u64);
        assert_eq!(resolve_with(100, &candidates, |_| None), Some(7));
    }

    #[test]
    fn grandchild_resolves_through_the_chain() {
        let mut candidates = HashMap::new();
        candidates.insert(100, 7u64);
        let edges = [(300, 200), (200, 100)];
        assert_eq!(resolve_with(300, &candidates, tree(&edges)), Some(7));
    }

    #[test]
    fn chain_ending_at_init_is_unresolved() {
        let candidates = HashMap::from([(100u32, 7u64)]);
        let edges = [(300, 1)];
        assert_eq!(resolve_with(300, &candidates, tree(&edges)), None);
    }

    #[test]
    fn self_parent_cycle_terminates_unresolved() {
        let candidates = HashMap::from([(100u32, 7u64)]);
        let edges = [(300, 300)];
        assert_eq!(resolve_with(300, &candidates, tree(&edges)), None);
    }

    #[test]
    fn parse_stat_ppid_survives_hostile_comm() {
        assert_eq!(
            parse_stat_ppid("300 (my (weird) comm) S 200 300 1"),
            Some(200)
        );
        assert_eq!(parse_stat_ppid("42 (bash) S 7 42 7"), Some(7));
        assert_eq!(parse_stat_ppid("garbage"), None);
    }
}
