use super::{
    home_unavailable, paneflow_ipc_reachable, refuse_symlink, resolve_plain_hook_command,
    with_last_lease, with_orphan_lease, HookInstall, HookInstallResult, HookInstallSkip, HookLease,
};
use paneflow_agent_config::{home_dir, read_optional_text, with_config_lock, write_text_atomic};
use std::env;
use std::path::{Path, PathBuf};

pub(crate) const HERMES_BLOCK_BEGIN: &str =
    "# >>> paneflow managed hooks (auto-installed; removed on session end) >>>";
const HERMES_BLOCK_END: &str = "# <<< paneflow managed hooks <<<";

fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(crate) fn hermes_managed_block() -> String {
    let command = |event| yaml_quote(&resolve_plain_hook_command(event));
    format!(
        "{HERMES_BLOCK_BEGIN}\n\
         hooks:\n\
         \x20 pre_llm_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 post_llm_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 pre_tool_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 post_tool_call:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         \x20 pre_approval_request:\n\
         \x20   - command: {}\n\
         \x20     timeout: 5\n\
         {HERMES_BLOCK_END}\n",
        command("UserPromptSubmit"),
        command("Stop"),
        command("PreToolUse"),
        command("PostToolUse"),
        command("PermissionRequest"),
    )
}

pub(crate) fn strip_hermes_managed_block(content: &str) -> Option<String> {
    let begin = content.find(HERMES_BLOCK_BEGIN)?;
    let end_relative = content[begin..].find(HERMES_BLOCK_END)?;
    let mut end = begin + end_relative + HERMES_BLOCK_END.len();
    if content[end..].starts_with('\n') {
        end += 1;
    }
    Some(format!("{}{}", &content[..begin], &content[end..]))
}

fn yaml_has_top_level_hooks(content: &str) -> bool {
    content
        .lines()
        .any(|line| line.starts_with("hooks:") || line == "hooks")
}

pub(crate) struct HermesHookConfigGuard {
    path: PathBuf,
    created_file: bool,
    lease: HookLease,
}

impl HermesHookConfigGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let home = home_dir().ok_or_else(home_unavailable)?;
        let directory = home.join(".hermes");
        let path = directory.join("config.yaml");
        if !paneflow_ipc_reachable() {
            Self::sweep_orphan(&path);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        let guard = Self::install_at(&directory)?;
        env::set_var("HERMES_ACCEPT_HOOKS", "1");
        Ok(HookInstall::Installed(guard))
    }

    pub(crate) fn install_at(directory: &Path) -> std::io::Result<Self> {
        refuse_symlink(directory, "Hermes")?;
        std::fs::create_dir_all(directory)?;
        let path = directory.join("config.yaml");
        let mut lease = HookLease::acquire(&path)?;
        let created_file = with_config_lock(&path, || {
            let existing = read_optional_text(&path)?;
            let created = existing.is_none();
            let content = existing.unwrap_or_default();
            let mut base = strip_hermes_managed_block(&content).unwrap_or(content);
            if yaml_has_top_level_hooks(&base) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "user Hermes config already has hooks",
                ));
            }
            if !base.is_empty() && !base.ends_with('\n') {
                base.push('\n');
            }
            base.push_str(&hermes_managed_block());
            write_text_atomic(&path, &base)?;
            if created {
                lease.mark_created()?;
            }
            Ok(created)
        })?;
        Ok(Self {
            path,
            created_file,
            lease,
        })
    }

    fn sweep_orphan(path: &Path) {
        let _ = with_orphan_lease(path, path, |created_file| {
            let Some(content) = read_optional_text(path)? else {
                return Ok(());
            };
            if let Some(cleaned) = strip_hermes_managed_block(&content) {
                if created_file && cleaned.trim().is_empty() {
                    std::fs::remove_file(path)
                } else {
                    write_text_atomic(path, &cleaned)
                }
            } else {
                Ok(())
            }
        });
    }
}

impl Drop for HermesHookConfigGuard {
    fn drop(&mut self) {
        let _ = with_last_lease(&self.path, &mut self.lease, |lease_created_file| {
            let Some(content) = read_optional_text(&self.path)? else {
                return Ok(());
            };
            let Some(cleaned) = strip_hermes_managed_block(&content) else {
                return Ok(());
            };
            if (self.created_file || lease_created_file) && cleaned.trim().is_empty() {
                std::fs::remove_file(&self.path)
            } else {
                write_text_atomic(&self.path, &cleaned)
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_config_survives_invalid_utf8() {
        let temp = tempfile::TempDir::new().unwrap();
        let directory = temp.path().join(".hermes");
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("config.yaml");
        std::fs::write(&config, [0xff]).unwrap();

        assert!(HermesHookConfigGuard::install_at(&directory).is_err());
        assert_eq!(std::fs::read(config).unwrap(), [0xff]);
    }

    #[test]
    fn preexisting_empty_config_survives_cleanup() {
        let temp = tempfile::TempDir::new().unwrap();
        let directory = temp.path().join(".hermes");
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("config.yaml");
        std::fs::write(&config, "").unwrap();

        let guard = HermesHookConfigGuard::install_at(&directory).unwrap();
        drop(guard);

        assert!(config.exists());
        assert_eq!(std::fs::read_to_string(config).unwrap(), "");
    }
}
