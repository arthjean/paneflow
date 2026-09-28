use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(windows)]
const PROBE_SCRIPT: &str =
    r#"printf '%s\n%s\n%s\n' "$(wslpath -w /)" "$HOME" "$(wslpath -u C:/ 2>/dev/null)""#;

static CACHED: Mutex<Option<Arc<WslRoots>>> = Mutex::new(None);

#[derive(Debug, PartialEq, Eq)]
pub(super) struct WslRoots {
    distro_root: PathBuf,
    home: String,
    mount_root: Option<String>,
}

impl WslRoots {
    pub(super) fn parse(probe_output: &str) -> Option<Self> {
        let mut lines = probe_output.lines();
        let distro_root = lines.next().filter(|line| line.starts_with(r"\\"))?;
        let home = lines.next().filter(|line| line.starts_with('/'))?;
        let mount_root = lines
            .next()
            .and_then(|line| line.strip_suffix("c/"))
            .filter(|root| root.starts_with('/') && root.ends_with('/'));
        Some(Self {
            distro_root: PathBuf::from(distro_root),
            home: home.to_owned(),
            mount_root: mount_root.map(str::to_owned),
        })
    }

    pub(super) fn home(&self) -> PathBuf {
        self.to_windows(&self.home)
    }

    pub(super) fn to_windows(&self, linux: &str) -> PathBuf {
        let (mut path, rest) = match self
            .mount_root
            .as_deref()
            .and_then(|root| drive_of(linux, root))
        {
            Some((drive, rest)) => (
                PathBuf::from(format!("{}:\\", drive.to_ascii_uppercase())),
                rest,
            ),
            None => (self.distro_root.clone(), linux),
        };
        path.extend(rest.split('/').filter(|segment| !segment.is_empty()));
        path
    }

    pub(super) fn to_linux(&self, path: &Path) -> Option<String> {
        if let Ok(rest) = path.strip_prefix(&self.distro_root) {
            let linux = append_components(String::new(), rest)?;
            return Some(if linux.is_empty() {
                "/".to_owned()
            } else {
                linux
            });
        }
        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return None;
        };
        let (Prefix::Disk(drive) | Prefix::VerbatimDisk(drive)) = prefix.kind() else {
            return None;
        };
        let mount = format!(
            "{}{}",
            self.mount_root.as_deref()?,
            char::from(drive).to_ascii_lowercase()
        );
        append_components(mount, components.as_path())
    }

    pub(super) fn display(&self, path: &Path) -> Option<String> {
        let linux = self.to_linux(path)?;
        Some(match linux.strip_prefix(&self.home) {
            Some("") => "~".to_owned(),
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => linux,
        })
    }
}

pub(super) fn roots() -> Option<Arc<WslRoots>> {
    let mut cached = CACHED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(roots) = cached.as_ref() {
        return Some(roots.clone());
    }
    let roots = Arc::new(WslRoots::parse(&probe()?)?);
    *cached = Some(roots.clone());
    Some(roots)
}

#[cfg(windows)]
fn probe() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("wsl.exe")
        .args(["--exec", "sh", "-c", PROBE_SCRIPT])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .output()
        .inspect_err(|error| log::warn!("path picker: could not start wsl.exe: {error}"))
        .ok()?;
    if !output.status.success() {
        log::warn!("path picker: the WSL probe exited with {}", output.status);
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(not(windows))]
fn probe() -> Option<String> {
    None
}

fn drive_of<'a>(linux: &'a str, mount_root: &str) -> Option<(char, &'a str)> {
    let rest = linux.strip_prefix(mount_root)?;
    let mut chars = rest.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    let rest = chars.as_str();
    (rest.is_empty() || rest.starts_with('/')).then_some((drive, rest))
}

fn append_components(mut linux: String, rest: &Path) -> Option<String> {
    for component in rest.components() {
        if let Component::Normal(name) = component {
            linux.push('/');
            linux.push_str(name.to_str()?);
        }
    }
    Some(linux)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE_OUTPUT: &str = "\\\\wsl.localhost\\Ubuntu\\\n/home/me\n/mnt/c/\n";

    fn roots() -> WslRoots {
        WslRoots::parse(PROBE_OUTPUT).expect("valid probe output")
    }

    #[test]
    fn the_probe_output_yields_the_distro_root_home_and_mount_root() {
        assert_eq!(
            roots(),
            WslRoots {
                distro_root: PathBuf::from(r"\\wsl.localhost\Ubuntu\"),
                home: "/home/me".to_owned(),
                mount_root: Some("/mnt/".to_owned()),
            }
        );
        let without_automount = WslRoots::parse("\\\\wsl$\\Debian\\\n/root\n\n").expect("parsed");
        assert_eq!(without_automount.mount_root, None);
        assert_eq!(WslRoots::parse("wsl: error\n"), None);
        assert_eq!(WslRoots::parse(""), None);
    }

    #[test]
    fn a_mounted_drive_is_recognized_only_at_a_segment_boundary() {
        assert_eq!(drive_of("/mnt/c/dev", "/mnt/"), Some(('c', "/dev")));
        assert_eq!(drive_of("/mnt/d", "/mnt/"), Some(('d', "")));
        assert_eq!(drive_of("/mnt/wsl/x", "/mnt/"), None);
        assert_eq!(drive_of("/home/me", "/mnt/"), None);
        assert_eq!(drive_of("/c/dev", "/"), Some(('c', "/dev")));
    }

    #[cfg(windows)]
    #[test]
    fn linux_paths_map_to_a_drive_or_the_distro_share() {
        let roots = roots();
        assert_eq!(
            roots.to_windows("/mnt/c/dev/app"),
            PathBuf::from(r"C:\dev\app")
        );
        assert_eq!(roots.to_windows("/mnt/d"), PathBuf::from(r"D:\"));
        assert_eq!(
            roots.to_windows("/home/me/project/"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\project")
        );
        assert_eq!(
            roots.to_windows("/"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\")
        );
        assert_eq!(
            roots.home(),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_map_back_to_linux_paths() {
        let roots = roots();
        assert_eq!(
            roots
                .to_linux(Path::new(r"\\wsl.localhost\Ubuntu\home\me\a b.txt"))
                .as_deref(),
            Some("/home/me/a b.txt")
        );
        assert_eq!(
            roots
                .to_linux(Path::new(r"\\wsl.localhost\Ubuntu\"))
                .as_deref(),
            Some("/")
        );
        assert_eq!(
            roots.to_linux(Path::new(r"C:\dev\app")).as_deref(),
            Some("/mnt/c/dev/app")
        );
        assert_eq!(roots.to_linux(Path::new(r"\\server\share\x")), None);
        let without_automount = WslRoots::parse("\\\\wsl$\\Debian\\\n/root\n\n").expect("parsed");
        assert_eq!(without_automount.to_linux(Path::new(r"C:\dev")), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_display_form_abbreviates_the_linux_home() {
        let roots = roots();
        assert_eq!(
            roots
                .display(Path::new(r"\\wsl.localhost\Ubuntu\home\me"))
                .as_deref(),
            Some("~")
        );
        assert_eq!(
            roots
                .display(Path::new(r"\\wsl.localhost\Ubuntu\home\me\src"))
                .as_deref(),
            Some("~/src")
        );
        assert_eq!(
            roots
                .display(Path::new(r"\\wsl.localhost\Ubuntu\home\meadow"))
                .as_deref(),
            Some("/home/meadow")
        );
        assert_eq!(
            roots.display(Path::new(r"C:\dev")).as_deref(),
            Some("/mnt/c/dev")
        );
    }
}
