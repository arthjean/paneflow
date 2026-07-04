//! Agent PID → surface resolution (US-017, prd-orchestration-v2).
//!
//! The `ai.*` hooks report the AGENT process's PID; a pane only knows its
//! direct PTY child (`terminal.child_pid`). When the agent was launched from
//! an interactive shell the agent is a grand-child (or deeper), so the link
//! is materialized by walking the parent-PID chain from the agent up until a
//! known `child_pid` is hit. Per-OS parent lookup mirrors `ports.rs`:
//! Linux reads `/proc/<pid>/stat`, macOS asks `libproc`, Windows uses a
//! ToolHelp process snapshot. An unresolved PID degrades gracefully to the
//! workspace-level badge, never to a wrong pane.
//!
//! The walk does I/O (`/proc` reads) - callers run it OFF the render thread
//! (`smol::unblock`) and deposit the result back on the main thread.

use std::collections::HashMap;

/// Hard bound on the ancestor walk. Realistic chains are 2-4 deep (pane
/// shell → wrapper → agent); 32 guards against a pathological or cyclic
/// (PID-reuse race) chain.
const MAX_DEPTH: usize = 32;

/// Resolve `pid` to a surface id by walking its ancestor chain against the
/// `child_pid → surface_id` candidate map. Pure walk - the platform lookup
/// is injected so the rule is unit-testable with a mocked process tree.
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
            // Stop at init/reaper (1) or a self-parent (defensive: a mocked
            // or corrupt chain must not spin to MAX_DEPTH).
            Some(parent) if parent > 1 && parent != current => current = parent,
            _ => return None,
        }
    }
    None
}

/// Platform resolution: walk the real process tree.
pub fn resolve_surface_for_pid(pid: u32, candidates: &HashMap<u32, u64>) -> Option<u64> {
    resolve_with(pid, candidates, parent_of)
}

#[cfg(target_os = "linux")]
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat_ppid(&stat)
}

/// Extract the ppid (field 4) from `/proc/<pid>/stat`. The comm field
/// (field 2) is parenthesized and may itself contain spaces, parens or
/// newlines, so fields are taken AFTER the LAST `)` - the kernel-documented
/// safe parse (proc(5)).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat_ppid(stat: &str) -> Option<u32> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    // after_comm = " R 1234 ..." → [state, ppid, …]
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

    // SAFETY: Win32 call. A successful snapshot handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut parent = None;
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is valid, and `entry` has the documented size.
    if unsafe { Process32FirstW(snap, &mut entry) } != 0 {
        loop {
            if entry.th32ProcessID == pid {
                parent = Some(entry.th32ParentProcessID);
                break;
            }
            // SAFETY: same invariants as Process32FirstW.
            if unsafe { Process32NextW(snap, &mut entry) } == 0 {
                break;
            }
        }
    }
    // SAFETY: `snap` is a valid handle returned above.
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
        // No edges needed: pid 100 IS the pane child (fast path for `up`).
        assert_eq!(resolve_with(100, &candidates, |_| None), Some(7));
    }

    #[test]
    fn grandchild_resolves_through_the_chain() {
        // pane shell 100 → wrapper 200 → agent 300.
        let mut candidates = HashMap::new();
        candidates.insert(100, 7u64);
        let edges = [(300, 200), (200, 100)];
        assert_eq!(resolve_with(300, &candidates, tree(&edges)), Some(7));
    }

    #[test]
    fn chain_ending_at_init_is_unresolved() {
        let candidates = HashMap::from([(100u32, 7u64)]);
        // Agent re-parented to init (orphan): 300 → 1.
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
        // comm may contain spaces AND parens - fields come after the LAST ')'.
        assert_eq!(
            parse_stat_ppid("300 (my (weird) comm) S 200 300 1"),
            Some(200)
        );
        assert_eq!(parse_stat_ppid("42 (bash) S 7 42 7"), Some(7));
        assert_eq!(parse_stat_ppid("garbage"), None);
    }
}
