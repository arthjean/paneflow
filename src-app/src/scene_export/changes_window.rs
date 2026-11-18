//! The Changes window: a Claude Code session beside the diff dock, which
//! reads a real repository. The repository is rebuilt from a local clone of
//! github.com/arthjean/claude-agent-sdk-rust as the working tree it held
//! while EP-010 was certified: HEAD at `BASE`, files at `WORK`, uncommitted.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, anyhow, ensure};

use super::fixture::{Layout, PaneSpec, WindowFixture, WorkspaceSpec};

const TERMINALS: &str = include_str!("changes_window.terminals.json");

/// The commit the working tree is compared against.
const BASE: &str = "9ee6e19";
/// The commit whose files make up the uncommitted working tree.
const WORK: &str = "5692fff";

/// Columns and rows of the Claude Code pane at 1893x1165.
pub(crate) const GRIDS: [(usize, usize); 1] = [(75, 48)];

/// Preferred width of the dock.
pub(crate) const DOCK_WIDTH: f32 = 682.;
/// The file whose diff is open; every other file is collapsed.
pub(crate) const OPEN_FILE: &str = "xtask/src/parity/mod.rs";
/// The file header at the top of the dock body, and how far it is scrolled
/// past the top edge.
pub(crate) const TOP_FILE: (&str, f32) = ("xtask/src/differential/coverage.rs", 18.);

fn git(repo: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .with_context(|| format!("running git {args:?}"))?;
    ensure!(status.success(), "git {args:?} failed: {status}");
    Ok(())
}

/// Recreates the repository under `out` and returns its path.
pub(crate) fn repository(out: &Path) -> Result<PathBuf> {
    let source = std::env::var_os("PANEFLOW_SCENE_SDK_REPO")
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow!(
                "set PANEFLOW_SCENE_SDK_REPO to a clone of \
                 https://github.com/arthjean/claude-agent-sdk-rust"
            )
        })?;
    let repo = out.join("claude-agent-sdk-rust");
    if repo.exists() {
        std::fs::remove_dir_all(&repo)?;
    }
    let status = Command::new("git")
        .args(["clone", "--quiet", "--no-checkout"])
        .arg(&source)
        .arg(&repo)
        .status()?;
    ensure!(status.success(), "cloning {}", source.display());
    git(&repo, &["checkout", "--quiet", "-B", "main", BASE])?;
    git(&repo, &["checkout", WORK, "--", "."])?;
    git(&repo, &["reset", "--quiet"])?;
    Ok(repo)
}

fn workspace(
    title: &'static str,
    git: Option<(usize, usize)>,
    tab: Option<&'static str>,
) -> WorkspaceSpec {
    WorkspaceSpec {
        title,
        cwd: "/home/arthur/dev".into(),
        git,
        tab,
        expanded: true,
    }
}

pub(crate) fn fixture(repo: &Path) -> WindowFixture {
    let mut arthjean = workspace("arthjean", None, None);
    arthjean.expanded = false;
    let mut sdk = workspace(
        "claude-agent-sdk-rust",
        Some((2521, 146)),
        Some("Épic EP-010 du PRD parity-hardening"),
    );
    sdk.cwd = repo.display().to_string();
    WindowFixture {
        workspaces: vec![
            workspace("paneflow", Some((1, 0)), Some("paneflow")),
            workspace("paneflow-agent-legacy-git-backup", Some((1, 0)), None),
            workspace("paneflow-browser", Some((1, 0)), Some("Avis sur le projet")),
            workspace("paneflow-gpui-bump", Some((1, 0)), None),
            workspace("cerata", None, Some("Avis sur le projet")),
            arthjean,
            sdk,
            workspace("corpus-engineer", None, None),
            workspace("crowbot", None, None),
            workspace("mistral-vibe-rs", None, None),
            workspace("neo-compute", Some((66, 0)), None),
            workspace("pf", None, None),
            workspace(
                "rust-doctor",
                Some((251, 91)),
                Some("Certification de l'épopée EP-006 du PRD"),
            ),
        ],
        active: 6,
        layout: Layout::Single(PaneSpec {
            agent: Some("claude"),
            title: "Claude Code",
            output: super::vt::stream(TERMINALS, "claude"),
        }),
        focused: 0,
    }
}
