use super::layout::{default_layout_pane, LayoutNode, SurfaceDefinition};
use serde::{Deserialize, Serialize};

/// Current on-disk schema version for [`SessionState`].
///
/// Bumped to 2 by US-018 (`prd-cli-tab-hierarchy-2026-Q3`), when the workspace
/// stopped owning one layout tree and started owning a list of tabs. A v1 file
/// is migrated by [`migrate_session_v1`] rather than rejected; see
/// [`SESSION_SCHEMA_VERSION_V1`].
pub const SESSION_SCHEMA_VERSION: u32 = 2;

/// The one legacy on-disk version Paneflow still reads: a workspace with a
/// single `layout` and the EP-003 `empty` marker, both migrated into
/// [`WorkspaceSession::tabs`].
pub const SESSION_SCHEMA_VERSION_V1: u32 = 1;

/// Cap on the tabs one migrated workspace may carry. Mirrors the app's live
/// `MAX_TABS_PER_WORKSPACE` (`src-app/src/workspace/mod.rs`), the same
/// read-cap / write-cap pairing [`super::layout::MAX_LAYOUT_LEAVES`] has
/// with the app's `MAX_PANES`: the write side refuses to open the 33rd tab,
/// so the read side must refuse to restore one.
pub const MAX_SESSION_TABS: usize = 32;

/// Top-level UI mode (`Diff` added by US-001 of
/// `prd-git-diff-mode-2026-Q3.md`).
///
/// `Cli` is the traditional terminal-multiplexer view. `Diff` is the
/// dedicated git/worktree diff surface (left git panel + diff area).
/// Default is `Cli`. Variant order mirrors the on-screen tab order
/// (CLI / Review) in the shared sidebar footer.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    #[default]
    Cli,
    Diff,
}

/// Deserialization is deliberately tolerant: any unknown mode string falls
/// back to [`AppMode::Cli`] instead of failing the parse. A session.json
/// written by a build that still had the removed Agents view carries
/// `"mode": "agents"`, and a derived `Deserialize` would reject the whole
/// file - costing the user every workspace and layout in it.
impl<'de> Deserialize<'de> for AppMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ModeVisitor;

        impl serde::de::Visitor<'_> for ModeVisitor {
            type Value = AppMode;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a UI mode string")
            }

            fn visit_str<E>(self, value: &str) -> Result<AppMode, E>
            where
                E: serde::de::Error,
            {
                Ok(match value {
                    "diff" => AppMode::Diff,
                    _ => AppMode::Cli,
                })
            }
        }

        deserializer.deserialize_str(ModeVisitor)
    }
}

/// Persisted session state written to `~/.cache/paneflow/session.json`.
///
/// Backward-compat note: every optional field carries `#[serde(default)]`,
/// and unknown keys are ignored, so a session.json written by an older build
/// (including one that still stored the removed Agents view) deserialises
/// cleanly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    /// Schema version for forward-compatible migrations.
    pub version: u32,
    /// Index of the active workspace at save time.
    pub active_workspace: usize,
    /// Ordered list of workspace snapshots.
    pub workspaces: Vec<WorkspaceSession>,
    /// Last UI mode the user was in, restored on boot.
    #[serde(default)]
    pub mode: AppMode,
    /// US-015 (prd-git-diff-mode-2026-Q3.md): the Git Diff view scope at save
    /// time, snake_case (`"project"` / `"multi_project"` / `"worktree"`),
    /// restored into `AppMode::Diff` on boot when reconstructable. Stored as a
    /// string so this config crate stays independent of the app's `DiffScope`
    /// type. Absent / `None` on sessions written before this field - defaults
    /// to the app's `DiffScope::default()` (Project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_scope: Option<String>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if hands the field by reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Who put a tab's current title there, and therefore what may replace it.
///
/// The variants are RANKED, weakest first, and a title may only be replaced by
/// one of strictly higher rank - plus, for the top two, by another of its own
/// kind. That single rule carries every guarantee tab naming makes:
///
/// - a name a human typed is never overwritten by anything automatic;
/// - the title a CLI generates for its session replaces the placeholder, and a
///   later, better one replaces it in turn;
/// - the placeholder taken from the first prompt does NOT get replaced by the
///   second prompt's, so a tab keeps naming the work it was opened for;
/// - a preset label ("Claude Code") is the weakest of all, which is the whole
///   point - it is what auto-naming exists to replace.
///
/// Rank, not content, decides. A tab opened from the preset picker already
/// carries a non-empty, non-human title, so "is it empty?" would refuse to
/// name exactly the tabs worth naming.
///
/// Ownership lives HERE, on the tab, and not on the agent session that
/// produced it. Sessions are torn down a few seconds after each turn ends, so
/// state kept there would reset every turn and let each new prompt rename the
/// tab - which is the bug this ranking replaced.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabTitleSource {
    /// The label of the preset that opened the tab, or any other name
    /// Paneflow gives a tab for want of a better one.
    #[default]
    Preset,
    /// The opening words of the first prompt sent to the agent in this tab. A
    /// placeholder: it lands instantly and says something, while the real
    /// title does not exist yet.
    Prompt,
    /// The title the agent's own CLI generated for the session - what its
    /// resume picker shows. The real name.
    Generated,
    /// Typed by a human. Nothing but another human edit may replace it.
    User,
}

impl TabTitleSource {
    /// Position in the ordering above. Only a strictly higher rank may
    /// overwrite, so this is the whole precedence rule in one place.
    fn rank(self) -> u8 {
        match self {
            Self::Preset => 0,
            Self::Prompt => 1,
            Self::Generated => 2,
            Self::User => 3,
        }
    }

    /// Whether a title of this kind may be replaced by another of the same
    /// kind.
    ///
    /// True for the two that describe live intent: a CLI that regenerates its
    /// session title has a better one, and a user renaming twice means the
    /// second name. False for the two that are one-shot: the second prompt of
    /// a session must not rename the tab away from what it was opened for, and
    /// a second preset label would mean a tab being re-purposed, which does
    /// not happen.
    fn replaces_itself(self) -> bool {
        matches!(self, Self::Generated | Self::User)
    }

    /// Whether a title written by `self` may be replaced by one from
    /// `incoming`.
    pub fn yields_to(self, incoming: Self) -> bool {
        incoming.rank() > self.rank() || (incoming == self && incoming.replaces_itself())
    }

    /// Whether this title is final enough that no automatic naming should keep
    /// looking for a better one. Reading a transcript to produce a title that
    /// would be refused is work for nothing.
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Generated | Self::User)
    }
}

/// Deserialization is tolerant for the same reason [`AppMode`]'s is - an
/// unreadable field must not cost the user every workspace in the file - and
/// it fails *safe*: anything unrecognized is read as `User`, because wrongly
/// locking a tab out of auto-naming is a nuisance the user undoes from the tab
/// menu, while wrongly unlocking one silently destroys a name they typed.
impl<'de> Deserialize<'de> for TabTitleSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SourceVisitor;

        impl serde::de::Visitor<'_> for SourceVisitor {
            type Value = TabTitleSource;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a tab title provenance string")
            }

            fn visit_str<E>(self, value: &str) -> Result<TabTitleSource, E>
            where
                E: serde::de::Error,
            {
                Ok(match value {
                    // "auto" was this field's single non-user value while the
                    // ladder was still a pair. It meant "Paneflow wrote this",
                    // which is `Preset` now. Without this arm it would fall
                    // through to `User` and freeze every tab a session file
                    // from that build carries.
                    "preset" | "auto" => TabTitleSource::Preset,
                    "prompt" => TabTitleSource::Prompt,
                    "generated" => TabTitleSource::Generated,
                    _ => TabTitleSource::User,
                })
            }
        }

        deserializer.deserialize_str(SourceVisitor)
    }
}

/// Snapshot of one workspace tab: a title and the pane layout that tab owns
/// (US-018). `layout: None` is an *empty* tab - a folder with no pane at all -
/// which is only ever written by v2. The v1 `layout: null` meant the opposite
/// ("one default pane"), which is why the migration materializes that pane
/// explicitly instead of carrying the ambiguity forward.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TabSession {
    /// User-facing tab title. Empty means unnamed - the sidebar derives a
    /// fallback label rather than persisting one.
    #[serde(default)]
    pub title: String,
    /// Who put [`Self::title`] there, or `None` when the file does not say.
    ///
    /// `Option` rather than a defaulted enum because the two cases genuinely
    /// differ. A file written before auto-naming existed carries no such
    /// field, and its titles are a mix of names people typed and preset
    /// labels - the app's restore path tells them apart by content
    /// (`restored_tab_title_source`). A file that DOES state its provenance
    /// is believed as written, so a tab someone deliberately renamed
    /// "Claude Code" keeps its lock instead of being guessed back open.
    #[serde(default)]
    pub title_source: Option<TabTitleSource>,
    /// Pane layout tree for this tab. `None` = the tab holds no pane.
    #[serde(default)]
    pub layout: Option<LayoutNode>,
    /// Absolute path of the git worktree this tab is bound to (discussion #41),
    /// or `None` for an unbound tab.
    ///
    /// Additive and defaulted, so it needs no schema version bump: a v2 file
    /// written before this field parses with `None`, and an older Paneflow
    /// reading a file that has it ignores the key rather than failing (no
    /// `deny_unknown_fields` anywhere in this schema). A path that no longer
    /// exists at restore is dropped, not resurrected.
    #[serde(default)]
    pub worktree: Option<String>,
}

impl TabSession {
    /// A tab holding no pane.
    pub fn empty() -> Self {
        Self::default()
    }

    /// An unnamed tab holding `layout`.
    pub fn with_layout(layout: LayoutNode) -> Self {
        Self {
            title: String::new(),
            title_source: Some(TabTitleSource::Preset),
            layout: Some(layout),
            worktree: None,
        }
    }
}

/// Snapshot of a single workspace for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSession {
    /// Workspace display title.
    pub title: String,
    /// Root working directory of the workspace.
    pub cwd: String,
    /// The workspace's tabs, in display order (US-018). Never written empty by
    /// Paneflow: a workspace always keeps one tab (FR-01), an empty folder
    /// being a single [`TabSession::empty`]. A v1 file has no such key, which
    /// is exactly what [`migrate_session_v1`] keys off.
    #[serde(default)]
    pub tabs: Vec<TabSession>,
    /// Index into [`WorkspaceSession::tabs`] of the tab visible at save time.
    /// Clamped at restore; this is a *persistence* index, never an address -
    /// no IPC or MCP surface exposes it (FR-07).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub active_tab: usize,
    /// v1 only: the single layout tree a workspace used to own. Always `None`
    /// once [`migrate_session_v1`] has run, so v2 never writes the key.
    #[serde(rename = "layout", default, skip_serializing_if = "Option::is_none")]
    pub legacy_layout: Option<LayoutNode>,
    /// v1 only (EP-003): the workspace deliberately held no pane, the marker
    /// that separated that state from the legacy `layout: null` meaning "one
    /// default pane". Superseded by an empty [`TabSession`]; always `false`
    /// once the migration has run, so v2 never writes the key.
    #[serde(rename = "empty", default, skip_serializing_if = "is_false")]
    pub legacy_empty: bool,
    /// User-defined command buttons rendered in this workspace's tab bar.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_buttons: Vec<ButtonCommand>,
    /// Workspace-relative directory paths expanded in the Files tree sidebar
    /// (PRD files-tree US-007). Additive + optional: absent in older
    /// `session.json` files, which deserialize to an empty list and never
    /// break restore of the other fields. The sidebar's open/closed state is
    /// deliberately NOT persisted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_paths: Vec<String>,
    /// Git worktrees Paneflow created for this workspace via `paneflow up`
    /// (EP-002, prd-orchestration-v2). Persisted so a crash/restart keeps the
    /// ownership record (teardown at close, `git worktree prune` at startup).
    /// Additive + optional like `expanded_paths`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_worktrees: Vec<ManagedWorktreeDef>,
    /// Whether the rail row of this workspace was unfolded at save time.
    /// Additive and optional: absent in older files and, being written only
    /// when folded, absent for every workspace of a user who never folds one.
    /// The rail has always come up unfolded, and that stays the answer when
    /// the key is missing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sidebar_collapsed: bool,
}

/// Migrate a v1 `session.json` in place to the v2 tab shape (US-018).
///
/// v1 gave each workspace one layout tree whose panes could each stack several
/// surfaces. v2 gives it a list of tabs whose panes hold exactly one surface,
/// so the extra surfaces have to go somewhere: the tree becomes the first tab
/// with every pane reduced to its focused surface, and each surface left over
/// is promoted - in traversal order - to its own single-pane tab named after
/// that surface. No surface is dropped, except past [`MAX_SESSION_TABS`],
/// which is logged with its count rather than swallowed.
///
/// Idempotent: a workspace that already carries `tabs` is left alone, and the
/// two legacy fields are always drained so a re-save writes pure v2.
pub fn migrate_session_v1(state: &mut SessionState) {
    for ws in &mut state.workspaces {
        migrate_workspace_v1(ws);
    }
    state.version = SESSION_SCHEMA_VERSION;
}

fn migrate_workspace_v1(ws: &mut WorkspaceSession) {
    let legacy_empty = std::mem::take(&mut ws.legacy_empty);
    let legacy_layout = ws.legacy_layout.take();
    if !ws.tabs.is_empty() {
        // Hand-written or already-migrated file: the tab list wins over the
        // legacy keys, which have just been drained.
        return;
    }
    let Some(mut root) = legacy_layout else {
        // v1 `layout: null` meant "one default pane"; the EP-003 `empty`
        // marker was the only way to say "no pane at all".
        ws.tabs.push(if legacy_empty {
            TabSession::empty()
        } else {
            TabSession::with_layout(default_layout_pane())
        });
        return;
    };
    let mut promoted = Vec::new();
    demote_panes_to_focused_surface(&mut root, &mut promoted);
    ws.tabs.push(TabSession::with_layout(root));
    ws.tabs.append(&mut promoted);
    if ws.tabs.len() > MAX_SESSION_TABS {
        let dropped = ws.tabs.len() - MAX_SESSION_TABS;
        tracing::warn!(
            workspace = %ws.title,
            dropped,
            cap = MAX_SESSION_TABS,
            "session v1 migration: workspace exceeds the tab cap, surplus tabs dropped"
        );
        ws.tabs.truncate(MAX_SESSION_TABS);
    }
    // The v1 file has no notion of an active tab, and the tree the user was
    // looking at is always the first one.
    ws.active_tab = 0;
}

/// Depth-first, left to right: keep each pane's focused surface where it is
/// and hand every other surface back as its own tab, in the order the panes
/// (and the surfaces inside them) appear in the tree.
fn demote_panes_to_focused_surface(node: &mut LayoutNode, promoted: &mut Vec<TabSession>) {
    match node {
        LayoutNode::Pane { surfaces } => {
            if surfaces.is_empty() {
                surfaces.push(SurfaceDefinition::default());
                return;
            }
            let focused = surfaces
                .iter()
                .position(|s| s.focus == Some(true))
                .unwrap_or(0);
            let mut drained: Vec<SurfaceDefinition> = std::mem::take(surfaces);
            surfaces.push(drained.remove(focused));
            for surface in drained {
                let title = surface_title(&surface);
                promoted.push(TabSession {
                    title,
                    // A promoted title is the surface's `custom_name`, which
                    // a v1 file cannot tell apart from a name the user typed
                    // on that pane. Stating no provenance is the honest
                    // answer, and lets the restore path judge it by content
                    // like every other legacy title.
                    title_source: None,
                    layout: Some(LayoutNode::Pane {
                        surfaces: vec![surface],
                    }),
                    // A v1 file predates tab worktrees by definition.
                    worktree: None,
                });
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children.iter_mut() {
                demote_panes_to_focused_surface(child, promoted);
            }
        }
    }
}

fn surface_title(surface: &SurfaceDefinition) -> String {
    surface
        .custom_name
        .as_deref()
        .or(surface.name.as_deref())
        .unwrap_or_default()
        .to_string()
}

/// A git worktree created (and therefore owned) by Paneflow for one pane of a
/// `paneflow up` workspace. Paths are stored absolute; `teardown` is `"auto"`
/// (remove at close when clean) or `"keep"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ManagedWorktreeDef {
    /// Worktree checkout directory.
    pub path: String,
    /// Main repository root (where `git worktree` commands run).
    pub repo_root: String,
    /// Branch checked out in the worktree (diagnostics only - never deleted).
    pub branch: String,
    /// Teardown policy: `"auto"` | `"keep"`. Unknown values read as `"auto"`;
    /// the data-loss protection is the clean-check, not this flag.
    pub teardown: String,
}

/// A user-defined command button rendered in a workspace's tab bar.
/// Clicking the button sends `{command}\r` to the active terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ButtonCommand {
    /// Stable identifier (opaque string) - survives reorderings and renames.
    pub id: String,
    /// Display name (also used as hover tooltip).
    pub name: String,
    /// Icon asset path relative to the `assets/` folder (e.g. `"icons/rocket.svg"`).
    pub icon: String,
    /// Shell command string, executed verbatim in the active terminal
    /// with a trailing `\r` appended (no bracketed-paste wrapping).
    pub command: String,
}
