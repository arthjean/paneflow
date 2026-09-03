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
    pub agents: Vec<String>,
    pub foreground_command: Option<String>,
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
const MAX_PIDS_PER_ROOT: usize = 512;

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

#[cfg(target_os = "linux")]
fn linux_representative_command(root_pid: u32, pids: &[u32]) -> Option<String> {
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
pub fn scan_panes(
    roots: &[(u64, u32)],
    agent_binaries: &[&str],
) -> std::collections::HashMap<u64, PaneScan> {
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
    use libproc::libproc::file_info::{ListFDs, ProcFDType, pidfdinfo};
    use libproc::libproc::net_info::{SocketFDInfo, SocketInfoKind, TcpSIState};
    use libproc::libproc::proc_pid::listpidinfo;

    const MAX_FDS_PER_PROC: usize = 1024;

    let Ok(fds) = listpidinfo::<ListFDs>(pid as i32, MAX_FDS_PER_PROC) else {
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
fn argv_of_macos(pid: u32) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];

    let mut size: libc::size_t = 0;
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

    let mut buf = vec![0u8; size];
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
fn macos_representative_command(pids: &[u32]) -> Option<String> {
    use libproc::libproc::proc_pid::name;

    let pid = pids.last().copied()?;
    name(pid as i32)
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

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

    let children_of = macos_children_map();
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &(key, root_pid) in roots {
        if root_pid == 0 {
            continue;
        }
        let pids = bfs_descendants_macos(root_pid, &children_of, &mut visited);

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
            let args = argv_of_macos(pid);
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
                agents,
                foreground_command: macos_representative_command(&pids),
            },
        );
    }

    results
}

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

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Vec::new();
    }

    let mut entries = Vec::with_capacity(256);
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
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
            if unsafe { Process32NextW(snap, &mut entry) } == 0 {
                break;
            }
        }
    }
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
fn windows_representative_command(
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
    let argv = unsafe { CommandLineToArgvW(wide.as_mut_ptr(), &mut argc) };
    if argv.is_null() || argc <= 0 {
        return command_line
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }

    let mut args = Vec::with_capacity(argc as usize);
    let slice = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
    for &ptr in slice {
        if ptr.is_null() {
            continue;
        }
        let mut len = 0usize;
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
            args.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                ptr, len,
            )));
        }
    }
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
                foreground_command: windows_representative_command(root_pid, &entries, &exe_by_pid),
            },
        );
    }

    results
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn scan_panes(
    _roots: &[(u64, u32)],
    _agent_binaries: &[&str],
) -> std::collections::HashMap<u64, PaneScan> {
    std::collections::HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_in_bfs_order_picks_nearest_root_first_and_dedups() {
        let comms = ["zsh", "claude", "node", "codex", "claude"];
        let agents = agents_in_bfs_order(comms.into_iter(), &["claude", "codex", "opencode"]);
        assert_eq!(agents, vec!["claude".to_string(), "codex".to_string()]);
    }

    #[test]
    fn agents_in_bfs_order_exact_match_only() {
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_scan_panes_detects_live_child_subtree() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
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
