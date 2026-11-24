# Paneflow Design System

## 1. Contract

### 1.1 Purpose

`DESIGN.md` is the design contract for the native Paneflow application: the
GPUI shell, its sidebars, the pane grid, the diff dock, Settings, menus,
dialogs, and toasts. It records the visual thesis, the tokens, the geometry,
the motion rules, and the component contracts that the code implements, so
that a UX or UI contributor can change a surface without re-deriving the
system from 30,000 lines of Rust.

It does not cover paneflow.dev. The web site has its own `DESIGN.md` in the
site repository, with a different register (brand and marketing). The two
share a lineage, not a stylesheet.

### 1.2 Authority

| Source | Owns |
| --- | --- |
| `DESIGN.md` | Visual thesis, tokens, geometry, motion, component contracts, states, validation gate |
| `AGENTS.md` | Engineering gates, cross-platform rules, commit conventions, no-comment policy |
| `ARCHITECTURE.md` | Thread model, render pipeline, why the render thread never blocks |
| `src-app/src/theme/` | Concrete palette values for the eight bundled variants |
| `src-app/src/ui_primitives.rs`, `src-app/src/settings/components.rs`, `src-app/src/app/constants.rs` | The shared primitives and constants every surface must consume |
| `docs/user/` | User-facing vocabulary (Agents, Workspaces, Settings pages) |

When sources disagree: product intent wins, then a **Canonical** rule here,
then the shared primitives, then any local render code. A visual change that
contradicts this document updates the document in the same pull request.

### 1.3 Status vocabulary

- **Canonical**: approved and ready to reuse.
- **Contextual**: correct only for the named surface.
- **Migration**: shipped, but not a precedent for new work.
- **Proposed**: an approved target that is not implemented.

`MUST` is required. `SHOULD` is the default and needs a documented reason to
diverge. `MAY` is optional.

### 1.4 Reference captures

`assets/images/demo-0.12.png` is the reference capture: Claude Code and fx
in parallel panes, the dock open on a Rust file beside the file tree, and the
Workspaces rail listing one tab per session. A pull request
that changes a surface ships a capture of that surface in the same theme
family, as `AGENTS.md` already requires.

## 2. The Thesis

### 2.1 What Paneflow is on screen

Paneflow is a cockpit for coding agents, not an IDE and not a terminal
emulator with tabs. The dominant idea of the screen is the grid of live pane
cards, each one a real terminal running a real agent. Everything else is
instrumentation around that grid: a rail of workspaces and tabs on the left,
a title bar that is mostly empty, docks that appear only when changes or files
are needed, and a footer row that opens **Settings**.

The code calls this shell the cockpit (`cockpit_chrome_background`,
`cockpit_backdrop_background` in `src-app/src/app/constants.rs`). Keep that
scene in mind when adding a surface: instruments in front, switches on the
rail, a thin canopy frame around it. Nothing on the rail competes with the
instruments.

### 2.2 Lineage

The chrome descends from two products and one platform layer, and the commit
history names them:

| Influence | What Paneflow took from it | Evidence |
| --- | --- | --- |
| Codex app (OpenAI) | The material language of the shell: one slightly brighter translucent highlight for hover and selection, no drop shadows, inline Settings that replace the main panel, the select, toggle, and card primitives, the sectioned sidebar rail | `refactor(ui): unify chrome on the Codex material language`, `feat(settings): shared Codex-style select, toggle, and card primitives`, `feat(settings): embed Codex-style inline settings`, `feat(theme): restore PaneFlow Light with a Codex-style light shell`, changelog 0.5.4 |
| Cursor | The diff dock chrome: file tabs as chips, the toolbar rail skin, the changes sidebar hierarchy, the compact graphite sidebar and pale blue accent of the Cursor preset | `feat(diff-dock): Cursor-style chrome and retire the Agents environment card`, `refactor(diff-dock): give the toolbar chips the sidebar rail skin`, `docs/user/themes.md` |
| T3 Code | The Changes surface of the diff dock: the 540 px dock, the list surface picker, the toolbar, and the geometry, tints and status icons of its diff view, which renders with `@pierre/diffs` | `src-app/src/diff/element.rs`, `src-app/src/app/diff_dock/surface_picker.rs`, `THIRD_PARTY_NOTICES.md` |
| Native window systems | Mica on Windows 11, the AppKit sidebar material on macOS, client-side decorations with platform caption glyphs | `feat(chrome): native compositor blur backdrop on Linux and macOS`, `feat(window-chrome): native Win11 caption glyphs in light and dark`, `feat(macos): add sidebar material setting` |

Paneflow borrows the reasoning of these products, not their pixels, with one
exception: the Changes surface ports the measurements of T3 Code's diff view
and keeps Paneflow's theme colors for its hues. The
bundled Vercel, Claude, and Cursor presets are identity swaps on top of one
structure; the structure is Paneflow's.

### 2.3 Three words

**Quiet.** The shell recedes. Neutrals carry no hue (`feat(theme): make the
shell neutrals hue-free`). The accent appears on links, selected metadata,
focus, one primary action, and nothing larger. Hover and selection are
translucent washes of the text color, not colored fills.

**Continuous.** Rows, cards, menus, and tooltips use the same continuous corner
approximation (`src-app/src/ui_primitives/squircle.rs`), so a hovered row and
the card around it read as one material. Separators are gone between chips
and tabs. The curve follows the Unpeel reference described in 5.2.
Hover, dim, and sidebar slide are interpolated,
never stepped.

**Native.** The window is client-decorated on every platform with the
platform's own caption glyphs. The sidebar reveals the OS material where one
exists. Fonts for the terminal come from the user's system, with a bundled
Nerd Font as the default.

### 2.4 Operating principles

1. The pane grid is the content. Chrome is a frame and MUST NOT gain weight,
   color, or motion that competes with a running terminal.
2. Depth comes from the surface ramp (`base`, `surface`, `overlay`, `subtle`)
   and from inset cards with masked corners. Drop shadows were removed in
   0.5.4 and MUST NOT return on chrome outside the contextual Zed Editor
   Controls menu in 5.4; the other shadows are the client-side window, the
   drag ghost, and the dialog card (`modal_card`) over its scrim. The menus
   and select popups of 4.9 carry a soft shadow, as do the floating
   surfaces of the Classic interface style.
3. One highlight material. Hovered, active, and selected states are alpha
   tints of one color per theme lightness, never a per-component fill.
4. Every rounded surface takes its radius from section 4.4. New radii are not
   introduced.
5. Motion explains state: hover, focus dim, sidebar slide, toast lifecycle.
   Nothing animates for decoration except the startup splash shimmer, the
   update banner shimmer, and status spinners. Any new animation MUST read
   `reduce_motion`; section 4.8 lists which existing ones do.
6. Color carries meaning first: added, modified, deleted, conflict, and
   error keep their hues across presets.
7. Density over decoration. Body text is 12 px, labels are 11 px, micro
   chips are 9 to 10 px. Whitespace is spent on the grid, not on padding.
8. Every surface holds with the native material on and off, in light and
   dark, and at the 800 by 500 minimum window.

## 3. Anatomy

### 3.1 The shell

The Classic interface style, the default, lays the window out edge to edge:
three full-height columns separated by 1 px lines, with the headers of the top
panes and the dock tabs in the title band.

```text
┌──────────────┬─────────────────────┬─────────────────────┬───────────────────────────┐
│ ▤ Files Help │ pane tabs  +  tools │ pane tabs  +  tools │ dock tabs  +  ▥ [ _ ☐ × ] │
│ primary      │ terminal            │ terminal            │ diff dock                 │
│ sidebar      │                     │                     │ 540px default             │
│ 240 to 480px ├─────────────────────┤                     │ 360px min                 │
│              │ pane tabs  +  tools │                     │                           │
│ Settings     │ terminal            │                     │                           │
└──────────────┴─────────────────────┴─────────────────────┴───────────────────────────┘
```

The Rounded style floats the same regions as cards under a full-width title
bar. The region table below gives its geometry; 4.9 lists what Classic
changes.

```text
┌ title bar: max(1.75rem, 32px), full width, drag region, caption glyphs ─────────────┐
│ ▤  Files  Help              · workspace name                      ▥  [ _  ☐  × ]    │
├──────────────┬───────────────────────────────────────────────────┬───────────────────┤
│ primary      │ main panel: inset card, 4px inset, 10px radius,   │ right rail        │
│ sidebar      │ corner masks painted in the shell color           │ sessions or files │
│ 240 to 480px │ ┌ pane card, 24px squircle ─┐ ┌ pane card ─────┐  │ 300px             │
│              │ │ header 40px: title, tools │ │                │  │ or diff dock      │
│ Workspaces   │ │ terminal, inset 10 / 6    │ │                │  │ 540px default     │
│ folder rows  │ └───────────────────────────┘ └────────────────┘  │ 360px min         │
│ tab rows     │              8px gutter, 80px min pane            │                   │
│ update banner│                                                   │                   │
│ Settings     │                                                   │                   │
└──────────────┴───────────────────────────────────────────────────┴───────────────────┘
```

| Region | Role | Geometry | Source |
| --- | --- | --- | --- |
| Window | Client-side decorations on every platform | Default 1200 by 800, minimum 800 by 500, corner radius 10, border 1, resize border 10, shadow black 0.4 blurred 5 when floating | `src-app/src/window_state.rs`, `src-app/src/app/constants.rs`, `src-app/src/window_chrome/csd.rs` |
| Title bar | Drag region, sidebar toggle, Files and Help menus, workspace name, diff dock toggle, caption controls | Height max(1.75 rem, 32 px); control size 20; edge inset 8; control spacing 12; macOS brand padding 80 for the traffic lights | `src-app/src/window_chrome/title_bar.rs` |
| Primary sidebar | Workspaces rail in Agents, navigation in Settings | The Workspaces rail is 432 wide by default, 90 % of the widest, and resizes from 240 to 480 by its right edge, with a detent on the default, capped so the main panel keeps 200 px beside the right rail; the Settings navigation is a fixed 300; slides in 280 ms | `src-app/src/app/sidebar/mod.rs`, `src-app/src/app/sidebar/resize.rs`, `src-app/src/app/render.rs` |
| Main panel | The inset card that holds the pane grid or a Settings page | Inset 4 on right and bottom, and on the left only when the sidebar is hidden; radius 10; four corner masks painted in the shell color, transparent while a chrome material is active, so no surface inside the panel paints its own square background | `src-app/src/app/render.rs`, `src-app/src/window_chrome/mod.rs` |
| Pane grid | Binary split tree of pane cards, one per workspace tab | Comfortable density: gutter 8, divider hit area 7, minimum pane 80. Compact density (`comfortable_density: false`): no outer gutter, a 1 px `border` divider whose 7 px hit area straddles both panes, minimum pane 80 | `src-app/src/layout/tree.rs`, `src-app/src/layout/render.rs` |
| Right rail | Sessions rail | Width 300 | `src-app/src/app/sessions_sidebar.rs` |
| File tree | The full width of the dock's Files tab; inside a file tab, below its shared toolbar, to the right of the editor | Beside a file: width 256 by default, resized from 160 to 560 by its left edge (7 px hit area, 1 px visible rule, arrow keys step 16, Home and End jump to the bounds), never leaving less than 256 for the editor and staying at 160 when the dock cannot hold both; remembered in `window-state.json` as `files_tree_width` | `src-app/src/app/files_sidebar/mod.rs`, `src-app/src/app/files_sidebar/integration.rs` |
| Diff dock | Side dock attached to a tab, holding the branch diff and editable file tabs | Width 540 default, 360 minimum, 1400 maximum, clamped to the room the panel has once the pane grid keeps a minimum pane (80) and its gutters, so it drops below 360 only when the panel cannot hold both with the sidebar at its minimum; no cap on file tabs | `src-app/src/app/diff_dock/model.rs` |
| Footer | The Settings row, with the IPC offline and update banners stacked above it | Persistent primary navigation | `src-app/src/app/sidebar_actions_menu.rs` |

### 3.2 Modes

Paneflow has one mode and one takeover surface.

| Mode | Sidebar | Main panel | Entry |
| --- | --- | --- | --- |
| Agents | Workspaces: folder rows, tab rows with branch, diffstat, and agent icon stack | Pane grid | Default |
| Settings | Back to the app, search field, three nav groups | One page at a time, centered column, 26 px heading | Footer row |

Settings is not a window. It replaces the main panel and reuses the sidebar
width for its navigation, so the shell never changes shape. The title bar
hides the workspace name while Settings is open.

### 3.3 Overlays

| Overlay | Placement | Shell | Source |
| --- | --- | --- | --- |
| Pane palette | Fills an empty tab, titled `New pane` | A centered 260 px column on a 24 px squircle of the terminal background: 13 px Semibold title, an optional branch row 28 tall, preset rows 34 tall with a 14 px agent mark, gap 2, list capped at 420 tall, inline error at 11 px | `src-app/src/app/pane_palette.rs` |
| Diff dock surface picker | Fills a fresh dock | A centered list 320 wide at most, padding 24: a 13 px Semibold title 12 above three 32 px rows, row gap 2, a 16 px muted icon, content gap 10, the keycap of the row's digit at the right | `src-app/src/app/diff_dock/surface_picker.rs` |
| Menus and selects | Deferred, anchored under the trigger | Radius 8, list padding 4, item height 28: the menu contract of 5.6, the same in every interface style | `src-app/src/ui_primitives/action_menu.rs`, `src-app/src/settings/components.rs` |
| Tooltip | After 800 ms | Squircle 14 on the title bar color with a 1 px border | `src-app/src/ui_primitives.rs` |
| Terminal search | Top right of the pane, 8 px inset | Squircle 14 on `subtle` with a 1 px `border`, 325 by 36, padding 14 left and 4 right, gap 8: the field, a `.*` mark only while regex mode is on, the match count, a 1 px divider, then previous, next and close as 28 px squircle icon buttons with the sidebar hover tint | `src-app/src/terminal/view.rs` |
| File finder | `secondary-p` outside a terminal or a text field, docked like the command palette | The command palette panel of 5.7 at 560 wide: the search field, then up to 200 files with a 14 px language icon and the matched characters in Semibold, each path elided from its start so the file name stays visible, the list never running past the window's bottom edge, or one muted 29 px status row (`No matching files.`, `Indexing files…`, `Showing the first 100,000 files.`); an empty query lists the files of this workspace opened from the dock, by frecency | `src-app/src/app/file_finder.rs` |
| Path picker | Anchored to the terminal cursor: under it, or above it with the field last when the space below cannot hold ten rows | The command palette panel of 5.7 at 420 wide, ten rows before the list scrolls | `src-app/src/terminal/path_picker/` |
| Toast | Bottom right, 18 px inset | Squircle 20 on `subtle`, minimum width 220, one header row and an optional action row; a one-line toast is too short for 20, so its corner caps at the height limit and reads as a capsule | `src-app/src/app/notifications.rs` |
| Worktree removal dialog | Centered | The card of the unsaved files dialog: squircle at `PANE_CARD_RADIUS`, 460 wide, padding 20 over a 0.55 black scrim, the blocking workspaces, tabs and sessions on `subtle` at 0.5 on radius 8, one kind word per row at 10 px muted, then `Cancel` and a destructive `Remove` | `src-app/src/app/worktree_remove.rs` |
| Quit dialog | Centered | The card of the unsaved files dialog at `PANE_CARD_RADIUS`, 460 wide: the question with the session and agent counts, one muted line of consequence, the `Remember my choice` toggle row on `subtle` at 0.5 on radius 8 (after a failed stop, a muted 11 px `Show details` trigger with a 12 px chevron instead, collapsed by default, that unfolds a `menu_panel` of the raw host errors, one per line in Geist Mono 11, scrolling past 160 px), then a footer whose alternative destructive action (`destructive_button`) sits alone on the leading edge, apart from `Cancel` and the default action on the trailing edge. The default action is the one Enter runs: a solid `switch_blue` button, or `Cancel` when every other choice ends sessions. Tab, Shift-Tab, left and right move a 1 px `text` ring at 0.35, drawn 3 px outside the control with a concentric radius, across the toggle row or the details trigger and the buttons; Enter or Space runs the ringed control and Escape cancels | `src-app/src/app/quit_dialog.rs` |
| System Info dialog | Centered | The card of the unsaved files dialog at squircle 20, 560 wide, padding 20: a 44 px app icon beside a 16 px Semibold title and the build line at 11 px muted, then one 11 px muted eyebrow and one `menu_panel` per report section (`System`, `Rendering`) whose rows set a 116 px muted label column beside the value in Geist Mono 12, then the privacy line at 11 px muted, `Close`, and a solid `Copy` button in the toggle blue `#339cff` with a white label (`solid_button`). Escape closes, Enter copies, and focus returns to where it was | `src-app/src/app/system_info_dialog.rs` |
| About dialog | Centered | The System Info card at squircle 20, 400 wide: a centered 64 px app icon, `Paneflow` at 16 px Semibold, the welcome tagline at 12 px muted, and the version on a Geist Mono 11 chip on `subtle` at radius 6; then one `menu_panel` of `menu_row` links (Website, Source code, Release notes of the running version) with a 14 px muted glyph, the label, and the address at 11 px muted on the trailing edge; then `© Arthur Jean · <license>` at 11 px muted and `Close`. Up and down select a link, Enter opens it or closes when none is selected, Escape closes, and focus returns to where it was | `src-app/src/app/about_dialog.rs` |

## 4. Foundations

### 4.1 Color architecture

Color resolves in three layers.

1. **Terminal theme**: 37 slots per variant (24 ANSI colors, 5 base colors,
   cursor, selection and its derived foreground, scrollbar thumb and track,
   link text, two title bar colors) plus a syntax palette for the diff and the editor.
   `src-app/src/theme/model.rs`, values in `src-app/src/theme/builtin.rs`.
2. **UI colors**: the semantic roles the chrome consumes, `UiColors`. A
   preset either ships its own `UiColors` (Vercel, Claude, Cursor,
   Tailwind) or lets Paneflow derive one from the terminal theme's lightness
   (Paneflow Dark and Light).
3. **Local tints**: alpha washes computed at render time from `text`,
   `muted`, or a fixed tint, listed in 4.3.

Components MUST consume `UiColors` through `crate::theme::ui_colors()`. A
hex literal in render code is allowed only for the fixed values in 4.3 or
inside `theme/builtin.rs`.

### 4.2 Semantic roles

| Role | Use | Paneflow Dark | Paneflow Light |
| --- | --- | --- | --- |
| `base` | Panel background, the work surface; the Settings page paints `settings_page_color` instead, which is `base` in dark and `card` blended 5% toward `text` in light, so cards keep their step above the page; while the terminal material is on it takes the pane card color, and where that is transparent (Mica on a light theme) a 0.5 `background` veil, so the page always separates from the sidebar | `#181818` | `#ffffff` |
| `surface` | Cards inside a panel, menu surfaces in dark | `#212121` | `#f7f7f7` |
| `card` | Modal cards, one step above `base`; a Settings card (`kit::group`) paints white at 0.031 in dark and 0.4 in light inside a `text` edge at 0.04 (0.09 in light) instead, so it stays readable over a material; `subtle` controls and `menu_panel` surfaces on it must stay one step above it | `#232323` | `#ffffff` |
| `overlay` | Shell chrome in dark, popups in light | `#141414` | `#ffffff` |
| `border` | Hairlines, card outlines, pane card border | `#252525` | `#e6e6e6` |
| `subtle` | Pills, search fields, toasts, resting control fill | `#2a2a2a` | `#eeeeee` |
| `muted` | Secondary text, icons at rest, eyebrows | `#a0a0a0` | `#636363` |
| `text` | Primary text, icons on hover | `#dddddd` | `#262626` |
| `accent` | Links, selected metadata, the one primary action | `#57d5c4` | `#4c6fff` |
| `vc_added`, `vc_modified`, `vc_deleted`, `vc_conflict` | Diffstat, status letters, change bars, attention border | `#57d992`, `#ffd166`, `#ff6f6a`, `#ffa657` | `#29681c`, `#df8e1d`, `#a00b2b`, `#fe640b` |
| `vc_added_background`, `vc_deleted_background` | Row washes in the diff for a theme that sets its own washes; every other theme washes rows with `vc_added` or `vc_deleted` at 0.07 over `base` | added, deleted at 0.12 | `#40a02b`, `#d20f39` at 0.16 |
| `vc_word_added`, `vc_word_deleted` | Word washes inside a changed diff row for a theme that sets its own washes; every other theme uses `vc_added` or `vc_deleted` at 0.20 in dark and 0.15 in light | `vc_added`, `vc_deleted` at 0.40 | `#40a02b`, `#d20f39` at 0.40 |
| `agent_error` | Failed agent state | `#ff6f6a` | `#a00b2b` |

The dark work surface is `#181818` and the dark chrome is `#141414`: the
panel is lighter than the shell around it, which is what makes the inset card
read as a card without a shadow. Light inverts the ramp: pure white work
surface, `#f7f7f7` cards, and a `#f3f4f9` title bar.

Every dark preset that does not ship its own `UiColors` is normalized by
`apply_surface_overrides` to the same chrome `#141414`, terminal `#181818`,
and border `#252525`, so the shell stays identical while the ANSI palette
changes. Light presets keep their own surfaces.

Diff colors on a dark theme fall back to Paneflow's canonical green and red
with opaque row washes unless the preset sets `use_theme_diff_washes`;
Vercel, Claude, and Tailwind do. Status hues are functional and MUST NOT be recolored
to match a brand when doing so weakens the meaning.

In every light preset, `vc_added` and `vc_deleted` are darkened from the wash
hue until they clear 4.5:1 on the title bar, the active and hover row tints,
`base`, `surface`, and the diff gutter where the row wash and the gutter wash
stack; the washes keep the lighter hue. `muted` clears 4.5:1 on the same
sidebar tints.

The Claude preset takes its palette from the Claude Code side of the Claude
desktop app, sampled on 2026-11-13 from screenshots of both themes at 100%
scale. Measured: the surfaces (dark `#191919` work area, `#1f1f1f` sidebar
and dialog, `#252525` bubble and field, `#2a2a2a` control fill, `#303030`
separator; light `#ffffff`, `#fafafa`, `#f3f3f3`, `#eeeeee`, `#e5e5e5`), the
text pair (`#f0f0f0` and `#9c9c9c`; `#191919` and `#666666`), the link blue
used as `accent` (`#6aabf1`; `#1757ac`), the danger red used as `agent_error`
(`#f17778`; `#a42528`), the scrollbar thumb, and, from the Claude Dark and
Claude Light code themes of its Settings, the keyword, function, type, string
and punctuation colors plus the diff rows, word chips and gutters that the
washes reproduce. Derived, because the app shows no source for them: cyan,
the bright ANSI column, the comment, number and constant syntax slots,
`vc_modified`, `vc_conflict`, the terminal selection (the app's `#257de0`
focus blue at 0.35 and 0.20), and the `#D97757` cursor, kept as the brand
mark of 4.3. Both variants are neutral greys: the earlier paper and graphite
values came from the chat side of the product.

The terminal selection foreground is never hand-tuned: it is recomputed at
theme load until it clears APCA Lc 45 against the selection background.

### 4.3 Tints and fixed values

| Tint | Dark | Light | Where |
| --- | --- | --- | --- |
| Sidebar row and pane tab active | `text` at 0.16 | same | `sidebar_tab_active_background` |
| Sidebar row and pane tab hover | `text` at 0.10 | same | `sidebar_tab_hover_background` |
| Tab icon card | title bar color blended with the active tint, then darkened 0.10 | darkened 0.05 | `sidebar_tab_icon_card_background` |
| Menu item selected | `text` at 0.10 | same | `select_item` |
| Menu item hover | `text` at 0.05 | same | `select_item` |
| Menu surface | `surface` lifted by 0.035 lightness | `overlay` | `select_menu_surface` |
| Menu border | `text` at 0.12 | same | `menu_surface`, on the surface shape of 4.9 |
| Control hover | `subtle` moved 6 percent toward `text` | same | `select_trigger`, `secondary_button` |
| Settings row and button tints | hover `text` at 0.05, pressed 0.09, well 0.08; never `subtle`, which several themes paint at the card's own value | hover 0.04, pressed 0.07, well 0.05 | `kit::Ink` |
| Settings text inks | secondary `text` at 0.6, tertiary 0.42 | 0.64, 0.45 | `kit::Ink` |
| Hairline | `text` at 0.04 | `text` at 0.09 | `kit::rule` |
| Unfocused pane dim | terminal background at `1 - unfocused_pane_opacity` | same | `unfocused_pane_opacity`, default 1.0, so off unless set |
| Attention border | `vc_conflict` at 0.7 | same | `pane.rs` |
| Icon button hover | the caller's hover color from 0 to 1 | same | `icon_button_sm`, `icon_button_md` |

Fixed values that deliberately do not follow the theme. They read as OS
controls or as system semantics rather than as brand, and are **Contextual**
to the surfaces named:

| Value | Where | Why |
| --- | --- | --- |
| `#339cff` | Toggle track when on, primary button fill under a white label | Platform toggle blue |
| `#ff453a` | Destructive button | System red, white label |
| `#007aff` | Pane and sidebar drop target, Paneflow terminal cursor | System blue for drag affordances |
| `#5aa6ff`, light `#0550ae` | Filter and settings search match text, `filter_match_color` | Match blue, identical in every preset of a variant; each value keeps 14 px semibold text at 4.5:1 on the active row tint |
| `#fbbf24`, light `#92400e` | Sidebar bell when an agent needs input | Amber request signal, identical in every preset of a variant; the light value keeps the 10 px word at 4.5:1 |
| `#83c3ff`, light `#0369a1` | Sidebar dot when an agent finished | Light blue completion signal, identical in every preset of a variant; the light value keeps the 10 px word at 4.5:1 |
| `#1a6ff6` | Title bar update pill when a release is known or staged for restart | Solid update blue with a white glyph and label, no border, identical in every preset; white text holds 4.52:1 at rest, and hover darkens the lightness by 0.05 like `solid_button` (5.3:1) instead of lightening |
| `#1a6ff6` at 0.30 to 0.07, light 0.16 to 0.04 | Sidebar star banner wash, a left-to-right gradient interpolated in Oklab; glyph `#8fb8ff`, light `#1a6ff6` (`star_banner_wash`, `star_banner_glyph_color`) | The update blue, so the one invitation in the app reads as a positive action and never as a status; the wash fades toward the dismiss button |
| `hsl(40 85% 55%)` | Callout warning, plan usage `Warn` level | Severity hue independent of preset |
| `#D97757` | Claude Code logo in the usage capsule, bubble, and tiles; base of the Claude Code activity ramp | Brand mark color, identical in every preset |

Plan usage has three levels, set by the used fraction of a window in
`UsageLevel::for_fraction` (`crates/paneflow-agent-config/src/usage/model.rs`):
`Normal` below 0.80 takes `accent`, `Warn` from 0.80 takes the callout warning
hue above, and `Critical` from 0.95 takes `agent_error`. `level_color` in
`src-app/src/ui_primitives/ring.rs` maps them for the ring and the tile
legend; the meters of Settings › Agents and of the usage bubble map them
through `kit::MeterTone::of` instead (`Normal` in the switch blue, `Warn` and
`Critical` in the kit warning and danger inks). A level color only ever comes from a
reported window: an account without a snapshot, billed through an API key, or
reporting no limits draws the bare track and never tints the capsule. The
activity registers use their own ramp, never the level colors: level 0 is
`subtle`, and levels 1 to 4 mix the agent base (`#D97757` for Claude Code,
`text` for every other agent) at 0.30, 0.52, 0.76, and 1.0.

### 4.4 Geometry

| Element | Radius | Corner | Border |
| --- | --- | --- | --- |
| Window | 10 | round | 1 px `border` on free edges |
| Main panel | 10 | round, masked | none |
| Pane card and right diff dock | 24 | squircle | 1 px `border`, or `vc_conflict` at 0.7 with attention; content stays inside the corner curve through its inset, 10 by 6 for a pane and 8 for the dock body, because GPUI clips to rectangles only. In compact density a pane rounds only the corners it shares with the outer edge of the grid, at the main panel radius 10, and draws its `border` only on those outer sides; attention outlines the whole pane. The dock keeps a full 1 px `border`, which is the line between the grid and the dock |
| System Info and About dialogs, toast | 20 | squircle | 1 px `border` at 0.6 on a dialog, none on a toast |
| Settings card | 12 | circular, the surface contract of 4.9 | none |
| Pane palette ground | 24 | squircle | none |
| Menu, select popup | 8 | circular, the menu contract of 5.6 in every style | 1 px `text` at 0.10 and the menu shadow |
| Menu row, select option | 4 | circular, concentric in 4 px of padding | none |
| Usage bubble | 14 | circular, the surface contract of 4.9 | 1 px `text` at 0.12 and the surface shadow |
| Primary sidebar workspace and tab rows | 9 | continuous corner approximation | no border |
| Primary sidebar header controls | 7 | continuous corner approximation | no border |
| Primary sidebar Settings button | 12 | continuous corner approximation | 36 px high, glyph 18 px for optical balance with the 20 px filter glyph |
| Primary sidebar filter | full | capsule | 36 px high, glyph 20 px |
| Primary sidebar usage capsule | 12 | continuous corner approximation | 36 px high, the Settings button skin |
| Usage ring | full | stroked circle, round caps | a `text` at 0.13 track; sizes 16 (stroke 2.5), 44 (4.5), and 76 (7); two concentric rings sit 3 px apart |
| Usage register cell | 2 | round | none; 8 px cells, 2 px gap |
| Primary sidebar inline hover actions | 6 | continuous corner approximation | no border |
| Tab icon cards, shared row skin, secondary button, tooltip, terminal search, title bar update pill, sidebar update banner, toast action button | 14 | squircle | tab icon card, tooltip and terminal search, 1 px `border` |
| Theme tile | 10 | round | 2 px `text` at 0.12, 0.32 on hover, 0.85 when selected |
| App icon tile | 18 | round, concentric with the 64 px icon plate through a 2 px gap | 2 px `text` at 0 at rest, 0.32 on hover, 0.85 when selected; tiles sit 6 px apart, in a wrapping row 10 px under the setting text; a tile carries no label, its name is the tooltip |
| Filter field, settings control, select trigger, title bar menu trigger | 8 | round | none |
| Drop overlay, drop placeholder | 8 | round | drop overlay 2 px blue |
| Theme mockup inner frame | 7 | round | none |
| Dismiss button (`dismiss_button`), sidebar update banner `Retry` | 24 tall, concentric with the host: 7 in the sidebar banner | squircle | none |
| Toolbar pill, sidebar IPC banner, sidebar branch chip, branch prompt field | 6 | round | IPC banner 1 px `border` |
| Title bar sidebar and diff dock toggles, branch prompt primary button | 5 | round | none |
| Icon button | 4 | round | none |
| Scrollbar thumb, header chip, filter clear | 3 | round | none |

Squircle means `squircle_fill` and `squircle_border` from
`src-app/src/ui_primitives/squircle.rs`, applied through `squircle_skin`,
`menu_surface`, or `tooltip_shell`. Plain `rounded()` is for
small circular controls and the explicitly round surfaces in the table.

A squircle is built where it paints. Each fill, border, band and stroke is
traced at its final bounds inside its element's paint and handed to
`paint_path` on every frame; nothing keeps or moves a built shape, because
GPUI `703b5ac3` makes `Path` opaque and bins it into tiles on every paint
(`gpui/src/path_builder.rs`). Under an opaque border the fill stops half a
border width inside the outer edge, so no fill edge coincides with a border
edge; under a translucent border it reaches the outer edge, so the border
composites over the fill across its whole width (`fill_depth_under_border`).
Stacked badge layers overlap the layer below by 0.5 device pixel
(`LAYER_CLEARANCE_DEVICE_PX`). The budget is 0.4 ms per frame for a strip of
12 tab badges, 48 shapes: `paths_tab_strip_12_badges` in the terminal suite
measures it (0.16 ms at the median on GPUI `703b5ac3`, WSL2 on the baseline
CPU), and the performance gates hold its allocations
(`bench/baselines/linux-x86_64/terminal-alloc.json`).

This table is the Rounded interface style, and its radii are the design
radii every call site passes. The surface shape and the Classic style of 4.9
resolve the same design radii to their own scales inside the squircle
primitives, so a
call site never branches on the style to pick a radius.

### 4.5 Spacing and sizes

| Measure | Value |
| --- | --- |
| Panel inset | 4 |
| Pane gutter | 8 in comfortable density, 1 px divider and no outer gutter in compact density |
| Pane content inset | 10 horizontal, 6 vertical |
| Pane header | One 44 px surface tab and action row; action gap 7 |
| Pane tab bar | 26 chips plus 6 below, 32 total; gap 3; 8 tabs maximum. A labelled tab keeps at least 96; when the bar cannot hold that tab beside its controls, the pane actions (split and pop-out, with their divider) go first, then the new tab button, and below 96 the tab shows only its icon in a 44 wide chip with no label and no close slot; the pane menu and the shortcuts keep every action. When the strip changes width it scrolls the active tab back into view, in the pane and in the dock. A sandboxed terminal tab shows the 12 px `shield-sandbox` glyph in `muted` between its label and the close slot, and its tooltip adds `Sandboxed: Paneflow and other panes are out of reach`; an icon-only tab drops the glyph and keeps the tooltip line (`sandbox_tab_marker` in `pane.rs`) |
| Diff dock tab bar | 44, the pane header chips (`unified_tab_chip`, `tab_close_button`, the terminal `tab_badge`), sized to the label between 64 and 208 instead of the pane's fixed 208; 32 tall, radius from the dock card frame; the strip scrolls with edge fades and the new tab button sits at the right beside the maximize action, with no close button since the title bar toggle closes the dock; a 1 px bottom divider, dropped while Changes is the active tab so its toolbar sits under the strip without a line; in Classic the strip sits in the title band at the title bar height with no divider (4.9) |
| Sidebar row | margin 8, padding 7 by 6, minimum height 32, content gap 3, icon-to-title gap 8, line height 20, spacing 2 (10 before the workspace that opens a new group), radius 9 |
| Sidebar tab icon stack | 16 px icons, cap 4, overlap 11, 24 by 24 icon card |
| Sidebar action button | 22, gap 1; folder glyph 15 in a 20 px slot |
| Sidebar footer | padding 0 top and 9.5 bottom; filter and gear 36 tall, gap 6, margin 8 shared with workspace rows; filter glyph 20, Settings glyph 18; filter text 15 with line height 20 and horizontal padding 10; banners margin 6 with 2 below, update banner 30 tall with padding 8 leading and 3 trailing |
| Sessions row | minimum height 32, terminal glyph on the tab title column; 5 rows per agent group before Show all |
| Settings row | padding 16 by 12, gap 16, a 32 px lead 12 before the text, control at most 240 wide (`kit::wide_row` uncaps it); section head 8 above its card, sections 32 apart, page text inset 17 |
| Select trigger | padding 10 by 6, width 190 to 260 |
| Menu | list padding 4, no item gap, item height 28, a 9 px divider slot, width fitted to the widest row between 160 and 320 (selects 200 to 280), max height 400 |
| Toggle | track 36 by 22, knob 18 |
| Icon buttons | small 20 outer with 12 icon, medium 24 outer with 13 icon |
| Toolbar pill | height 24, padding 8, gap 5 |
| Filter field | padding 10 by 6, gap 6, 13 px search icon, 16 px clear button with a 10 px glyph |
| Toast | inset 18, padding 16 by 12, minimum width 220, action buttons 26 tall |
| Scrollbar | width 6, gutter 10, minimum thumb 24, inset 2. Terminal panes overlay it: shown on any viewport move, held 1 s, faded out over 200 ms; hovering the gutter or dragging pins it, grows the thumb to the full gutter and reveals the track over 120 ms; `reduce_motion` snaps both |
| Diff | body inset 8 on the sides and bottom, row 20, file header 32 and sticky at the same height, 8 after an expanded file, hunk separator 24 inset 8 with a 32 expand cell, a gap of 2 and radius 6, gutter of 2 characters, the line number (3 digits at least) and 1 character, then a 2 px gap in the background color and 1 character before the code, change bar 4, split divider 2, column header 30, minimum split column 360, revert chip 56 by 16 inset 10 |
| Changes toolbar and tree | toolbar row 28 with 8 below, controls 24, layout segments 20 tall, icons 14; tree 40 % of the dock between 160 and 256 wide, header 28, rows 24 inset 6, indentation 12, icons 14, chevrons 12 |
| Code editor | 12 px mono, caret 2, scrollbar 15, minimum thumb 25; git marker column 6 left of the numbers, bar 4 radius 2 inset 1, deleted dot 8, hover grows 3 to the left; the numbers end 2 px inside a gutter at least 32 wide, and the code starts 16 px past it, room for the comment button as in T3 Code; rows 18 whether a line wraps or not; find bar 440 wide inset 6 from the top right, rows 36, controls 28 |

### 4.6 Typography

| Role | Family | Size and weight | Where |
| --- | --- | --- | --- |
| Interface | Geist, bundled, set on the root element | 12 Normal for body, Medium for titles in rows | Everything that is not a terminal or code |
| Labels | Geist | 11 Normal muted for eyebrows and descriptions, Semibold for `section_eyebrow` | Settings, rails, pills |
| Micro | Geist | 9 to 10 | Header chips, hints |
| Emphasis | Geist | 13 Medium | Row titles that need to outrank body |
| Title | Geist | 14 Semibold | Pane header, empty-state titles, callout titles |
| Page heading | Geist | 26 | Settings page title |
| Dialog title | Geist | 16 | About, System Info |
| Splash | Geist | 34 | Startup wordmark |
| Terminal | User choice among fixed-pitch families; default the bundled JetBrainsMono Nerd Font, with the Mono-suffixed name kept as an alias | 13 pt default, weight, line height, and cell width configurable | Panes |
| Code | `resolve_font_family(None)`, the terminal default | 12 | Editor, theme preview |
| Diff | The code family for lines, line numbers and the header diffstat; Geist for the file path and the hunk separator label | 13 for lines, 12 in the file header, 11 in the hunk separator | Changes tab |

The named constants live in `src-app/src/ui_primitives.rs`: `LABEL_XS` 10,
`LABEL_SM` 11, `BODY` 12, `BODY_EMPHASIS` 13, `TITLE` 14. Use them instead of
`px()` literals for interface text.

Bundled families (`src-app/assets/fonts/`): Geist, Geist Mono, IBM Plex Mono,
IBM Plex Sans, JetBrainsMono Nerd Font, Lilex. The Nerd Font ships in its
non-Mono variant so icon glyphs keep their designed size; the renderer
constrains them to their cells.

Sentence case everywhere. Titles truncate with a tooltip past 13 characters
and cap at 24 in the pane header; diff tab labels cap at 22 and file headers
at 64.

### 4.7 Iconography

Icons are single-color stroke SVGs in `src-app/assets/icons/`, painted with
`text_color` so they follow `muted` at rest and `text` on hover. Agent marks
live in `src-app/assets/agents/` and in the icons folder for Claude, Codex,
OpenCode, and Pi; a mark ships monochrome when the brand allows it and as a
multicolor image otherwise (`render_logo` decides per logo).

Workspace folder rows draw the glyph pair that `workspace_icon` (Settings,
Appearance, **Workspace icon**) selects, centered in the 20 px folder slot and
crossfading from closed to open with the disclosure. `sidebar/folder_glyph.rs`
owns the table:

| Style | Closed and open glyph | Size | Ink |
| --- | --- | --- | --- |
| `Default` | the shared Hugeicons `folder.svg` and `folder-open.svg` stroke glyphs | 15 px | `muted` |
| `Standard` | Hugeicons chevron right and chevron down, no folder | 16 px | `muted` |
| `Metal` | filled glass folder, an alpha gradient mask | 16 px | `text` |
| `Windows` | yellow gradient folder | 16 px | its own colors |
| `Windows 95` | 16-color pixel folder on a 16-cell grid, on the original 32-cell grid at a scale factor of 2 or more | 16 px | its own colors |
| `Pixel` | outlined pixel folder on a 16-cell grid | 16 px | `muted` |

A masked glyph takes its ink from the theme; a colored one is drawn as an
image and keeps its own. A pixel-grid glyph is sized so one cell covers a whole
device pixel at scale 1 and 2, and its slot margin is even so it never sits on
a half pixel: do not give it a size that is not a multiple of its grid. The
setting restyles workspace rows only. The welcome
screen, the diff dock, and the workspace settings keep the two `Default` files. The
sidebar footer settings button uses the matching `settings.svg` gear.

| Size | Use |
| --- | --- |
| 10 | Filter clear glyph |
| 11 | Sidebar agent state glyphs (bell, error, pull request) and the thinking matrix |
| 12 | Small icon button, select chevron, drag ghost |
| 13 | Medium icon button, filter search, preset logo, menu check mark, path picker recent clock |
| 14 | Title bar sidebar and diff dock toggles, editor logos, sidebar footer banners and gear |
| 15 | Toast error icon, sidebar folder, sidebar header glyphs |
| 16 | Sidebar tab icon, callout icon, diff dock tab icon, diff file header chevron, status icon and rename arrow, hunk separator icon, usage ring in the sidebar capsule |
| 18 | Empty-state glyph |

File type icons come from `src-app/src/file_icons.rs`, which resolves a file
name to one of 79 tokens: the exact lowercase name, then Paneflow's and T3
Code's extensions from the longest compound one (`test.ts` before `ts`), then
the built-in names, then the built-in extensions, then `default`. Each token
has a 16 px SVG under `icons/files/light/` and `icons/files/dark/` with its
colors baked in, and a row takes the folder of its theme (a `base` lighter
than 0.5 takes `light`). The tree, the dock tabs, the Changes tree, the file
finder and the path picker share that resolver. Folders keep their chevrons
and never get an icon. `scripts/generate-file-icons.ts` regenerates both
folders and the tables from a T3 Code checkout.

The usage ring (`ring` and `concentric_rings` in
`src-app/src/ui_primitives/ring.rs`) is the one gauge glyph of the app: a track
plus a value arc with round caps, drawn from twelve o'clock clockwise with
`PathBuilder::stroke`. A value of 0 shows the track alone, 1.0 closes the
circle, and anything outside [0, 1] is clamped. Below 8 px, or with a stroke
of at least the radius, nothing is drawn. It has no motion, so reduced motion
changes nothing. The 16 px size sits in the sidebar capsule, the 76 px
concentric pair (week outside, session inside at 0.72, the agent logo in the
middle) leads each Settings › Usage tile, and the 44 px size is reserved.

### 4.8 Motion

| Motion | Duration | Easing | Notes |
| --- | --- | --- | --- |
| Hover on any control | 120 ms, scaled by the distance left to travel | ease-out quint | `animated_hover`, retargets mid-flight, pauses during a drag; the settings button skins (`raised_skin`, `soft_skin`, `ghost_skin`) blend their squircle fill through `animated_squircle_skin`. Press feedback stays instant. Menu and select rows keep an instant highlight |
| Toggle switch | 180 ms, scaled by the distance left to travel | ease-out quint | `animated_toggle`: the knob slides and the track crossfades between off and `switch_blue`; retargets mid-flight, and a switch seen for the first time rests |
| Pane header buttons | 120 ms | ease-out quint | Tint of the action buttons and the close glyph; the close slot itself toggles with the header hover |
| Unfocused pane dim | 130 ms, scaled by distance | ease-out quint | Overlay of the terminal background |
| Primary sidebar slide | 280 ms | cubic ease-out | Panel inset and gutter follow the width; double-clicking the resize handle glides the width back to the default on the same curve |
| Resize edge nudge | 220 ms | smoothstep out over the first 32 %, smoothstep home over the rest | `resize_edge`: a drag that reaches the floor or the ceiling of a resizable edge pushes it 5 px past the limit and brings it home, once per visit to the limit however long the pointer is held there; the stored size never leaves its range. Used by the Workspaces sidebar |
| Resize detent settle | 120 ms | cubic ease-out | `resize_edge`: when a drag catches or leaves a detent, the edge eases over the gap instead of jumping, trailing the pointer by the detent reach at most. Used by the Workspaces sidebar's default width |
| Workspaces rows | 180 ms | cubic ease-out | `SidebarRowMotion`: folding or unfolding a workspace folder, or filtering the rail, fades each tab row while its measured height grows or shrinks, the group gap below a folder follows the last row, and the folder glyph crossfades between `folder.svg` and `folder-open.svg`; a new toggle retargets from the current state, and a folder seen for the first time rests |
| Workspaces selection | 150 ms | cubic ease-out | `SidebarSelectionMotion`: selecting or releasing a row fades its `accent` wash and turns a workspace title to `text`; where it touches a selected row, the wash between them fades in and the shared corners square off at the pace of the slower of the two; a new toggle retargets from the current state. Rows closed from a selection fade and fold over 180 ms, through `SidebarRowMotion` in the folder grouping, and are removed 200 ms after the close is confirmed. Reduced motion applies every state at once and removes the rows immediately |
| Menu reveal | 140 ms | cubic ease-out | `menu_reveal`: every menu, select popup, context menu, and submenu fades in from 0 while dropping 4 px into place; the pane palette's `New branch` form and the branch row it folds back to use the same reveal. No exit animation |
| Toast | 180 ms in, 1440 ms hold, 180 ms out | ease-in-out | 8 px lift on entry, 8 px drop on exit |
| Status spinner | 1 s loop | linear rotate | Empty states (`panel_empty_state`). Static under reduced motion |
| Comet spinner | 700 ms loop | linear rotate | `comet_spinner` on `icons/comet.svg`: a ring stroked at 0.12 of its size whose opacity ramps linearly from 0 to 1 clockwise from the top, with a round head at the top; every busy state of the title bar update pill. Static under reduced motion |
| Sidebar thinking matrix | 720 ms cycle | stepped | 3 by 3 dots of 3 px, gap 1, trailing opacities 0.81, 0.49, 0.26 over a 0.06 base |
| Startup splash | 2600 ms shimmer, 900 ms minimum on screen | linear | Letters at 0.54 alpha, shimmer to 0.82 |
| Tooltip | 800 ms delay | none | `delayed_tooltip`. `accessible_control` already calls it with the control's label, and GPUI panics in a debug build on a second tooltip for one element: a control built with `accessible_control` takes no other tooltip, and a test that builds the element catches the mistake (`every_app_icon_tile_builds_with_a_single_tooltip`) |

Both spinners turn through GPUI's `TimedTransformation::rotation`
(`ui_primitives::spin`): the glyph is painted once and the window turns it in
the retained scene on each frame, so a spinner never rebuilds its view
(`spinners_turn_for_sixty_frames_without_rendering_their_view_again`). The
phase is app-wide, not per element: every rotation is locked to the app's
shared animation clock, so two spinners of the same period always point the
same way, and one that appears mid-turn joins the others instead of starting
from the top. The sidebar thinking matrix keeps its stepped `SpinnerClock`,
which already shares one clock across the sidebar.

`reduce_motion` (Settings, Appearance) sets both
`ui_primitives::reduce_motion()` and GPUI's `cx.reduce_motion()`
(`ui_primitives::set_reduce_motion`), and every animation that reads either
one rests: among them `animated_hover` settles instantly, the primary sidebar
toggles without the slide, stops at its resize limits without the nudge and
snaps onto its default-width detent without the settle,
the Workspaces rows fold and filter without
motion, `menu_reveal` mounts menus at rest, toasts and the empty-state and
comet spinners hold still, and the sidebar thinking matrix holds its first
frame. The terminal drops its sub-line pixel offset so a trackpad scrolls the
scrollback by whole lines. The pixel offset only ever exists while Paneflow
scrolls its own scrollback: it is 0 whenever the program reads the mouse, the
alternate screen scrolls alternately, or `reduce_motion` is on, and turning
`reduce_motion` on drops an offset already shown on the next frame, without
waiting for a scroll event. The unfocused pane dim does not read the flag and
keeps fading. The config description promises a static frame for decorative
animations; that promise is **Proposed** until every remaining animation reads
the flag. Feedback is never removed, only its interpolation.

### 4.9 Interface styles

`interface_style` (Settings, Appearance, **Interface style**) selects the
layout and shape language of the whole app. Classic is the default; a config
that still says `paneflow`, the former name of Rounded, selects Rounded. `ui_primitives/interface_style.rs` holds the
active style, set at boot, on every config reload, and when the setting
changes, which also refreshes every window. Color roles, type, spacing outside
menus and the path picker, and motion are shared by every style.

| Style | Corners | Floating surfaces |
| --- | --- | --- |
| Classic (default) | Circular corners on a 6, 8, 12, 14 scale, in the edge-to-edge shell described below | A `text` edge at 0.07; toasts carry a deeper two-layer shadow (0 4 16 at 0.16, 0 1 4 at 0.12), and menus and select popups the panel shadow of the shadow inventory below |
| Rounded | The continuous corners and radii of 4.4 on the chrome; circular corners on a 6, 8, 12, 14 scale on menus, select popups, the usage bubble and settings cards | A `text` edge at 0.12 on menus and select popups, which carry the panel shadow of the shadow inventory below; toasts carry a two-layer shadow (0 6 20 at 0.10, 0 1 3 at 0.10); a `border` edge at 0.6 and no shadow elsewhere |

The Rounded style draws two shapes. Its chrome (pane cards, tabs, rail rows,
dialogs, toasts, the command palette, the pane palette, buttons) keeps the
squircles of 4.4. Four surface families draw the surface shape instead: popup
and context menus, select popups, the sidebar usage bubble, and settings
cards. `interface_style::Shape` names the three shapes (`Rounded`, `Surface`,
`Classic`), and `surface_shape` maps Rounded to Surface and leaves Classic
unchanged. The builders of those surfaces in `settings/components.rs`
(`menu_surface_filled`, `floating_menu_panel`, `select_menu`, `select_option`,
`menu_row`, `menu_divider`, `card_tint`) and the
usage bubble run inside `with_surface_shape`, and a squircle primitive keeps
the shape it was built under until it paints. `pick(rounded, classic)` chooses
a value by style; `pick_surface(rounded, surface, classic)` is for a value
that changes on a surface, such as the menu label size. The generic
`select_item` row keeps its 12 px text outside a menu.

The command palette and the Clone repository dialog keep the Rounded shape
in every style: `render_command_palette` and `render_clone_repo` build inside
`with_rounded_shape`, as do the lazily built rows of the palette list, and
they read the Rounded constants of `ui_primitives/quick_pick.rs` directly.

`surface_corner` maps a design radius from 4.4 to the surface scale at paint
time, reached through `interface_style::corner`:

| Design radius (4.4) | Surface radius | Surfaces |
| --- | --- | --- |
| 20 and above | 12 | Settings card |
| 16 to 19 | 14 | Usage bubble |
| 8 to 15 | 6 under 30 px of height, 8 from 30 px | Focus rings |
| Under 8 | The design radius, capped at 6 | Small marks |

The surface contract:

| Surface | Contract | Source |
| --- | --- | --- |
| Menus, context menus, select popups | The menu contract of 5.6, identical in every interface style | `ui_primitives/action_menu.rs`, `settings/components.rs` |
| Usage bubble | Radius 14, padding 6, a `text` edge at 0.12 and the surface shadow | `settings/components.rs` |
| Settings card | Radius 12 with a 1 px edge in every interface style: `kit::group`, the Keyboard Shortcuts sections included; a hoverable row that touches the card rounds the corners it touches (`kit::Place`), because GPUI clips to a rectangle | `settings/kit.rs` |

Classic takes its geometry from the Claude Code side of the Claude desktop
app, measured on 2026-11-13 at 100% scale from its accessibility tree and from
circle fits on screenshots: 32 px sidebar rows, 28 px toolbar buttons, 28 px
menu items, selects, inputs and 36 px picker rows at radius 8; 24 px inline
buttons at radius 6; code chips at radius 4; the dialog, the message bubble
and the composer at radius 12; a menu at radius 14 with 6 px of padding.
`classic_corner` maps a design radius to it:

| Design radius (4.4) | Classic radius | Surfaces |
| --- | --- | --- |
| 20 and above | 12 | Settings card, dialog, toast, and a pane card outside the edge-to-edge shell |
| 16 to 19 | 14 | Menu, select popup, path picker panel |
| 8 to 15 | 6 under 26 px of height, 8 from 26 px | Buttons, sidebar controls, tooltips, inputs, terminal search, tab icon cards |
| Under 8 | The design radius, capped at 6 | Header controls, inline actions, badges, chips |

A corner never exceeds half the short side of its surface. Fills and borders
paint as quads; the path API (`squircle_path` and its band and stroke
variants) traces the same circular corners. A radius
passed to a squircle primitive is always a design radius: resolving it twice
gives a wrong corner, so a surface that needs an exact value passes one that
its scale maps to itself (under 8, or 8 on a control at least 26 px tall in
Classic and 30 px tall on a surface), passes a design radius whose band gives
it, or paints its own `rounded()` quad.

Classic overrides, each behind `interface_style::pick`, `classic()` or
`circular()`, which is true on a surface and in Classic:

| Surface | Classic contract | Source |
| --- | --- | --- |
| Menus and context menus | Radius 14, padding 6, rows 28 tall at radius 8 with no gap: 6 plus 8 keeps a row concentric in the surface. Labels are 13 px Medium with a 10 px inset on both sides; shortcuts are 12 px Medium in `muted`; dividers are `text` at 0.12 with the same 10 px inset and 4 px above and below; the submenu chevron is 16 px and overhangs the inset by 4 px. In Geist, 13 px Medium reproduces the label width and stem weight measured on Claude Code desktop, which sets 14 px Medium in the system font. Every value comes from `menu_text_size`, `menu_text_weight`, `menu_row_inset`, `menu_shortcut`, `menu_divider` and `menu_chevron` | `settings/components.rs` |
| Path picker | The panel takes the menu design radius (14 in Classic), so its radius 8 rows stay concentric in 6 px of padding; rows are 36 px tall with no spacing, and the highlighted row is a `text` wash at 0.14 with the regular text colors, not the selection blue | `terminal/path_picker/render.rs` |
| Toast | Radius 12 on the menu surface with the edge and shadow, padding 14 by 10, one row: the 16 px error glyph, the message in Medium, then the actions as raised buttons in the inverse fill on the trailing edge | `app/notifications.rs` |
| Window corners | The shell is edge to edge, so a flush pane card, the diff dock card or the Settings panel can sit in a corner of the window. GPUI clips content to rectangles only, so a surface that does rounds that corner itself to 9, the 10 of the client-side window inside its 1 px border, and only where the window draws that corner round (client decorations on that side, not tiled, maximized or fullscreen) and nothing stands between the surface and the window edge: no primary sidebar on the left, no right rail on the right, and the panes in the title band at the top. Every other flush corner stays square. The main window sets which corners qualify on each frame (`title_band::TitleBand::window_corners`), and `flush_corner_radii` reads it | `app/render.rs`, `layout/density.rs`, `window_chrome/title_band.rs` |
| Pane tabs | Radius 8 instead of the capsule, in the pane and in the dock; the close slot is a radius 4 square | `pane.rs` |
| Pane drop veil | Radius 8 | `pane.rs` |
| Settings card | Radius 12 and no border, as on a surface | `settings/components.rs` |
| New pane branch select | The 28 px trigger takes radius 8, the radius of the launcher rows stacked under it at the same width: `menu_item_radius` | `app/pane_palette.rs` |
| Rail rows | Radius 8 instead of 9 on the 32 px rows of the Workspaces rail (workspace, tab, agent, keyboard cursor ring) and of the Settings navigation: `rail_row_radius` | `app/constants.rs` |
| Sidebar filter and Settings search | Radius 8 instead of the capsule, with no edge; a `text` fill at 0.05 while the input shows | `ui_primitives.rs` |
| Dialogs | Every `modal_card` resolves to radius 12 | `settings/components.rs` |

The Classic toast was not observed in the reference app, so its layout is
Paneflow's own. Colors stay the theme's in every style: the reference greys
are not imported.

Classic also changes the shell, after T3 Code: `flush_shell` in
`app/cli_diff_dock.rs` lays the window out edge to edge.

| Region | Classic layout | Source |
| --- | --- | --- |
| Main panel | No inset, no radius and no corner masks. Panes join edge to edge whatever `comfortable_density` says, with square outer corners and no outer border | `rendered_pane_density` in `app/render.rs`; `flush_radius`, `flush_border` in `layout/density.rs` |
| Dividers | 1 px `border` lines: one down the whole window at the right edge of the primary sidebar, one at the left edge of the dock, one at the left edge of the sessions rail under the title band | `app/render.rs`, `app/cli_diff_dock.rs` |
| Title band | The title bar paints bare, its background and drag region only, under the panel. The shell overlays the brand (sidebar toggle, Files, Help) at the top left and the trailing controls (update pill, dock toggle, caption controls) at the top right | `render_brand`, `render_trailing` in `window_chrome/title_bar.rs` |
| Pane headers | Every pane header is as tall as the title bar instead of 44. The pane grid starts at the top of the window, so the headers of the top row sit in the title band. The top-left header insets its tabs 8 px past the brand when the sidebar is hidden, and the top-right header keeps its actions 8 px clear of the trailing controls when no dock is open; both widths are measured by zero-size probes. The empty part of a top-row header drags the window and zooms it on a double-click, while tabs, the new tab button and the header actions keep the press | `window_chrome/title_band.rs`, `pane.rs` |
| Dock | The dock tab strip sits in the title band above the dock column, on the dock card background, followed by the trailing controls. The dock column has no header row and no card border, and the strip starts past the brand when the dock fills the panel with the sidebar hidden | `render_diff_dock_title_strip` in `app/diff_dock/mod.rs` |
| Settings, welcome, empty workspace | A plain title band with a 1 px `border` line under it | `app/render.rs` |

Toggles, status dots, avatars, segmented pills, and the usage ring keep their
full radius in every style. The window and the main panel keep radius 10.

#### Shadow inventory

Every shadow Paneflow paints, as offset x y, blur, spread, then black or a
named color at an alpha. GPUI draws each one with Raph Levien's closed-form
blurred rounded rectangle and bounds it to three times its blur past the
shape; upstream states the approximation differs from the previous renderer
by at most 5/255 per channel (#66812), and no value below compensates for it.

| Surface | Shadow | Source |
| --- | --- | --- |
| Client-side window, floating | 0 0, blur 5, black 0.40 | `window_chrome/csd.rs` |
| Menus and select popups, every style | 0 18, blur 44, spread -18, black 0.80 in dark; 0 16, blur 40, spread -18, black 0.55 in light | `panel_shadow` in `ui_primitives/action_menu.rs` |
| Toast, Rounded | 0 6, blur 20, black 0.10, then 0 1, blur 3, black 0.10 | `floating_shadow` in `settings/components.rs`, called by `app/notifications.rs` |
| Toast, Classic | 0 4, blur 16, black 0.16, then 0 1, blur 4, black 0.12 | `floating_shadow` |
| Dialog card (`modal_card`), drag ghosts, fleet search, custom buttons dialog | GPUI `shadow_lg`: 0 10, blur 15, spread -3, black 0.10, then 0 4, blur 6, spread -4, black 0.10 | `settings/components.rs`, `app/drag.rs`, `pane_drag.rs`, `app/fleet_search.rs`, `app/custom_buttons_modal.rs` |
| Clone progress bubble | 0 10, blur 28, black 0.28, scaled by the intro and leave motion | `app/clone_bubble.rs` |
| Active tab capsule of the detached pane window | 0 1, blur 2, black 0.24 in dark; 0 1, blur 3, black 0.10 in light | `unified_tab_chip` in `pane.rs` |
| Raised button at rest | 0 1, blur 2, its own fill at 0.24, removed while pressed | `raised_skin` in `settings/components.rs` |
| Select trigger | 0 1, blur 2, black 0.05, plus a 0 1 black 0.04 edge in light or a 0 -1 white 0.06 edge in dark, removed while pressed | `select_trigger` in `settings/components.rs` |

On Linux, the wgpu renderer dithers gradients since GPUI `703b5ac3`
(`caead827d7`): an ordered 4 by 4 Bayer offset of at most 1/255 on color and
1.5/255 on alpha. The gradients are the tab badge fills (`tinted_badge_colors`
and the graphite tile in `pane.rs`) and the sidebar resize edge glow
(`app/sidebar/resize.rs`).

## 5. Component Contracts

### 5.1 Title bar

Full width on every platform, drag region, double-click zooms, right-click
shows the window menu where the platform has one. Left rail: the sidebar
toggle (20 px, radius 5, resting tint when the sidebar is hidden), then
`Files` and `Help` triggers (height 20, padding 6, radius 8, 12 px, muted
until hovered or open). Center: a 3 px muted dot and the workspace name at 12
px Medium, hidden in Settings. Right: the diff dock toggle, then the caption
controls. The toggle mirrors the sidebar toggle (20 px, radius 5, the 14 px
`icons/layout-sidebar-right.svg` glyph in `muted`) and is the only control
that opens the dock from the chrome: pane headers carry none. It holds the
resting tint while the dock is open, the reverse of the sidebar toggle, which
is tinted while its sidebar is hidden. Its tooltip reads `Show Changes dock`
or `Hide Changes dock`. It opens the dock on the active tab's checkout and
always closes an open dock; `toggle_diff_dock` (`Cmd/Ctrl+Alt+B`) and the
command palette run the same toggle. It is absent in Settings and when no
workspace is shown. On Windows, when the caption glyphs are visible, a
centered 1 px by 16 px `border` divider follows it after an 8 px gap; with no
caption controls it keeps the 8 px edge inset. The title bar
still carries its own IPC pill code, but the cockpit shell never renders it
(`tb.cockpit = true` in `app/render.rs`); that code is **Migration**, and the
sidebar footer owns the IPC banner. Between the center and the diff dock
toggle sits the update pill, the single control for every update state
(`resolve_update_pill` in `app/self_update_flow.rs`): height 24, padding 10,
gap 5, squircle 14 through `squircle_skin`, no border, 12 px Medium like
`solid_button`. Any known
release raises it, whether the startup check, the four-hourly recheck, or
`Help > Check for Updates…` (the `PaneFlow` menu on macOS) found it.
`v<x.y.z> available` and `Restart to update` are solid `#1a6ff6` with a white
download or refresh glyph and label; a click installs or restarts. Every
actionable state fires on click release, never on press, and exposes the
button role with its label as the accessible name. Once `Restart to update` is
clicked the button stays blue with its label while the sessions stop and the
app restarts, the refresh glyph swapped for the white comet spinner, and further
clicks do nothing.
`Downloading update…` and `Installing update…` show the 12 px comet spinner in
`text` on `subtle` at 0.7 opacity; `Update via rpm-ostree` and `Update via package
manager` show the tool glyph on `subtle`; a failed install reads `Retry update`
with the 12 px alert glyph in `vc_deleted` on its 0.12 wash and retries on
click. No state carries a dismiss control: the pill stays until the release is
installed. A manual check lays its transient states over the same slot: `Checking for
updates…` with the comet spinner on `subtle` at 0.7 opacity, then `Paneflow is up to
date` in `vc_added` with no glyph, or `Retry update check` with the alert glyph
in `vc_deleted`, each on its 0.12 wash and gone after 3 s. On Windows the caption glyphs are native Windows 11
shapes; on macOS the traffic lights get 80 px of brand padding. The title bar
draws no bottom hairline inside the cockpit shell; the panel inset separates
it from the content. In the Classic style the bar paints bare and the shell
places its brand and trailing controls over the pane headers and the dock
tabs of the title band (4.9).

### 5.2 Primary sidebar, Agents mode

The primary sidebar uses the bundled Geist at 14 px, Medium for workspace and tab titles, with more generous row sizing than the Unpeel reference. Branch labels use 14 px Geist text in the same family as workspace titles; diff statistics use 12 px Geist text. Simple rows have a 32 px minimum height and 9 px corners; A row fits on one line: the title, then its checkout's branch in muted text after a 6 px gap with the branch or pull request glyph, the branch truncating before the title, and the diff statistics on the trailing edge. A checkout is printed once: the workspace row carries the branch and the diff statistics of the workspace's checkout, folded or not, and a tab row carries them only when the tab is bound to a worktree of its own, so a tab row with a branch is a worktree. Grouped by status, where no workspace row stands above it, every tab row carries its checkout's. Folder glyphs are 15 px inside 20 px slots. A workspace title rests in `muted`, the color of its folder glyph, and turns `text` under the pointer, so the tab titles, in `text`, read first. The active tab takes the active row tint, every other row the hover tint under the pointer. A workspace title, tab title, or branch longer than 13 characters shows its full value in a tooltip, and a session row's tooltip leads with its label. Other application surfaces retain Geist.

All squircle fills and borders use three cubic Bezier segments per
corner in `src-app/src/ui_primitives/squircle.rs`. This is an approximation of
Apple's continuous corner profile using normalized UIKit control points
documented by [Liam Rosenfeld](https://liamrosenfeld.com/posts/apple_icon_quest/).
Unpeel delegates its shape to SwiftUI's `RoundedRectangle` with `.continuous`:
radius 9 for rows, 7 for footer buttons, and 6 for session action buttons.
UIKit control points are not proof of exact SwiftUI rendering. Native macOS
path extraction and a rendered comparison remain required before claiming
visual parity. The footer deliberately uses larger controls than Unpeel: 36 px
high with a 20 px filter glyph, an 18 px Settings glyph, a full-radius capsule
filter, and radius 12 for Settings,
following the supplied larger reference.
The sidebar uses the same shared renderer as cards, menus, tooltips, Settings,
pane surfaces, and the diff dock. Each component retains its own radius.

The rail's right edge resizes it. A 7 px handle runs from under the title bar
to the bottom of the window, 2 px over the rail and 5 px over the main panel,
absent in Settings, while the rail is hidden, and during its slide. Under the
pointer, or while dragging, it shows the column resize cursor and a 1 px rule
in `text` at 0.16 on the rail's last pixel column, fading to nothing toward
both ends so it never meets the panel's rounded corners; the rule is invisible
at rest and goes out while the drag is held against a limit. The width follows
the pointer from where it was pressed, between 240 px and the smaller of
480 px and the window width minus the right rail and the room the main
panel needs: 200 px, or while the diff dock is open the dock's 360 px minimum
plus a minimum pane and its gutters, so in a narrow window the sidebar
narrows before the dock or the pane grid gives way; rows, the title bar rail, the footer and the drag previews follow it.
Reaching either limit plays the resize edge nudge of 4.8. The default width,
432 px, is a detent with no mark of its own: while the pointer asks for a
width within 12 px of it the edge holds on 432 px and the rule turns solid
`accent` over its whole height, so the default is found by feel and widths
within 12 px of it cannot be chosen; catching and leaving the detent play the
resize detent settle of 4.8. A window too narrow to show the default has no
detent. The cursor stays the
column resize cursor over the whole window until the button is released.
Double-clicking the handle glides back to the default. A narrower window caps
the width shown without forgetting the chosen one. The chosen width persists
in `window-state.json` (`sidebar_width`), written once a gesture ends and
absent at the default; a missing or unusable value falls back to the default.
The handle
is a vertical splitter named `Resize the Workspaces sidebar` and has no
keyboard path yet.

Header row `Workspaces` at label size with two 22 px icon buttons (the
Customize sidebar menu behind a filter glyph, and new workspace behind a
folder-plus glyph), and a third, `New tab` behind a 15 px plus glyph, while
the rail is grouped by status. Once a space is declared, the space picker replaces the
`Workspaces` label: a 22 px trigger at radius 7 holding, in a 20 px slot on
the folder column, the active space's 16 px mark (its image cropped to a
circle, else the plate of its color, else a 1 px `muted` outline;
Default shows its own neutral plate, bone ink on an anthracite ground),
then the name at 13 px Medium in `text`
and a 12 px chevron. It takes the hover tint, and the active row tint while
its menu is open. The menu (`menu_panel`, 240 wide) lists Default, then the
declared spaces with their 16 px mark, a hidden space's attention mark and a
check on the current one, then `New space…` and `Manage spaces`, whose 13 px
glyphs sit centered in the same 16 px slot so every label starts on one edge. While a hidden
space has an agent waiting, failed, or finished unseen, a 22 px button beside
the trigger shows the most urgent row glyph (bell, circle-x, or dot) and the
total above one; it goes to that agent, and its tooltip names the space. A
space color fills a swatch and never tints text. A workspace is a folder row; its tabs are child rows with
inline rename, hover actions, and reorder by drag. A workspace row shows its
title, the branch of its checkout with its glyph, and the
diffstat in `vc_added` and `vc_deleted`; a tab row shows its title, and the
same two only for a worktree of its own. A title and the label that follows it
(branch, or workspace suffix) keep their full width while the line holds both.
Past that the title gives way first: the label takes all the room a short
title leaves and never less than half of the line, so neither ends in an
ellipsis while the other leaves it room (`sidebar_title_with_trailing`).
Status sits at the trailing edge of
both row kinds, on one X. An agent that needs you, failed, or finished shows a
status badge and no glyph: `Need you` in amber, `Error` in `agent_error`, and
`Done` in light blue with the unread count, each in 12 px Medium on its own
color at 0.16 (0.08 on a light theme), 18 px tall with 6 px of padding, in a
radius 6 squircle sitting on the whole pixel below the line center. While it
thinks, the dot matrix stands alone in the agent's accent when every working
agent of the row shares one runtime that declares one, and `muted` otherwise.
The tree and the Agents section rank a session with the same function, so a
row and its agent never disagree. A tab's pull request never takes the slot: it
replaces the branch glyph in front of the branch, in GitHub's state color, with
the merge glyph once merged and the pull request glyph otherwise, and the state
lives in the branch tooltip. A folded workspace with no agent to report shows
its best pull request in the slot as that 11 px glyph alone, with no word;
while branches are shown, its own checkout's request already sits on its
branch label, and the slot takes the best one among its worktree tabs. Amber and light blue are the fixed colors from 4.3. The slot is shared with the hover action: under the
pointer the status turns invisible, its width kept, and the button paints
over it. A `Customize sidebar` menu on the rail header toggles branch,
diffstat, pull request, status, and indent guide per value, then, below a menu
rule, the Agents section behind a 13 px sparkles glyph (`sidebar_show.agents`,
on by default) and the usage capsule. `Status` sits behind the
`notification-01` glyph (`sidebar_show.status`, on by default); turned off, the
workspace and tab rows lose their status badge, their matrix, the title
shimmer and the status tooltip, and a folded workspace's pull request takes
the trailing slot by the same rule as a row with no agent. Off means the
status shows once, not never: while the Agents section is folded or hidden
(`sidebar_show.agents: false`), the tree rows show their status again, so the
sidebar never loses it. The Agents section, the hidden-space
attention button, the pane header marker, notifications and
`Cmd/Ctrl+Shift+J` do not read it. `Shimmer` follows `Status` behind the
`ai-magic` glyph (`sidebar_show.shimmer`, on by default): a working tab's
title rests in `muted` while a band sweeps it up to white, or to black on a
light theme, the extreme on the side of `text`. Turned off, the title keeps
`text` and holds still, and the matrix alone says the agent works.

`Group by` follows `Show` in the `Customize sidebar` menu, with the current
grouping in `muted` before its chevron; its submenu holds `Folder` and
`Status`, a check on the current one (`sidebar_group_by`, `folder` by
default). `Folder` is the tree above. `Status` replaces the folder rows with
the Agents section's group headers, same order, colors, 24 px height, count
and 8 px gap above every header but the first: `Need you`, `Error`, `Done`,
`Working`, `Idle`. Under them sit the tab rows of every workspace of the
active space, in workspace then tab order, 2 px apart. A tab takes the status
of its most urgent agent, the one its row badge shows, and `Idle` when it runs
none; there is no `No status` group. A tab row loses the folder slot and the
indent guide, so its title starts on the header's edge, 15 px from the rail
edge, and the title is followed by ` · ` and the workspace name in `muted`,
as on an Agents row, the title shrinking first; the branch, the diffstat, the
status and the close button keep their places, and the tooltip of a long name
gives `title · workspace`. The rows always show their status, whatever
`sidebar_show.status` says, since the rail is grouped by it. Grouped by
status, the rail hides the Agents section, which would list the same agents
twice, and `Show` drops its `Agents` and `Indent guide` lines; `Expand all`
leaves the menu; the drop dividers are not drawn, so rows do not reorder by
drag, and the workspace menu is reached by returning to `Folder`. With no tab
open, one 32 px row reads `No tabs open` at 13 px `muted`, inset 15.

`New tab`, shown only while grouped by status, is the third header button,
after `New workspace`, on the hover tint and the same tint while its menu is
open. It opens the launcher in a new tab of a workspace: at once when the
active space has one workspace, the folder picker when it has none, and
otherwise through a 208 px menu 32 px below the button, right-aligned: the
caption `New tab in` at 12 px Medium `muted`, then one row per workspace of
the active space in rail order, a 13 px folder glyph and the name, scrolling
past 320 px.

The Agents section sits between the workspace tree and the footer, with no
separator. Where the tree or the Agents list scrolls, content hidden past its
top or bottom edge fades out over 48 px instead (`scroll_edge_fade`): a
gradient to the surface color under Themed, and a per-row opacity ramp under
Blended and Mica, whose translucent surface a color gradient cannot erase.
Nothing fades while the list fits. The section's 36 px header (padding 8
horizontal) holds a 20 px toggle with no fill, Cerata's section header: the 12 px Hugeicons `arrow-down-01` chevron,
then `Agents` at 12 px Medium 4 px after it, both `muted` and turning `text`
under the pointer.
At the header's trailing edge, a 22 px button at radius 7 holds the 15 px
Hugeicons `preference-horizontal` glyph in `muted` (`Customize agent rows`).
It opens a 172 px menu 10 px above itself, right-aligned, on the usage
bubble's fill (under Mica or Blended, the active tab tint laid opaque over
the title bar color, since the compositor blur reaches only the desktop and a
translucent fill would show the rows beneath; the menu surface under Themed),
that toggles the runtime logo, the workspace suffix and the
working title's shimmer (`sidebar_show.agent_logo`, `agent_workspace`,
`agent_shimmer`, all on by default). A row
always shows its status: hiding it would hide what the section is grouped by,
and `sidebar_show.agents` already hides the whole section. Either toggle
leaves a row 32 px tall. When the config declares at least one space, a
divider and an `All spaces` line follow, behind the Hugeicons `layers-01`
glyph (`sidebar_show.agent_all_spaces`, off by default): turned on, the
section lists every space's agents, the active space's first within each
status group, and a row from another space ends its workspace suffix with
` · ` and that space's name, `Default` for the default space. The suffix
toggle hides both, and the row's spoken label names the space either way. Clicking the header collapses the section to that
row, the chevron turning -90 degrees over 180 ms (cubic
ease-out, instant under `reduce_motion`); nothing expands it on its own, not
even an agent turning blocked.

The agents are grouped by the status a row shows, in a fixed order: `Need
you`, `Error`, `Done`, `Working`, `Idle`, `No status`. A status no agent has
gets no group. Each group opens with a 24 px header: its label at 12 px Medium,
then, 6 px after it, the number of agents in `muted` with tabular figures, the
text starting 15 px from the sidebar edge, on the leading edge of the row
content (margin 8, padding 7). The label takes the status color for the three
groups that wait on you, amber for `Need you`, `agent_error` for `Error` and
light blue for `Done`, the fixed colors from 4.3, and `muted` for `Working`,
`Idle` and `No status`. Every header but the first has 8 px above it; rows
within a group are 2 px apart. Within a group the rows keep the urgency order
of `order_agents`, the most recent first, the order `Cmd/Ctrl+Shift+J` also
follows. A recognized agent without OSC 7501 capability sits in `Idle` during
its 3 s grace, then in `No status`.

Each row is one recognized agent session of the active space, or of any
space under `All spaces`, on one 32 px
line of the tab row shell (margin 8, padding 7 by 6, radius 9): the runtime
logo at 15 px in the 20 px folder slot, in the runtime's accent (`#D97757` for
Claude Code) or `text`, the tab title at 14 px Medium (the runtime's display
name when the title is empty) followed by ` · ` and the workspace name at
14 px `muted`, then the status at the trailing edge. Neither a meta line nor a
gap between ranks exists any more. A title and workspace wider than the row
end in an ellipsis on one line, shared by the tree's title and label rule, and the
tooltip of a long name gives `title · workspace` in full. The status is the tree's
`Need you`, `Error` or `Done` badge; for `Working`, the matrix in the
runtime's tint with the title shimmering as a tab's does, `muted` under a
white band, both holding still under `reduce_motion` and the title keeping
`text` when `sidebar_show.agent_shimmer` is off; for `Idle`, nothing. A finished row moves from `Done` to
`Idle` once its tab has been seen. No field an OSC 7501 report declares
(`title`, `message`, `app`) reaches a header or a row. A recognized agent that
has declared nothing for 3 s shows a `No status` status badge in `muted`; its
tooltip reads `No OSC 7501 status received. Claude Code reports it from version
2.1.295.` when the catalog knows the version, else `No OSC 7501 status received
from {agent}.` Before 3 s the row shows no status. The row of the active tab of
the shown workspace takes the active tint, every other row the hover tint under
the pointer; a click activates the workspace, the tab and the pane, and writes
nothing to the PTY. With no agent, one 32 px row reads `No agents running` at
13 px `muted`, inset 15.

The section's height is its header, its group headers and rows with their
gaps, and a 4 px bottom margin, capped at 330 px with the list scrolling past
it, headers scrolling with their rows; six agents in two groups fit under the
cap. A 9 px handle straddles its top
edge, absent when the section is collapsed or empty: under the pointer, or
while dragging, it shows the vertical resize cursor and a 1 px rule in `text`
at 0.16, invisible at rest, `text` at 0.35 while it holds keyboard focus.
Dragging sets the height between the section header, one group header and one
row (96 px) and the smallest of 620 px,
the content, and the available height minus 120 px kept for the tree, which
also applies to the automatic height; the header always stays visible.
Double-clicking the handle returns to the content height. From the focused
tree, Tab focuses the handle; up and down step 16 px, Home and End go to the
bounds, Enter returns to the content height, and Escape or Shift-Tab gives
focus back to the tree. The chosen height and the collapsed state persist in
`window-state.json` (`agents_section_height`, `agents_section_collapsed`),
written once a gesture ends; a missing or unreadable value falls back to the
content height, expanded.

The section is a list named `Agents` and each row a list item named by
runtime, title, workspace and state (`Claude Code, Attention sidebar,
paneflow, waiting for input`), selected when it holds the active tab, and a
`No status` row carries its tooltip as its description; each group header is a
level 2 heading named by its label and count (`Need you, 2 agents`), and a row
gives its position within its group. The header button exposes `Agents` and its
expanded state, the handle a horizontal splitter with its value and bounds.
The tree's keyboard cursor runs on past the last tree row into the section,
skips it while it is collapsed or hidden, steps from agent to agent without
ever stopping on a group header, follows an agent that changes group, wears
the same cursor ring (`text` at 0.35), and Enter on a row focuses its pane as
a click does.

Workspace and tab rows can be selected together and closed in one go
(`src-app/src/app/sidebar/selection.rs`). `Cmd/Ctrl`+click toggles a row in
the selection without activating it or folding its workspace. `Shift`+click
replaces the selection with the range from the last toggled row, else from the
active tab, to the clicked row, and from the focused tree `Shift`+Up and
`Shift`+Down extend it from the cursor. A selected workspace covers its tabs,
which show selected with it; toggling one of them trades the workspace for its
other tabs. A range takes a workspace whole only when its row is an end of the
range or every tab row it shows is inside, so crossing a folder row never
selects tabs below the range. A plain click or `Escape` clears the selection,
and a row that leaves the rail leaves it, as does every workspace pick once
the rail is grouped by status.

A selected row takes an `accent` wash at 0.16 over its own tint and a
workspace title turns `text`; the wash is the only mark, the row keeps its
hover action, and its accessible name ends with `, selected`. Selected rows
that touch read as one block: the wash also fills the 2 px between them and
the corners on the shared edge square off, for the row's own tint, hover wash
and cursor ring too. Rows touch inside a workspace and between folded
workspaces, never across the 10 px that separates a workspace from the next,
and with the indent guide on a folder row does not join its narrower tab
rows. The cursor ring stays hidden after the pointer selects a row and
returns with the first key pressed in the tree. The selection adds no surface
of its own: no bar, no counter. A selection click focuses the tree, so
`Delete` or `Backspace` closes the selection, as does the context menu of a
selected row, which holds `Close 2 workspaces and 3 tabs` and `Clear
selection` in place of the row's own menu. A single row closes at once, as its hover action does.
Two or more ask first, in the card of the worktree removal dialog: the title
`Close 2 workspaces and 3 tabs?`, the number of sessions that will stop, up to
eight rows with their kind word, then `Cancel` and a destructive `Close
selection`; Enter confirms and Escape cancels. Unsaved files are then asked
about once for the whole selection, the rows leave with the motion of 4.8, and
one toast names what was closed.

Drop placeholder while dragging: margin 6, radius 8, blue at 0.10 with a 0.22
border and a 2 px line.

The footer stacks, top to bottom: the IPC offline banner when the socket is
disabled (margin 6, padding 8 by 6, radius 6, 1 px `border` on `subtle`, a
14 px alert glyph and `IPC offline` at 12 px Medium), the manual-check failed
banner when the last manual check could not reach the feed (margin 6, height
30, padding 8 leading and 3 trailing, squircle 14 on the active row tint, a
14 px `vc_deleted` alert glyph, `Update check failed` at 12 px Medium, a 24 px
`Retry` button at 12 px Medium on `text` at 0.08 to 0.12, and a
`dismiss_button`, both squircle 7 so they sit concentric in the banner; the
banner itself is not clickable), the star banner in place of both when neither
shows, from the first launch until it is used or dismissed (the geometry of the
manual-check banner filled with the star wash of 4.3, a 14 px `star` glyph in
the star blue and `Star Paneflow on GitHub` at 12 px Medium on `text`, a
tooltip naming the repository, and a `dismiss_button` at squircle 7, the only
part with a hover state; a click opens the repository, and either action
retires it for good through `~/.paneflow/star-prompt.json`), then a 36 px footer with a flexible
fully rounded capsule filter on the left and a 36 px icon-only Settings button on the right. The
filter shows an outlined circle with three descending horizontal lines at rest
and on hover. Only while the input is focused, it shows a solid circle with
three horizontal cutouts, white in dark themes and black in light themes.
It reveals `Filter` and
the hover tint on hover, and keeps the input visible while focused or nonempty.
Focus and a nonempty query take the active row tint; a trailing clear button
resets the query. Filtering matches workspace names, paths, branches, and tab
titles without changing their order or saved expansion state. Matching tabs
highlight matching text in the match blue of 4.3 (`#5aa6ff`, light `#0550ae`) with semibold weight, as do
workspace names and visible checkout labels. Matching ignores letter case.
Matching tabs
are revealed while filtering. Settings retains its 14 px gear and tooltip,
with the active row tint when open and the hover tint otherwise.

When at least one Claude Code or Codex account exists and
`usage.plan_limits` and `sidebar_show.usage` are both on, a usage capsule sits
between the filter and the Settings button (`src-app/src/app/usage/render.rs`).
It is 36 px high at radius 12 with the Settings button's hover and open tints,
and holds the 16 px ring of 4.7 and the most constraining percentage of the
active space's accounts (the default accounts when the space declares none) in
13 px Medium with tabular numerals, 7 px apart. `usage.sidebar_agents` narrows
those accounts, and the bubble's, to the listed agents; with none left the
capsule is not drawn. The value stays at most about five minutes old without
any interaction (ARCHITECTURE.md, "Plan usage and the activity ledger"). Its tooltip, after 800 ms,
reads `Plan usage: {account}, {window} {percent}`, or `Plan usage: no data
yet` over a bare track. Its accessible name starts with the visible text
(`54% plan usage, Claude Code week`) and exposes the expanded state. Tab
reaches it from the filter, and Enter or Space opens the bubble of 5.6. The
filter keeps at least 120 px: below that, the capsule drops its number and
keeps the ring and its accessible name. With no account, or with either
setting off, the capsule is absent and the filter takes its place.

Filter changes animate row opacity and occupied height over 180 ms with a
cubic ease-out. Exiting rows remain until the transition completes, and
surviving rows move as the released space collapses. A new query retargets
from the current visibility. Measured row heights preserve checkout metadata
geometry. Reduced motion applies the filtered list immediately; workspace or
tab structure changes reset the transition to avoid stale rows.

### 5.3 Pane card

A 24 px squircle filled with the terminal background, 1 px `border`. With
`comfortable_density` off, the card loses its own radius and gutter: it fills
its cell of the grid, rounds only the corners on the grid's outer edge, and the
1 px divider between cells stands in for the shared borders. The Classic style
is always in that compact form, with square outer corners, and its header row
takes the title bar height; the top row sits in the title band (4.9). The
header shares the 44 px surface tab row, with no repeated pane name: a 6 px status dot, 9 px
chips for progress and worktree, and on the right 22 px action buttons that
appear only on the focused pane (split vertical, split horizontal),
and a `Z` chip on `accent` when the pane is zoomed.
There is no separate pane close button; closing the last surface closes the pane. The identity
pill was removed in 0.9; the sidebar carries identity, the pane carries
title and state.

The 44 px tab bar uses the detached-window style: 32 px
capsules with a 6 px left inset in docked panes, matching the top inset,
terminal tiles, full titles with ellipsis, and a 16 px circular
close slot with a 12 px glyph shown on hover. Only the active tab has a
resting capsule fill; other tabs gain a background on hover. Pane actions
sit to the right of New tab, separated by a
1 px by 16 px divider. Agent sessions lives in the right panel header and
targets the active pane. The diff dock is a window-level panel, so its toggle
lives in the title bar (5.1), not in the pane header. Diff attribution lives in the surface tab tooltip.
Dragging a surface tab within the layout uses the gray pane drop preview.
A center drop moves the tab into the target pane; an edge drop splits the
target with the same surface entity. Moving the last tab removes the source
pane. Zoomed layouts and detached windows do not accept these tab transfers.
In docked panes, New tab uses the same 22 px button and 14 px icon dimensions
as the actions. New tab, the divider, and the actions hide on unfocused panes
while retaining their layout space so tabs do not shift when focus changes.
The status cluster before them (attention dot, declared status or progress
chip, search hits) stays visible on every pane: it is what an unfocused pane
must still say.
The chips sit in a strip that scrolls horizontally with the wheel,
without a scrollbar; the active chip scrolls into view when it changes.
Where chips are hidden past an edge, a 28 px fade to the card background
signals them. A 26 by 26 `+` chip is pinned at the visible end of the bar,
outside the strip; it opens the new-tab menu and
disappears at the 8 tab cap. That menu carries the same presets as the New
pane palette, Terminal then the visible agents then the workspace custom
buttons, and launches the chosen one in the active tab's directory. It is
216 wide on the shared menu geometry from 5.6, and its right edge aligns with
the chip in both the docked and the detached bar. The `New tab` action keeps
opening a terminal directly. Closing the last tab closes the pane. Diff
panes have no tab bar.

State layers, painted in this order: card fill, content, dim layer, drag
overlay, border. Unfocused panes in a multi-pane workspace fade
under a 0.3 overlay by default. The drop overlay is blue at 0.10 with a 2 px
blue border, radius 8, margin 8, and a swap variant with its own tint.

### 5.4 Diff dock

#### Detached pane window

A detached pane has one 44px caption and surface-tab row, modeled on the supplied
Superlogical reference. Closing the native window returns the pane to its
workspace; there is no separate return button. Tabs are 32px high, up to 208px wide, with ellipsis and
horizontal overflow. Only the active tab has a capsule background and a small
elevation shadow: the `text` tint of the sidebar rail in dark themes, the `card`
fill in light ones so the tab reads raised instead of greyed, with a `text`
0.05 hover on the inactive tabs. Its outline follows the light-theme accent but
stays neutral in dark themes. The caption has no bottom divider. The unified new-tab button
has a 32px square hover surface, 7px horizontal margins, and a 20px icon. This
capsule is a reference-specific exception to the standard squircle row skin.
Terminal tab icons use a fixed dark terminal tile with a green prompt,
independent of the shell theme. The body has
a 7px inset and 10px corners. The original pane header and tab row are hidden.
The native shell uses the existing colors, tooltips, window
controls, and platform decoration policy. macOS reserves space for traffic lights;
Linux respects server decorations and the system button layout.
Windows separates the surface tabs from the right window controls with a centered
1px by 16px vertical divider when those controls are visible.
Detached windows follow the Chrome material setting for their caption and the
frame around the terminal, including changes made while the window is open.
They use the main window's platform backdrop policy, with a separate native
material per macOS window. Terminal material remains a separate setting.
Dark Windows terminal cards tint the native backdrop with the theme background
at 35% opacity, behind the terminal text, to keep wallpaper colors subdued.

The initial window is 800 by 600 logical pixels, with a 420 by 280 minimum.
These compact dimensions are a contextual exception to the main workspace shell.
Restored bounds are clamped to an available display. Detaching or returning preserves
terminal state and reflows docked siblings. A fully detached layout shows controls
to reveal its windows or return its panes. Closing a detached window returns the
pane, while explicit Close Pane retains its normal destructive meaning.

The dock attaches to a tab, 540 px wide by default, and opens on a surface
picker: the title `Open a surface` over a list of Changes, Terminal and File,
each row carrying its icon and the keycap of the digit that opens it. The
picker takes the focus when the dock opens empty, so `1`, `2`, `3`, the arrow
keys with Enter, and Escape work at once, and the focus returns to where it
was when the picker goes away. The dock starts with no content tabs: Changes
is created only when selected in the picker or the `+` menu, which also offers
File and Terminal and, below a separator, `Reopen Closed Tab`. File opens the
single `Files` tab, titled with the 16 px folder-tree icon, whose body is the
file tree at the full width of the dock; opening a file from it replaces the
Files tab with the file's tab. Every content tab is closable, including Changes and the
first tab. File tabs have no cap. A middle click closes a file tab, through the
same two-click arming as its `×` when it is modified. A right click on a tab
opens the tab menu of 5.6: `Copy Path`, `Copy Relative Path` and `Send to
Agent` on a file tab, then `Close`, `Close Others`, `Close to the Right`, `Close All`, and below a
separator `Reopen Closed Tab`, disabled while the dock has closed nothing; a
right click on the empty strip offers `Close All` and `Reopen Closed Tab`.
Batch closes never touch Changes or Terminal and keep modified file tabs,
reported by a `N modified tabs kept` toast. Each dock remembers its 20 most
recent closed files, newest first, and reopens one at its scroll position.
Every dock's Changes, Files and file tabs come back after a restart in the
same order with the same active tab and the tree shown or hidden as it was;
only the active file is read at launch,
the others on their first activation, and a file gone since is dropped
silently. Closing the last tab returns to the picker; switching sessions
preserves the chosen tabs. Tabs are chips with the sidebar rail skin: no
separators, active chip on the active tint, inactive chips wash in on hover.

The Changes tab follows T3 Code. Its toolbar is one 28 px row with no border
above or below: `Uncommitted` and the branch chip on the left; on the right
the total diffstat (hidden under 460 px of width), refresh, collapse or expand
all, the Unified and Split segments, the whitespace toggle, the file tree
toggle and the `…` options menu. The tree lists the changed files at the right
of the diff, single-child folders flattened, with the language icon and a
status letter per file; a click reveals the file in the diff.

A file header is a 32 px row on the code background with no separator band,
and it stays pinned at that height while its file scrolls. From the left: a
16 px chevron in `muted` at 0.8, the 16 px status icon (added in `vc_added`,
deleted in `vc_deleted`, modified and renamed in `vc_modified` at 0.65), then
the full path in Geist 12 in one color, cut from the start with an ellipsis
when it does not fit. A renamed file shows its previous path at 0.7 and an
arrow before the new one. The diffstat is right-aligned in two columns of 4
characters, additions then deletions. The pointer draws a 3 px edge of `text`
at 0.24 on the left of the header, never a full band. An expanded file is
followed by 8 px of background.

Lines are 20 px tall in the code font at 13. The gutter holds the change bar
and one right-aligned line number, `text` at 0.65 on a context line and the
change hue on a changed line; a 2 px gap in the background color separates it
from the code. A changed line washes both its gutter and its code cell with
the change hue at 0.07 over `base`, the effective tint of the reference, and
a theme that sets its own washes keeps them. The change bar is 4 px, solid
for an addition and striped in 1 px dashes for a deletion. Hidden context is a
24 px hunk separator inset 8 px from both edges: a 32 px expand cell and the
`N unmodified lines` label in Geist 11, `text` at 0.5 on `text` at 0.03, with
6 px outer corners; the icon points up before the first hunk of a file, down
after the last, and both ways between two hunks. In Split, the two sides are
2 px apart and a side with no line is hatched in `text` at 0.10.

The words that differ inside a modified block sit on the change hue at 0.20
in dark and 0.15 in light, painted between the line wash and the text at the
x positions of the shaped line, so tabs and CJK glyphs align with the
rectangle. A run that scrolls past the cell edge is clipped to the cell. A
block whose two sides differ only by whitespace is muted: the line wash drops
to 0.035, no word rectangle is painted, and the 4 px change bar sits at 0.5
alpha. Highlight `None` keeps the change bar and the colored line number and
drops every wash. `Highlight` and `Whitespace` live in the Changes options
menu and are session-scoped like Split and Unified. Highlight is
applied at paint time to cached word-level rows: switching Words, Lines, or
None requires no Git work or row rebuild and preserves the scroll position.
Whitespace changes the comparison and rebuilds rows off the render thread;
the previous rows stay on screen until the swap, and a failed dock rebuild
keeps them under an error banner.

A file tab in the dock carries git markers in a 6 px column left of the line
numbers, computed against `HEAD` off the render thread and kept current by a
block tracker that shifts blocks on every keystroke and re-diffs only the
touched blocks after a 150 ms pause. Added and modified blocks paint a 4 px bar
with radius 2 and a 1 px inset in `vc_added` or `vc_modified`; a deleted block
is an 8 px dot in `vc_deleted` centered on the boundary, pulled down to the
first row when the deletion sits at the top. Hovering a marker widens it 3 px
to the left and shows the pointer; clicking opens a `menu_surface` popup
anchored to the block's row, 280 to 520 px wide within the editor, flipping
above the row when there is no room below. The popup names the block
(`Modified lines 12-15`, `Deleted 3 lines after 20`, `Added 4 lines`), shows
the base text of a modified or deleted block in the code font with syntax runs
on the `vc_deleted_background` wash (12 rows visible, scrollable, 200 lines
shown with an `and N more lines` foot), and offers `Copy` and `Revert`; an
added block offers `Revert` alone. Escape, a click outside, or an agent write
closes it. In the Changes tab, hovering a block of a modified file shows a
`Revert` chip on the block's first row, 56 by 16 on the sidebar hover tint,
inset 10 from the right edge; the click writes the base lines back through the
atomic save path and refreshes the dock, and refuses with the error banner when
the file has unsaved changes in a dock tab or changed on disk since the build.

The file surface shares one frame, retaining the 40 px tab bar and its
26 px tabs, original spacing, typography and corner radius. A 40 px
breadcrumb toolbar spans both the editor and the tree. The breadcrumbs are the
project then each folder down to the file, 12 px, 22 px tall segments with a
4 px inset and the sidebar hover tint, separated by 18 px chevron buttons; the
row scrolls horizontally and shows its last segment when a file opens. A
folder segment reveals that folder in the tree: expanded, selected and
centered. A chevron opens a menu of 5.6 listing the folder on its left,
folders first, the open file checked; a folder entry shows its content in
place under a `Back to <name>` entry, a file entry opens the file. The listing
is read off the render thread and states `Loading folder…`,
`This folder is empty.` or `This folder is no longer available.`, capped at
300 entries. The tree starts below that toolbar beside a 1 px left rule that is
also its resize handle, at the width of 3.1. A root that cannot be read shows
`Unable to load folder.` and a `Retry` button. The tree's own toolbar is
40 px tall with a 6 px inset and 4 px gaps, and holds, in this order,
`Refresh files`, the `Search files` field and `Expand all` or `Collapse all`;
the two buttons are 28 px squircles at `ROW_RADIUS` with 14 px muted icons
(`menu/refresh`, `menu/expand-all`, `menu/collapse-all`), reachable from the
keyboard and labelled by their tooltip. While a refresh rereads every loaded
folder its icon turns once per 900 ms; `Expand all` opens every folder git
does not ignore, at most 2,000, and then toasts
`Expanded the first 2,000 folders`. The search field fills the rest of the
row at 28 px height, 8 px corners and a 1 px border. It searches the names of
every file of the project, read off the render thread with `.gitignore`
applied, 120 ms after the last keystroke, up to 256 characters and 200
results: the tree then shows only the matches under their expanded parent
folders, matched characters in `accent` semibold, with
`More matches available. Refine your search.` in 12 px muted under the
toolbar when more exist and `No matching files.` when none do. Escape empties
the field and brings back the folds of before the search. A folder being read
shows the 14 px loader in place of its chevron, still under reduce motion; a
folder whose read failed shows `Unable to load folder.` at the right of its
row in 11 px muted, and a click on the row reads it again. Tree rows are 28 px tall with 13 px labels, 14 px icons, 18 px indentation and
6 px selection corners. These dimensions follow the Codex App reference
provided for this surface and are contextual exceptions to the rail skin.

Tree rows carry Zed's version control decoration, summed from
`git status` over the tree root. The label takes `vc_conflict`, then
`vc_deleted`, then `vc_modified`, then `vc_added` for an addition or an
untracked path, and falls back to `text`. A file also shows a status letter at
the right of the row, 11 px bold in a 14 px slot: `!` for a conflict, `U` for
untracked, then `D` and `M` for the worktree side before the same two for the
index, and `A` for a staged addition. A directory shows a 6 px dot at 0.5
opacity in that slot instead, rolling up every descendant. Entries git
ignores, and on Windows those with the hidden attribute, show at
`DIMMED_OPACITY` 0.55 and still open; a dot file shows at full opacity, and
only `.git` stays out of the tree. An ignored folder is read and watched only
while it is expanded. The tree selects, expands the parents of and centers
the file of the active tab without taking focus, unless that file lies outside
the root or is ignored.

A right click on a row opens a menu of 5.6: `Open with Default App`, the
platform's reveal entry (`Reveal in Finder`, `Reveal in File Explorer`,
`Reveal in File Manager`), `Open With` and the editors found on the machine
(disabled when none is), then `Copy Path` and `Copy Relative Path`, then
below a separator `Send to Agent`. On Linux
the reveal asks `org.freedesktop.FileManager1` to select the file and opens
the parent folder when the service does not answer. A launch that fails
toasts the application and its error.

`Send to Agent` inserts the path in the terminal of an agent of the shown
workspace and focuses it, never submitting: `@path` for Claude Code, Codex and
Gemini, the bare path for the other runtimes, relative to the workspace root
and quoted by the runtime's rule when it holds a space. With one agent the
entry acts at once; with several it opens a submenu in the Agents order, each
row carrying the runtime logo at 14 px, the tab title and the runtime name;
with none the entry is disabled and reads `No agent in this workspace`.

A tree row drags as an `overlay` chip holding its 16 px file icon and its
name, and the drag opens nothing. A terminal that accepts input shows a 2 px
`accent` border at 0.7 and an `accent` wash at 0.08 while the chip is over it;
an ended or disconnected terminal shows no zone and ignores the drop. A drop on
an agent's terminal inserts the `Send to Agent` text, a drop on a shell inserts
the path with that shell's quoting, as a file dropped from the system does.

The tree button sits immediately after Editor Controls in the file toolbar.
It and `secondary-alt-f` toggle the embedded tree; on the Files tab the
shortcut closes that tab.
Opening it selects an existing file tab or creates the Files tab. Closing
the last file or Files tab closes the tree; Changes and Terminal remain
available. Selecting those surfaces temporarily hides the tree and gives them
the full dock width. Sessions can remain open alongside the integrated tree.
Tree visibility remains local to the active agent session.

The file header places Zed's 14 px `filter.svg` icon after `Ln` and `Col`.
Its Editor Controls menu offers Minimap, Scrollbar and Word Wrap toggles,
scoped to the open file tab. The minimap starts hidden; scrollbars start
visible; Word Wrap starts from `editor.word_wrap` (off by default) and a toggle
never writes the setting back. It is an action menu of 5.6 whose rows are
toggles. Escape, an outside click, or a second click on the trigger dismisses
it.

Word Wrap folds each line at the last space or tab that fits the text width,
or mid-word when a word is wider than the row. A wrapped tab has no horizontal
scrollbar. Continuation rows are 18 px like any other row, carry no line
number, and keep the git marker bar spanning every row of its lines. Up and
Down walk visual rows and keep the caret's column inside the row. A width
change keeps the line at the top of the viewport in place.

`secondary-f` opens the find bar in the top right corner of the file tab: a
squircle on `subtle` with a 1 px border that turns `agent_error` on an invalid
regular expression, whose message sits on its own row below. Its first row
holds the replace chevron, the query field, Match Case and Regular Expression
toggles (Aa, .*), the `N of M` count, then Previous, Next and Close. The query
is case-insensitive by default; `:120` jumps to line 120 instead. Every match
gets a translucent warm band and the current one a stronger band, both under
the selection. Enter and Shift+Enter step through matches, Escape closes the
bar and returns focus to the text. The replace row (Replace and Replace All)
is absent on a read-only file; Replace All is one undo step.

Opening a file at a line or a range of lines, `:N` in the find bar for
instance, centers it and washes it with the same warm hue at 0.14. The
wash leaves on the next keystroke, click or scroll.

Resting the pointer on the editor gutter for 200 ms shows the comment button
of T3 Code's diff view: a square as tall as the row (18 px), radius 4, filled
`#009eff`, with a 16 px plus (`icons/comment-plus.svg`, 1.5 px rounded strokes)
in the code background color (`#171717` in dark themes, `white` in light
ones). It sits right after the hovered line's number, from 2 px before the
gutter edge to where the code text starts, so it covers neither the number nor
the code; the pointer keeps it shown across its whole width. The tooltip reads
"Comment on this line (drag for a range)"; the button follows the pointer from
line to line and leaves with it. A click comments that line and a
drag down or up the gutter comments the range in the comment palette below.
A right click on the text opens
a menu of 5.6: `Comment Selection`, disabled without a selection, then `Copy`,
`Paste` and `Select All`. From the keyboard, `secondary-shift-m` comments the
selected lines, or the caret's line without a selection.

The comment reproduces T3 Code's diff comment annotation, measured from its
dark capture, so it uses its own palette rather than the theme accent. Dark
themes: commented lines `#253345` with their gutter `#294665` and their numbers
in `#67a3e6`; the block `#2f3e51` with its gutter `#2e4a6a`; the field
`#364557`; T3 Code's primary blue `#376bf0`; `#a3a3a3` for the placeholder and
`Cancel`, and the hint at 0.7 of it. Light themes: primary `#1b4ed8`, lines at
0.10 of it, gutters at 0.18, the block at 0.06, a white field with a `black` at
0.12 border, `#71717a` for the muted text. In both, the gutter tint stops 2 px
short of the gutter edge, so a seam of code background separates it from the
text and the block.

The block sits under the last line of its range and spans the gutter and the
code area up to the scrollbar or minimap; the lines below move down by its
height and scrolling stays continuous. Its content is padded 12 by 14 (T3 Code
pads 12 by 12; the 2 extra pixels keep the block a whole number of 18 px code
rows, the field's rows plus 4) and holds a field and a footer 6 px apart, in
the interface font. The field is a 1 px box, radius 8, padded 9 on the sides,
3 on top and 9 below, whose text is 14 px on 18 px rows with the placeholder
"Add a comment…"; at rest its border is `white` at 0.11. It
soft-wraps and grows with its text from 3 to 12 rows (68 px at 3, as in T3
Code), then scrolls. While it has focus its border turns primary and a 3 px
inset ring of primary at 0.24 lines it. The 24 px footer carries the send
shortcut hint "⌘/Ctrl Enter to send", worded as T3 Code words it on every
platform, its ⌘ drawn with `icons/command-key.svg` at 9 px because the interface
font has no such glyph, in `LABEL_XS`
on the leading edge, then 24 px buttons 4 apart on the trailing edge, radius 6,
`BODY` medium, padded 8: `Cancel`, with `white` at 0.08 and `text` on hover,
and `Comment`, primary with a white label. While the field is blank the
send button is drawn as T3 Code's group opacity of 0.64 renders it: primary at
0.64 under a label of white at 0.64 over the block. The field takes focus and
is named after the range ("Comment on L12-L15"), a click anywhere in its box
focuses it, Enter adds a line, `secondary-enter` sends, Escape and `Cancel`
close the block and return focus to the text. One comment exists per tab: opening another replaces an empty one
and asks before discarding typed text. The block follows its lines through
edits and closes when all of them are deleted. `Comment` picks the agent
as the tree entry does, with the last agent chosen for the workspace first in
the list, and pastes into its terminal, in one bracketed paste that is never
submitted: the relative path with `:12` or `:12-15`, a blank line, the comment,
a blank line, then the quoted lines in a code fence tagged with the file
extension. The comment is cut at 16,000 characters and the quoted lines at
32,000, each ending with "… truncated …". The block closes once the terminal
took the text; a refusal keeps it open and toasts the reason. The large-file
viewer and the non-text previews have no gutter button.

A file tab picks its viewer by extension, then by its first 8 KB: images,
CSV and TSV tables, Markdown, archives, then text. A file nothing can show
lands on the No preview screen, centered: the 32 px file-type icon, "No preview
for this file" in `TITLE` semibold, a `muted` line with the size and the
reason, then three 26 px bordered buttons on one wrapping row, "Open with
Default App" on `surface`, the reveal label, and "Open With", which opens the
installed editors as an action menu of 5.6 and is disabled when there are none.
A file that vanished shows "File no longer exists." with a Retry button.

Text between 10 MB and 256 MB opens in a read-only viewer under the `overlay`
banner "Large file: read-only, no syntax colors", whose right end reads
"Indexing…" and then "N lines · size". Its 18 px rows reuse the editor's mono
font and gutter; a click selects a line, Shift extends the range, and the copy
refuses more than 16 MB with a toast.

The image viewer has a 32 px toolbar with a 1 px bottom border: the `W × H`
size (or "Loading image…"), then 24 px icon buttons Zoom out, the zoom
percentage in `text`, Zoom in, Fit and Actual size. It opens on Fit, which never
passes 100% and keeps 16 px around the image; the zoom steps by 1.25 between 5%
and 800%, and a larger image pans by wheel or drag.

Markdown and delimited files carry a 28 px eye or code toggle before Editor
Controls, labeled "Show rendered markdown" and "Show markdown source", or
"Show table" and "Show source". The table has a fixed `overlay` header row,
24 px rows separated by 1 px `border` hairlines, a `muted` line-number column,
and columns sized from the first 100 rows between 60 and 400 px. An archive
lists Name, Size and Modified in 26 px rows, folders first, under a breadcrumb
of the archive's own folders and a Back row; Name keeps at least 120 px, so
Size and Modified shrink first in a narrow dock. Truncation and hidden columns
use the same `overlay` banner as the large-text viewer, whose own label
ellipsizes before its status does.

Editor scrollbars follow Zed's 15 px tracks, square thumbs with a 25 px
minimum, and a 1 px left border on the vertical track and thumb. Both axes
support centered track clicks and dragging. Git change markers occupy the
vertical track. Changes diffs keep the same vertical track permanently
enabled, with centered clicks and dragging, without editor controls or a minimap.
The track occupies its own gutter so it never covers diff content.
The minimap uses the bundled `.ZedMono` alias at 2 px, Black,
with a 1.618 line height. Its width is capped at 15% of the text area and 80
miniature columns, and it hides below 20 columns. Its viewport thumb has an
open left border; clicking centers the editor and dragging follows document
progress. Only the miniature rows in view are shaped, with syntax filling
inside the editor's existing frame budget.

### 5.5 Settings

Navigation uses the same Geist font and row scale as the workspace sidebar:
14 px text with 20 px line height, 17 px icons, 32 px minimum row height,
2 px row spacing, 8 px outer margins, and continuous corners at radius 9.
The search field is the sidebar filter field (`filter_field`): a 36 px
capsule with the 20 px filter-circle glyph, 15 px text, the active row tint
while focused or nonempty, the hover tint otherwise, and the same 18 px
trailing clear button. Its input stays visible at rest. The back button
follows the navigation row sizing.

Search covers page content, not only the navigation. Every setting row
carries a `SettingCopy` (title and description) declared in
`src-app/src/settings/search.rs`, which is the single source for the row copy
and for the search index. A query matches a section when it is a
case-insensitive substring of its label, its keywords, a block header, or a
setting title or description. Typing a query that no longer matches the open
section moves the panel to the first matching section in navigation order
without taking focus from the field. On the open page, section headers,
titles and descriptions highlight the match in the match blue of 4.3 semibold, the
same treatment as the workspace filter. Rows whose copy does not match
collapse, along with the hairline between them, and a block (header plus
card) collapses when none of its rows or its header match. Dynamic content
such as agent lists, profiles, worktrees and templates follows its block. The
collapse animates opacity and measured height over 180 ms with a cubic
ease-out, retargets from the current state when the query changes, and
settles instantly under reduced motion. A page reached through a section
change shows its final state directly. When only a navigation keyword matches
the section, the page stays whole.

Navigation reuses the sidebar width: `Back to the app`, a search field, and
three groups labeled Personal, Terminal, Integrations. A navigation row keeps
the geometry of a Workspaces rail row: the 15 px glyph is centered in the 20 px
slot of the rail, its top on `sidebar_glyph_top` (the whole pixel that puts its
center between the cap-height and x-height axes of the 14 px label), then the
8 px title gap, so a glyph stands as far from the leading edge of the row tint
as from its top and bottom (`settings_nav_icon`). Pages are a centered
column built from one kit, `src-app/src/settings/kit.rs`, which every page
uses: the 26 px heading over one 13 px page description in the secondary ink,
both inset 17 px so they line up with the text of the rows, then sections 32 px
apart, each a section head (12 px Medium, an optional hint in the tertiary ink)
8 px above a card of radius 12. A row (`kit::row`) holds an optional 32 px lead
(`disc`, `avatar`, `mark_tile`), a title at 13 Medium over lines at 12 in the
secondary ink, and its control at the trailing edge; rows are split by
`kit::rule`. Switches are 40 by 24 with an 18 px knob inset 3, in the system
blue when on. Buttons are `kit::button_with` in the Primary, Secondary, Ghost
and Danger tones, and icon-only actions are `kit::icon_button`. Selects open a
`select_menu` under the trigger. Destructive actions use the fixed red
button. The Appearance page leads with three theme tiles (System, Light,
Dark, 134 tall, radius 10, 2 px border) holding a mockup painted from the
preset, a live diff sample, then the preset select and the preferences.
The App icon row is folded by default: a select trigger shows the current
icon at 20 px, its name, and a 12 px chevron that turns half a turn as the row
opens. Opening grows the row to its measured height over 200 ms with a cubic
ease-out while the gallery fades in and settles 6 px down; reduced motion opens
it at once. The gallery is a well of `text` at 0.035, radius 14, padding 12,
holding every icon at 44 px in a 76 px cell above its name at 11 px (`text`
Medium when selected, `muted` otherwise), with 14 px between rows. The selected
icon sits in a 2 px `text` ring at 0.85 with radius 14, and the others show
that ring at 0.32 on hover. Picking an icon keeps the gallery open, and leaving
the page folds it.

The Keyboard Shortcuts page follows the Settings grammar of every other page:
sections of cards of rows, each row a name with its control at its end. Under
the 26 px title, one 13 px line in `muted` on a 20 px line says what the page
is for (`The keys for what you do most. Click one to change it.`) and is the
only place the page teaches: while a shortcut records it reads
`Press the new shortcut. Escape cancels, Backspace removes it.` (`Delete` on
macOS), and when the pressed chord belongs to another action it reads
`{keys} is already assigned to {action}. Press it again to reassign.` in
`vc_conflict`, a second press of the same chord taking it. `Restore defaults`
(a `ghost_icon_button` with the undo glyph) sits at the heading's trailing
edge, shown only while a binding differs from its default, and resets every
binding at once. Under the heading, 16 px below its note and 20 px above the first section, the
page's search field is the `filter_field` of the command palette
(`FilterFieldStyle::palette`: a 34 px well of `text` at 0.07, radius 9, 14 px
text) reading `Search by name or keys`, with the clear button and, at its
trailing edge, a 24 px `Find by keys` toggle inset 5 px on its three sides at
radius 4, concentric with the well (9 = 4 + 5), holding the 14 px keyboard of
`keyboard-keys.svg` in `muted`, `accent` over `accent` at 0.15 while armed.
That glyph is the settings keyboard without its cable, redrawn so its ink is
centered in its box like the search glyph's, since the cable pushes the
keyboard body 2.5 units low. The query is
parsed rather than matched as text: each token that reads as a chord
(`ctrl+shift+d`, `Shift+Ctrl+D`, `cmd-k`, `⌘⇧D`, or modifiers alone such as
`ctrl+alt`) is compared with every binding as a keystroke, whatever the order
or spelling of its modifiers, `cmd` reading as the platform key on macOS and as
Ctrl or Super elsewhere; the other words go through the command palette's
matcher over the action, then its keys, then a subsequence of the action, then
its section and action id, and `unassigned` finds the unbound actions. With a
query, sections and their rows are ordered by how well they match (exact chord
before a chord within a larger one, then the palette's ranks), the matched
characters of each action and section name and the matched chord in its
recorder take the semibold `filter_match_color`, the first row carries the
keyboard cursor, and the note reads `Showing {n} of {total} shortcuts. Press
Enter to change the selected one.` As the query changes, the rows and
sections that leave fold to nothing in place and fade while the ones that
arrive unfold from nothing, over 180 ms with a cubic ease-out, the motion of
the sidebar filter and of the Settings search (`TRANSITION_SECONDS`); the
hairline between two rows follows the smaller of them, the gap above a section
follows it, and a new query reverses every row from where it stands. With
Reduce motion the list changes at once. Armed, the toggle empties the field, its
placeholder reads `Press the keys to look up…` and the note
`Press the keys to see what they do. Escape cancels.`; the next chord fills
the field and disarms it. A printable key pressed anywhere on the page starts
a search, `Ctrl+F` (`⌘F` on macOS) selects the field, Escape clears the query
and then leaves the field, and the Settings search in the sidebar, which also
finds each shortcut and opens this page, hands its query to the field; a query
that names the page itself (`keyboard`, `shortcuts`, `hotkey`) hands none.
Each section is its group's name at 11 px in
`muted` on a 16 px line, then, for a context-bound group, the context as a
sentence on a second line at 0.7 of that ink (`While a pane has focus.`), then
8 px, then one `kit::group` card whose 44 px rows are separated by
`kit::rule` from edge to edge; sections are 24 px apart and they never fold.
A row is the action at 12 px Medium in `text`, truncated with its full text in
a tooltip and never narrower than 96 px, and its control 16 px after it: a
24 px recorder button filled with `text` at 0.06 and 0.12 on hover, the
fill of `soft_skin` so it reads on any card surface (pointing hand),
showing the chord as one run of 12 px Medium text, alternate chords separated
by a regular `or` in `muted`, or a muted `None` when unassigned. A
binding that differs from its default puts a `Reset` ghost button at 12 px
Medium in `muted` 8 px before the recorder. Clicking the recorder records:
it reads `Press a shortcut…` over the system blue `#007aff` at 0.12 inside a
1 px ring of that blue, a held conflicting chord shows in `vc_conflict` at
0.14 with its edge at 0.5, and clicking it again or anywhere else cancels.
Up and Down move a keyboard cursor through the visible rows, from the field
as from the page, drawn as the
focus ring of the Usage page around the recorder and scrolled into view with
its section header for the first row; Enter records the row under it, or the
first result of a query. A query
that matches nothing centers a 20 px keyboard glyph over one 12 px muted line
that quotes it, 48 px below the field, with no card; a lone complete chord
that nothing uses reads `Nothing uses {keys}` over `It is free to assign.` at
0.7 of that ink. The list shows no scrollbar, like
every Settings page, and the last section keeps 32 px below it. A wheel
notch glides
instead of jumping: it moves the list's destination by GPUI's 60 px (three
20 px lines), and the list closes on it exponentially with a 60 ms time
constant, so successive notches pile up on the destination and the list
settles within a pixel about 250 ms after the last one. A touchpad or a
touchscreen moves the list with the fingers at once, and the keyboard cursor
glides its row into view the same way. Every other Settings page scrolls
the same way, without a scrollbar or its gutter. With Reduce motion every one
of them lands at once (`src-app/src/widgets/smooth_scroll.rs`).

The Agents page (`src-app/src/settings/tabs/agents/`) follows the same list
and drill-in grammar for the agents found on PATH. The heading is followed by
one 12 px muted intro line and one card of inset rows (squircle 14 in 6 px of
padding): the 24 px logo, the name at 13 Medium over `{binary} {version}`,
its profile count, and `Hidden from the picker` at 11 muted, then
`{version} available` after the 12 px up arrow when an update exists (a
spinner while it runs), the picker toggle, and a chevron. A hidden agent dims
its logo to 0.45 and its name to `muted`. Agents that are not installed sit
under the `Not installed` eyebrow with their count, as one card of 16 px logos
at 0.55 beside their 12 px muted names, wrapped with 14 px gaps, each naming
the binary it looks for in a tooltip; no row, no folding. With no agent on
PATH, a welcome card says `No coding agent on your PATH` at 14 Semibold over
one 12 px muted sentence and the same strip, centered. Profiles whose agent is
not installed, or that cannot be read, are listed under `Other launch presets`, and
their editor keeps the base agent select. `Permissions` closes the page.
An account named by its email shows the address hidden
(`settings/private_text.rs`): a stand-in of the same length, its letters
drawn from a hash of the address and its `@ . - _ +` kept in place, blurred
by 32 copies on a 4 px Vogel disk at 0.08 each, so the real address is never
painted while hidden. A click, Enter or Space shows that one address and
another hides it, unpersisted; the change blurs or sharpens the real text over
180 ms with a cubic ease-out (at once under reduced motion) before the
stand-in takes its place. The usage bubble of 5.6 hides the same addresses
and shares the reveals with this page, keyed by the account folder.

Clicking an agent opens its page under an `Agents › {name}` breadcrumb, and
Escape closes the profile editor or the update method select, then returns to
the list. The page stacks, 24 px apart: an identity card (16 px padding) with
the 28 px logo on a 48 px plate (`text` at 0.06, radius 12), `Version {n}` at
14 Semibold over the update status at 12 (`vc_deleted` on failure) and, while
an update is pending, the open sessions it will not reach, then `Update to
{n}` (inverse fill, up arrow) or a spinner at its trailing edge. Under
`Installation`, one card holds `Update with` (a 220 px select, only when
several methods apply, with its errors and second copy warning), `Update
command`, and `Location`, the last two in the terminal font with a copy
button. Under `Integration`, `Hooks and MCP` carries its toggle. Under
`Profiles`, with a soft `New profile` once one exists, inset rows show each
profile's name over its variables and arguments as chips, and a pencil
appears on hover; with none, one row offers `New profile`. Clicking a row
swaps it for the editor, a squircle 14 of `text` at 0.04 with 12 px padding:
`Name`, `Variables`, `Arguments`, the `Launches {command}` preview or the
error, then `Delete` (a ghost in `vc_deleted` that asks again as the red
`Delete profile`), `Cancel`, and the blue `Save`.

The Spaces page is a list that drills into one page per space. With no space
yet, the heading stands alone over a welcome card: the `Default` plate and the
blue, green, and magenta plates at 36 px, each in a 3 px ring of the card
surface and overlapping by 12 px, then `Keep your contexts apart` at 14
Semibold, one 12 px muted sentence capped at 380 px and centered, and preset
chips (`Work` blue, `Personal` green, `Client` magenta, then `Custom` with the
add glyph). A chip is a squircle 14 of `text` at 0.06, 0.1 on hover, holding a
14 px mark and a 12 Medium label, its mark side 2 px tighter than its text side.
A preset creates that space with its color and opens it; `Custom` creates the
next `Space {n}` and focuses its name. Once a space exists, the heading carries
a `New space` button at its trailing edge, preceded by the limit reason once 9
spaces exist, and one 12 px muted intro line follows. One card lists `Default`,
then every space, as inset rows: squircle 14 inside the 6 px card padding, so
they sit concentric in the 20 radius card, with a 24 px mark, the name at 13
Medium over one muted line (the workspace count, the email or folder of each
account the space sets, then the count of its other variables), and a
chevron. A row
shows `Hooks not added` in `vc_deleted` when Paneflow could not add its hooks
to one of the space's accounts, or the count of environment warnings after the
amber triangle. `Default` has no chevron and does not open.

Clicking a space opens its page, and the heading becomes a breadcrumb at 26
Semibold: `Spaces` in `muted` (`text` on hover, back to the list on click), an
18 px chevron, and the space name truncated. Escape steps back: it closes an
account menu, the environment editor or the delete question first, then
returns to the list. The
page stacks, 24 px apart: an identity card (16 px padding) with the 56 px mark
and, beside it, the name field (32 px tall, 14 Medium, 320 px at most) over the
color marks at 16 px in 24 px circular targets, the first one aligned with the
field edge: an outline for no color, then one plate per color, fixed across
themes. `Remove image` sits at the trailing end of the color row while an
image is set. The mark wears a 22 px badge at its bottom right (card surface
ring of 2 px, `text` at 0.14 inside, an 11 px add or pencil glyph), and hovering
it lays a black scrim at 0.5 with a white 18 px pencil over it. Clicking it
opens the system file picker; the chosen image is decoded off the render
thread, oriented, cropped to its centered square, scaled to 96 px, and stored as
a PNG in the home's `avatars/` directory, which the mark then draws in a circle
with the color as its loading and fallback state. Under the `Accounts`
eyebrow, one card holds a row per account agent that is installed or that the
space already sets (Claude Code, Codex): the 18 px logo, the name at 12 Medium
over a state line, then card buttons and a 220 px select at the trailing edge.
The state line names the default account (`Your default account, {email}`),
the chosen account's plan and meters, `Checking {folder}…`, or, in
`vc_deleted`, `No account is signed in to {folder}` beside `Sign in`. While a
sign-in runs it reads `Finish signing in in your browser` beside `Open the
page` and `Cancel`, and Claude's code field opens under the row at the text
column. A missing or failed hooks install adds a second line with `Add hooks`
or `Retry`. The select lists `Default` with the default account's email, each
signed-in account (its email, then its plan or state in `muted`), the space's
folder when no account matches it, and `Add account…` behind an add glyph;
the agent's default folder is never offered. Under the `Environment`
eyebrow, with a soft `Edit` at its trailing edge, a card lists the variables
other than the account keys in the terminal font at 11 px, name in `text` on a
180 px column and value in `muted`, with a secret value masked and an empty one
reading `Turned off` for a credential, `Empty` otherwise; with none, one row
says so and offers `Add variable`. Editing swaps the list for one plain intro
line, `Name` and `Value` column labels at 11 Medium `muted`, and the fields,
with `AWS_PROFILE` and `work` as example placeholders, `Add variable`,
`Cancel`, and the blue `Save`; the first field takes focus. Warnings follow the
card. A credential that Paneflow's environment sets warns only when the space
sets that tool's account, and carries `Turn off {KEY} here`, which adds the key
with an empty value. The last card holds `Delete
space` over the fate of its workspaces and a `Delete` ghost in `vc_deleted`;
the confirmation asks `Delete {name}?` beside `Cancel` and the red `Delete
space`. Buttons inside a card are squircle 14 of `text` at 0.06, 0.1 on hover,
because `subtle` vanishes on the dark card. Errors read in `vc_deleted` and
warnings in `text` after an amber glyph, because the fixed red and
`vc_modified` fall under 4.5:1 as text on the light card.

The Workspaces page follows the same list and drill-in grammar for workspace
templates (`src-app/src/settings/tabs/workspaces/`). With no template, a
welcome card shows the artifact itself: a 184 by 112 plate (`text` at 0.04,
radius 13, 5 px inset) holding a main tile with the first visible agent's logo
at 24 px beside a command tile and a shell tile (`text` at 0.07, radius 8,
16 px glyphs), then `Open your whole setup in one click` at 14 Semibold, one
12 px muted sentence capped at 380 px, and three preset chips in the Spaces
chip skin: `Agent, server, shell` (Main left: the agent, `bun dev`, a shell),
`Two agents` (side by side, the first two visible agents), and `Blank` (one
shell, add glyph). Each chip's mark is a 16 by 12 miniature of its layout.
Once templates exist, the heading carries `New template` (inverse fill), which
creates a blank template, opens it, and selects its name, then one 12 px muted
intro line and one card of inset rows (squircle 14 in 6 px of padding): a 40 by
28 layout miniature (`text` at 0.06, radius 7, cells at 0.24), the name at 13
Medium over `{folder with ~} · {n} panes` at 11 muted, the 14 px logos of its
agents (three at most), and a chevron.

A template page has the 26 px breadcrumb of the Spaces page with `Launch`
(inverse fill, play glyph, disabled without panes) at its trailing edge.
`Launch` opens the project folder as a workspace when it is not open yet (the
folder is probed off the render thread), runs the template there, and closes
Settings on it. The page stacks, 24 px apart: an identity card (16 px padding)
with the 32 px name field over a ghost folder button showing the path with
`~`; the `Layout` eyebrow, whose trailing edge holds four full-radius 28 px
pills, each an 18 by 12 miniature of a preset with its name as the tooltip,
and a soft `Add pane`; a card whose 6 px inset holds the layout canvas, 196 px
tall (264 from five panes, and never less than 52 px per stacked row), drawn
from the template's own layout tree. Each pane is a tile (squircle 14,
concentric with the card, `text` at 0.045, 0.075 on hover, gaps of 6). A tile
84 px tall or more stands upright: its agent logo or command or shell glyph at
20 px over its name at 12 Medium and what runs in it at 11 muted, centered. A
shorter tile, such as a row of a stacked layout, lies flat: an 18 px glyph,
then the name over what runs in it, left-aligned, 14 px from the edge. The
second line is dropped when it repeats the name. Clicking a tile selects it:
`text` at 0.08 inside a 1.5 px ring of `text` at 0.4. Under the
`Pane · {name}` eyebrow, with a `Remove pane` ghost in `vc_deleted`, one card edits the selected pane: `Runs` as an `Agent` |
`Command` | `Shell` pill group, then the agent select (logo, name) and
`Prompt`, or `Command`, then `Name` and `Folder`. The last card holds
`Duplicate template` and `Delete template`, whose confirmation reads
`Delete {name}?` beside `Cancel` and the red `Delete template`. Nothing on the
page has a Save button: a text field commits on Enter or when the pointer goes
down anywhere else while it has focus, and every pill, tile, and select writes
at once. An invalid value keeps the previous one on disk and explains itself
in `vc_deleted` under its card. Escape closes the agent select, then the
delete question, then returns to the list.

A space plate (`space_avatar`) follows the abstract plates of paneflow.dev:
a duotone of one dark mineral ground and one luminous ink, mixed in OKLab,
lit by a long-exposure streak. A main band 0.17 wide runs along the plate's
angle with a thin echo below it and a wide halo around it; value noise
stretched along the band smears it and breaks it, and a grain of 0.022 in
lightness settles over the whole. The pairs are Default bone `#ddd5c8` on
`#1d1a22`, Red oxide `#e07a52` on `#22101c`, Green moss `#a8c98f` on
`#142e25`, Yellow ochre `#e6a85a` on `#2a180b`, Blue slate `#8fa6f0` on
`#121c40`, Magenta plum `#d9a0e8` on `#221030`, and Cyan ice `#9fd6e0` on
`#0f2633`. Each plate is drawn at twice the size it shows, at least 16 and at
most 256 px, with 2 by 2 subsamples per pixel and only the noise octaves that
size resolves, then cached per color and size, because GPUI scales an image
without mipmaps and a large plate shrunk to 16 px aliases into hard stripes.
While a plate loads, the mark shows its ink mixed 45 percent into its ground.

The Usage page (`src-app/src/app/usage/page.rs`), under Integrations between
Agents and Plugins with the 15 px `chart-bar-line` glyph, has the kit heading
with `Updated {relative time}` at 12 px in the tertiary ink and a `Refresh`
icon button at its trailing edge, which reprobes the accounts and reloads the
ledger, over the page description. Under the `Plan limits` eyebrow,
a two-column grid of tiles (kit cards, one column under 560 px of
page width) shows each account: the 76 px concentric rings, the agent name in
13 px Medium, the account label, a legend of swatch, window, percentage, and
reset, and the pace line. A tile without data shows the state text of the
bubble instead of a gauge. When accounts of both agents exist, or the only
agent with an account is hidden from the sidebar, a `Sidebar` eyebrow follows
with one kit card: a row per agent
(the agent's 32 px mark tile, `{Agent} in the sidebar`, one line, the kit switch)
that writes `usage.sidebar_agents`. The tiles and the registers always show
every account, whatever the switches say. Tab reaches the switches after
`Refresh`, Enter or Space flips the focused one, and focus draws a 1.5 px
`text` capsule ring at 0.35, 3 px outside the switch. Under the
`Activity by agent` eyebrow, whose
trailing edge holds a `Turns` | `Tokens` | `Active time` segmented control of
full-radius pills (Turns by default, not persisted), one card stacks a register
per account with activity in the last 53 weeks: Claude Code first, then Codex,
then the rest alphabetically. Each register has a header row (14 px logo,
agent name at 13 px Medium, account label at 11 px muted, total on the
trailing edge) above a 53 by 7 grid of the 8 px cells of 4.4, Monday first,
months above at 10.5 px muted and Mon, Wed, Fri on the left. One scale serves
every register: levels 1 to 4 are the quartiles of the nonzero values of the
shown metric, colored with the ramp of 4.3. The strip scrolls horizontally
with the tab strip fades and opens on today. A detail line under the
registers names the hovered or focused day for each agent, beside `One scale
for all agents`. Each register is a keyboard grid: Tab enters on today, the
arrows move by day or week, Home and End reach the ends of the row, and each
cell is named `{Tuesday, August 18, 2026}: {41 turns}` or `no activity`.
Below the card, one 12 px row in the tertiary ink, inset 17, reads `Claude Code keeps transcripts
for 30 days by default.`, then the import state (`Importing past activity…
{n} files` while the worker reads, `Imported the last {n} days` after an
import cut short by its limits) and an `Import past activity` icon button
with the download glyph, hidden while an import runs. With no account,
a single card says so over a link row to Settings › Agents; with no activity
anywhere, the register card is one row behind the chart disc.

The General page holds `Defaults` (the editor, shell and on-quit selects as
kit rows) and `Notifications` (the native notifications switch). The Plugins
page holds the `MCP bridge` row, whose button reads `Install MCP bridge`,
`Repair`, or `Reinstall` (Secondary once every agent has it) and spins while it
runs, a failure row in `vc_deleted` under it, then an `Agents` card with one
row per detected agent and its state at the trailing edge.

### 5.6 Menus, selects, tooltips

Every menu in the app, context menus, title bar menus, sidebar and dock
menus and their submenus, is an action menu (`ui_primitives/action_menu.rs`),
and select popups share its skin through `select_menu`, `select_option`,
`menu_panel`, `floating_menu_panel`, `menu_row` and `menu_divider` in
`settings/components.rs`. The geometry is the same in every interface style:
radius 8 with circular corners on the menu surface fill
(`select_menu_surface`, or a host fill such as the Agents rows menu), a 1 px
`text` edge at 0.10, one shadow (0 18 44 -18 black at 0.80 in dark, 0 16 40
-18 at 0.55 in light), 4 px of padding, and 28 px rows at radius 4 with an
8 px inset and an 8 px gap, so 4 plus 4 keeps a row concentric in the menu; a
unit test in `settings/components.rs` fails if the constants stop agreeing.
Menus are monochrome. A row reads glyph, label, then its trailing marks: a
16 px glyph in `muted` at 0.8 (a Hugeicons stroke glyph from `icons/menu/`,
or an editor or agent logo drawn as a mask in the same tint, never its brand
colors; a space keeps its own mark), the label at 14 px Regular in Geist, a muted value
(`detail`), the mnemonic or a global shortcut at 12 px Medium in `muted`,
the 16 px submenu chevron, and a 14 px check on a toggled or selected item.
Hover and the keyboard highlight share one `text` wash at 0.06, and a
pointer move clears the keyboard highlight so only one row is lit. Dividers
are `text` at 0.08, inset 8 px, with 4 px above and below; group headings are
12 px Medium in `muted` on a 28 px row. Disabled rows sit at 0.64 opacity.
Destructive rows (closing a workspace, a tab or a pane, removing a worktree)
keep the regular colors and carry no mnemonic, so a stray key cannot run
them. An action menu fits its width to
the widest row, measured in Geist, between 160 and 320; select popups run
from 200 to 280; every list scrolls past 400 px.
An open action menu takes keyboard focus and gives it back when it closes:
a row's mnemonic letter runs it, Up, Down, Home and End move the highlight
across enabled rows and wrap, Enter or Space runs the highlighted row, Right
or Enter opens a submenu and Left or Escape closes it, and Escape or a press
outside closes the menu. Toggles keep the menu open. A submenu opens on hover
or click 4 px beside its parent, on the side that fits in the window, with
its first row level with the row that opened it.
Panels that are not menus, the command palette, the pane palette list, the
theme picker, the clone dialog and the Welcome rows, keep the 28 px
`select_item` row. Tooltips are squircle 14 on the title
bar color, padding 8 by 6, small text, shown after 800 ms through
`delayed_tooltip`.

The usage bubble opens from the sidebar capsule of 5.2: a 284 px
`menu_surface` whose bottom edge sits 52 px above the sidebar's, revealed with
`menu_reveal`.
Escape, an outside click, or a second click closes it and returns focus to
the capsule. Inside the surface it speaks the Settings › Agents type and inks
of the kit (`settings/kit.rs`) at menu density, without a card: every line
starts on one 10 px content inset. Its header pairs the `Plan usage` title
(13 Medium) with `Updated {relative time}` in a 12 px tertiary `kit::line`,
taken from the oldest snapshot shown. One row per account follows, padded 8 px
vertically, the active space's accounts first, split by `kit::rule`: a 24 px
`kit::avatar_of` seeded by the account title, 8 px before a 13 Medium title
top-aligned with it (the email when one is known, hidden as in
Settings › Agents (5.5), otherwise the account label, truncated with a
tooltip), the `In use` `kit::badge` on an account the active space or the
defaults use, a 12 px tertiary line `{agent}` or `{agent} · {plan}`, then 4 px
lower the account's windows, 4 px apart, each a `kit::meter` (52 px label,
88 px bar, 34 px percentage) with the reset time in its tooltip. A week window
whose projection needs attention adds one 12 px line: the 12 px warning glyph
in the kit warning ink and `At this pace you reach the weekly limit
{Mon 12:00}, {2 d 21 h} before it resets.` when it overshoots, or `Weekly
limit reached. It resets {Thu 09:00}.` with the glyph in the danger ink once
the week is used up. An on-track week adds nothing here; its `On track` line
shows only in Settings › Usage. An account without limits shows one tertiary
sentence that says why and names the fix (no snapshot yet, API key billing,
no limits reported, reset since the snapshot). A `kit::rule` with 4 px above
and below separates the accounts from the last row, a 28 px menu row
`Open usage settings` at 13 px with a 12 px tertiary chevron and the kit hover
tint, that opens Settings › Usage. The list scrolls past 420 px with the header
and the last row fixed. Up and Down move a 1.5 px `text` ring at 0.35 across
the accounts and that row; Enter or Space on an account with a hidden email
shows or hides it, and on the last row opens Settings › Usage. The hover and
the ring are concentric with the bubble: their radius is the bubble's resolved
radius minus its padding (14 - 4 = 10), never the menu row radius.

### 5.7 Palettes

The pane palette fills an empty tab named
`New pane` with a centered 260 px column: a 13 px Semibold title, an
optional branch row, one 34 px row per preset with its 14 px agent mark and
a `not installed` marker at 10 px where the binary is missing, and an inline
error in `vc_deleted` at 11 px. The branch row's menu opens with a
`New branch…` item above a hairline; picking it retitles the column `New
branch` with a 20 px back chevron at its left edge and replaces the row with
an unlabeled form: a 28 px framed input (6.3) with an 11 px branch
mark and the `Branch name (optional)` input, under it a 28 px quiet row
`from <branch>` with a chevron (filled at `text` 0.05 on hover or while its
select of the repository's branches is open, defaulting to the project's
checkout), a `Worktree` row of the same shape with a 26 by 16 toggle and a
10 px muted caption (`separate folder`, or `switch this checkout` when off,
persisted as `worktrees.for_new_branches`), under it the 10 px muted
destination path of the worktree to be created while the toggle is on
(elided from the left of the row when it overflows), a hairline, and the eyebrow
`Open with` over the presets. Escape or the chevron folds the form back; a preset
creates the branch and its worktree, then opens there. An empty name starts
a detached checkout at the base. The `Create branch
here…` prompt that names it is a 420 wide card on the overlay surface, radius
10: a title, one line of context, the name field as a framed input, a `Create branch` button
filled with `accent` at 0.15, and the hint `Enter creates · Esc cancels`. The
`Clone repository` modal is the menu surface of 5.6, 544 wide, docked 96 from
the top of the window over a 0.4 black scrim. The command palette docks at the
same height on its own panel: the menu surface at squircle 13 with padding 6,
a fixed 560 wide so it never resizes while navigating, then a 34 px search well (the 0.07 `text`
tint on squircle 9, no border, a 14 px glyph and 14 px text; a scope shows as
a 24 px chip inset 5 on every side), then 29 px rows 2 px apart in a
`uniform_list` capped at 400. A row is a squircle 8 pill in the selection blue
when selected, an optional 14 px glyph, the label at 14 px, the current value
or binding at 13 px muted, and a chevron when it opens a scope or a category.
With an empty query the palette opens on its categories (Panes, Tabs,
Workspaces, Sidebar, Terminal, Markdown, Agents, Appearance, Paneflow), each a
row with its 14 px glyph and a chevron, hidden when none of its commands
applies. Above them sit the contextual commands that apply right now (a
pending agent write, a conversation notice, an open search). A category opens
like a scope, with its title as the chip, and its query filters only that
category; typing at the top level searches every command. Escape or Backspace
on an empty query steps back one level. Typing filters
on whole words, arrows move, Enter dispatches, and the palette never lists
itself. The path picker reuses that panel, 420 wide, anchored to the terminal
cursor instead of docked: it opens under the cursor, or above it when the
space below cannot hold ten rows, and then puts the field last and the best
match nearest the field. A row carries a 14 px `folder.svg` or file-type icon,
the path at 14 px truncated in the middle, and a 13 px muted `clock.svg` on a
recently inserted path. With nothing typed it lists the recent paths under the
pane's working directory, then that folder, folders first. A query without a
separator matches fuzzily, file names before folders, across at most 100,000
entries of the working directory walked with its ignore files; a query with a
separator or a leading `~` lists that folder instead, with dotfiles only for a
fragment that starts with a dot. Tab completes the selected row into the
field, Enter inserts it at the cursor through the paste path, relative to the
working directory or absolute outside it and quoted for the pane's shell, and
Escape closes. The clone modal is a quick pick: a field reading `Provide repository URL or pick
a repository source.` over the hairline, then rows on the `select_item` skin
with a 14 px glyph, the label, and a group name at 11 px muted on the trailing
edge. With nothing typed the single row is `Clone from GitHub` in the `remote
sources` group; a typed URL becomes a `Clone <url>` row and Enter clones it.
Picking GitHub swaps the field for `Repository name (type to search)` and
lists the repositories `gh repo list` returns, one row per `owner/name`,
filtered on whole words; Escape steps back to the sources, then closes.
An error replaces the rows with one 12 px red line. A running clone replaces
them with a progress block: `Cloning <name>` at 12 px with the percentage at
11 px muted on the trailing edge, a 4 px full-radius track in the 0.10 `text`
tint filled in `#3fa266` from the left, and an 11 px muted line naming git's
phase and throughput. Before git reports anything a 30% `#3fa266` segment
sweeps the track on a 1.4 s ease-in-out loop. The percentage weights git's
phases: enumerating and counting 0 to 5, compressing 5 to 10, receiving
objects 10 to 80, resolving deltas 80 to 95, updating files 95 to 100. Enter
asks for a destination folder and clones into it; Escape is ignored while a
clone runs. A target on GitHub, whether a `github.com` URL, an `owner/name`
shorthand, or a row of the list, clones through `gh repo clone`, which
carries gh's credentials for private repositories and adds an `upstream`
remote on a fork; when gh is missing or signed out the clone falls back to
`git clone`, expanding the shorthand to `https://github.com/owner/name.git`.
Every other target clones with git.

### 5.8 Feedback

Toasts stack bottom right on a squircle 20 of `subtle` with 12.5 px text and
26 px squircle 14 action buttons at 12 px Medium on `text` at 0.08 to 0.12.
A success toast carries no icon; error messages are detected
and get the 15 px error icon. The release toast is an ordinary toast: the line
`Updated to Paneflow x.y.z` and a `View release notes` action that opens
`paneflow.dev/docs/changelog/<tag>` and dismisses it. It lands 1500 ms after
boot, once the window is painted, and holds four times the default. Callouts (`widgets/callout.rs`) are 16
px icon, 14
Semibold title, 13 muted description, with the fixed warning hue. Empty states (`panel_empty_state`) center an 18 px
muted glyph, an optional 14 Semibold title, and a 12 px muted message; the
glyph spins while scanning.

### 5.9 Welcome and empty states

Paneflow opens on this screen whenever it has no workspace to show: a first
run, or a session that restored nothing. It never fabricates a workspace from
the directory it was launched in.

With no workspace open, the main panel holds one pane card, the 20 px squircle
of 5.3 filled with the terminal background inside the 8 px grid gutter, so the
work surface never flattens into the rail. It centers a 460 wide column: a 44 px app
icon beside a 16 px Semibold headline and an 11 px muted line, `Welcome back
to Paneflow` when the machine has recent workspaces and `Welcome to
Paneflow` otherwise, then `Get started`, then either `Recent workspaces` or
`Configure`, then one 11 px muted line naming the agent CLIs found on the
machine. A section is an 11 px Semibold muted eyebrow followed by a hairline
that fills the rest of the row, then 28 px rows on the `select_item` skin,
each a 14 px muted glyph, the label, and the binding at 11 px muted on the
trailing edge. Recent workspaces cap at five rows and answer `secondary-1` to
`secondary-5`, which are free precisely because no workspace is open; the
list lives in `~/.paneflow/recents.json`, is capped at eight, and drops
folders that no longer exist.

The Workspaces rail states the same choice in 192 px: an 11 px muted caption,
an `Open folder` row with its binding, an `or` at 10 px between two
hairlines, and a `Clone repository` row.

A workspace with no pane shows the pane palette of 5.7 in place of its grid,
attached to the tab the workspace already owns, so a new or emptied workspace
is never an inert panel. That palette cannot be dismissed while it is the
workspace's only surface.

## 6. Interaction

### 6.1 Keyboard first

`secondary` maps to Cmd on macOS and Ctrl elsewhere. Every overlay has a
binding, every binding is remappable in Settings, Keyboard Shortcuts, and
every modal answers Enter and Escape.

| Surface | Default |
| --- | --- |
| Split horizontal, vertical | `secondary-shift-d`, `secondary-shift-e` |
| Focus across the grid | `alt-arrow` |
| New tab, next, previous | `secondary-shift-t`, `secondary-]`, `secondary-[` |
| Workspaces 1 to 9 | `secondary-1` to `secondary-9` |
| Focus the Workspaces sidebar | `secondary-alt-s`; then up, down, home, end move, on into the Agents section rows; enter opens, or focuses an agent's pane; space, left, right fold; F2 renames a tab; delete closes it; alt-up and alt-down reorder; tab focuses the Agents resize handle; escape returns to the pane. Grouped by status, the cursor walks the tab rows in group order and skips the headers; fold and reorder keys do nothing |
| File tree in the dock | `secondary-alt-f` |
| Jump to next waiting agent | `secondary-shift-j`: agents waiting for input, then errored, then finished and not yet seen, in the Agents section order, active space first |
| Layout presets | `secondary-alt-1` to `secondary-alt-4` |
| Command palette | `secondary-shift-p` |
| Open a project file | `secondary-p`, outside a terminal and a text field |
| Insert a path at the terminal cursor | `secondary-shift-i` |
| Settings | `secondary-,` |

### 6.2 Pointer

The shell cursor is Arrow. `PointingHand` appears only on rows and buttons
that act; text fields show the text cursor; dividers show column or row
resize. Hover reveals destructive or secondary controls (the pane close
button, sidebar row actions) rather than showing them at rest; the primary
pane actions stay visible. Drag and drop is available for panes into
panes, panes into the sidebar, tabs between workspaces, workspaces to
reorder the rail, and sessions into panes (`PaneDrag`, `TabDrag`,
`WorkspaceDrag`, `SessionDrag`); every target draws the blue placeholder
from 4.3, and the drag
ghost is a 6 px chip with 13 px Medium text, a 12 px icon, and the one
allowed large shadow.

### 6.3 Focus and attention

Shared text inputs use a `text` colored insertion caret, 1 px wide and
vertically centered. Its height is the font size plus 2 px, capped at the
line height, so it does not span the full input row.

Form fields use the framed input of `settings::components::input_frame`: a
squircle of radius 8 filled with `text` at 0.03 and outlined at 1 px in
`text` at 0.14. Focus fades in an `accent` outline with a 3 px outer ring of
`accent` at 0.24, the ring growing from 0 over 160 ms with a cubic ease-out
(instant under reduced motion); a rejected value swaps both for
`agent_error` at 0.64 and 0.2. Search and filter bars, palettes, and inline
renames keep their quieter fields. A clickable field that is not editable,
such as a folder picker, takes the same surface without the ring.

In Settings, Tab and Shift+Tab walk every control in reading order: the
shared buttons, switches and selects (`KeyboardStopExt` in
`settings::components`), the text fields, and each page's own controls such
as the Spaces rows, color marks and breadcrumb. The Usage page keeps its own
cursor, and a shortcut being recorded keeps the keys. Keyboard focus draws a
2 px inset `accent` ring at the control's radius; a control with an opaque
fill (raised and solid buttons, the space image) wears the field ring instead,
1 px of `accent` with 4 px of `accent` at 0.24 outside. The ring is inset by
default because GPUI paints an outer shadow under the whole element, which
floods a transparent control. Pointer focus draws nothing. Enter and Space
press the focused control.

Focus is shown by the pane border and the title bar, not by dimming: every
pane keeps full contrast by default, and `unfocused_pane_opacity` below 1.0
opts back into fading the siblings. An agent that needs the user gets the
`vc_conflict` border at 0.7 and a sidebar dot; clicking anywhere in the panel
acknowledges visible completions. `secondary-shift-j` jumps through those
panes. Native OS notifications fire only
while the window is unfocused.

## 7. Platform Materials

| Platform | Backdrop | Sidebar | Terminal | Chrome |
| --- | --- | --- | --- | --- |
| Windows 11 build 22621 and later | Mica by default (`window_backdrop: auto`), transparent by choice; `blurred` and `acrylic` in `paneflow.json` resolve to `auto`, so blur needs `PANEFLOW_WINDOW_BACKDROP=blurred` for one launch | Reveals the backdrop when `windows_chrome_material` is on; with `windows_material_veil` (the Blended style) the backdrop switches to the Acrylic system backdrop (`DWMSBT_TRANSIENTWINDOW`, a blur of what lies behind the window, not GPUI's `Blurred` accent, which lags on drag) and the sidebar reveals the Linux theme veil over it (0.72 dark, 0.80 light) | Transparent default background when `windows_terminal_material` is on, masked to the panel; the panel inset gutters are repainted opaque only while `windows_chrome_material` is off; with `windows_material_veil` the terminal card takes the Linux 0.35 shade over the veil | Native caption glyphs in light and dark |
| Windows 10 and older 11 | Opaque | Opaque card | Opaque | Same glyphs |
| macOS | Transparent window surface; under Blended (`background_style`), GPUI's untinted `Blurred` view behind the whole window, dropped to Themed in fullscreen and while the system setting Reduce transparency is on | The theme veil over the blur (0.72 dark, 0.80 light) | The 0.35 shade over the veil under Blended, opaque under Themed | Traffic lights with 80 px brand padding |
| Linux | Opaque shell by default. `linux_chrome_material` or `linux_terminal_material` makes the surface transparent and paints the shell as a theme veil (0.72 on dark themes, 0.80 on light ones, since a light veil lets a mid-tone wallpaper grey it); Paneflow requests blur only when `window_backdrop` is `blurred`, one of the two materials makes the surface transparent, and the Wayland compositor offers ext-background-effect-v1 or KDE blur, and logs once when it falls back; otherwise any blur comes from the compositor, for instance Blur my Shell on GNOME | Opaque, tinted from the title bar color; reveals the veil when `linux_chrome_material` is on | Opaque; with `linux_terminal_material` the terminal card adds a 0.35 shade over the veil, black on dark themes and white on light ones, so the terminal reads denser than the sidebar | Client-side decorations with GPUI's generic glyphs, or server-side when `window_decorations: server` |

The right dock follows the Terminal column: while the terminal material is
on, its card takes the terminal card color (`pane_card_background`), and the
diff rows, the editor, and the file tree paint no `base` fill
(`RowPalette::on_material`), so the material shows through. File headers,
sticky headers, change washes, and hunk bars keep their colors.

Rules that follow:

1. A surface MUST hold with the material off. Never rely on transparency for
   contrast or separation; the corner masks and the surface ramp carry the
   card reading on their own.
2. Anything drawn over the material MUST be `transparent_black` where the
   material should show and the opaque shell color where it should not. The
   helpers in `constants.rs` decide per platform; do not branch on
   `target_os` in render code for color.
3. Windows-gated code is linted only by the Windows job. Read `AGENTS.md`
   before adding a `#[cfg(windows)]` item to a file that has a `mod tests`.

## 8. What Not To Do

These are the Paneflow-specific bans, in addition to the generic ones a
design review would raise anywhere.

- Drop shadows on chrome, cards, rows, menus, or toasts outside the contextual
  menus and select popups of 5.6, the usage bubble of 4.9, and the
  floating surfaces of the Classic style in 4.9. The window, the drag ghost, and the dialog card (`modal_card`) retain
  their shadows.
- Separators between tabs, chips, or toolbar buttons. The floating chip
  language replaced full-height bordered tabs in 0.5.5.
- Identity pills, badges, or logos in the pane header. The sidebar owns
  identity.
- Accent fills on anything larger than a button, or at more than 0.15. A
  button tinted with `accent` at 0.15, like the branch prompt's `Create
  branch`, is the ceiling.
- Hue in a neutral. If a gray reads warm or cool, it is a bug.
- A new radius, a new text size, or a new hover color. Pick from sections
  4.4, 4.6, and 4.3.
- A style branch at a call site to pick a radius. Pass the design radius of
  4.4 and let the primitives of 4.9 resolve it.
- A per-component light or dark branch when a `UiColors` role models the
  state. Lightness checks belong in `constants.rs` and `components.rs`.
- Motion that does not explain a state change, and any animation that keeps
  running under `reduce_motion`.
- A tooltip without the 800 ms delay, or a control without a tooltip when
  its glyph is not self-evident.
- Blocking the render thread for a visual. Snapshots, git, and file walks go
  through `smol::unblock`; see `ARCHITECTURE.md`.

## 9. Delivery Gate

Before a UI change is ready for review, confirm on a real build:

1. Paneflow Dark and Paneflow Light, plus one of Vercel, Claude, or Cursor,
   both variants.
2. The native material on and off on the platform you are on, and a stated
   inspection of the other two platforms if you cannot run them.
3. `reduce_motion` on: nothing still moves except live status.
4. The 800 by 500 minimum window, a hidden primary sidebar, and a right rail
   or the diff dock open at the same time.
5. Long titles, long paths, and missing optional data: truncation follows
   4.6 and rows never wrap.
6. Hover, active, selected, disabled, loading, empty, error, and attention
   states of every control you touched.
7. A capture of the surface in the pull request, and a line listing which
   of the eight variants and which platforms you actually ran.

Shared primitives to reach for first, in `src-app/src/ui_primitives.rs`:
`AnimatedHoverExt`, `squircle_skin`, `icon_button_sm`, `icon_button_md`,
`toolbar_pill`, `filter_pill`, `filter_field`, `highlight_matches`,
`section_eyebrow`, `panel_empty_state`, `text_tooltip`, `delayed_tooltip`.
In `src-app/src/settings/search.rs`: `SettingCopy`, `SearchCard`, `Block`. In `src-app/src/settings/kit.rs`, for every Settings page:
`Ink`, `section`, `group`, `row`, `link_row` with `Place`, `rule`, `switch`,
`button_with`, `icon_button`, `search_group`. In
`src-app/src/settings/components.rs`: `select_trigger`, `select_menu`,
`select_item`, `menu_surface`. Add a primitive only when two surfaces
already need the same behavior.

## 10. Known Gaps

- Custom user themes are not loaded. New palettes ship as presets in
  `theme/builtin.rs` with both variants, a `UiColors`, and a syntax palette.
- The `System` theme tile resolves to a concrete variant at click time; it is
  not a persistent follow-the-OS mode.
- `window_decorations` and `window_backdrop` are read once at startup.
- The Linux sidebar cannot reveal a native material. The translucent shell is
  opt-in, and it is blurred only with `window_backdrop: blurred` on a Wayland
  compositor that offers ext-background-effect-v1 or KDE blur; elsewhere,
  without an external blur, it shows the raw wallpaper through the veil.
- `reduce_motion` stops hover interpolation, the toggle slide, the sidebar slide, the resize edge
  nudge, the resize detent settle, the Workspaces row motion, menu reveals, the empty-state and comet spinners, and the thinking matrix only; the other animations
  listed in 4.8 ignore it.
