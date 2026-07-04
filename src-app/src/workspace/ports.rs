//! Per-pane TCP listening-port + agent-process detection (EP-005 US-012).
//!
//! One entry point, [`scan_panes`]: given `(terminal_key, root_pid)` pairs,
//! it returns a per-terminal [`PaneScan`] attributing LISTEN ports and
//! recognised agent binaries to each terminal's PTY process subtree. Each
//! port carries an OS-side frontend classification ([`PortEntry`]) derived
//! from the socket-owning process's argv - the sidebar's clickable chips key
//! off this, not off PTY-text scraping (which is timing-dependent and stays
//! enrichment-only: exact URLs, backend labels).
//!
//! Cost contract (US-012): the process table is traversed ONCE per tick -
//! a shared `visited` set spans all roots so no pid is walked twice, each
//! pid's `comm` is read at most once, and `/proc/net/tcp[6]` is parsed a
//! single time for the whole scan (the pre-refactor code re-walked the
//! descendants once for ports and once for agents, so this is strictly
//! cheaper per tick at any pane count).
//!
//! Three platform branches:
//! - **Linux** - `/proc/{pid}/task/{pid}/children` BFS, `/proc/{pid}/comm`,
//!   `/proc/{pid}/fd` socket inodes cross-referenced with `/proc/net/tcp[6]`.
//! - **macOS** - `libc::proc_listchildpids` BFS, `libproc` name +
//!   `listpidinfo::<ListFDs>`/`pidfdinfo::<SocketFDInfo>` (naturally
//!   per-pid, so per-subtree attribution needs no global socket table).
//! - **Windows** - ToolHelp process snapshot BFS plus `GetExtendedTcpTable`
//!   owner-PID tables for IPv4 and IPv6 LISTEN sockets.
//! - **Everything else (BSDs)** - stub returning an empty map; the sidebar
//!   chips and tab badges degrade to absent without error.
//!
//! BFS (not DFS) ordering is load-bearing: US-013 picks the agent binary
//! NEAREST the subtree root ("the agent you launched, not its children"),
//! which is exactly breadth-first visit order. Both walkers cap at 512 PIDs
//! per root to bound memory on fork-bombs.

#[cfg(target_os = "linux")]
use super::git::read_capped;

/// One LISTEN port owned by a terminal's subtree.
#[derive(Debug, Clone, PartialEq)]
pub struct PortEntry {
    pub port: u16,
    /// `Some(display_label)` when the socket-owning process's argv matches a
    /// known frontend dev server (Vite, Next.js, …). The OS-side classifier
    /// sees the actual socket owner, so chip clickability no longer depends
    /// on the PTY text scrape having caught the announcement line inside its
    /// scan window.
    pub frontend: Option<&'static str>,
}

/// Per-terminal scan result (EP-005 US-012).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneScan {
    /// LISTEN ports owned by the terminal's subtree, sorted by port number
    /// and deduplicated (a dual-stack v4+v6 bind is one entry).
    pub ports: Vec<PortEntry>,
    /// Recognised agent binary names found in the subtree, in BFS
    /// (root-proximity) order, deduplicated. `first()` is the pane's
    /// identity-pill agent (US-013); the union across panes feeds the
    /// workspace-level `detected_agents` aggregate.
    pub agents: Vec<String>,
    /// Best-effort foreground command for surface naming, resolved by the same
    /// off-thread process scan so IPC/UI callers never do process-table I/O.
    pub foreground_command: Option<String>,
}

/// Soft cap on PIDs walked per root subtree (fork-bomb bound, both
/// platforms). Checked at dequeue time, so one last fanout batch can
/// overshoot it by up to one process's child count - the bound is
/// "≈512", which is all the memory guarantee needs.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
const MAX_PIDS_PER_ROOT: usize = 512;

// ---------------------------------------------------------------------------
// Platform-neutral pure helpers (unit-tested on every host)
// ---------------------------------------------------------------------------

/// Filter a BFS-ordered stream of process names down to recognised agent
/// binaries, preserving first-seen (nearest-root) order and deduplicating.
/// Exact basename match only - `claude-code-cli` or a wrapper script must
/// not trigger (parity with the historical `AI_PROCESS_NAMES` contract).
///
/// Consumed by the platform `scan_panes` paths and the unit tests.
#[cfg(any(target_os = "linux", target_os = "macos", windows, test))]
fn agents_in_bfs_order<'a>(
    comms_in_bfs_order: impl Iterator<Item = &'a str>,
    agent_binaries: &[&str],
) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for comm in comms_in_bfs_order {
        if agent_binaries.contains(&comm) && !found.iter().any(|f| f == comm) {
            found.push(comm.to_string());
            if found.len() == agent_binaries.len() {
                break;
            }
        }
    }
    found
}

#[cfg(any(target_os = "linux", test))]
fn command_from_nul_args(bytes: &[u8]) -> Option<String> {
    let parts: Vec<String> = bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Parse one `/proc/net/tcp`-format line into `(port, socket_inode)` for a
/// LISTEN-state (0A) socket. Pure string parsing, platform-neutral so the
/// fixture test runs on every host; header/malformed lines yield `None`.
/// Gated to Linux + test builds: only the `/proc` scan consumes it at
/// runtime, and macOS/Windows compile with `-D warnings` (dead_code).
#[cfg(any(target_os = "linux", test))]
fn parse_listen_line(line: &str) -> Option<(u16, u64)> {
    let mut fields = line.split_whitespace();
    let _sl = fields.next()?;
    // Field 1 is local_address (hex_ip:hex_port)
    let local = fields.next()?;
    let _remote = fields.next()?;
    // Field 3 is TCP state; 0A = LISTEN
    if fields.next()? != "0A" {
        return None;
    }
    // Fields 4..8 (queues, timers, retrnsmt, uid, timeout) precede the inode.
    let inode = fields.nth(5)?.parse::<u64>().ok()?;
    let port = u16::from_str_radix(local.split(':').next_back()?, 16).ok()?;
    Some((port, inode))
}

/// Frontend dev servers recognisable from the socket owner's argv. The table
/// is deliberately frontend-only: a hit arms a CLICKABLE sidebar chip, so
/// precision beats recall here - backend labels keep flowing from the
/// PTY-text enrichment path, where a mislabel is cosmetic.
#[cfg(any(target_os = "linux", target_os = "macos", windows, test))]
const FRONTEND_ARGV: &[(&str, &str)] = &[
    ("vite", "Vite"),
    ("next", "Next.js"),
    ("nuxt", "Nuxt"),
    ("nuxi", "Nuxt"),
    ("astro", "Astro"),
    ("remix", "Remix"),
    ("webpack-dev-server", "Webpack"),
    ("ng", "Angular"),
    ("react-scripts", "React"),
];

/// Classify a process's argv into a frontend dev-server label.
///
/// Matches per-argument BASENAMES (directory components and `.js`-family
/// extensions stripped) so `node /…/node_modules/.bin/vite` hits while
/// `/srv/invite/server.js` cannot. One special case: Next.js rewrites its
/// process title to `next-server (vX.Y.Z)` - a single argv token, matched by
/// prefix. Only the leading args are inspected; launchers always carry the
/// tool name up front.
#[cfg(any(target_os = "linux", target_os = "macos", windows, test))]
fn classify_frontend_argv<'a>(args: impl Iterator<Item = &'a str>) -> Option<&'static str> {
    for arg in args.take(8) {
        if arg
            .get(..11)
            .is_some_and(|p| p.eq_ignore_ascii_case("next-server"))
        {
            return Some("Next.js");
        }
        let base = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
        let base = base
            .strip_suffix(".js")
            .or_else(|| base.strip_suffix(".mjs"))
            .or_else(|| base.strip_suffix(".cjs"))
            .or_else(|| base.strip_suffix(".ts"))
            .unwrap_or(base);
        for &(key, label) in FRONTEND_ARGV {
            if base.eq_ignore_ascii_case(key) {
                return Some(label);
            }
        }
    }
    None
}

#[cfg(any(windows, test))]
fn normalize_process_basename(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if base
            .get(base.len().saturating_sub(suffix.len())..)
            .is_some_and(|s| s.eq_ignore_ascii_case(suffix))
        {
            return &base[..base.len() - suffix.len()];
        }
    }
    base
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

/// BFS the descendants of `root_pid` via `/proc/{pid}/task/{pid}/children`
/// (requires `CONFIG_PROC_CHILDREN=y`; absent kernels yield just the root).
/// `visited` is SHARED across the tick's roots so a pid reparented between
/// subtrees is only ever attributed once. Returns pids in breadth-first
/// order, root first.
#[cfg(target_os = "linux")]
fn bfs_descendants_linux(root_pid: u32, visited: &mut std::collections::HashSet<u32>) -> Vec<u32> {
    // Fast path: /proc/<pid>/task/<pid>/children. If that file is MISSING for
    // the root (an `Err`, NOT an empty `Ok`), the kernel was built without
    // CONFIG_PROC_CHILDREN (hardened / minimal / some container kernels) - fall
    // back to a ppid map so agent-CLI and dev-server detection still work there
    // instead of seeing only the shell.
    let root_children = format!("/proc/{root_pid}/task/{root_pid}/children");
    if read_capped(std::path::Path::new(&root_children), 4096).is_err() {
        return bfs_descendants_via_ppid_linux(root_pid, visited);
    }

    let mut result = Vec::new();
    if !visited.insert(root_pid) {
        return result;
    }
    result.push(root_pid);
    let mut queue = std::collections::VecDeque::from([root_pid]);
    while let Some(pid) = queue.pop_front() {
        if result.len() >= MAX_PIDS_PER_ROOT {
            break;
        }
        let children_path = format!("/proc/{pid}/task/{pid}/children");
        if let Ok(content) = read_capped(std::path::Path::new(&children_path), 4096) {
            for token in content.split_whitespace() {
                if let Ok(child_pid) = token.parse::<u32>()
                    && visited.insert(child_pid)
                {
                    result.push(child_pid);
                    queue.push_back(child_pid);
                }
            }
        }
    }
    result
}

/// Fallback descendant walk for kernels without `CONFIG_PROC_CHILDREN` (the
/// `children` file is absent): scan every `/proc/<pid>/stat` ppid (proc(5)
/// field 4) once into a parent→children map, then BFS it. Same
/// `MAX_PIDS_PER_ROOT` bound and shared-`visited` semantics as the fast path.
/// Only reached on the rare no-`children` kernel, so the extra full `/proc`
/// scan is acceptable.
#[cfg(target_os = "linux")]
fn bfs_descendants_via_ppid_linux(
    root_pid: u32,
    visited: &mut std::collections::HashSet<u32>,
) -> Vec<u32> {
    let mut children_of: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if let Some(ppid) = ppid_of_linux(pid) {
                children_of.entry(ppid).or_default().push(pid);
            }
        }
    }

    let mut result = Vec::new();
    if !visited.insert(root_pid) {
        return result;
    }
    result.push(root_pid);
    let mut queue = std::collections::VecDeque::from([root_pid]);
    while let Some(pid) = queue.pop_front() {
        if result.len() >= MAX_PIDS_PER_ROOT {
            break;
        }
        if let Some(kids) = children_of.get(&pid) {
            for &child in kids {
                if visited.insert(child) {
                    result.push(child);
                    queue.push_back(child);
                }
            }
        }
    }
    result
}

/// ppid (proc(5) field 4) of `pid` from `/proc/<pid>/stat`. Fields are taken
/// after the LAST `)` because the comm field (field 2) is parenthesized and may
/// itself contain spaces/parens - the kernel-documented safe parse.
#[cfg(target_os = "linux")]
fn ppid_of_linux(pid: u32) -> Option<u32> {
    let stat = read_capped(std::path::Path::new(&format!("/proc/{pid}/stat")), 4096).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// argv of a pid from `/proc/{pid}/cmdline` (NUL-separated). 4 KiB cap -
/// the classifiable token always sits in the leading args; non-UTF-8 argv
/// degrades to "unclassified", never an error.
#[cfg(target_os = "linux")]
fn cmdline_args_linux(pid: u32) -> Vec<String> {
    let path = format!("/proc/{pid}/cmdline");
    read_capped(std::path::Path::new(&path), 4096)
        .map(|content| {
            content
                .split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn linux_command_for_pid(pid: u32) -> Option<String> {
    if let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline"))
        && let Some(command) = command_from_nul_args(&bytes)
    {
        return Some(command);
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let trimmed = comm.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(target_os = "linux")]
fn linux_foreground_command(root_pid: u32, pids: &[u32]) -> Option<String> {
    let children_path = format!("/proc/{root_pid}/task/{root_pid}/children");
    let target = match read_capped(std::path::Path::new(&children_path), 4096) {
        Ok(content) => content
            .split_whitespace()
            .last()
            .and_then(|pid| pid.parse::<u32>().ok())
            .unwrap_or(root_pid),
        Err(_) => pids
            .iter()
            .copied()
            .filter(|pid| *pid != root_pid)
            .max()
            .unwrap_or(root_pid),
    };
    linux_command_for_pid(target)
}

/// Collect socket inodes from `/proc/{pid}/fd/` for one PID.
#[cfg(target_os = "linux")]
fn socket_inodes_of(pid: u32, inodes: &mut Vec<u64>) {
    let fd_dir = format!("/proc/{pid}/fd");
    if let Ok(entries) = std::fs::read_dir(&fd_dir) {
        for entry in entries.flatten() {
            if let Ok(link) = std::fs::read_link(entry.path()) {
                let link_str = link.to_string_lossy();
                if let Some(rest) = link_str.strip_prefix("socket:[")
                    && let Some(inode_str) = rest.strip_suffix(']')
                    && let Ok(inode) = inode_str.parse::<u64>()
                {
                    inodes.push(inode);
                }
            }
        }
    }
}

/// Scan every terminal's PTY subtree in one pass (see module docs for the
/// cost contract). `roots` pairs an opaque caller key (the terminal entity
/// id) with the PTY child pid; `agent_binaries` is the recognition set -
/// derived by the caller from `TerminalAgent::ALL` (US-012 vocabulary
/// unification; matching is exact against `/proc/<pid>/comm`, which the
/// kernel truncates to 15 chars - every current binary name fits).
#[cfg(target_os = "linux")]
pub fn scan_panes(
    roots: &[(u64, u32)],
    agent_binaries: &[&str],
) -> std::collections::HashMap<u64, PaneScan> {
    let mut results: std::collections::HashMap<u64, PaneScan> = std::collections::HashMap::new();
    if roots.is_empty() {
        return results;
    }

    // 1. One shared subtree walk (each pid visited once per tick).
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut subtrees: Vec<(u64, Vec<u32>)> = Vec::with_capacity(roots.len());
    for &(key, root_pid) in roots {
        if root_pid == 0 {
            continue;
        }
        let pids = bfs_descendants_linux(root_pid, &mut visited);
        subtrees.push((key, pids));
    }

    // 2. Agents per subtree: read each pid's comm once, match in BFS order.
    //    3. Socket inodes per subtree → inode → (subtree index, pid) map.
    let mut inode_owner: std::collections::HashMap<u64, (usize, u32)> =
        std::collections::HashMap::new();
    for (idx, (key, pids)) in subtrees.iter().enumerate() {
        let foreground_command = roots
            .iter()
            .find(|(root_key, _)| root_key == key)
            .and_then(|(_, root_pid)| linux_foreground_command(*root_pid, pids));
        let comms: Vec<String> = if agent_binaries.is_empty() {
            Vec::new()
        } else {
            pids.iter()
                .filter_map(|pid| {
                    std::fs::read_to_string(format!("/proc/{pid}/comm"))
                        .ok()
                        .map(|s| s.trim().to_string())
                })
                .collect()
        };
        let agents = agents_in_bfs_order(comms.iter().map(String::as_str), agent_binaries);

        for &pid in pids {
            let mut inodes: Vec<u64> = Vec::new();
            socket_inodes_of(pid, &mut inodes);
            for inode in inodes {
                // First owner wins; subtrees are disjoint (shared `visited`)
                // so a duplicate inode here means a shared/inherited socket -
                // keep the earlier (older pane) attribution deterministically.
                inode_owner.entry(inode).or_insert((idx, pid));
            }
        }

        results.insert(
            *key,
            PaneScan {
                ports: Vec::new(),
                agents,
                foreground_command,
            },
        );
    }

    // 4. /proc/net/tcp[6] parsed ONCE for the whole tick, streamed
    //    line-by-line. The previous single capped read (256 KiB) silently
    //    dropped the tail on socket-heavy hosts (Docker, busy dev boxes),
    //    making ports vanish for whole ticks; streaming keeps memory at one
    //    line while reading arbitrarily many sockets. The line cap below
    //    only bounds a pathological /proc - and the scan runs under
    //    `smol::unblock`, never on the render thread. The owning pid's argv
    //    classifies the port (cached per pid).
    const MAX_TCP_LINES: usize = 65_536;
    let mut class_cache: std::collections::HashMap<u32, Option<&'static str>> =
        std::collections::HashMap::new();
    let mut per_idx_ports: Vec<Vec<PortEntry>> = vec![Vec::new(); subtrees.len()];
    for path in &["/proc/net/tcp", "/proc/net/tcp6"] {
        use std::io::BufRead;
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        for line in std::io::BufReader::new(file).lines().take(MAX_TCP_LINES) {
            let Ok(line) = line else {
                break;
            };
            let Some((port, inode)) = parse_listen_line(&line) else {
                continue;
            };
            if let Some(&(idx, pid)) = inode_owner.get(&inode) {
                let frontend = *class_cache.entry(pid).or_insert_with(|| {
                    let args = cmdline_args_linux(pid);
                    classify_frontend_argv(args.iter().map(String::as_str))
                });
                per_idx_ports[idx].push(PortEntry { port, frontend });
            }
        }
    }
    for (idx, (key, _)) in subtrees.iter().enumerate() {
        let mut ports = std::mem::take(&mut per_idx_ports[idx]);
        // Dual-stack v4+v6 binds yield two sockets on one port - keep one
        // entry, preferring a classified one.
        ports.sort_by_key(|e| (e.port, e.frontend.is_none()));
        ports.dedup_by_key(|e| e.port);
        if let Some(scan) = results.get_mut(key) {
            scan.ports = ports;
        }
    }

    results
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

/// macOS ppid→children map over every visible process, built once per scan.
///
/// `libc::proc_listchildpids` is deliberately NOT used: on modern macOS it
/// returns 0 children for an unprivileged caller, so the old per-node subtree
/// walk found nothing and the workspace card never lit its agent dot. Instead
/// we enumerate all pids (`listpids(ProcAllPIDS)`) and read each one's parent
/// from `proc_bsdinfo.pbi_ppid` - the very same `proc_pidinfo(PROC_PIDTBSDINFO)`
/// query that `name()` already succeeds with for same-user processes. Mirrors
/// the Linux `bfs_descendants_via_ppid_linux` fallback. Processes we can't
/// inspect (EPERM on SIP-protected / other-user pids, dead-pid races) are
/// skipped - our agents are same-user PTY children, always readable.
#[cfg(target_os = "macos")]
fn macos_children_map() -> std::collections::HashMap<u32, Vec<u32>> {
    use libproc::libproc::bsd_info::BSDInfo;
    use libproc::libproc::proc_pid::pidinfo;
    use libproc::processes::{ProcFilter, pids_by_type};

    let mut children_of: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    let pids = match pids_by_type(ProcFilter::All) {
        Ok(pids) => pids,
        Err(e) => {
            // Wholesale enumeration failure - NOT a routine per-pid EPERM skip:
            // every port badge and agent dot on macOS goes dark at once. This
            // is the `proc_listchildpids`-class failure mode, so make it
            // diagnosable in paneflow-debug.log. Latched to log ONCE: this runs
            // on the periodic scan, and a per-tick warn would be the very noise
            // the `cwd_now(pid=0)` fix removed.
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                log::warn!(
                    "macos process enumeration failed (pids_by_type: {e}) - port \
                     badges and agent detection will be unavailable"
                );
            }
            return children_of;
        }
    };
    for pid in pids {
        if pid == 0 {
            continue;
        }
        if let Ok(info) = pidinfo::<BSDInfo>(pid as i32, 0) {
            children_of.entry(info.pbi_ppid).or_default().push(pid);
        }
    }
    children_of
}

/// macOS descendant walker - BFS over the prebuilt `children_of` ppid map
/// (see [`macos_children_map`]). Kernel equivalent of the Linux
/// `/proc/{pid}/task/{pid}/children` traversal; `visited` is shared across
/// roots (same single-walk contract as Linux). Returns pids in BFS order.
#[cfg(target_os = "macos")]
fn bfs_descendants_macos(
    root_pid: u32,
    children_of: &std::collections::HashMap<u32, Vec<u32>>,
    visited: &mut std::collections::HashSet<u32>,
) -> Vec<u32> {
    let mut result = Vec::new();
    if !visited.insert(root_pid) {
        return result;
    }
    result.push(root_pid);
    let mut queue = std::collections::VecDeque::from([root_pid]);

    while let Some(pid) = queue.pop_front() {
        if result.len() >= MAX_PIDS_PER_ROOT {
            break;
        }
        if let Some(kids) = children_of.get(&pid) {
            for &child in kids {
                if visited.insert(child) {
                    result.push(child);
                    queue.push_back(child);
                }
            }
        }
    }

    result
}

/// macOS LISTEN ports for one PID, appended to `ports`.
///
/// Walks the PID's file descriptors via `libproc::listpidinfo::<ListFDs>`,
/// queries `pidfdinfo::<SocketFDInfo>` for every Socket FD, and filters to
/// TCP sockets in the `Listen` state. `insi_lport` in `TcpSockInfo.tcpsi_ini`
/// is the kernel's inpcb local port cast to `c_int`; the low 16 bits hold
/// the network-byte-order u16, so we mask + `from_be` to get host order.
#[cfg(target_os = "macos")]
fn listen_ports_of(pid: u32, ports: &mut Vec<u16>) {
    use libproc::libproc::file_info::{ListFDs, ProcFDType, pidfdinfo};
    use libproc::libproc::net_info::{SocketFDInfo, SocketInfoKind, TcpSIState};
    use libproc::libproc::proc_pid::listpidinfo;

    // Typical ulimit default on macOS is 256-4096 FDs per process. 1024 is
    // a sensible over-provisioning ceiling - the buffer is uninitialised
    // memory so allocation cost is a single malloc, not a zeroing pass.
    const MAX_FDS_PER_PROC: usize = 1024;

    let Ok(fds) = listpidinfo::<ListFDs>(pid as i32, MAX_FDS_PER_PROC) else {
        // EPERM / dead-process races / SIP-restricted targets → skip
        // silently. `listpidinfo` already wraps the error string, which is
        // more noise than signal during routine UI-triggered scans.
        return;
    };

    for fd in fds {
        if !matches!(ProcFDType::from(fd.proc_fdtype), ProcFDType::Socket) {
            continue;
        }

        let Ok(sfi) = pidfdinfo::<SocketFDInfo>(pid as i32, fd.proc_fd) else {
            continue;
        };

        if sfi.psi.soi_kind != SocketInfoKind::Tcp as libc::c_int {
            continue;
        }

        // SAFETY: when `soi_kind == Tcp`, the kernel guarantees the
        // `soi_proto` union's `pri_tcp` arm is the active one. The union is
        // POD (`SocketInfoProto` holds `#[repr(C)]` structs all the way
        // down) so reading a different arm would only produce garbage port
        // bytes, not UB - but we gate on `soi_kind` to keep the data
        // meaningful.
        let tcp = unsafe { sfi.psi.soi_proto.pri_tcp };

        if TcpSIState::from(tcp.tcpsi_state) as i32 != TcpSIState::Listen as i32 {
            continue;
        }

        let net_port = (tcp.tcpsi_ini.insi_lport as u32 & 0xFFFF) as u16;
        let port = u16::from_be(net_port);
        if port != 0 {
            ports.push(port);
        }
    }
}

/// argv of a pid via `sysctl(KERN_PROCARGS2)` - macOS's equivalent of Linux
/// `/proc/{pid}/cmdline`. EPERM (other-user pids, SIP-protected targets) and
/// malformed buffers degrade to an empty vec: the port then simply stays
/// unclassified, parity with the Linux non-UTF-8 fallback.
#[cfg(target_os = "macos")]
fn argv_of_macos(pid: u32) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];

    let mut size: libc::size_t = 0;
    // SAFETY: standard 3-int MIB size probe - a null buffer with a size
    // out-param is the documented sysctl(3) calling convention; nothing is
    // written besides `size`.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size == 0 {
        return Vec::new();
    }

    // The probed size covers the full argv+env block, bounded by the
    // kernel's ARG_MAX (1 MiB) - a transient allocation on the unblock
    // thread, freed before the scan returns.
    let mut buf = vec![0u8; size];
    // SAFETY: `buf` provides exactly `size` writable bytes; the kernel
    // writes at most `size` and updates it to the written length.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Vec::new();
    }
    buf.truncate(size);
    parse_procargs2(&buf)
}

/// Pure parser for the `KERN_PROCARGS2` buffer layout: `argc: c_int`, the
/// NUL-terminated exec path, a NUL padding run, then `argc` NUL-separated
/// argv strings (env vars follow and are ignored). Platform-neutral so the
/// fixture test runs on every host.
#[cfg(any(target_os = "macos", test))]
fn parse_procargs2(buf: &[u8]) -> Vec<String> {
    let Some(argc_bytes) = buf.get(..4) else {
        return Vec::new();
    };
    let argc = i32::from_ne_bytes([argc_bytes[0], argc_bytes[1], argc_bytes[2], argc_bytes[3]])
        .max(0) as usize;
    if argc == 0 {
        return Vec::new();
    }
    let rest = &buf[4..];
    // Skip the exec path, then its NUL padding run.
    let path_end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    let args_start = rest[path_end..]
        .iter()
        .position(|&b| b != 0)
        .map(|off| path_end + off)
        .unwrap_or(rest.len());
    rest[args_start..]
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .take(argc)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

#[cfg(target_os = "macos")]
fn macos_foreground_command(pids: &[u32]) -> Option<String> {
    use libproc::libproc::proc_pid::name;

    let pid = pids.last().copied()?;
    name(pid as i32)
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Scan every terminal's PTY subtree in one pass (macOS). libproc's socket
/// queries are naturally per-pid, so per-subtree attribution falls out of
/// the BFS partition without a global socket table. Same shared-`visited` /
/// single-walk contract as the Linux branch.
#[cfg(target_os = "macos")]
pub fn scan_panes(
    roots: &[(u64, u32)],
    agent_binaries: &[&str],
) -> std::collections::HashMap<u64, PaneScan> {
    use libproc::libproc::proc_pid::name;

    let mut results: std::collections::HashMap<u64, PaneScan> = std::collections::HashMap::new();
    if roots.is_empty() {
        return results;
    }

    // One ppid→children snapshot for the whole scan - every root's subtree is
    // carved out of it, so the full `listpids` enumeration happens once, not
    // per pane.
    let children_of = macos_children_map();
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &(key, root_pid) in roots {
        if root_pid == 0 {
            continue;
        }
        let pids = bfs_descendants_macos(root_pid, &children_of, &mut visited);

        // `libproc::name` returns the kernel's `p_comm` - same semantics
        // and 16-char limit as Linux `/proc/<pid>/comm`. EPERM (sandbox /
        // SIP) skips silently.
        let comms: Vec<String> = if agent_binaries.is_empty() {
            Vec::new()
        } else {
            pids.iter()
                .filter_map(|&pid| name(pid as i32).ok().map(|n| n.trim().to_string()))
                .collect()
        };
        let agents = agents_in_bfs_order(comms.iter().map(String::as_str), agent_binaries);

        let mut ports: Vec<PortEntry> = Vec::new();
        for &pid in &pids {
            let mut pid_ports: Vec<u16> = Vec::new();
            listen_ports_of(pid, &mut pid_ports);
            if pid_ports.is_empty() {
                continue;
            }
            // argv fetched only for pids that actually own a LISTEN socket.
            let args = argv_of_macos(pid);
            let frontend = classify_frontend_argv(args.iter().map(String::as_str));
            ports.extend(
                pid_ports
                    .into_iter()
                    .map(|port| PortEntry { port, frontend }),
            );
        }
        // Dual-stack v4+v6 binds yield two sockets on one port - keep one
        // entry, preferring a classified one.
        ports.sort_by_key(|e| (e.port, e.frontend.is_none()));
        ports.dedup_by_key(|e| e.port);

        results.insert(
            key,
            PaneScan {
                ports,
                agents,
                foreground_command: macos_foreground_command(&pids),
            },
        );
    }

    results
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[derive(Clone, Debug)]
struct WindowsProcessEntry {
    pid: u32,
    parent_pid: u32,
    exe: String,
}

#[cfg(windows)]
fn windows_process_entries() -> Vec<WindowsProcessEntry> {
    use std::mem;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: Win32 call; a successful snapshot handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Vec::new();
    }

    let mut entries = Vec::with_capacity(256);
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is valid, and `entry` has the documented `dwSize`.
    if unsafe { Process32FirstW(snap, &mut entry) } != 0 {
        loop {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
            entries.push(WindowsProcessEntry {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                exe,
            });
            // SAFETY: same invariants as Process32FirstW; stops at exhaustion.
            if unsafe { Process32NextW(snap, &mut entry) } == 0 {
                break;
            }
        }
    }
    // SAFETY: `snap` is a valid handle returned by CreateToolhelp32Snapshot.
    unsafe { CloseHandle(snap) };
    entries
}

#[cfg(windows)]
fn bfs_descendants_windows(
    root_pid: u32,
    entries: &[WindowsProcessEntry],
    visited: &mut std::collections::HashSet<u32>,
) -> Vec<u32> {
    let mut children_of: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    for entry in entries {
        children_of
            .entry(entry.parent_pid)
            .or_default()
            .push(entry.pid);
    }

    let mut result = Vec::new();
    if !visited.insert(root_pid) {
        return result;
    }
    result.push(root_pid);
    let mut queue = std::collections::VecDeque::from([root_pid]);
    while let Some(pid) = queue.pop_front() {
        if result.len() >= MAX_PIDS_PER_ROOT {
            break;
        }
        if let Some(children) = children_of.get(&pid) {
            for &child in children {
                if visited.insert(child) {
                    result.push(child);
                    queue.push_back(child);
                }
            }
        }
    }
    result
}

#[cfg(windows)]
fn windows_foreground_command(
    root_pid: u32,
    entries: &[WindowsProcessEntry],
    exe_by_pid: &std::collections::HashMap<u32, String>,
) -> Option<String> {
    let mut current = root_pid;
    let mut visited = std::collections::HashSet::new();
    while visited.insert(current) {
        match entries
            .iter()
            .filter(|entry| entry.parent_pid == current)
            .max_by_key(|entry| entry.pid)
        {
            Some(child) => current = child.pid,
            None => break,
        }
    }
    exe_by_pid
        .get(&current)
        .map(|exe| normalize_process_basename(exe).to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(windows)]
fn windows_port_from_network_order(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

#[cfg(windows)]
fn windows_listen_ports_by_pid() -> std::collections::HashMap<u32, Vec<u16>> {
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
        MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    fn collect_table<TTable, TRow>(
        family: u32,
        row_slice: unsafe fn(*const TTable) -> Vec<TRow>,
        row_pid_port: fn(&TRow) -> (u32, u16),
        out: &mut std::collections::HashMap<u32, Vec<u16>>,
    ) {
        let mut size = 0u32;
        // SAFETY: first call intentionally passes a null buffer so the API
        // fills `size` with the required byte count.
        let rc = unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if rc != ERROR_INSUFFICIENT_BUFFER || size == 0 {
            return;
        }

        let word_count = (size as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buf = vec![0usize; word_count];
        // SAFETY: `buf` has at least `size` bytes with pointer-size alignment;
        // the API writes a table selected by `family` + table class.
        let rc = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr().cast(),
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if rc != NO_ERROR {
            return;
        }

        // SAFETY: the successful call initialized the buffer as `TTable`.
        for row in unsafe { row_slice(buf.as_ptr().cast::<TTable>()) } {
            let (pid, port) = row_pid_port(&row);
            if pid != 0 && port != 0 {
                out.entry(pid).or_default().push(port);
            }
        }
    }

    unsafe fn ipv4_rows(table: *const MIB_TCPTABLE_OWNER_PID) -> Vec<MIB_TCPROW_OWNER_PID> {
        let count = unsafe { (*table).dwNumEntries as usize };
        let first = unsafe { (*table).table.as_ptr() };
        unsafe { std::slice::from_raw_parts(first, count) }.to_vec()
    }

    unsafe fn ipv6_rows(table: *const MIB_TCP6TABLE_OWNER_PID) -> Vec<MIB_TCP6ROW_OWNER_PID> {
        let count = unsafe { (*table).dwNumEntries as usize };
        let first = unsafe { (*table).table.as_ptr() };
        unsafe { std::slice::from_raw_parts(first, count) }.to_vec()
    }

    let mut by_pid: std::collections::HashMap<u32, Vec<u16>> = std::collections::HashMap::new();
    collect_table(
        AF_INET as u32,
        ipv4_rows,
        |row| {
            (
                row.dwOwningPid,
                windows_port_from_network_order(row.dwLocalPort),
            )
        },
        &mut by_pid,
    );
    collect_table(
        AF_INET6 as u32,
        ipv6_rows,
        |row| {
            (
                row.dwOwningPid,
                windows_port_from_network_order(row.dwLocalPort),
            )
        },
        &mut by_pid,
    );
    for ports in by_pid.values_mut() {
        ports.sort_unstable();
        ports.dedup();
    }
    by_pid
}

#[cfg(windows)]
fn argv_of_windows(pid: u32) -> Vec<String> {
    windows_command_line(pid)
        .map(|line| windows_command_line_to_argv(&line))
        .unwrap_or_default()
}

#[cfg(windows)]
fn windows_command_line(pid: u32) -> Option<String> {
    use std::mem;
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
    use windows_sys::Win32::Foundation::{CloseHandle, UNICODE_STRING};
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PEB, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_READ, RTL_USER_PROCESS_PARAMETERS,
    };

    const MAX_COMMAND_LINE_BYTES: usize = 64 * 1024;

    if pid == 0 {
        return None;
    }

    unsafe fn read_remote<T: Copy>(
        handle: windows_sys::Win32::Foundation::HANDLE,
        ptr: *const T,
    ) -> Option<T> {
        let mut value: T = unsafe { mem::zeroed() };
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                ptr.cast(),
                (&mut value as *mut T).cast(),
                mem::size_of::<T>(),
                &mut read,
            )
        };
        (ok != 0 && read == mem::size_of::<T>()).then_some(value)
    }

    unsafe fn read_unicode_string(
        handle: windows_sys::Win32::Foundation::HANDLE,
        value: UNICODE_STRING,
    ) -> Option<String> {
        let len = value.Length as usize;
        if len == 0
            || len > MAX_COMMAND_LINE_BYTES
            || !len.is_multiple_of(2)
            || value.Buffer.is_null()
        {
            return None;
        }
        let mut bytes = vec![0u16; len / 2];
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                value.Buffer.cast(),
                bytes.as_mut_ptr().cast(),
                len,
                &mut read,
            )
        };
        (ok != 0 && read == len).then(|| String::from_utf16_lossy(&bytes))
    }

    // SAFETY: the handle is closed before returning on every path.
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        return None;
    }

    let result = (|| {
        let mut info: PROCESS_BASIC_INFORMATION = unsafe { mem::zeroed() };
        let status = unsafe {
            NtQueryInformationProcess(
                handle,
                ProcessBasicInformation,
                (&mut info as *mut PROCESS_BASIC_INFORMATION).cast(),
                mem::size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status < 0 || info.PebBaseAddress.is_null() {
            return None;
        }
        let peb: PEB = unsafe { read_remote(handle, info.PebBaseAddress.cast())? };
        if peb.ProcessParameters.is_null() {
            return None;
        }
        let params: RTL_USER_PROCESS_PARAMETERS =
            unsafe { read_remote(handle, peb.ProcessParameters.cast())? };
        unsafe { read_unicode_string(handle, params.CommandLine) }
    })();

    // SAFETY: `handle` is owned by this function.
    unsafe { CloseHandle(handle) };
    result
}

#[cfg(windows)]
fn windows_command_line_to_argv(command_line: &str) -> Vec<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    let mut wide: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut argc = 0i32;
    // SAFETY: `wide` is NUL-terminated and lives until CommandLineToArgvW
    // returns. The returned allocation is released with LocalFree.
    let argv = unsafe { CommandLineToArgvW(wide.as_mut_ptr(), &mut argc) };
    if argv.is_null() || argc <= 0 {
        return command_line
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }

    let mut args = Vec::with_capacity(argc as usize);
    // SAFETY: CommandLineToArgvW returns `argc` pointers on success.
    let slice = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
    for &ptr in slice {
        if ptr.is_null() {
            continue;
        }
        let mut len = 0usize;
        // SAFETY: each pointer is a NUL-terminated UTF-16 string owned by the
        // CommandLineToArgvW result allocation.
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
            args.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                ptr, len,
            )));
        }
    }
    // SAFETY: `argv` was allocated by CommandLineToArgvW.
    unsafe { LocalFree(argv.cast()) };
    args
}

#[cfg(windows)]
pub fn scan_panes(
    roots: &[(u64, u32)],
    agent_binaries: &[&str],
) -> std::collections::HashMap<u64, PaneScan> {
    let mut results: std::collections::HashMap<u64, PaneScan> = std::collections::HashMap::new();
    if roots.is_empty() {
        return results;
    }

    let entries = windows_process_entries();
    let exe_by_pid: std::collections::HashMap<u32, String> = entries
        .iter()
        .map(|entry| (entry.pid, entry.exe.clone()))
        .collect();
    let listen_ports = windows_listen_ports_by_pid();

    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &(key, root_pid) in roots {
        if root_pid == 0 {
            continue;
        }
        let pids = bfs_descendants_windows(root_pid, &entries, &mut visited);
        let comms: Vec<String> = if agent_binaries.is_empty() {
            Vec::new()
        } else {
            pids.iter()
                .filter_map(|pid| exe_by_pid.get(pid))
                .map(|exe| normalize_process_basename(exe).to_string())
                .collect()
        };
        let agents = agents_in_bfs_order(comms.iter().map(String::as_str), agent_binaries);

        let mut ports = Vec::new();
        for pid in pids {
            let Some(pid_ports) = listen_ports.get(&pid) else {
                continue;
            };
            let argv = argv_of_windows(pid);
            let frontend = classify_frontend_argv(argv.iter().map(String::as_str)).or_else(|| {
                exe_by_pid.get(&pid).and_then(|exe| {
                    classify_frontend_argv([normalize_process_basename(exe)].into_iter())
                })
            });
            ports.extend(
                pid_ports
                    .iter()
                    .copied()
                    .map(|port| PortEntry { port, frontend }),
            );
        }
        ports.sort_by_key(|e| (e.port, e.frontend.is_none()));
        ports.dedup_by_key(|e| e.port);
        results.insert(
            key,
            PaneScan {
                ports,
                agents,
                foreground_command: windows_foreground_command(root_pid, &entries, &exe_by_pid),
            },
        );
    }

    results
}

// ---------------------------------------------------------------------------
// Stub (BSDs / other targets)
// ---------------------------------------------------------------------------

/// Stub for unsupported platforms. An empty map means every tab renders without
/// badges or pills and workspace aggregates stay empty - degradation without
/// error.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn scan_panes(
    _roots: &[(u64, u32)],
    _agent_binaries: &[&str],
) -> std::collections::HashMap<u64, PaneScan> {
    std::collections::HashMap::new()
}

// ---------------------------------------------------------------------------
// Tests (platform-neutral helpers)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // US-013 AC: "plusieurs binaires agents dans le même sous-arbre → le
    // plus proche de la racine" - first-seen BFS order wins, duplicates
    // collapse to the first occurrence.
    #[test]
    fn agents_in_bfs_order_picks_nearest_root_first_and_dedups() {
        let comms = ["zsh", "claude", "node", "codex", "claude"];
        let agents = agents_in_bfs_order(comms.into_iter(), &["claude", "codex", "opencode"]);
        assert_eq!(agents, vec!["claude".to_string(), "codex".to_string()]);
    }

    #[test]
    fn agents_in_bfs_order_exact_match_only() {
        // A wrapper named `claude-code-cli` must not trigger (exact
        // basename contract, parity with the old AI_PROCESS_NAMES match).
        let comms = ["claude-code-cli", "Claude", "claudex"];
        assert!(agents_in_bfs_order(comms.into_iter(), &["claude"]).is_empty());
    }

    #[test]
    fn agents_in_bfs_order_empty_inputs() {
        assert!(agents_in_bfs_order(std::iter::empty(), &["claude"]).is_empty());
        assert!(agents_in_bfs_order(["claude"].into_iter(), &[]).is_empty());
    }

    #[test]
    fn command_from_nul_args_joins_argv() {
        assert_eq!(
            command_from_nul_args(b"cargo\0run\0--release\0"),
            Some("cargo run --release".to_string())
        );
        assert_eq!(
            command_from_nul_args(b"\0node\0\0server.js\0"),
            Some("node server.js".to_string())
        );
        assert_eq!(command_from_nul_args(b""), None);
        assert_eq!(command_from_nul_args(b"\0\0"), None);
    }

    #[test]
    fn parse_listen_line_filters_listen_state_and_malformed_lines() {
        // LISTEN (port 0x1F90 = 8080, inode 4242) parses; header,
        // ESTABLISHED (01) and garbage lines yield None.
        let listen = "   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0";
        assert_eq!(parse_listen_line(listen), Some((8080, 4242)));
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode";
        assert_eq!(parse_listen_line(header), None);
        let established = "   1: 0100007F:0050 0100007F:1234 01 00000000:00000000 00:00000000 00000000  1000        0 9999 1 0000000000000000 100 0 0 10 0";
        assert_eq!(parse_listen_line(established), None);
        assert_eq!(parse_listen_line("garbage line"), None);
        assert_eq!(parse_listen_line(""), None);
    }

    #[test]
    fn classify_frontend_argv_matches_basenames_and_titles() {
        // node running the .bin shim - the canonical vite/next launch shape.
        let argv = ["node", "/repo/node_modules/.bin/vite"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Vite"));
        // bun executing the package bin JS directly.
        let argv = ["bun", "/repo/node_modules/vite/bin/vite.js"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Vite"));
        // Next.js rewrites its process title to one "next-server (vX)" token.
        let argv = ["next-server (v15.3.2)"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Next.js"));
        let argv = ["node", "/repo/node_modules/.bin/next", "dev"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Next.js"));
        let argv = ["node", "/usr/lib/node_modules/@angular/cli/bin/ng", "serve"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Angular"));
    }

    #[test]
    fn classify_frontend_argv_rejects_lookalikes() {
        // Basename matching, not substring: a path that merely CONTAINS a
        // framework name must not arm a clickable chip.
        let argv = ["node", "/srv/invite/server.js"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), None);
        let argv = ["node", "/srv/vitesse-app/index.js"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), None);
        let argv = ["python3", "-m", "http.server"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), None);
        assert_eq!(classify_frontend_argv(std::iter::empty()), None);
    }

    #[test]
    fn normalize_process_basename_strips_common_windows_wrappers() {
        assert_eq!(normalize_process_basename(r"C:\tools\codex.exe"), "codex");
        assert_eq!(normalize_process_basename("vite.CMD"), "vite");
        assert_eq!(normalize_process_basename("vite.cmd"), "vite");
        assert_eq!(normalize_process_basename("script.ps1"), "script");
    }

    #[cfg(windows)]
    #[test]
    fn windows_port_from_network_order_decodes_low_word() {
        assert_eq!(windows_port_from_network_order(0x901F), 8080);
    }

    #[cfg(windows)]
    #[test]
    fn windows_command_line_argv_classifies_node_frontend() {
        let args = windows_command_line_to_argv(
            r#""C:\Program Files\nodejs\node.exe" "C:\repo\node_modules\.bin\vite" --host"#,
        );
        assert_eq!(
            classify_frontend_argv(args.iter().map(String::as_str)),
            Some("Vite")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn scan_panes_detects_current_process_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let scan = scan_panes(&[(1, std::process::id())], &[]);
        let ports = scan
            .get(&1)
            .map(|s| s.ports.iter().map(|e| e.port).collect::<Vec<_>>())
            .unwrap_or_default();

        assert!(
            ports.contains(&port),
            "scan_panes must detect a live listener owned by the root pid; got {ports:?}"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn scan_panes_ignores_pid_zero_roots() {
        let scan = scan_panes(&[(1, 0)], &[]);
        assert!(
            scan.is_empty(),
            "pid 0 is a display-only sentinel and must not scan the system tree"
        );
    }

    #[test]
    fn parse_procargs2_extracts_argv_after_exec_path() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&2i32.to_ne_bytes());
        // Exec path + NUL padding run, then argc args, then env (ignored).
        buf.extend_from_slice(b"/usr/local/bin/node\0\0\0\0");
        buf.extend_from_slice(b"node\0/repo/node_modules/.bin/vite\0");
        buf.extend_from_slice(b"PATH=/usr/bin\0");
        assert_eq!(
            parse_procargs2(&buf),
            vec![
                "node".to_string(),
                "/repo/node_modules/.bin/vite".to_string()
            ]
        );
        assert!(parse_procargs2(&[]).is_empty());
        assert!(parse_procargs2(&[1, 0, 0]).is_empty());
        assert!(parse_procargs2(&0i32.to_ne_bytes()).is_empty());
    }

    // Regression for the workspace-card blue dot on Apple: the macOS subtree
    // scan must find a live PTY descendant and resolve its `p_comm`. This
    // failed silently while the walk relied on `proc_listchildpids`, which
    // returns 0 children for an unprivileged caller - `detected_agents` stayed
    // empty and the dot never lit. We spawn the real, signed `/bin/sleep`
    // (p_comm == "sleep"; no code-signing confound) as a child of the test
    // process and assert it surfaces.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_scan_panes_detects_live_child_subtree() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        // Let the kernel register the new process's BSD info before we probe.
        std::thread::sleep(std::time::Duration::from_millis(250));

        let roots = [(1u64, std::process::id())];
        let scan = scan_panes(&roots, &["sleep"]);

        let _ = child.kill();
        let _ = child.wait();

        let agents = scan.get(&1).map(|s| s.agents.clone()).unwrap_or_default();
        assert!(
            agents.iter().any(|a| a == "sleep"),
            "macOS subtree scan must detect the live `sleep` child; got {agents:?}"
        );
    }
}
