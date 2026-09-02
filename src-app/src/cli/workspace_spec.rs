use std::collections::HashMap;

use serde::Deserialize;

use crate::layout::MAX_PANES;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSpec {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub layout: LayoutPreset,
    #[serde(default)]
    pub port_base: Option<u16>,
    #[serde(default)]
    pub panes: Vec<PaneSpec>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutPreset {
    #[default]
    EvenH,
    EvenV,
    MainVertical,
    Tiled,
}

impl LayoutPreset {
    pub fn as_ipc(self) -> &'static str {
        match self {
            LayoutPreset::EvenH => "even_h",
            LayoutPreset::EvenV => "even_v",
            LayoutPreset::MainVertical => "main_vertical",
            LayoutPreset::Tiled => "tiled",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneSpec {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub focus: Option<bool>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub copy_env: Option<bool>,
    #[serde(default)]
    pub setup: Option<String>,
    #[serde(default)]
    pub setup_timeout_secs: Option<u64>,
    #[serde(default)]
    pub worktree_teardown: Option<String>,
}

pub fn load(src: &str) -> Result<WorkspaceSpec, String> {
    let spec: WorkspaceSpec = toml::from_str(src).map_err(|e| e.to_string())?;
    spec.validate()?;
    Ok(spec)
}

impl WorkspaceSpec {
    fn validate(&self) -> Result<(), String> {
        if self.panes.is_empty() {
            return Err("workspace spec has no [[panes]]".to_string());
        }
        if self.panes.len() > MAX_PANES {
            return Err(format!(
                "too many panes ({} > {MAX_PANES})",
                self.panes.len()
            ));
        }
        for (i, pane) in self.panes.iter().enumerate() {
            validate_pane(i, pane)?;
        }
        Ok(())
    }
}

pub(super) fn validate_pane(i: usize, pane: &PaneSpec) -> Result<(), String> {
    if pane.agent.is_some() && pane.command.is_some() {
        return Err(format!(
            "pane {i}: set either `agent` or `command`, not both"
        ));
    }
    validate_worktree_fields(i, pane)
}

fn validate_worktree_fields(i: usize, pane: &PaneSpec) -> Result<(), String> {
    match pane.worktree.as_deref() {
        Some(branch) => {
            if branch.is_empty() {
                return Err(format!("pane {i}: `worktree` must name a branch"));
            }
            if branch.starts_with('-') {
                return Err(format!(
                    "pane {i}: branch '{branch}' must not start with '-'"
                ));
            }
            if crate::workspace::worktree::branch_slug(branch).is_empty() {
                return Err(format!(
                    "pane {i}: branch '{branch}' has no filesystem-safe name \
                     (dot-only names are not allowed)"
                ));
            }
            if pane.cwd.is_none() {
                return Err(format!(
                    "pane {i}: `worktree` requires `cwd` (to locate the git repository)"
                ));
            }
        }
        None => {
            for (field, set) in [
                ("copy_env", pane.copy_env.is_some()),
                ("setup", pane.setup.is_some()),
                ("setup_timeout_secs", pane.setup_timeout_secs.is_some()),
                ("worktree_teardown", pane.worktree_teardown.is_some()),
            ] {
                if set {
                    return Err(format!("pane {i}: `{field}` requires `worktree`"));
                }
            }
        }
    }
    if let Some(policy) = pane.worktree_teardown.as_deref()
        && !matches!(policy, "auto" | "keep")
    {
        return Err(format!(
            "pane {i}: `worktree_teardown` must be \"auto\" or \"keep\", got '{policy}'"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_spec() {
        let spec = load(
            r#"
            name = "feat-x"
            layout = "main_vertical"

            [[panes]]
            cwd = "/tmp"
            agent = "claude"
            prompt = "do the thing"
            focus = true

            [[panes]]
            command = "cargo watch"
            "#,
        )
        .expect("valid spec");
        assert_eq!(spec.name.as_deref(), Some("feat-x"));
        assert_eq!(spec.layout, LayoutPreset::MainVertical);
        assert_eq!(spec.panes.len(), 2);
        assert_eq!(spec.panes[0].agent.as_deref(), Some("claude"));
        assert_eq!(spec.panes[0].focus, Some(true));
        assert_eq!(spec.panes[1].command.as_deref(), Some("cargo watch"));
    }

    #[test]
    fn layout_defaults_to_even_h() {
        let spec = load("[[panes]]\nagent = \"codex\"\n").expect("valid");
        assert_eq!(spec.layout, LayoutPreset::EvenH);
        assert_eq!(spec.layout.as_ipc(), "even_h");
    }

    #[test]
    fn rejects_unknown_field() {
        let err = load("agnt = \"claude\"\n[[panes]]\nagent = \"claude\"\n").unwrap_err();
        assert!(
            err.contains("agnt") || err.contains("unknown"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_agent_and_command_together() {
        let err = load("[[panes]]\nagent = \"claude\"\ncommand = \"vim\"\n").unwrap_err();
        assert!(err.contains("either"), "got: {err}");
    }

    #[test]
    fn rejects_empty_panes() {
        let err = load("name = \"x\"\n").unwrap_err();
        assert!(err.contains("no [[panes]]"), "got: {err}");
    }

    #[test]
    fn parses_worktree_fields() {
        let spec = load(
            "port_base = 4000\n[[panes]]\ncwd = \"/tmp\"\nagent = \"claude\"\nworktree = \"feat/x\"\ncopy_env = false\nsetup = \"bun install\"\nworktree_teardown = \"keep\"\n",
        )
        .expect("valid");
        assert_eq!(spec.port_base, Some(4000));
        let p = &spec.panes[0];
        assert_eq!(p.worktree.as_deref(), Some("feat/x"));
        assert_eq!(p.copy_env, Some(false));
        assert_eq!(p.setup.as_deref(), Some("bun install"));
        assert_eq!(p.worktree_teardown.as_deref(), Some("keep"));
    }

    #[test]
    fn worktree_requires_cwd() {
        let err = load("[[panes]]\nagent = \"claude\"\nworktree = \"feat/x\"\n").unwrap_err();
        assert!(err.contains("requires `cwd`"), "got: {err}");
    }

    #[test]
    fn worktree_branch_must_not_look_like_a_flag() {
        let err = load("[[panes]]\ncwd = \"/tmp\"\nagent = \"claude\"\nworktree = \"--force\"\n")
            .unwrap_err();
        assert!(err.contains("must not start with '-'"), "got: {err}");
    }

    #[test]
    fn worktree_branch_dot_only_is_rejected() {
        for branch in ["..", ".", "..."] {
            let err = load(&format!(
                "[[panes]]\ncwd = \"/tmp\"\nagent = \"claude\"\nworktree = \"{branch}\"\n"
            ))
            .unwrap_err();
            assert!(
                err.contains("filesystem-safe"),
                "branch {branch}: got: {err}"
            );
        }
    }

    #[test]
    fn worktree_companion_fields_require_worktree() {
        let err = load("[[panes]]\nagent = \"claude\"\nsetup = \"bun install\"\n").unwrap_err();
        assert!(err.contains("`setup` requires `worktree`"), "got: {err}");
        let err = load("[[panes]]\nagent = \"claude\"\ncopy_env = true\n").unwrap_err();
        assert!(err.contains("`copy_env` requires `worktree`"), "got: {err}");
    }

    #[test]
    fn worktree_teardown_value_is_validated() {
        let err = load(
            "[[panes]]\ncwd = \"/tmp\"\nagent = \"claude\"\nworktree = \"x\"\nworktree_teardown = \"delete\"\n",
        )
        .unwrap_err();
        assert!(err.contains("auto"), "got: {err}");
    }

    #[test]
    fn rejects_too_many_panes() {
        let src = "[[panes]]\nagent = \"claude\"\n".repeat(MAX_PANES + 1);
        let err = load(&src).unwrap_err();
        assert!(err.contains("too many panes"), "got: {err}");
    }
}
