//! Skills browser: scan the well-known skills directories
//! (`~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`),
//! parse each `<dir>/SKILL.md` frontmatter, and render the result as
//! a tabbed grid. One tab per source directory; cards inside each
//! tab share the visual language of the Agents welcome page
//! (`ui.surface` background, neutral border, 10 px radius).

use crate::PaneFlowApp;
use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, FontWeight, IntoElement, ParentElement,
    SharedString, Styled, div, prelude::*, px, svg,
};
use std::fs;
use std::path::{Path, PathBuf};

/// Which source directory the Skills page is currently filtered to.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SkillsTab {
    #[default]
    Claude,
    Codex,
    Agents,
}

impl SkillsTab {
    const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Agents => "Agents",
        }
    }

    /// The source tag we stamp on each `SkillEntry` during discovery
    /// (matches the parent directory under `~/`).
    const fn source_tag(self) -> &'static str {
        match self {
            Self::Claude => ".claude",
            Self::Codex => ".codex",
            Self::Agents => ".agents",
        }
    }
}

/// A single skill resolved from a `SKILL.md` frontmatter block.
#[derive(Clone, Debug)]
pub(crate) struct SkillEntry {
    id: String,
    name: String,
    description: String,
    /// Agent dir tag (e.g. ".claude") -- used to bucket entries per tab.
    source: String,
}

const FRONTMATTER_SCAN_BYTES: usize = 16 * 1024;
/// Trimmed so the preview fills ~3 lines at the card width without
/// overflowing the fixed-height card (any spill is clipped anyway).
const DESCRIPTION_MAX_CHARS: usize = 160;
/// Preferred card width: the flex-basis that drives how many columns the
/// `flex_wrap` grid packs. Cards then `flex_grow` to share the leftover row
/// width so the grid fills the whole panel instead of leaving a dead gutter
/// on wide windows.
const CARD_WIDTH_PX: f32 = 300.0;
/// Upper bound on a grown card so a sparse last row (e.g. 2 cards) doesn't
/// stretch them across the full width.
const CARD_MAX_WIDTH_PX: f32 = 440.0;
/// Every card is the same height so the wrapped grid aligns into clean
/// rows instead of the ragged staircase variable-length descriptions
/// would otherwise produce.
const CARD_HEIGHT_PX: f32 = 122.0;

pub(crate) fn render_skills_page(
    active_tab: SkillsTab,
    copied_id: Option<String>,
    skills: Vec<SkillEntry>,
    loading: bool,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let ui = crate::theme::ui_colors();
    let filtered: Vec<SkillEntry> = skills
        .into_iter()
        .filter(|s| s.source == active_tab.source_tag())
        .collect();

    let body: AnyElement = if loading && filtered.is_empty() {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .py(px(48.))
            .gap(px(6.))
            .child(
                div()
                    .text_color(ui.text)
                    .text_size(px(13.))
                    .child("Loading skills"),
            )
            .into_any_element()
    } else if filtered.is_empty() {
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .py(px(48.))
            .gap(px(6.))
            .child(
                div()
                    .text_color(ui.text)
                    .text_size(px(13.))
                    .child("No skills found"),
            )
            .child(
                div()
                    .text_color(ui.muted)
                    .text_size(px(12.))
                    .child(SharedString::from(format!(
                        "Drop skill folders under ~/{}{}skills.",
                        active_tab.source_tag(),
                        std::path::MAIN_SEPARATOR,
                    ))),
            )
            .into_any_element()
    } else {
        let copied_for_cards = copied_id.clone();
        let cards = filtered.into_iter().map(|s| {
            let is_copied = copied_for_cards
                .as_deref()
                .is_some_and(|c| c == s.id.as_str());
            render_skill_card(s, is_copied, ui, cx)
        });
        // `flex_wrap` packs as many ~`CARD_WIDTH_PX` cards per row as fit the
        // panel; each card's `flex_grow` then shares the row's leftover width
        // so the grid fills edge-to-edge (no dead right gutter) and reflows to
        // fewer columns on narrow windows.
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(12.))
            .w_full()
            .children(cards)
            .into_any_element()
    };

    div()
        .id("agents-skills-page")
        .flex()
        .flex_col()
        .size_full()
        .overflow_y_scroll()
        // Keep the entire Agents right panel on the shared #181818 surface.
        .bg(ui.base)
        .text_color(ui.text)
        .px(px(20.))
        .py(px(16.))
        .gap(px(14.))
        .child(
            div().w_full().flex().flex_col().gap(px(4.)).child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(ui.text)
                                    .child("Skills"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(ui.muted)
                                    .child("Skills discovered across your agent home directories."),
                            ),
                    )
                    .child(render_refresh_button(loading, ui, cx)),
            ),
        )
        .child(div().w_full().child(render_tab_bar(active_tab, ui, cx)))
        .child(div().w_full().child(body))
        .into_any_element()
}

fn render_refresh_button(
    loading: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    div()
        .id("agents-skills-refresh")
        .flex_none()
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor(CursorStyle::PointingHand)
        .text_color(ui.muted)
        .hover(move |d| d.bg(crate::settings::components::with_alpha(ui.text, 0.08)))
        .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.refresh_agents_skills(cx);
        }))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/refresh.svg")
                .text_color(if loading { ui.accent } else { ui.muted }),
        )
        .tooltip(crate::ui_primitives::text_tooltip("Refresh skills"))
        .into_any_element()
}

fn render_tab_bar(
    active: SkillsTab,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let tab = |this_tab: SkillsTab| {
        let is_active = this_tab == active;
        let label = this_tab.label();
        let id: SharedString = format!("agents-skills-tab-{}", label.to_lowercase()).into();
        let mut row = div()
            .id(id)
            .px(px(12.))
            .py(px(6.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(FontWeight::NORMAL);
        if is_active {
            row = row.bg(ui.subtle).text_color(ui.text);
        } else {
            row = row.text_color(ui.muted).hover(|s| {
                let ui = crate::theme::ui_colors();
                s.bg(ui.subtle).text_color(ui.text)
            });
        }
        row.on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            if this.agents_view.agents_skills_tab != this_tab {
                this.agents_view.agents_skills_tab = this_tab;
                cx.notify();
            }
        }))
        .child(label)
        .into_any_element()
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.))
        .child(tab(SkillsTab::Claude))
        .child(tab(SkillsTab::Codex))
        .child(tab(SkillsTab::Agents))
        .into_any_element()
}

fn render_skill_card(
    skill: SkillEntry,
    is_copied: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let skill_id = skill.id.clone();
    let name_for_copy = skill.name.clone();
    let id_for_mark = skill.id.clone();
    let copy_id: SharedString = format!("skill-copy-{skill_id}").into();
    let card_id: SharedString = format!("skill-card-{}", skill.id).into();
    let group: SharedString = format!("skill-grp-{}", skill.id).into();

    // Copy affordance. Hidden until the card is hovered (40 always-on "Copy"
    // labels were the main source of visual noise), then brightens on its
    // own hover. Once copied it stays lit with a check so the confirmation
    // reads at a glance.
    let mut copy_btn = div()
        .id(copy_id)
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .px(px(7.))
        .py(px(3.))
        .rounded(px(5.))
        .text_size(px(11.))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _w, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(name_for_copy.clone()));
            this.mark_skill_copied(id_for_mark.clone(), cx);
        }));
    if is_copied {
        copy_btn = copy_btn
            .bg(ui.subtle)
            .text_color(ui.text)
            .child(
                svg()
                    .size(px(10.))
                    .flex_none()
                    .path("icons/check.svg")
                    .text_color(ui.text),
            )
            .child("Copied");
    } else {
        copy_btn = copy_btn
            .opacity(0.)
            .text_color(ui.muted)
            .group_hover(group.clone(), |s| s.opacity(1.))
            .hover(|s| {
                let ui = crate::theme::ui_colors();
                s.text_color(ui.text)
            })
            .child("Copy");
    }

    // Uniform-height card; hover amplifies an accent ring (no dimming of the
    // others) and reveals the Copy button. Background stays `ui.surface` so
    // the ring is the only emphasis - consistent with the active-pane cue.
    div()
        .id(card_id)
        .group(group)
        .flex()
        .flex_col()
        .gap(px(7.))
        .h(px(CARD_HEIGHT_PX))
        // Flexible width: `w` acts as the flex-basis (drives wrap), `flex_grow`
        // fills the row's leftover space, `max_w` caps a grown card.
        .w(px(CARD_WIDTH_PX))
        .flex_grow()
        .max_w(px(CARD_MAX_WIDTH_PX))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(10.))
        .bg(ui.surface)
        .border_1()
        .border_color(ui.border)
        .hover(|s| {
            let ui = crate::theme::ui_colors();
            s.border_color(ui.accent.opacity(0.6))
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .w_full()
                .flex_none()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui.text)
                        .child(SharedString::from(skill.name.clone())),
                )
                .child(copy_btn),
        )
        .when(!skill.description.is_empty(), |d| {
            d.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(ui.muted)
                    .child(SharedString::from(skill.description)),
            )
        })
        .into_any_element()
}

pub(crate) fn discover_skills() -> Vec<SkillEntry> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };
    let sources: [(&str, PathBuf); 3] = [
        (".claude", home.join(".claude/skills")),
        (".codex", home.join(".codex/skills")),
        (".agents", home.join(".agents/skills")),
    ];
    let mut out: Vec<SkillEntry> = Vec::new();
    for (label, dir) in sources {
        if !dir.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let (name, description) = match read_frontmatter(&skill_md) {
                Some(pair) => pair,
                None => continue,
            };
            let name = if name.is_empty() {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("(unnamed)")
                    .to_string()
            } else {
                name
            };
            out.push(SkillEntry {
                id: skill_identity(label, &path, out.len()),
                name,
                description: ellipsize(description, DESCRIPTION_MAX_CHARS),
                source: label.to_string(),
            });
        }
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

fn skill_identity(source: &str, path: &Path, ordinal: usize) -> String {
    let folder = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(sanitize_element_id)
        .unwrap_or_else(|| format!("skill-{ordinal}"));
    format!("{}-{}", sanitize_element_id(source), folder)
}

fn sanitize_element_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn read_frontmatter(path: &Path) -> Option<(String, String)> {
    let raw = read_capped(path, FRONTMATTER_SCAN_BYTES).ok()?;
    parse_frontmatter(&raw)
}

/// Pull `(name, description)` from a `SKILL.md`'s YAML frontmatter. Handles
/// inline scalars, quoted strings, and `>`/`|` block scalars (folded into a
/// single whitespace-collapsed line). Pure over the raw text so it can be
/// unit-tested without touching the filesystem.
fn parse_frontmatter(raw: &str) -> Option<(String, String)> {
    let mut lines = raw.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    // Collect the frontmatter block (up to the closing `---`) so a
    // multi-line block scalar (`description: >` / `|`) can consume its
    // indented continuation lines by index.
    let mut fm: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.trim() == "---" {
            closed = true;
            break;
        }
        fm.push(line);
    }
    if !closed {
        return None;
    }

    let mut name = String::new();
    let mut description = String::new();
    let mut i = 0;
    while i < fm.len() {
        let line = fm[i];
        if let Some(rest) = line.strip_prefix("name:") {
            name = strip_yaml_value(rest);
            i += 1;
        } else if let Some(rest) = line.strip_prefix("description:") {
            let inline = rest.trim();
            if inline.starts_with('>') || inline.starts_with('|') {
                // Block scalar: gather the following indented lines, fold
                // them into one whitespace-collapsed string (good enough for
                // a one-line preview, regardless of folded vs literal).
                i += 1;
                let mut parts: Vec<&str> = Vec::new();
                while i < fm.len() {
                    let cont = fm[i];
                    if cont.trim().is_empty() {
                        i += 1;
                        continue;
                    }
                    if cont.starts_with(' ') || cont.starts_with('\t') {
                        parts.push(cont.trim());
                        i += 1;
                    } else {
                        break;
                    }
                }
                description = parts
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
            } else {
                description = strip_yaml_value(rest);
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    Some((name, description))
}

fn strip_yaml_value(raw: &str) -> String {
    let v = raw.trim();
    if v.len() >= 2
        && let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
    {
        return inner.to_string();
    }
    if v.len() >= 2
        && let Some(inner) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))
    {
        return inner.to_string();
    }
    v.to_string()
}

fn read_capped(path: &Path, max_bytes: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut buf = vec![0u8; max_bytes];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn ellipsize(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut cut: String = text.chars().take(max_chars).collect();
    if let Some(last_space) = cut.rfind(' ') {
        cut.truncate(last_space);
    }
    cut.push('\u{2026}');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_quoted_description() {
        let raw = "---\nname: clerk-cli\ndescription: \"Manage Clerk apps.\"\n---\nbody";
        let (name, desc) = parse_frontmatter(raw).expect("frontmatter");
        assert_eq!(name, "clerk-cli");
        assert_eq!(desc, "Manage Clerk apps.");
    }

    #[test]
    fn folds_block_scalar_description() {
        // Regression: `description: >` used to surface a literal ">" because
        // the parser took the inline token instead of the indented block.
        let raw = "---\nmodel: opus\nname: frontend-design\ndescription: >\n  Creates distinctive\n  frontend interfaces\n  with intent.\nargument-hint: \"[x]\"\n---\nbody";
        let (name, desc) = parse_frontmatter(raw).expect("frontmatter");
        assert_eq!(name, "frontend-design");
        assert_eq!(desc, "Creates distinctive frontend interfaces with intent.");
    }

    #[test]
    fn folds_literal_block_scalar_description() {
        let raw = "---\nname: x\ndescription: |\n  Line one\n  Line two\n---\n";
        let (_, desc) = parse_frontmatter(raw).expect("frontmatter");
        assert_eq!(desc, "Line one Line two");
    }

    #[test]
    fn missing_opening_fence_is_none() {
        assert!(parse_frontmatter("name: x\ndescription: y\n").is_none());
    }

    #[test]
    fn missing_closing_fence_is_none() {
        assert!(parse_frontmatter("---\nname: x\ndescription: y\nbody").is_none());
    }

    #[test]
    fn absent_description_yields_empty() {
        let (name, desc) = parse_frontmatter("---\nname: solo\n---\n").expect("frontmatter");
        assert_eq!(name, "solo");
        assert!(desc.is_empty());
    }
}
