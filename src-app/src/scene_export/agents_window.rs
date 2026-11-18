use super::fixture::{Layout, PaneSpec, WindowFixture, WorkspaceSpec};

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

const TERMINALS: &str = include_str!("agents_window.terminals.json");

/// Columns and rows of the Claude Code, shell and fx panes at 1893x1165.
pub(crate) const GRIDS: [(usize, usize); 3] = [(70, 48), (70, 23), (70, 23)];

pub(crate) fn fixture() -> WindowFixture {
    let mut arthjean = workspace("arthjean", None, None);
    arthjean.expanded = false;
    WindowFixture {
        workspaces: vec![
            workspace("paneflow", Some((1, 0)), Some("paneflow")),
            workspace("paneflow-agent-legacy-git-backup", Some((1, 0)), None),
            workspace("paneflow-browser", Some((1, 0)), Some("Avis sur le projet")),
            workspace("paneflow-gpui-bump", Some((1, 0)), None),
            workspace("cerata", None, Some("Avis sur le projet")),
            arthjean,
            workspace(
                "claude-agent-sdk-rust",
                Some((2521, 146)),
                Some("Épic EP-010 du PRD parity-hardening"),
            ),
            workspace("corpus-engineer", None, None),
            workspace("crowbot", None, None),
            workspace("mistral-vibe-rs", None, None),
            workspace("neo-compute", Some((66, 0)), None),
            workspace("pf", None, None),
            workspace("rust-doctor", Some((251, 91)), Some("Claude Code")),
        ],
        active: 12,
        layout: Layout::LeftAndColumn {
            left: PaneSpec {
                agent: Some("claude"),
                title: "Claude Code",
                output: super::vt::stream(TERMINALS, "claude"),
            },
            top: PaneSpec {
                agent: None,
                title: "/home/arthur/dev/rust-doctor",
                output: super::vt::stream(TERMINALS, "rust_doctor"),
            },
            bottom: PaneSpec {
                agent: Some("fx"),
                title: "fx",
                output: super::vt::stream(TERMINALS, "fx"),
            },
        },
        focused: 1,
    }
}
