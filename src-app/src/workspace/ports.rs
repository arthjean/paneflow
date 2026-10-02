#[cfg(target_os = "linux")]
use super::git::read_capped;

#[derive(Debug, Clone, PartialEq)]
pub struct PortEntry {
    pub port: u16,
    pub frontend: Option<&'static str>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneScan {
    pub ports: Vec<PortEntry>,
    pub foreground_command: Option<String>,
}

const MAX_PIDS_PER_ROOT: usize = 512;

#[cfg(any(target_os = "linux", test))]
fn command_from_nul_args(bytes: &[u8]) -> Option<String> {
    let parts: Vec<String> = bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    (!parts.is_empty()).then(|| {
        parts
            .iter()
            .map(|part| quote_command_arg(part))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

#[cfg(any(target_os = "linux", test))]
fn quote_command_arg(arg: &str) -> String {
    if !arg.chars().any(|c| c.is_whitespace() || c == '"') {
        return arg.to_string();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

#[cfg(any(target_os = "linux", test))]
fn parse_listen_line(line: &str) -> Option<(u16, u64)> {
    let mut fields = line.split_whitespace();
    let _sl = fields.next()?;
    let local = fields.next()?;
    let _remote = fields.next()?;
    if fields.next()? != "0A" {
        return None;
    }
    let inode = fields.nth(5)?.parse::<u64>().ok()?;
    let port = u16::from_str_radix(local.split(':').next_back()?, 16).ok()?;
    Some((port, inode))
}

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

#[cfg(target_os = "linux")]
fn bfs_descendants_linux(root_pid: u32, visited: &mut std::collections::HashSet<u32>) -> Vec<u32> {
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

#[cfg(target_os = "linux")]
fn ppid_of_linux(pid: u32) -> Option<u32> {
    let stat = read_capped(std::path::Path::new(&format!("/proc/{pid}/stat")), 4096).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

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

#[cfg(any(unix, test))]
fn foreground_pid(
    root_pid: u32,
    pids: &[u32],
    foreground_pgid: Option<u32>,
    pgid_of: impl Fn(u32) -> Option<u32>,
) -> u32 {
    foreground_pgid
        .filter(|group| *group > 0 && *group <= i32::MAX as u32)
        .and_then(|group| {
            pids.iter()
                .copied()
                .find(|pid| pgid_of(*pid) == Some(group))
        })
        .unwrap_or(root_pid)
}

#[cfg(any(target_os = "linux", test))]
fn stat_process_groups(stat: &str) -> Option<(u32, Option<u32>)> {
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    let pgrp = fields.get(2)?.parse().ok()?;
    let tpgid = fields
        .get(5)?
        .parse::<i64>()
        .ok()
        .and_then(|group| u32::try_from(group).ok());
    Some((pgrp, tpgid))
}

#[cfg(any(target_os = "macos", test))]
fn read_growing_list<T>(
    initial: Option<usize>,
    mut read: impl FnMut(usize) -> Option<Vec<T>>,
) -> Option<Vec<T>> {
    const FALLBACK_CAPACITY: usize = 1024;
    const MAX_CAPACITY: usize = 1 << 20;
    let mut capacity = initial
        .filter(|count| *count > 0)
        .unwrap_or(FALLBACK_CAPACITY)
        .min(MAX_CAPACITY);
    let mut partial = None;
    loop {
        let Some(list) = read(capacity) else {
            return partial;
        };
        if list.len() < capacity || capacity >= MAX_CAPACITY {
            return Some(list);
        }
        partial = Some(list);
        capacity = capacity.saturating_mul(2).min(MAX_CAPACITY);
    }
}

#[cfg(target_os = "linux")]
fn linux_representative_command(root_pid: u32, pids: &[u32]) -> Option<String> {
    let groups = |pid: u32| {
        read_capped(std::path::Path::new(&format!("/proc/{pid}/stat")), 4096)
            .ok()
            .and_then(|stat| stat_process_groups(&stat))
    };
    let foreground = groups(root_pid).and_then(|(_, tpgid)| tpgid);
    let target = foreground_pid(root_pid, pids, foreground, |pid| {
        groups(pid).map(|(pgrp, _)| pgrp)
    });
    linux_command_for_pid(target)
}

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

#[cfg(target_os = "linux")]
pub fn scan_panes(roots: &[(u64, u32)]) -> std::collections::HashMap<u64, PaneScan> {
    let mut results: std::collections::HashMap<u64, PaneScan> = std::collections::HashMap::new();
    if roots.is_empty() {
        return results;
    }

    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut subtrees: Vec<(u64, Vec<u32>)> = Vec::with_capacity(roots.len());
    for &(key, root_pid) in roots {
        if root_pid == 0 {
            continue;
        }
        let pids = bfs_descendants_linux(root_pid, &mut visited);
        subtrees.push((key, pids));
    }

    let mut inode_owner: std::collections::HashMap<u64, (usize, u32)> =
        std::collections::HashMap::new();
    for (idx, (key, pids)) in subtrees.iter().enumerate() {
        let foreground_command = roots
            .iter()
            .find(|(root_key, _)| root_key == key)
            .and_then(|(_, root_pid)| linux_representative_command(*root_pid, pids));
        for &pid in pids {
            let mut inodes: Vec<u64> = Vec::new();
            socket_inodes_of(pid, &mut inodes);
            for inode in inodes {
                inode_owner.entry(inode).or_insert((idx, pid));
            }
        }

        results.insert(
            *key,
            PaneScan {
                ports: Vec::new(),
                foreground_command,
            },
        );
    }

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
        ports.sort_by_key(|e| (e.port, e.frontend.is_none()));
        ports.dedup_by_key(|e| e.port);
        if let Some(scan) = results.get_mut(key) {
            scan.ports = ports;
        }
    }

    results
}

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

#[cfg(target_os = "macos")]
fn listen_ports_of(pid: u32, ports: &mut Vec<u16>) {
    use libproc::libproc::bsd_info::BSDInfo;
    use libproc::libproc::file_info::{ListFDs, ProcFDType, pidfdinfo};
    use libproc::libproc::net_info::{SocketFDInfo, SocketInfoKind, TcpSIState};
    use libproc::libproc::proc_pid::{listpidinfo, pidinfo};

    let open_files = pidinfo::<BSDInfo>(pid as i32, 0)
        .ok()
        .map(|info| info.pbi_nfiles as usize);
    let Some(fds) = read_growing_list(open_files, |capacity| {
        listpidinfo::<ListFDs>(pid as i32, capacity).ok()
    }) else {
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

#[cfg(target_os = "macos")]
fn macos_representative_command(root_pid: u32, pids: &[u32]) -> Option<String> {
    use libproc::libproc::bsd_info::BSDInfo;
    use libproc::libproc::proc_pid::{name, pidinfo};

    const NO_CONTROLLING_TTY: u32 = u32::MAX;
    let info = |pid: u32| pidinfo::<BSDInfo>(pid as i32, 0).ok();
    let foreground = info(root_pid)
        .filter(|root| root.e_tdev != NO_CONTROLLING_TTY)
        .map(|root| root.e_tpgid);
    let pid = foreground_pid(root_pid, pids, foreground, |pid| {
        info(pid).map(|process| process.pbi_pgid)
    });
    name(pid as i32)
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(target_os = "macos")]
pub fn scan_panes(roots: &[(u64, u32)]) -> std::collections::HashMap<u64, PaneScan> {
    let mut results: std::collections::HashMap<u64, PaneScan> = std::collections::HashMap::new();
    if roots.is_empty() {
        return results;
    }

    let children_of = macos_children_map();
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &(key, root_pid) in roots {
        if root_pid == 0 {
            continue;
        }
        let pids = bfs_descendants_macos(root_pid, &children_of, &mut visited);

        let mut ports: Vec<PortEntry> = Vec::new();
        for &pid in &pids {
            let mut pid_ports: Vec<u16> = Vec::new();
            listen_ports_of(pid, &mut pid_ports);
            if pid_ports.is_empty() {
                continue;
            }
            let args = paneflow_host::process::process_argv(pid).unwrap_or_default();
            let frontend = classify_frontend_argv(args.iter().map(String::as_str));
            ports.extend(
                pid_ports
                    .into_iter()
                    .map(|port| PortEntry { port, frontend }),
            );
        }
        ports.sort_by_key(|e| (e.port, e.frontend.is_none()));
        ports.dedup_by_key(|e| e.port);

        results.insert(
            key,
            PaneScan {
                ports,
                foreground_command: macos_representative_command(root_pid, &pids),
            },
        );
    }

    results
}

#[cfg(windows)]
fn bfs_descendants_windows(
    root_pid: u32,
    entries: &[paneflow_host::process::WindowsProcessEntry],
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
fn windows_representative_command(
    root_pid: u32,
    entries: &[paneflow_host::process::WindowsProcessEntry],
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
        .map(|exe| crate::agent_launcher::executable_stem(exe).to_string())
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
pub fn scan_panes(roots: &[(u64, u32)]) -> std::collections::HashMap<u64, PaneScan> {
    let mut results: std::collections::HashMap<u64, PaneScan> = std::collections::HashMap::new();
    if roots.is_empty() {
        return results;
    }

    let entries = paneflow_host::process::windows_process_entries_named().unwrap_or_default();
    let exe_by_pid: std::collections::HashMap<u32, String> = entries
        .iter()
        .map(|entry| (entry.pid, entry.name.clone()))
        .collect();
    let listen_ports = windows_listen_ports_by_pid();

    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &(key, root_pid) in roots {
        if root_pid == 0 {
            continue;
        }
        let pids = bfs_descendants_windows(root_pid, &entries, &mut visited);
        let mut ports = Vec::new();
        for pid in pids {
            let Some(pid_ports) = listen_ports.get(&pid) else {
                continue;
            };
            let argv = paneflow_host::process::process_argv(pid).unwrap_or_default();
            let frontend = classify_frontend_argv(argv.iter().map(String::as_str)).or_else(|| {
                exe_by_pid.get(&pid).and_then(|exe| {
                    classify_frontend_argv(
                        [crate::agent_launcher::executable_stem(exe)].into_iter(),
                    )
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
                foreground_command: windows_representative_command(root_pid, &entries, &exe_by_pid),
            },
        );
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_representative_is_the_foreground_group_not_the_deepest_helper() {
        let groups =
            std::collections::HashMap::from([(10u32, 10u32), (20, 20), (30, 30), (31, 30)]);
        let pgid_of = |pid: u32| groups.get(&pid).copied();
        let pids = [10, 20, 30, 31];
        assert_eq!(foreground_pid(10, &pids, Some(30), pgid_of), 30);
        assert_eq!(
            foreground_pid(10, &pids, Some(10), pgid_of),
            10,
            "an idle shell is named after itself, not its background helper"
        );
        assert_eq!(foreground_pid(10, &pids, None, pgid_of), 10);
        assert_eq!(foreground_pid(10, &pids, Some(0), pgid_of), 10);
        assert_eq!(foreground_pid(10, &pids, Some(99), pgid_of), 10);
    }

    #[test]
    fn stat_process_groups_reads_the_group_and_the_terminal_foreground_group() {
        let stat = "4242 (my (odd) prog) S 4000 4242 4000 34816 4300 4194304 0 0";
        assert_eq!(stat_process_groups(stat), Some((4242, Some(4300))));
        let detached = "7 (daemon) S 1 7 7 0 -1 4194304";
        assert_eq!(stat_process_groups(detached), Some((7, None)));
        assert_eq!(stat_process_groups("garbage"), None);
    }

    #[test]
    fn a_descriptor_list_past_1024_entries_is_read_in_full() {
        let open: Vec<u32> = (0..1100).collect();
        let reads = std::cell::RefCell::new(Vec::new());
        let read = |capacity: usize| {
            reads.borrow_mut().push(capacity);
            Some(open.iter().copied().take(capacity).collect::<Vec<_>>())
        };
        let listed = read_growing_list(None, read).unwrap();
        assert_eq!(listed.len(), 1100, "the listener past 1024 is reached");
        assert_eq!(*reads.borrow(), vec![1024, 2048]);

        let mut first = true;
        let flaky = |capacity: usize| {
            if std::mem::take(&mut first) {
                Some(open.iter().copied().take(capacity).collect::<Vec<_>>())
            } else {
                None
            }
        };
        assert_eq!(
            read_growing_list(Some(1024), flaky).map(|list| list.len()),
            Some(1024),
            "a failed retry keeps what the first read found"
        );
    }

    #[test]
    fn command_from_nul_args_joins_argv() {
        assert_eq!(
            command_from_nul_args(b"cargo\0run\0--release\0"),
            Some("cargo run --release".to_string())
        );
        assert_eq!(
            command_from_nul_args(b"/opt/Program Files/node\0dev server.js\0"),
            Some("\"/opt/Program Files/node\" \"dev server.js\"".to_string())
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
        let argv = ["node", "/repo/node_modules/.bin/vite"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Vite"));
        let argv = ["bun", "/repo/node_modules/vite/bin/vite.js"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Vite"));
        let argv = ["next-server (v15.3.2)"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Next.js"));
        let argv = ["node", "/repo/node_modules/.bin/next", "dev"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Next.js"));
        let argv = ["node", "/usr/lib/node_modules/@angular/cli/bin/ng", "serve"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), Some("Angular"));
    }

    #[test]
    fn classify_frontend_argv_rejects_lookalikes() {
        let argv = ["node", "/srv/invite/server.js"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), None);
        let argv = ["node", "/srv/vitesse-app/index.js"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), None);
        let argv = ["python3", "-m", "http.server"];
        assert_eq!(classify_frontend_argv(argv.into_iter()), None);
        assert_eq!(classify_frontend_argv(std::iter::empty()), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_port_from_network_order_decodes_low_word() {
        assert_eq!(windows_port_from_network_order(0x901F), 8080);
    }

    #[test]
    fn scan_panes_detects_current_process_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let scan = scan_panes(&[(1, std::process::id())]);
        let ports = scan
            .get(&1)
            .map(|s| s.ports.iter().map(|e| e.port).collect::<Vec<_>>())
            .unwrap_or_default();

        assert!(
            ports.contains(&port),
            "scan_panes must detect a live listener owned by the root pid; got {ports:?}"
        );
    }

    #[test]
    fn scan_panes_ignores_pid_zero_roots() {
        let scan = scan_panes(&[(1, 0)]);
        assert!(
            scan.is_empty(),
            "pid 0 is a display-only sentinel and must not scan the system tree"
        );
    }
}
