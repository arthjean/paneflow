use super::owned_files::PANEFLOW_TS_BASENAME;
use super::{
    home_unavailable, paneflow_ipc_reachable, refuse_symlink, with_last_lease, with_orphan_lease,
    HookInstall, HookInstallResult, HookInstallSkip, HookLease,
};
use paneflow_agent_config::{
    config_dir, read_optional_text, with_config_lock, write_json_atomic, write_text_atomic,
};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const OPENCODE_PLUGIN_SOURCE: &str = include_str!("../../assets/opencode-paneflow-status.ts");

fn opencode_config_dir() -> Option<PathBuf> {
    config_dir().map(|directory| directory.join("opencode"))
}

pub(crate) struct OpenCodePluginGuard {
    plugin_path: PathBuf,
    config_path: PathBuf,
    created_config: bool,
    lease: HookLease,
}

impl OpenCodePluginGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let directory = opencode_config_dir().ok_or_else(home_unavailable)?;
        if !paneflow_ipc_reachable() {
            Self::sweep_orphan(&directory);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        Self::install_at(&directory).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(directory: &Path) -> std::io::Result<Self> {
        let config_path = directory.join("opencode.json");
        if !config_path.exists() && directory.join("opencode.jsonc").exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "OpenCode uses opencode.jsonc; refusing a competing JSON config",
            ));
        }

        let plugins_dir = directory.join("plugins");
        refuse_symlink(&plugins_dir, "OpenCode plugin")?;
        std::fs::create_dir_all(&plugins_dir)?;
        let plugin_path = plugins_dir.join(PANEFLOW_TS_BASENAME);
        let plugin_config_path = plugin_path.to_str().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "OpenCode plugin path is not valid Unicode",
            )
        })?;
        let mut lease = HookLease::acquire(&plugin_path)?;
        with_config_lock(&plugin_path, || {
            write_text_atomic(&plugin_path, OPENCODE_PLUGIN_SOURCE)
        })?;

        let result = with_config_lock(&config_path, || {
            let existing = read_optional_text(&config_path)?;
            let created_config = existing.is_none();
            let mut root = match existing {
                None => serde_json::json!({}),
                Some(content) => serde_json::from_str(&content)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
            };
            merge_opencode_plugin_entry(&mut root, plugin_config_path);
            write_json_atomic(&config_path, &root)?;
            if created_config {
                lease.mark_created()?;
            }
            Ok(created_config)
        });
        let created_config = match result {
            Ok(created) => created,
            Err(error) => {
                let _ = with_last_lease(&plugin_path, &mut lease, |_| {
                    match std::fs::remove_file(&plugin_path) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(error),
                    }
                });
                return Err(error);
            }
        };

        Ok(Self {
            plugin_path,
            config_path,
            created_config,
            lease,
        })
    }

    fn sweep_orphan(directory: &Path) {
        let plugin_path = directory.join("plugins").join(PANEFLOW_TS_BASENAME);
        let config_path = directory.join("opencode.json");
        let _ = with_orphan_lease(&plugin_path, &config_path, |created_config| {
            let _ = std::fs::remove_file(&plugin_path);
            let Some(content) = read_optional_text(&config_path)? else {
                return Ok(());
            };
            let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
                return Ok(());
            };
            let before = root.clone();
            remove_opencode_plugin_entry(&mut root);
            if root == before {
                return Ok(());
            }
            if created_config && root.as_object().is_some_and(serde_json::Map::is_empty) {
                std::fs::remove_file(&config_path)
            } else {
                write_json_atomic(&config_path, &root)
            }
        });
    }
}

impl Drop for OpenCodePluginGuard {
    fn drop(&mut self) {
        let _ = with_last_lease(&self.config_path, &mut self.lease, |lease_created_config| {
            let _ = std::fs::remove_file(&self.plugin_path);
            let Some(content) = read_optional_text(&self.config_path)? else {
                return Ok(());
            };
            let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
                return Ok(());
            };
            remove_opencode_plugin_entry(&mut root);
            if (self.created_config || lease_created_config)
                && root.as_object().is_some_and(serde_json::Map::is_empty)
            {
                std::fs::remove_file(&self.config_path)
            } else {
                write_json_atomic(&self.config_path, &root)
            }
        });
    }
}

fn merge_opencode_plugin_entry(root: &mut serde_json::Value, plugin_path: &str) {
    if !root.is_object() {
        *root = serde_json::json!({});
    }
    let Some(root) = root.as_object_mut() else {
        return;
    };
    let plugins = root
        .entry("plugin")
        .or_insert_with(|| serde_json::json!([]));
    let Some(plugins) = plugins.as_array_mut() else {
        return;
    };
    plugins.retain(|entry| !is_paneflow_plugin_entry(entry));
    plugins.push(serde_json::Value::String(plugin_path.to_owned()));
}

fn remove_opencode_plugin_entry(root: &mut serde_json::Value) {
    let Some(root) = root.as_object_mut() else {
        return;
    };
    if let Some(plugins) = root
        .get_mut("plugin")
        .and_then(|value| value.as_array_mut())
    {
        plugins.retain(|entry| !is_paneflow_plugin_entry(entry));
    }
    if root
        .get("plugin")
        .and_then(|value| value.as_array())
        .is_some_and(Vec::is_empty)
    {
        root.remove("plugin");
    }
}

fn is_paneflow_plugin_entry(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|value| {
        Path::new(value).file_name().and_then(OsStr::to_str) == Some(PANEFLOW_TS_BASENAME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_config_survives_invalid_utf8() {
        let temp = tempfile::TempDir::new().unwrap();
        let directory = temp.path().join("opencode");
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("opencode.json");
        std::fs::write(&config, [0xff]).unwrap();

        assert!(OpenCodePluginGuard::install_at(&directory).is_err());
        assert_eq!(std::fs::read(config).unwrap(), [0xff]);
    }

    #[test]
    fn preexisting_empty_config_survives_cleanup() {
        let temp = tempfile::TempDir::new().unwrap();
        let directory = temp.path().join("opencode");
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("opencode.json");
        std::fs::write(&config, "{}").unwrap();

        let guard = OpenCodePluginGuard::install_at(&directory).unwrap();
        drop(guard);

        assert!(config.exists());
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(config).unwrap()).unwrap();
        assert_eq!(root, serde_json::json!({}));
    }
}
