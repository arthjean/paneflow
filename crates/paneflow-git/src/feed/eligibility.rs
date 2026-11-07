use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Ineligible {
    #[error("{} is on a network file system ({kind})", path.display())]
    NetworkFileSystem { path: PathBuf, kind: String },
    #[error("{} is on a FUSE file system ({kind})", path.display())]
    Fuse { path: PathBuf, kind: String },
    #[error("{} is a Windows drive mounted into WSL ({kind})", path.display())]
    WslDrive { path: PathBuf, kind: String },
    #[error("{} is a WSL path seen from Windows", path.display())]
    WslNetworkPath { path: PathBuf },
    #[error("{} is on a file system whose events are not vouched for ({kind})", path.display())]
    UnsupportedFileSystem { path: PathBuf, kind: String },
    #[error("could not identify the file system of {}: {cause}", path.display())]
    Unidentified { path: PathBuf, cause: String },
    #[error("no Paneflow cache directory to hold the fence marker")]
    NoCacheDirectory,
    #[error("could not create the fence marker directory {}: {cause}", path.display())]
    MarkerDirectory { path: PathBuf, cause: String },
    #[error("{needed} more inotify watches would pass half of max_user_watches ({limit}) with {in_use} already in use")]
    WatchBudget {
        needed: usize,
        in_use: usize,
        limit: usize,
    },
    #[error("could not scan the worktree: {cause}")]
    Scan { cause: String },
    #[error("could not register the watches: {cause}")]
    Registration { cause: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "each platform builds only the verdicts its own file system check can reach"
    )
)]
pub(crate) enum Verdict {
    Local,
    Network,
    Fuse,
    WslDrive,
    WslNetworkPath,
    Unsupported,
}

impl Verdict {
    fn into_result(self, path: &Path, kind: &str) -> Result<(), Ineligible> {
        let path = path.to_path_buf();
        let kind = kind.to_string();
        match self {
            Verdict::Local => Ok(()),
            Verdict::Network => Err(Ineligible::NetworkFileSystem { path, kind }),
            Verdict::Fuse => Err(Ineligible::Fuse { path, kind }),
            Verdict::WslDrive => Err(Ineligible::WslDrive { path, kind }),
            Verdict::WslNetworkPath => Err(Ineligible::WslNetworkPath { path }),
            Verdict::Unsupported => Err(Ineligible::UnsupportedFileSystem { path, kind }),
        }
    }
}

#[cfg(any(target_os = "linux", test))]
const LINUX_LOCAL_FILE_SYSTEMS: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "btrfs", "tmpfs", "ramfs", "overlay", "f2fs", "jfs", "reiserfs",
    "nilfs2", "ubifs", "zfs",
];

#[cfg(any(target_os = "linux", test))]
const LINUX_NETWORK_FILE_SYSTEMS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "ceph",
    "glusterfs",
    "afs",
    "lustre",
    "gpfs",
    "9p",
    "virtiofs",
    "sshfs",
];

#[cfg(any(target_os = "linux", test))]
pub(crate) fn classify_linux_mount(kind: &str, source: &str, options: &str) -> Verdict {
    if kind == "drvfs"
        || (kind == "9p"
            && (source == "drvfs" || options.split([',', ';']).any(|o| o == "aname=drvfs")))
    {
        return Verdict::WslDrive;
    }
    if kind == "fuse" || kind == "fuseblk" || kind.starts_with("fuse.") {
        return Verdict::Fuse;
    }
    if LINUX_NETWORK_FILE_SYSTEMS.contains(&kind) {
        return Verdict::Network;
    }
    if LINUX_LOCAL_FILE_SYSTEMS.contains(&kind) {
        return Verdict::Local;
    }
    Verdict::Unsupported
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mount {
    pub(crate) point: PathBuf,
    pub(crate) kind: String,
    pub(crate) source: String,
    pub(crate) options: String,
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let separator = fields.iter().position(|field| *field == "-")?;
            if separator < 6 || fields.len() < separator + 3 {
                return None;
            }
            Some(Mount {
                point: PathBuf::from(decode_mountinfo_path(fields[4])),
                kind: fields[separator + 1].to_string(),
                source: fields[separator + 2].to_string(),
                options: fields.get(separator + 3).copied().unwrap_or("").to_string(),
            })
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn decode_mountinfo_path(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = bytes.get(index + 1..index + 4).filter(|digits| {
            bytes[index] == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
        });
        match escape {
            Some(digits) => {
                let value = digits
                    .iter()
                    .fold(0u32, |value, digit| value * 8 + u32::from(digit - b'0'));
                decoded.push(u8::try_from(value).unwrap_or(b'?'));
                index += 4;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn mount_of<'a>(mounts: &'a [Mount], path: &Path) -> Option<&'a Mount> {
    let mut best: Option<&Mount> = None;
    for mount in mounts {
        if !path.starts_with(&mount.point) {
            continue;
        }
        let deeper = best
            .is_none_or(|best| mount.point.components().count() >= best.point.components().count());
        if deeper {
            best = Some(mount);
        }
    }
    best
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn classify_macos_file_system(kind: &str, local: bool) -> Verdict {
    if kind.starts_with("macfuse") || kind.starts_with("osxfuse") || kind.starts_with("fuse") {
        return Verdict::Fuse;
    }
    if matches!(
        kind,
        "nfs" | "smbfs" | "afpfs" | "webdav" | "cifs" | "ftp" | "cddafs"
    ) || !local
    {
        return Verdict::Network;
    }
    if matches!(kind, "apfs" | "hfs") {
        return Verdict::Local;
    }
    Verdict::Unsupported
}

#[cfg(any(windows, test))]
pub(crate) fn classify_windows_path(path: &str) -> Option<Verdict> {
    let normalized = path.replace('/', "\\");
    let server = match normalized
        .strip_prefix("\\\\?\\UNC\\")
        .or_else(|| normalized.strip_prefix("\\\\.\\UNC\\"))
    {
        Some(rest) => rest,
        None if normalized.starts_with("\\\\?\\") || normalized.starts_with("\\\\.\\") => {
            return None;
        }
        None => normalized.strip_prefix("\\\\")?,
    };
    let host = server.split('\\').next().unwrap_or("").to_ascii_lowercase();
    if host == "wsl$" || host == "wsl.localhost" {
        Some(Verdict::WslNetworkPath)
    } else {
        Some(Verdict::Network)
    }
}

#[cfg(any(windows, test))]
pub(crate) fn classify_windows_volume(file_system: &str, remote: bool) -> Verdict {
    if remote {
        return Verdict::Network;
    }
    if file_system.eq_ignore_ascii_case("NTFS") || file_system.eq_ignore_ascii_case("ReFS") {
        Verdict::Local
    } else {
        Verdict::Unsupported
    }
}

pub fn check_file_system(path: &Path) -> Result<(), Ineligible> {
    platform::check(path)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;

    use super::{classify_linux_mount, mount_of, parse_mountinfo, Ineligible};

    pub(super) fn check(path: &Path) -> Result<(), Ineligible> {
        let unidentified = |cause: String| Ineligible::Unidentified {
            path: path.to_path_buf(),
            cause,
        };
        let text = std::fs::read_to_string("/proc/self/mountinfo")
            .map_err(|error| unidentified(error.to_string()))?;
        let mounts = parse_mountinfo(&text);
        let mount = mount_of(&mounts, path)
            .ok_or_else(|| unidentified("no mount holds the path".to_string()))?;
        classify_linux_mount(&mount.kind, &mount.source, &mount.options)
            .into_result(path, &mount.kind)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::{classify_macos_file_system, Ineligible};

    pub(super) fn check(path: &Path) -> Result<(), Ineligible> {
        let unidentified = |cause: String| Ineligible::Unidentified {
            path: path.to_path_buf(),
            cause,
        };
        let raw = CString::new(path.as_os_str().as_bytes())
            .map_err(|error| unidentified(error.to_string()))?;
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(raw.as_ptr(), &mut stat) } != 0 {
            return Err(unidentified(std::io::Error::last_os_error().to_string()));
        }
        let kind = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let local = stat.f_flags & (libc::MNT_LOCAL as u32) != 0;
        classify_macos_file_system(&kind, local).into_result(path, &kind)
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

    use super::{classify_windows_path, classify_windows_volume, Ineligible};

    const VOLUME_PATH_CAPACITY: usize = 32 * 1024;

    pub(super) fn check(path: &Path) -> Result<(), Ineligible> {
        let display = path.to_string_lossy();
        if let Some(verdict) = classify_windows_path(&display) {
            return verdict.into_result(path, "unc");
        }
        let unidentified = |cause: String| Ineligible::Unidentified {
            path: path.to_path_buf(),
            cause,
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut volume = vec![0u16; VOLUME_PATH_CAPACITY];
        let capacity = u32::try_from(volume.len()).unwrap_or(u32::MAX);
        if unsafe { GetVolumePathNameW(wide.as_ptr(), volume.as_mut_ptr(), capacity) } == 0 {
            return Err(unidentified(std::io::Error::last_os_error().to_string()));
        }
        let end = volume
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(volume.len());
        volume.truncate(end);
        let root = String::from_utf16_lossy(&volume);
        if let Some(verdict) = classify_windows_path(&root) {
            return verdict.into_result(path, "unc");
        }
        volume.push(0);
        let remote = unsafe { GetDriveTypeW(volume.as_ptr()) } == DRIVE_REMOTE;
        let mut name = [0u16; 64];
        let name_capacity = u32::try_from(name.len()).unwrap_or(0);
        let described = unsafe {
            GetVolumeInformationW(
                volume.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                name.as_mut_ptr(),
                name_capacity,
            )
        };
        if described == 0 {
            return Err(unidentified(std::io::Error::last_os_error().to_string()));
        }
        let end = name
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(name.len());
        let kind = String::from_utf16_lossy(&name[..end]);
        classify_windows_volume(&kind, remote).into_result(path, &kind)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use std::path::Path;

    use super::Ineligible;

    pub(super) fn check(path: &Path) -> Result<(), Ineligible> {
        Err(Ineligible::UnsupportedFileSystem {
            path: path.to_path_buf(),
            kind: std::env::consts::OS.to_string(),
        })
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn check_budget(needed: usize, in_use: usize, limit: usize) -> Result<(), Ineligible> {
    if in_use.saturating_add(needed) > limit / 2 {
        return Err(Ineligible::WatchBudget {
            needed,
            in_use,
            limit,
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) const DEFAULT_MAX_USER_WATCHES: usize = 8192;

#[cfg(target_os = "linux")]
pub(crate) fn max_user_watches() -> usize {
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(DEFAULT_MAX_USER_WATCHES)
}

#[cfg(target_os = "linux")]
pub fn inotify_watches_in_use() -> usize {
    let Ok(entries) = std::fs::read_dir("/proc/self/fdinfo") else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .map(|info| {
            info.lines()
                .filter(|line| line.starts_with("inotify wd:"))
                .count()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        check_budget, classify_linux_mount, classify_macos_file_system, classify_windows_path,
        classify_windows_volume, mount_of, parse_mountinfo, Ineligible, Verdict,
    };

    const MOUNTINFO: &str = "\
22 1 8:32 / / rw,relatime - ext4 /dev/sdc rw,discard,errors=remount-ro
23 22 0:5 / /dev rw,nosuid - devtmpfs none rw,size=8000000k
41 22 0:46 / /mnt/c rw,noatime - 9p drvfs rw,aname=drvfs;path=C:\\;uid=1000;gid=1000;symlinkroot=/mnt/
42 22 0:47 / /mnt/nfs rw - nfs4 server:/export rw,vers=4.2
43 22 0:48 / /mnt/smb rw - cifs //server/share rw,vers=3.1.1
44 22 0:49 / /home/me/remote rw,nosuid - fuse.sshfs me@host:/srv rw,user_id=1000
45 22 0:50 / /home/me/with\\040space rw - xfs /dev/sdd rw
46 22 0:51 / /run/user/1000 rw - tmpfs tmpfs rw,mode=700
";

    fn verdict_for(path: &str) -> Verdict {
        let mounts = parse_mountinfo(MOUNTINFO);
        let mount = mount_of(&mounts, Path::new(path)).expect("a mount holds every path");
        classify_linux_mount(&mount.kind, &mount.source, &mount.options)
    }

    #[test]
    fn linux_mounts_are_classified_by_the_deepest_mount_holding_the_path() {
        assert_eq!(verdict_for("/home/me/repo"), Verdict::Local);
        assert_eq!(verdict_for("/mnt/c/dev/paneflow"), Verdict::WslDrive);
        assert_eq!(verdict_for("/mnt/nfs/repo"), Verdict::Network);
        assert_eq!(verdict_for("/mnt/smb/repo"), Verdict::Network);
        assert_eq!(verdict_for("/home/me/remote/repo"), Verdict::Fuse);
        assert_eq!(verdict_for("/home/me/with space/repo"), Verdict::Local);
        assert_eq!(verdict_for("/run/user/1000/marker"), Verdict::Local);
    }

    #[test]
    fn unknown_linux_file_systems_are_not_vouched_for() {
        assert_eq!(
            classify_linux_mount("vfat", "/dev/sdb1", "rw"),
            Verdict::Unsupported
        );
        assert_eq!(
            classify_linux_mount("9p", "host", "rw,trans=virtio"),
            Verdict::Network
        );
        assert_eq!(
            classify_linux_mount("fuseblk", "/dev/sdb2", "rw"),
            Verdict::Fuse
        );
        assert_eq!(classify_linux_mount("drvfs", "C:", "rw"), Verdict::WslDrive);
    }

    #[test]
    fn mountinfo_paths_decode_octal_escapes() {
        let mounts = parse_mountinfo(MOUNTINFO);
        assert!(mounts
            .iter()
            .any(|mount| mount.point == Path::new("/home/me/with space")));
    }

    #[test]
    fn macos_file_systems_need_a_local_apfs_or_hfs_volume() {
        assert_eq!(classify_macos_file_system("apfs", true), Verdict::Local);
        assert_eq!(classify_macos_file_system("hfs", true), Verdict::Local);
        assert_eq!(classify_macos_file_system("smbfs", false), Verdict::Network);
        assert_eq!(classify_macos_file_system("nfs", false), Verdict::Network);
        assert_eq!(classify_macos_file_system("apfs", false), Verdict::Network);
        assert_eq!(classify_macos_file_system("macfuse", true), Verdict::Fuse);
        assert_eq!(
            classify_macos_file_system("msdos", true),
            Verdict::Unsupported
        );
    }

    #[test]
    fn windows_unc_paths_to_wsl_and_other_servers_are_refused() {
        for path in [
            r"\\wsl$\Ubuntu\home\me\repo",
            r"\\wsl.localhost\Ubuntu-26.04\home\me\repo",
            r"\\?\UNC\wsl.localhost\Ubuntu\home",
            r"\\WSL$\Debian\srv",
            "//wsl.localhost/Ubuntu/home",
        ] {
            assert_eq!(
                classify_windows_path(path),
                Some(Verdict::WslNetworkPath),
                "{path}"
            );
        }
        for path in [r"\\server\share\repo", r"\\?\UNC\nas\git\repo"] {
            assert_eq!(
                classify_windows_path(path),
                Some(Verdict::Network),
                "{path}"
            );
        }
        for path in [r"C:\dev\paneflow", r"\\?\C:\dev\paneflow", r"\\.\D:\repo"] {
            assert_eq!(classify_windows_path(path), None, "{path}");
        }
    }

    #[test]
    fn windows_volumes_need_local_ntfs_or_refs() {
        assert_eq!(classify_windows_volume("NTFS", false), Verdict::Local);
        assert_eq!(classify_windows_volume("ReFS", false), Verdict::Local);
        assert_eq!(classify_windows_volume("NTFS", true), Verdict::Network);
        assert_eq!(
            classify_windows_volume("FAT32", false),
            Verdict::Unsupported
        );
        assert_eq!(
            classify_windows_volume("exFAT", false),
            Verdict::Unsupported
        );
    }

    #[test]
    fn the_watch_budget_keeps_half_of_max_user_watches_for_other_programs() {
        assert_eq!(check_budget(4_828, 0, 524_288), Ok(()));
        assert_eq!(check_budget(4_096, 0, 8_192), Ok(()));
        assert_eq!(
            check_budget(4_097, 0, 8_192),
            Err(Ineligible::WatchBudget {
                needed: 4_097,
                in_use: 0,
                limit: 8_192
            })
        );
        assert!(check_budget(100, 4_000, 8_192).is_err());
    }
}
