use super::owned_files::{cleanup_owned_file, sweep_owned_file};
use super::{
    config_dir_is_symlink, home_unavailable, hook_config_error, install_hook_config_file,
    paneflow_ipc_reachable, plain_hook_handler, with_last_lease, with_orphan_lease, HookInstall,
    HookInstallResult, HookInstallSkip, HookLease, InvalidJsonPolicy,
};
use paneflow_agent_config::claude_hooks::reconcile_matcher_hooks_replacing_invalid_container;
use paneflow_agent_config::{home_dir, read_optional_text, with_config_lock, write_json_atomic};
use serde_json::{json, Value};
use std::env;
use std::path::{Path, PathBuf};

pub(crate) const MUSE_HOOK_EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "UserPromptSubmit"),
    ("PreToolUse", "PreToolUse"),
    ("PostToolUse", "PostToolUse"),
    ("PermissionRequest", "PermissionRequest"),
    ("PostLLMCall", "Stop"),
    ("Stop", "Stop"),
];
pub(crate) const MUSE_HOOKS_BASENAME: &str = "paneflow-hooks.json";
pub(crate) const MUSE_SETTINGS_BASENAME: &str = "settings.json";
pub(crate) const MUSE_HOOK_ENV_VARS: &[&str] = &[
    "PANEFLOW_WORKSPACE_ID",
    "PANEFLOW_SURFACE_ID",
    "PANEFLOW_SOCKET_PATH",
    "PANEFLOW_AI_TOOL",
    "PANEFLOW_AI_PID",
];
const SCHEMA_VERSION_KEY: &str = "schema_version";
const MANAGED_HOOKS_PATH_KEY: &str = "managed_hooks_path";
const MANAGED_HOOKS_ENV_VARS_KEY: &str = "managed_hooks_env_vars";

pub(crate) struct MuseHookConfigGuard {
    settings_path: PathBuf,
    config_dir: PathBuf,
    created_settings: bool,
    created_dir: bool,
    settings_lease: HookLease,
    hooks_path: PathBuf,
    hooks_lease: HookLease,
}

impl MuseHookConfigGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let directory = muse_config_dir().ok_or_else(home_unavailable)?;
        if !paneflow_ipc_reachable() {
            sweep_orphan(&directory);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        Self::install_at(&directory).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(directory: &Path) -> std::io::Result<Self> {
        let hooks_path = directory.join(MUSE_HOOKS_BASENAME);
        let mut installed = install_hook_config_file(
            directory,
            MUSE_SETTINGS_BASENAME,
            "Muse Code",
            |root| merge_muse_settings(root, &hooks_path),
            InvalidJsonPolicy::Refuse,
        )?;
        let hooks_lease = HookLease::acquire(&hooks_path).and_then(|lease| {
            let mut root = json!({});
            merge_muse_hooks(&mut root)?;
            with_config_lock(&hooks_path, || write_json_atomic(&hooks_path, &root))?;
            Ok(lease)
        });
        let hooks_lease = match hooks_lease {
            Ok(lease) => lease,
            Err(error) => {
                let owned = installed.created_file;
                let _ = with_last_lease(&installed.path, &mut installed.lease, |lease_created| {
                    restore_settings(&installed.path, owned || lease_created)
                });
                if installed.created_directory {
                    let _ = std::fs::remove_dir(directory);
                }
                return Err(error);
            }
        };
        Ok(Self {
            settings_path: installed.path,
            config_dir: directory.to_path_buf(),
            created_settings: installed.created_file,
            created_dir: installed.created_directory,
            settings_lease: installed.lease,
            hooks_path,
            hooks_lease,
        })
    }

    #[cfg(test)]
    pub(crate) fn hooks_path(&self) -> &Path {
        &self.hooks_path
    }
}

impl Drop for MuseHookConfigGuard {
    fn drop(&mut self) {
        cleanup_owned_file(&self.hooks_path, &mut self.hooks_lease);
        let owned = self.created_settings;
        let settings_path = &self.settings_path;
        let _ = with_last_lease(settings_path, &mut self.settings_lease, |lease_created| {
            restore_settings(settings_path, owned || lease_created)
        });
        if self.created_dir {
            let _ = std::fs::remove_dir(&self.config_dir);
        }
    }
}

fn merge_muse_hooks(root: &mut Value) -> std::io::Result<()> {
    let events: Vec<&str> = MUSE_HOOK_EVENTS
        .iter()
        .map(|(foreign, _)| *foreign)
        .collect();
    reconcile_matcher_hooks_replacing_invalid_container(root, &events, |foreign| {
        let canonical = MUSE_HOOK_EVENTS
            .iter()
            .find_map(|(candidate, canonical)| (*candidate == foreign).then_some(*canonical))
            .unwrap_or(foreign);
        json!({ "hooks": [plain_hook_handler(canonical)] })
    })
    .map(|_| ())
    .map_err(hook_config_error)
}

pub(crate) fn muse_config_dir() -> Option<PathBuf> {
    match env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value).join("muse")),
        _ => home_dir().map(|home| home.join(".config").join("muse")),
    }
}

fn merge_muse_settings(root: &mut Value, hooks_path: &Path) -> std::io::Result<()> {
    let invalid = |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, message);
    let Some(object) = root.as_object_mut() else {
        return Err(invalid("Muse Code settings root is not an object"));
    };
    let hooks_path = hooks_path
        .to_str()
        .ok_or_else(|| invalid("Muse Code hook path is not valid UTF-8"))?;
    match object.get(MANAGED_HOOKS_PATH_KEY) {
        None | Some(Value::Null) => {}
        Some(Value::String(existing)) if existing == hooks_path => {}
        Some(_) => {
            return Err(invalid(
                "user Muse Code settings already point managed_hooks_path elsewhere",
            ))
        }
    }
    if object
        .get(MANAGED_HOOKS_ENV_VARS_KEY)
        .is_some_and(|value| !value.is_null() && !value.is_array())
    {
        return Err(invalid(
            "user Muse Code settings hold a non-array managed_hooks_env_vars",
        ));
    }
    object.entry(SCHEMA_VERSION_KEY).or_insert_with(|| json!(1));
    object.insert(MANAGED_HOOKS_PATH_KEY.to_owned(), json!(hooks_path));
    let env_vars = object
        .entry(MANAGED_HOOKS_ENV_VARS_KEY)
        .or_insert_with(|| json!([]));
    if env_vars.is_null() {
        *env_vars = json!([]);
    }
    if let Some(entries) = env_vars.as_array_mut() {
        for name in MUSE_HOOK_ENV_VARS {
            if !entries.iter().any(|entry| entry.as_str() == Some(name)) {
                entries.push(json!(name));
            }
        }
    }
    Ok(())
}

pub(crate) fn remove_muse_settings(root: &mut Value) {
    let Some(object) = root.as_object_mut() else {
        return;
    };
    let owns_managed_path = object
        .get(MANAGED_HOOKS_PATH_KEY)
        .and_then(Value::as_str)
        .is_some_and(|path| {
            Path::new(path)
                .file_name()
                .is_some_and(|name| name == MUSE_HOOKS_BASENAME)
        });
    if !owns_managed_path {
        return;
    }
    object.remove(MANAGED_HOOKS_PATH_KEY);
    if let Some(entries) = object
        .get_mut(MANAGED_HOOKS_ENV_VARS_KEY)
        .and_then(Value::as_array_mut)
    {
        entries.retain(|entry| {
            !entry
                .as_str()
                .is_some_and(|name| MUSE_HOOK_ENV_VARS.contains(&name))
        });
    }
    if object
        .get(MANAGED_HOOKS_ENV_VARS_KEY)
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        object.remove(MANAGED_HOOKS_ENV_VARS_KEY);
    }
}

fn settings_is_bare(root: &Value) -> bool {
    root.as_object().is_some_and(|object| {
        object.is_empty()
            || (object.len() == 1 && object.get(SCHEMA_VERSION_KEY) == Some(&json!(1)))
    })
}

fn restore_settings(path: &Path, owned: bool) -> std::io::Result<()> {
    let Some(content) = read_optional_text(path)? else {
        return Ok(());
    };
    let Ok(mut root) = serde_json::from_str::<Value>(&content) else {
        return Ok(());
    };
    let before = root.clone();
    remove_muse_settings(&mut root);
    if root == before {
        return Ok(());
    }
    if owned && settings_is_bare(&root) {
        std::fs::remove_file(path)
    } else {
        write_json_atomic(path, &root)
    }
}

fn sweep_orphan(directory: &Path) {
    if config_dir_is_symlink(directory) {
        return;
    }
    let settings_path = directory.join(MUSE_SETTINGS_BASENAME);
    let _ = with_orphan_lease(&settings_path, &settings_path, |created| {
        restore_settings(&settings_path, created)
    });
    sweep_owned_file(&directory.join(MUSE_HOOKS_BASENAME));
}
