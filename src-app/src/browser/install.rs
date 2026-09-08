use std::path::{Path, PathBuf};

use paneflow_browser_protocol::Availability;

const MANIFEST: &str = include_str!("../../../native/browser/manifest.toml");

pub const RUNTIME_SUBDIR: &str = "lib/paneflow/browser";
pub const HOST_SUBDIR: &str = "lib/paneflow/paneflow-browser-host";
pub const STAMP: &str = "verified-manifest.sha256";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Environment,
    Installed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub host_binary: PathBuf,
    pub runtime_root: PathBuf,
    pub source: Source,
}

pub fn target_triple() -> String {
    format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
}

pub fn declared_availability() -> Availability {
    declared_availability_in(MANIFEST, &target_triple())
}

pub fn declared_availability_in(manifest: &str, target: &str) -> Availability {
    let Ok(value) = toml::from_str::<toml::Value>(manifest) else {
        return Availability::Absent;
    };
    match value
        .get("targets")
        .and_then(|targets| targets.get(target))
        .and_then(|entry| entry.get("availability"))
        .and_then(toml::Value::as_str)
    {
        Some("development") => Availability::Development,
        Some("human_qualified") => Availability::HumanQualified,
        Some("agent_qualified") => Availability::AgentQualified,
        _ => Availability::Absent,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandbox {
    Setuid,
    UserNamespace,
}

impl Sandbox {
    pub fn label(self) -> &'static str {
        match self {
            Self::Setuid => "root-owned setuid helper",
            Self::UserNamespace => "unprivileged user-namespace",
        }
    }
}

pub fn sandbox_mechanism(runtime_root: &Path) -> Result<Sandbox, String> {
    let helper = runtime_root.join("Release/chrome-sandbox");
    if setuid_helper(&helper) {
        return Ok(Sandbox::Setuid);
    }
    unprivileged_user_namespaces().map(|()| Sandbox::UserNamespace).map_err(|reason| {
        format!(
            "the browser sandbox is unavailable: {reason}, and {} is not a root-owned setuid helper",
            helper.display()
        )
    })
}

fn setuid_helper(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o4111 == 0o4111
    })
}

fn unprivileged_user_namespaces() -> Result<(), String> {
    if read_sysctl("/proc/sys/user/max_user_namespaces").is_some_and(|value| value == 0) {
        return Err("user.max_user_namespaces is 0".to_string());
    }
    if read_sysctl("/proc/sys/kernel/unprivileged_userns_clone").is_some_and(|value| value == 0) {
        return Err("kernel.unprivileged_userns_clone is 0".to_string());
    }
    if read_sysctl("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .is_some_and(|value| value != 0)
    {
        return Err("kernel.apparmor_restrict_unprivileged_userns is enabled".to_string());
    }
    Ok(())
}

fn read_sysctl(path: &str) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

pub fn locate() -> Option<Layout> {
    from_environment(
        std::env::var_os(super::RUNTIME_ENV).map(PathBuf::from),
        std::env::var_os(super::HOST_ENV).map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
    )
    .or_else(|| from_prefix(std::env::current_exe().ok()?.as_path()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready(Layout, Sandbox),
    Unusable(String),
    Absent,
}

pub fn detect() -> Readiness {
    match locate() {
        Some(layout) => classify(layout, &super::supervisor::manifest_digest()),
        None => Readiness::Absent,
    }
}

pub fn classify(layout: Layout, expected: &str) -> Readiness {
    match std::fs::read_to_string(layout.runtime_root.join(STAMP)) {
        Ok(stamp) if stamp.trim() == expected => match sandbox_mechanism(&layout.runtime_root) {
            Ok(sandbox) => Readiness::Ready(layout, sandbox),
            Err(reason) => Readiness::Unusable(reason),
        },
        Ok(stamp) => Readiness::Unusable(format!(
            "the installed browser runtime at {} was verified against manifest {}, but this build expects {}",
            layout.runtime_root.display(),
            stamp.trim(),
            expected
        )),
        Err(error) if layout.source == Source::Environment => Readiness::Unusable(format!(
            "the browser runtime at {} has no verification stamp: {error}",
            layout.runtime_root.display()
        )),
        Err(error) => Readiness::Unusable(format!(
            "the installed browser runtime at {} is unreadable: {error}",
            layout.runtime_root.display()
        )),
    }
}

pub fn from_environment(
    runtime_root: Option<PathBuf>,
    host_binary: Option<PathBuf>,
    current_exe: Option<&Path>,
) -> Option<Layout> {
    let runtime_root = runtime_root?;
    let host_binary = host_binary.or_else(|| {
        current_exe?
            .parent()
            .map(|dir| dir.join("paneflow-browser-host"))
    })?;
    Some(Layout {
        host_binary,
        runtime_root,
        source: Source::Environment,
    })
}

pub fn from_prefix(current_exe: &Path) -> Option<Layout> {
    let prefix = current_exe.parent()?.parent()?;
    let runtime_root = prefix.join(RUNTIME_SUBDIR);
    let host_binary = prefix.join(HOST_SUBDIR);
    if !runtime_root.join(STAMP).is_file() || !host_binary.is_file() {
        return None;
    }
    Some(Layout {
        host_binary,
        runtime_root,
        source: Source::Installed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "paneflow-browser-install-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|moment| moment.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn install(prefix: &Path) -> PathBuf {
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::create_dir_all(prefix.join(RUNTIME_SUBDIR)).unwrap();
        std::fs::create_dir_all(prefix.join(HOST_SUBDIR).parent().unwrap()).unwrap();
        std::fs::write(prefix.join("bin/paneflow"), b"app").unwrap();
        std::fs::write(prefix.join(HOST_SUBDIR), b"host").unwrap();
        std::fs::write(prefix.join(RUNTIME_SUBDIR).join(STAMP), b"digest\n").unwrap();
        prefix.join("bin/paneflow")
    }

    #[test]
    fn an_installed_prefix_is_discovered_without_any_environment_variable() {
        let root = scratch("prefix");
        let exe = install(&root.join("usr"));
        let layout = from_prefix(&exe).unwrap();
        assert_eq!(layout.source, Source::Installed);
        assert_eq!(layout.runtime_root, root.join("usr").join(RUNTIME_SUBDIR));
        assert_eq!(layout.host_binary, root.join("usr").join(HOST_SUBDIR));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_prefix_without_a_verification_stamp_or_host_is_not_an_installation() {
        let root = scratch("partial");
        let exe = install(&root.join("usr"));
        std::fs::remove_file(root.join("usr").join(RUNTIME_SUBDIR).join(STAMP)).unwrap();
        assert_eq!(from_prefix(&exe), None);
        std::fs::write(root.join("usr").join(RUNTIME_SUBDIR).join(STAMP), b"d").unwrap();
        std::fs::remove_file(root.join("usr").join(HOST_SUBDIR)).unwrap();
        assert_eq!(from_prefix(&exe), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_environment_override_keeps_the_development_source_and_the_sibling_host() {
        let layout = from_environment(
            Some(PathBuf::from("/tmp/runtime")),
            None,
            Some(Path::new("/tmp/target/release/paneflow")),
        )
        .unwrap();
        assert_eq!(layout.source, Source::Environment);
        assert_eq!(
            layout.host_binary,
            PathBuf::from("/tmp/target/release/paneflow-browser-host")
        );
        assert_eq!(
            from_environment(None, Some(PathBuf::from("/tmp/host")), None),
            None
        );
    }

    #[test]
    fn a_runtime_verified_against_another_manifest_reports_a_repairable_installation() {
        if sandbox_mechanism(Path::new("/nonexistent")).is_err() {
            return;
        }
        let root = scratch("classify");
        let exe = install(&root.join("usr"));
        let layout = from_prefix(&exe).unwrap();
        match classify(layout.clone(), "digest") {
            Readiness::Ready(found, _) => assert_eq!(found, layout),
            other => panic!("a matching stamp must be Ready: {other:?}"),
        }
        let Readiness::Unusable(reason) = classify(layout.clone(), "other") else {
            panic!("a foreign stamp must not be Ready");
        };
        assert!(
            reason.contains("digest") && reason.contains("other"),
            "{reason}"
        );
        std::fs::remove_file(layout.runtime_root.join(STAMP)).unwrap();
        assert!(matches!(classify(layout, "digest"), Readiness::Unusable(_)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_capability_manifest_decides_the_distributed_availability() {
        let manifest = |value: &str| {
            format!(
                "contract_version = 3\n[targets.\"x86_64-unknown-linux-gnu\"]\navailability = \"{value}\"\n"
            )
        };
        let target = "x86_64-unknown-linux-gnu";
        assert_eq!(
            declared_availability_in(&manifest("development"), target),
            Availability::Development
        );
        assert_eq!(
            declared_availability_in(&manifest("human_qualified"), target),
            Availability::HumanQualified
        );
        assert_eq!(
            declared_availability_in(&manifest("agent_qualified"), target),
            Availability::AgentQualified
        );
        assert_eq!(
            declared_availability_in(&manifest("absent"), target),
            Availability::Absent
        );
        assert_eq!(
            declared_availability_in(&manifest("development"), "aarch64-unknown-linux-gnu"),
            Availability::Absent
        );
        assert!(
            matches!(
                declared_availability(),
                Availability::Absent | Availability::Development
            ),
            "the shipped manifest must stay below a qualified capability until the US-028 release verdict"
        );
    }
}
