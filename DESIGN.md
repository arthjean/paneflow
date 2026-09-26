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
| Native window systems | Mica on Windows 11, the AppKit sidebar material on macOS, client-side decorations with platform caption glyphs | `feat(chrome): native compositor blur backdrop on Linux and macOS`, `feat(window-chrome): native Win11 caption glyphs in light and dark`, `feat(macos): add sidebar material setting` |

Paneflow borrows the reasoning of these products, not their pixels. The
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
   Controls menu in 5.4 and the release toast in 5.8; the other shadows are the
   client-side window, the drag ghost, and the dialog card (`modal_card`)
   over its scrim.
3. One highlight material. Hovered, active, and selected states are alpha
   tints of one color per theme lightness, never a per-component fill.
4. Every rounded surface takes its radius from section 4.4. New radii are not
   introduced.
5. Motion explains state: hover, focus dim, sidebar slide, toast lifecycle.
   Nothing animates for decoration except the startup splash shimmer, the
   update banner shimmer, and status spinners. Any new animation MUST read
   `reduce_motion`; section 4.8 lists which existing ones do.
6. Color carries meaning first: added, modified, deleted, conflict, error,
   and the eight broadcast groups keep their hues across presets.
7. Density over decoration. Body text is 12 px, labels are 11 px, micro
   chips are 9 to 10 px. Whitespace is spent on the grid, not on padding.
8. Every surface holds with the native material on and off, in light and
   dark, and at the 800 by 500 minimum window.

## 3. Anatomy

### 3.1 The shell

```text
┌ title bar: max(1.75rem, 32px), full width, drag region, caption glyphs ─────────────┐
│ ▤  Files  Help              · workspace name                         [ _  ☐  × ]    │
├──────────────┬───────────────────────────────────────────────────┬───────────────────┤
│ primary      │ main panel: inset card, 4px inset, 10px radius,   │ right rail        │
│ sidebar      │ corner masks painted in the shell color           │ sessions or files │
│ 300px        │ ┌ pane card, 24px squircle ─┐ ┌ pane card ─────┐  │ 300px             │
│              │ │ header 40px: title, tools │ │                │  │ or diff dock      │
│ Workspaces   │ │ terminal, inset 10 / 6    │ │                │  │ 880px default     │
│ folder rows  │ └───────────────────────────┘ └────────────────┘  │ 360px min         │
│ tab rows     │              8px gutter, 80px min pane            │                   │
│ update banner│                                                   │                   │
│ Settings     │                                                   │                   │
└──────────────┴───────────────────────────────────────────────────┴───────────────────┘
```

| Region | Role | Geometry | Source |
| --- | --- | --- | --- |
| Window | Client-side decorations on every platform | Default 1200 by 800, minimum 800 by 500, corner radius 10, border 1, resize border 10, shadow black 0.4 blurred 5 when floating | `src-app/src/window_state.rs`, `src-app/src/app/constants.rs`, `src-app/src/window_chrome/csd.rs` |
| Title bar | Drag region, sidebar toggle, Files and Help menus, workspace name, caption controls | Height max(1.75 rem, 32 px); control size 20; edge inset 8; control spacing 12; macOS brand padding 80 for the traffic lights | `src-app/src/window_chrome/title_bar.rs` |
| Primary sidebar | Workspaces rail in Agents, navigation in Settings | Width 300; slides in 280 ms | `src-app/src/app/sidebar/mod.rs`, `src-app/src/app/render.rs` |
| Main panel | The inset card that holds the pane grid or a Settings page | Inset 4 on right and bottom, and on the left only when the sidebar is hidden; radius 10; four corner masks painted in the shell color, transparent while a chrome material is active, so no surface inside the panel paints its own square background | `src-app/src/app/render.rs`, `src-app/src/window_chrome/mod.rs` |
| Pane grid | Binary split tree of pane cards, one per workspace tab | Gutter 8, divider hit area 7, minimum pane 80 | `src-app/src/layout/tree.rs`, `src-app/src/layout/render.rs` |
| Right rail | Sessions rail | Width 300 | `src-app/src/app/sessions_sidebar.rs` |
| File tree | Inside the file dock, below its shared toolbar, to the right of the editor | Width 250, shrinking to preserve 200 px for the editor | `src-app/src/app/files_sidebar/mod.rs` |
| Diff dock | Side dock attached to a tab, holding the branch diff and editable file tabs | Width 880 default, 360 minimum, 1400 maximum, clamped to the room the panel has; 8 file tabs maximum | `src-app/src/app/diff_dock/model.rs` |
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
| Diff dock surface picker | Fills a fresh dock | Three cards 122 by 98, gap 12, radius 10, grid padding 16 | `src-app/src/app/diff_dock/surface_picker.rs` |
| Composer | Scrim over the whole pane, panel docked at its bottom | Black scrim at 0.25 on the 20 px squircle; panel with margin 8, padding 8, gap 6, 1 px border, radius 8; header chips 10 px; input max height 180 | `src-app/src/pane.rs` |
| Menus and selects | Deferred, anchored under the trigger | Squircle 18, list padding 4, item height 28 | `src-app/src/settings/components.rs` |
| Tooltip | After 800 ms | Squircle 14 on the title bar color with a 1 px border | `src-app/src/ui_primitives.rs` |
| Terminal search | Top right of the pane, 8 px inset | Squircle 14 on `subtle` with a 1 px `border`, 325 by 36, padding 14 left and 4 right, gap 8: the field, a `.*` mark only while regex mode is on, the match count, a 1 px divider, then previous, next and close as 28 px squircle icon buttons with the sidebar hover tint | `src-app/src/terminal/view.rs` |
| Toast | Bottom right, 18 px inset | Radius 8 on `subtle`, minimum width 220, one header row and an optional action row | `src-app/src/app/notifications.rs` |
| Worktree removal dialog | Centered | The card of the close dialog: squircle at `PANE_CARD_RADIUS`, 460 wide, padding 20 over a 0.55 black scrim, the blocking workspaces, tabs and sessions on `subtle` at 0.5 on radius 8, one kind word per row at 10 px muted, then `Cancel` and a destructive `Remove` | `src-app/src/app/worktree_remove.rs` |
| System Info dialog | Centered | The card of the close dialog at squircle 20, 560 wide, padding 20: a 44 px app icon beside a 16 px Semibold title and the build line at 11 px muted, then one 11 px muted eyebrow and one `menu_panel` per report section (`System`, `Rendering`) whose rows set a 116 px muted label column beside the value in Geist Mono 12, then the privacy line at 11 px muted, `Close`, and a solid `Copy` button in the toggle blue `#339cff` with a white label (`solid_button`). Escape closes, Enter copies, and focus returns to where it was | `src-app/src/app/system_info_dialog.rs` |
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
| `base` | Panel and settings background, the work surface | `#181818` | `#ffffff` |
| `surface` | Cards inside a panel, menu surfaces in dark | `#212121` | `#f7f7f7` |
| `card` | Settings cards and modal cards, one step above `base`; `subtle` controls and `menu_panel` surfaces on it must stay one step above it | `#232323` | `#ffffff` |
| `overlay` | Shell chrome in dark, popups in light | `#141414` | `#ffffff` |
| `border` | Hairlines, card outlines, pane card border | `#252525` | `#e6e6e6` |
| `subtle` | Pills, inputs, toasts, resting control fill | `#2a2a2a` | `#eeeeee` |
| `muted` | Secondary text, icons at rest, eyebrows | `#a0a0a0` | `#636363` |
| `text` | Primary text, icons on hover | `#dddddd` | `#262626` |
| `accent` | Links, selected metadata, the one primary action | `#57d5c4` | `#4c6fff` |
| `vc_added`, `vc_modified`, `vc_deleted`, `vc_conflict` | Diffstat, status letters, change bars, attention border | `#57d992`, `#ffd166`, `#ff6f6a`, `#ffa657` | `#29681c`, `#df8e1d`, `#a00b2b`, `#fe640b` |
| `vc_added_background`, `vc_deleted_background` | Row washes in the diff | added, deleted at 0.12 | `#40a02b`, `#d20f39` at 0.16 |
| `vc_word_added`, `vc_word_deleted` | Word washes inside a changed diff row | `vc_added`, `vc_deleted` at 0.40 | `#40a02b`, `#d20f39` at 0.40 |
| `group_1` to `group_8` | Broadcast group stripe and picker | blue, green, yellow, red, violet, teal, orange, periwinkle | Catppuccin Latte hues |
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
Vercel and Tailwind do. Status hues are functional and MUST NOT be recolored
to match a brand when doing so weakens the meaning.

In every light preset, `vc_added` and `vc_deleted` are darkened from the wash
hue until they clear 4.5:1 on the title bar, the active and hover row tints,
`base`, `surface`, and the diff gutter where the row wash and the gutter wash
stack; the washes keep the lighter hue. `muted` clears 4.5:1 on the same
sidebar tints.

The terminal selection foreground is never hand-tuned: it is recomputed at
theme load until it clears APCA Lc 45 against the selection background.

### 4.3 Tints and fixed values

| Tint | Dark | Light | Where |
| --- | --- | --- | --- |
| Sidebar row active | white at 0.11 | `#262626` at 0.08 | `sidebar_tab_active_background` |
| Sidebar row hover | white at 0.07 | `#262626` at 0.04 | `sidebar_tab_hover_background` |
| Tab icon card | title bar color blended with the active tint, then darkened 0.10 | darkened 0.05 | `sidebar_tab_icon_card_background` |
| Menu item selected | `text` at 0.10 | same | `select_item` |
| Menu item hover | `text` at 0.05 | same | `select_item` |
| Menu surface | `surface` lifted by 0.035 lightness | `overlay` | `select_menu_surface` |
| Menu border | `border` at 0.6 | same | `menu_surface` |
| Control hover | `subtle` moved 6 percent toward `text` | same | `select_trigger`, `secondary_button` |
| Hairline | `border` at 0.5 | same | `hairline` |
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
| `#3a83f7` | Title bar pill when a manual check finds a release | Solid update blue with a white glyph, label, and `×`, identical in every preset |
| `hsl(40 85% 55%)` | Callout warning | Severity hue independent of preset |

### 4.4 Geometry

| Element | Radius | Corner | Border |
| --- | --- | --- | --- |
| Window | 10 | round | 1 px `border` on free edges |
| Main panel | 10 | round, masked | none |
| Pane card and right diff dock | 24 | squircle | 1 px `border`, or `vc_conflict` at 0.7 with attention; content stays inside the corner curve through its inset, 10 by 6 for a pane and 8 for the dock body, because GPUI clips to rectangles only |
| Settings card, System Info and About dialogs | 20 | squircle | none on a settings card, 1 px `border` at 0.6 on a dialog |
| Pane palette ground | 24 | squircle | none |
| Menu, select popup | 18 | squircle | 1 px `border` at 0.6 |
| Primary sidebar workspace and tab rows | 9 | continuous corner approximation | no border |
| Primary sidebar header controls | 7 | continuous corner approximation | no border |
| Primary sidebar Settings button | 12 | continuous corner approximation | 36 px high, glyph 18 px for optical balance with the 20 px filter glyph |
| Primary sidebar filter | full | capsule | 36 px high, glyph 20 px |
| Primary sidebar inline hover actions | 6 | continuous corner approximation | no border |
| Tab icon cards, shared row skin, secondary button, menu item, tooltip, terminal search, title bar manual check pill | 14 | squircle | tab icon card, tooltip and terminal search, 1 px `border` |
| Theme tile | 10 | round | 2 px `text` at 0.12, 0.32 on hover, 0.85 when selected |
| Sidebar update banner, filter field, settings control, select trigger, title bar menu trigger | 8 | round | none |
| Toast, composer, drop overlay, drop placeholder | 8 | round | drop overlay 2 px blue |
| Toast action button, theme mockup inner frame | 7 | round | none |
| Toolbar pill, sidebar IPC banner, sidebar branch chip, branch prompt field | 6 | round | IPC banner 1 px `border` |
| Title bar sidebar toggle, branch prompt primary button | 5 | round | none |
| Icon button, composer chip | 4 | round | none |
| Scrollbar thumb, header chip, filter clear | 3 | round | none |

Squircle means `squircle_fill` and `squircle_border` from
`src-app/src/ui_primitives/squircle.rs`, applied through `squircle_skin`,
`setting_card`, `menu_surface`, or `tooltip_shell`. Plain `rounded()` is for
small circular controls and the explicitly round surfaces in the table.

### 4.5 Spacing and sizes

| Measure | Value |
| --- | --- |
| Panel inset | 4 |
| Pane gutter | 8 |
| Pane content inset | 10 horizontal, 6 vertical |
| Pane header | One 44 px surface tab and action row; action gap 7 |
| Pane tab bar | 26 chips plus 6 below, 32 total; gap 3; 8 tabs maximum |
| Sidebar row | margin 8, padding 7 by 6, minimum height 32, content gap 3, icon-to-title gap 8, line height 20, spacing 2 (10 before the workspace that opens a new group), radius 9 |
| Sidebar tab icon stack | 16 px icons, cap 4, overlap 11, 24 by 24 icon card |
| Sidebar action button | 22, gap 1; folder glyph 15 in a 20 px slot |
| Sidebar footer | padding 0 top and 9.5 bottom; filter and gear 36 tall, gap 6, margin 8 shared with workspace rows; filter glyph 20, Settings glyph 18; filter text 15 with line height 20 and horizontal padding 10; banners margin 6 with 2 below, update banner 30 tall with padding 8 |
| Sessions row | minimum height 32, terminal glyph on the tab title column; 5 rows per agent group before Show all |
| Settings row | padding 12 by 10, gap 16; section header bottom padding 8 |
| Select trigger | padding 10 by 6, width 190 to 260 |
| Menu | list padding 4, item gap 1, item height 28, width 200 to 280, max height 320 |
| Toggle | track 36 by 22, knob 18 |
| Icon buttons | small 20 outer with 12 icon, medium 24 outer with 13 icon |
| Toolbar pill | height 24, padding 8, gap 5 |
| Filter field | padding 10 by 6, gap 6, 13 px search icon, 16 px clear button with a 10 px glyph |
| Toast | inset 18, padding 12 / 14 by 11, minimum width 220, action buttons 26 tall; the release toast is inset 12, padding 12, width 448, close button 20, action button 26 tall |
| Scrollbar | width 6, gutter 10, minimum thumb 24, inset 2. Terminal panes overlay it: shown on any viewport move, held 1 s, faded out over 200 ms; hovering the gutter or dragging pins it, grows the thumb to the full gutter and reveals the track over 120 ms; `reduce_motion` snaps both |
| Diff | body inset 8 on the sides and bottom, row 18, file header 32, fold row 32, sticky header 24, gutter 36, change bar 4, split divider 3, column header 30, minimum split column 360, revert chip 56 by 16 inset 10 |
| Code editor | 12 px mono, caret 2, scrollbar 15, minimum thumb 25; git marker column 6 left of the numbers, bar 4 radius 2 inset 1, deleted dot 8, hover grows 3 to the left |

### 4.6 Typography

| Role | Family | Size and weight | Where |
| --- | --- | --- | --- |
| Interface | Geist, bundled, set on the root element | 12 Normal for body, Medium for titles in rows | Everything that is not a terminal or code |
| Labels | Geist | 11 Normal muted for eyebrows and descriptions, Semibold for `section_eyebrow` | Settings, rails, pills |
| Micro | Geist | 9 to 10 | Header chips, composer chips, hints |
| Emphasis | Geist | 13 Medium | Row titles that need to outrank body |
| Title | Geist | 14 Semibold | Pane header, empty-state titles, callout titles |
| Page heading | Geist | 26 | Settings page title |
| Dialog title | Geist | 16 | About, System Info |
| Splash | Geist | 34 | Startup wordmark |
| Terminal | User choice among fixed-pitch families; default the bundled JetBrainsMono Nerd Font, with the Mono-suffixed name kept as an alias | 13 pt default, weight, line height, and cell width configurable | Panes |
| Code and diff | `resolve_font_family(None)`, the terminal default | 12 | Diff dock, editor, theme preview |

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

Workspace folder rows use the shared Hugeicons `folder.svg` and
`folder-open.svg` stroke glyphs, tinted with `muted` at 15 px. The welcome
screen, the diff dock, and the workspace settings use the same two files. The
sidebar footer settings button uses the matching `settings.svg` gear.

| Size | Use |
| --- | --- |
| 10 | Filter clear glyph |
| 11 | Sidebar agent state glyphs (bell, error, pull request) and the thinking matrix |
| 12 | Small icon button, select chevron, drag ghost |
| 13 | Medium icon button, filter search, preset logo, menu check mark |
| 14 | Title bar sidebar toggle, editor logos, sidebar footer banners and gear |
| 15 | Toast icon, sidebar folder, sidebar header glyphs |
| 16 | Sidebar tab icon, callout icon, diff dock tab icon, diff file header file-type icon |
| 17 | Diff file header generic glyph |
| 18 | Empty-state glyph |
| 20 | Diff file header Rust icon, which needs the larger box |

File type icons come from `src-app/src/file_icons.rs`, which maps extensions
to `icons/languages/`.

### 4.8 Motion

| Motion | Duration | Easing | Notes |
| --- | --- | --- | --- |
| Hover on any control | 120 ms, scaled by the distance left to travel | ease-out quint | `animated_hover`, retargets mid-flight, pauses during a drag |
| Pane header buttons | 120 ms | ease-out quint | Tint of the action buttons and the close glyph; the close slot itself toggles with the header hover |
| Unfocused pane dim | 130 ms, scaled by distance | ease-out quint | Overlay of the terminal background |
| Primary sidebar slide | 280 ms | cubic ease-out | Panel inset and gutter follow the width |
| Workspaces rows | 180 ms | cubic ease-out | `SidebarRowMotion`: folding or unfolding a workspace folder, or filtering the rail, fades each tab row while its measured height grows or shrinks, the group gap below a folder follows the last row, and the folder glyph crossfades between `folder.svg` and `folder-open.svg`; a new toggle retargets from the current state, and a folder seen for the first time rests |
| Menu reveal | 140 ms | cubic ease-out | `menu_reveal`: every menu, select popup, context menu, and submenu fades in from 0 while dropping 4 px into place; the pane palette's `New branch` form and the branch row it folds back to use the same reveal. No exit animation |
| Toast | 180 ms in, 1440 ms hold, 180 ms out | ease-in-out | 8 px lift on entry, 8 px drop on exit |
| Status spinner | 1 s loop | linear rotate | Title bar pill while a manual check runs, empty states |
| Sidebar thinking matrix | 720 ms cycle | stepped | 3 by 3 dots of 3 px, gap 1, trailing opacities 0.81, 0.49, 0.26 over a 0.06 base |
| Startup splash | 2600 ms shimmer, 900 ms minimum on screen | linear | Letters at 0.54 alpha, shimmer to 0.82 |
| Tooltip | 800 ms delay | none | `delayed_tooltip` |

`reduce_motion` (Settings, Appearance) is honored in five places today:
`animated_hover` settles instantly, the primary sidebar toggles without
the slide, the Workspaces rows fold and filter without motion, `menu_reveal`
mounts menus at rest, and the sidebar thinking matrix holds its first frame. The dim fade, toasts, spinners, and the
shimmers keep animating. The config description promises a static frame for
decorative animations; that promise is **Proposed** until the remaining
animations read the flag. Feedback is never removed, only its interpolation.

## 5. Component Contracts

### 5.1 Title bar

Full width on every platform, drag region, double-click zooms, right-click
shows the window menu where the platform has one. Left rail: the sidebar
toggle (20 px, radius 5, resting tint when the sidebar is hidden), then
`Files` and `Help` triggers (height 20, padding 6, radius 8, 12 px, muted
until hovered or open). Center: a 3 px muted dot and the workspace name at 12
px Medium, hidden in Settings. Right: the caption controls. The title bar
still carries its own automatic update and IPC pill code, but the cockpit
shell never renders it (`tb.cockpit = true` in `app/render.rs`); that code is
**Migration**, and the sidebar footer owns both banners. The same slot,
between the center and the caption controls, does render the manual check
pill raised by `Help > Check for Updates…` and the `PaneFlow` menu on macOS:
height 24, padding 8, gap 5, squircle 14 through `squircle_skin`, no border, 11 px Medium.
`Checking for updates…` shows the 11 px spinning loader on `subtle` at 0.7
opacity; `Paneflow is up to date` sits in `vc_added` on its 0.12 wash and
leaves after 3 s; `v<x.y.z> available` is solid `#3a83f7` with a white
download glyph and label (click installs); `Update check failed` sits in
`vc_deleted` on its 0.12 wash and leaves after 3 s. The two colored states
carry no glyph. Only the manual check
raises this pill; the automatic check keeps to the footer banner. On Windows the caption glyphs are native Windows 11
shapes; on macOS the traffic lights get 80 px of brand padding. The title bar
draws no bottom hairline inside the cockpit shell; the panel inset separates
it from the content.

### 5.2 Primary sidebar, Agents mode

The primary sidebar uses the platform system UI font at 14 px, Medium for workspace and tab titles, with more generous row sizing than the Unpeel reference. Branch labels use 14 px system text in the same family as workspace titles; diff statistics use 12 px system text. Simple rows have a 32 px minimum height and 9 px corners; Paneflow-specific Git metadata retains its second line. Folder glyphs are 15 px inside 20 px slots. The active tab takes the active row tint, every other row the hover tint under the pointer. A workspace title, tab title, or branch longer than 13 characters shows its full value in a tooltip, and a session row's tooltip leads with its label. Other application surfaces retain Geist.

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

Header row `Workspaces` at label size with two 22 px icon buttons (the
Customize sidebar menu behind a filter glyph, and new workspace behind a
folder-plus glyph). A workspace is a folder row; its tabs are child rows with
inline rename, hover actions, and reorder by drag. A tab row shows the tab
title, the branch with its glyph, and the
diffstat in `vc_added` and `vc_deleted`. Status sits at the trailing edge of
both row kinds, on one X, as an 11 px glyph and a 10 px word: an amber bell
and `Input` when the agent needs input, an `agent_error` circle-x and
`Error` when it failed, the dot matrix alone while it thinks, in the agent's accent when it declares one and `muted` otherwise, a light
blue 7 px dot and `Done` with the unread count once it is finished.
With no agent to report, the tab's pull request takes the slot in GitHub's
state color: `Review`, `Draft`, or `Merged`. The bell and the dot use the
fixed colors from 4.3. The slot is shared with the hover action: under the
pointer the status turns invisible, its width kept, and the button paints
over it. A `Customize sidebar` menu on the rail header toggles branch,
diffstat, pull request, and indent guide per value.

Drop placeholder while dragging: margin 6, radius 8, blue at 0.10 with a 0.22
border and a 2 px line.

The footer stacks, top to bottom: the IPC offline banner when the socket is
disabled (margin 6, padding 8 by 6, radius 6, 1 px `border` on `subtle`, a
14 px alert glyph and `IPC offline` at 12 px Medium), the manual-check failed
banner when the last manual check could not reach the feed (margin 6, height
30, padding 8, radius 8, the active row tint, a 14 px `vc_deleted` alert
glyph, `Update check failed` at 12 px Medium, a 13 px bold `×` to dismiss;
0.8 rising to 1.0 on hover, click retries), then a 36 px footer with a flexible
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

Filter changes animate row opacity and occupied height over 180 ms with a
cubic ease-out. Exiting rows remain until the transition completes, and
surviving rows move as the released space collapses. A new query retargets
from the current visibility. Measured row heights preserve checkout metadata
geometry. Reduced motion applies the filtered list immediately; workspace or
tab structure changes reset the transition to avoid stale rows.

### 5.3 Pane card

A 24 px squircle filled with the terminal background, 1 px `border`. The
header shares the 44 px surface tab row, with no repeated pane name: a 6 px status dot, 9 px
chips for progress and worktree, and on the right 22 px action buttons that
appear only on the focused pane (split vertical, split horizontal,
the diff dock), and a `Z` chip on `accent` when the pane is zoomed.
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
targets the active pane. Diff attribution lives in the surface tab tooltip.
Dragging a surface tab within the layout uses the gray pane drop preview.
A center drop moves the tab into the target pane; an edge drop splits the
target with the same surface entity. Moving the last tab removes the source
pane. Zoomed layouts and detached windows do not accept these tab transfers.
In docked panes, New tab uses the same 22 px button and 14 px icon dimensions
as the actions. New tab, the divider, and the actions hide on unfocused panes
while retaining their layout space so tabs do not shift when focus changes.
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
overlay, broadcast stripe (3 px of the group color, inset by the radius top
and bottom), border, composer. Unfocused panes in a multi-pane workspace fade
under a 0.3 overlay by default. The drop overlay is blue at 0.10 with a 2 px
blue border, radius 8, margin 8, and a swap variant with its own tint.

### 5.4 Diff dock

#### Detached pane window

A detached pane has one 44px caption and surface-tab row, modeled on the supplied
Superlogical reference. Closing the native window returns the pane to its
workspace; there is no separate return button. Tabs are 32px high, up to 208px wide, with ellipsis and
horizontal overflow. Only the active tab has a capsule background and a small
elevation shadow. Its outline follows the light-theme accent but stays neutral
in dark themes. The caption has no bottom divider. The unified new-tab button
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

The dock attaches to a tab and opens on a surface picker (three cards, 122 by
98). It starts with no content tabs: Changes is created only when selected in
the picker or the `+` menu, which also offers File and Terminal. Every content
tab is closable, including Changes and the first tab. Closing the last tab
returns to the picker; switching sessions preserves the chosen tabs.
Tabs are chips with the sidebar rail skin: no separators, active chip on
the active tint, inactive chips wash in on hover. Change bars are 4 px,
dashed for deletions; file headers are 32 px rows that collapse to a 24 px
sticky header while scrolling, with the file-type icon on the left and the
diffstat right-aligned.

Changed rows paint in two tones. The line wash is `vc_added_background` or
`vc_deleted_background`, 0.12 in dark and 0.16 in light, and the words that
differ inside a modified block sit on `vc_word_added` or `vc_word_deleted` at
0.40, painted between the line wash and the text at the x positions of the
shaped line, so tabs and CJK glyphs align with the rectangle. A run that
scrolls past the cell edge is clipped to the cell. A block whose two sides
differ only by whitespace is muted: the line wash drops to 0.08, no word
rectangle is painted, and the 4 px change bar sits at 0.5 alpha. Highlight
`None` keeps the change bar and gutter tint and drops every wash. Both
`Highlight` and `Whitespace` live in the Changes options menu
next to Layout and are session-scoped like Split and Unified. Highlight is
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
breadcrumb toolbar spans both the editor and the tree. The tree
starts below that toolbar with a 1 px left divider. Its width is 250 px,
shrinking only when needed to leave 200 px for the editor. The search field
has an 8 px outer inset, 28 px height, 8 px corners and a 1 px border. Tree
rows are 28 px tall with 13 px labels, 14 px icons, 18 px indentation and
6 px selection corners. These dimensions follow the Codex App reference
provided for this surface and are contextual exceptions to the rail skin.

Tree rows carry Zed's version control decoration, summed from
`git status` over the tree root. The label takes `vc_conflict`, then
`vc_deleted`, then `vc_modified`, then `vc_added` for an addition or an
untracked path, and falls back to `text`. A file also shows a status letter at
the right of the row, 11 px bold in a 14 px slot: `!` for a conflict, `U` for
untracked, then `D` and `M` for the worktree side before the same two for the
index, and `A` for a staged addition. A directory shows a 6 px dot at 0.5
opacity in that slot instead, rolling up every descendant. Ignored paths never
reach the tree, so they never carry a status.

The tree button sits immediately after Editor Controls in the file toolbar.
It and `secondary-alt-f` toggle the embedded tree.
Opening it selects an existing file tab or creates a file picker tab. Closing
the last file or file picker tab closes the tree; Changes and Terminal remain
available. Selecting those surfaces temporarily hides the tree and gives them
the full dock width. Sessions can remain open alongside the integrated tree.
Tree visibility remains local to the active agent session.

The file header places Zed's 14 px `filter.svg` icon after `Ln` and `Col`.
Its Editor Controls menu offers Minimap and Scrollbar toggles, scoped to the
open file tab. The minimap starts hidden; scrollbars start visible. This menu
is a contextual exception to the shared menu skin: it follows Zed's 200 px
minimum width, 6 px corners, 1 px border, 4 px vertical padding, 14 px check
slot on the left, and small two-layer shadow. Palette roles remain theme-aware.
Escape, an outside click, or a second click on the trigger dismisses it.

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

Navigation uses the same system font and row scale as the workspace sidebar:
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
three groups labeled Personal, Terminal, Integrations. Pages are a centered
column with a 26 px heading, eyebrow labels at 11 px muted, and cards
(squircle 20 on the `card` role). Rows are `toggle_row` or a
`setting_text` plus control: title 12 Medium, description 11 muted, control
right-aligned. Toggles are 36 by 22 with an 18 px white knob. Selects open a
`select_menu` under the trigger. Destructive actions use the fixed red
button. The Appearance page leads with three theme tiles (System, Light,
Dark, 134 tall, radius 10, 2 px border) holding a mockup painted from the
preset, a live diff sample, then the preset select and the preferences.

The Keyboard Shortcuts page is a register, not a tree. Under the heading, one
12 px muted line carries the contract: click a row, press the new chord,
Backspace clears it, Escape cancels, and a dot marks a change. One 36 px
capsule field on `subtle` filters the list, with a `Name` | `Key` segmented
pair of full-radius pills at its trailing edge: `Name` filters on the action
label or an ASCII spelling of the chord, `Key` captures the next chord and
shows what owns it. Groups are the 11 px muted eyebrow of 5.5 with the count
on the trailing edge and, for context-bound groups, the context after a
middle dot (`Terminal · while a pane has focus`); they never fold. Each group
is one `menu_panel` of 5.6 (squircle 18, lifted surface, 0.6 border, 7 px
padding, 1 px gap) whose rows are `menu_row` (34 px, squircle 14, `text` at
0.05 on hover): a 5 px accent dot when the binding differs from the default,
the action at 12 px, and the chord as keycaps (20 px, radius 5, `subtle` with
a 1 px edge lifted 8 percent toward `text`, 11 Medium, gap 3), or a dashed
`Unassigned` cap. Hovering a changed row reveals a 24 px reset glyph before
the caps. While recording, the caps give way to one solid `Press keys…` cap
in the caret blue of 6.3 with a white label, and nothing else on the row
changes. A chord another action owns is held rather than saved, drawn in
`vc_conflict` with `Also <action> · press again to take it`, and a second
press of the same chord takes it. The list ends with one more menu panel that
counts the changed bindings and holds `Reset all to defaults` as a menu row;
the confirmation swaps the row for the question, a `Cancel` menu row, and the
red `Reset`. The scrollbar of the list sits at the panel edge, as in the
panes and the docks, and the fixed heading, intro line, and filter keep a
20 px gap above the scrolling rows.

### 5.6 Menus, selects, tooltips

Popups share `menu_surface`, except the Zed Editor Controls menu in 5.4:
squircle 18, lifted surface, 0.6 border.
Every menu is built from `menu_panel` and `menu_row` in
`settings/components.rs`, which own the geometry: 34 px squircle rows at radius
14, a 1 px gap, and 7 px of surface padding. The squircle clamps a corner to a
third of the row height, so a 34 px row paints 11 and the 7 px inset lands it
concentrically inside the 18 surface; a unit test in that module fails if the
three constants stop agreeing. Rows carry `text` washes for hover and
selection, 12 px text, a 12 px chevron on triggers, and a 13 px check on the
selected item. Widths run from 200 to 280 and the list scrolls past 400 px.
Panels that are not menus, the command palette, the pane palette list, the
theme picker, the clone dialog and the Welcome rows, keep the 28 px
`select_item` row. Tooltips are squircle 14 on the title
bar color, padding 8 by 6, small text, shown after 800 ms through
`delayed_tooltip`.

### 5.7 Composer, palette

The Composer dims the whole pane under a 0.25 black scrim and docks a
bordered panel at the bottom: a `Composer` label at 11 px Medium, then 10 px
chips on radius 4 for the broadcast toggle (`Single pane` on `subtle`, or
`Broadcast: group` on `accent` at 0.15), `agent generating - Enter queues`
on `vc_modified` at
0.15 while the agent is busy, and a cancel chip when prompts are queued;
Enter submits, Escape closes. The pane palette fills an empty tab named
`New pane` with a centered 260 px column: a 13 px Semibold title, an
optional branch row, one 34 px row per preset with its 14 px agent mark and
a `not installed` marker at 10 px where the binary is missing, and an inline
error in `vc_deleted` at 11 px. The branch row's menu opens with a
`New branch…` item above a hairline; picking it retitles the column `New
branch` with a 20 px back chevron at its left edge and replaces the row with
an unlabeled form: a 28 px `subtle` field on radius 6 with an 11 px branch
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
10: a title, one line of context, the name field, a `Create branch` button
filled with `accent` at 0.15, and the hint `Enter creates · Esc cancels`. The
`Clone repository` modal is the menu surface of 5.6, 544 wide, docked 96 from
the top of the window over a 0.4 black scrim. The command palette docks at the
same height on its own panel: the menu surface at squircle 13 with padding 6,
420 to 640 wide by its widest row, then a 34 px search well (the 0.07 `text`
tint on squircle 9, no border, a 14 px glyph and 14 px text; a scope shows as
a 24 px chip inset 5 on every side), then 29 px rows 2 px apart in a
`uniform_list` capped at 400. A row is a squircle 8 pill in the selection blue
when selected, an optional 14 px glyph, the label at 14 px, the current value
or binding at 13 px muted, and a chevron when it opens a scope. Typing filters
on whole words, arrows move, Enter dispatches, and the palette never lists
itself. The clone modal is a quick pick: a field reading `Provide repository URL or pick
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

Toasts stack bottom right on `subtle` with a 15 px icon, 12.5 px text, and
26 px action buttons on `text` at 0.08 to 0.12. Error messages are detected
and get the error icon. The release toast is **Contextual** and does not
follow that shape: it is a Zed notification frame, 448 wide, inset 12,
padding 12, gap 8, radius 8, a 1 px `text` hairline at 0.10 on the title bar
color, the same fill as the sidebar, and Zed's four-layer elevation shadow. No
icon, a 14 px `text` line reading `Updated to PaneFlow x.y.z`, a 20 px close
button on its right with an 11 px `muted` glyph and a `text` wash at 0.08 on
hover, and one 26 px squircle button on `text` at 0.08 to 0.12. The button and
the whole surface open `paneflow.dev/docs/changelog/<tag>`, then dismiss it.
It lands 1500 ms after boot, once the window is painted, and it is the one
toast that never auto-closes: only the close button, the surface click, or
another toast in the queue removes it. Callouts (`widgets/callout.rs`) are 16
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
| Focus the Workspaces sidebar | `secondary-alt-s`; then up, down, home, end move; enter opens; space, left, right fold; F2 renames a tab; delete closes it; alt-up and alt-down reorder; escape returns to the pane |
| File tree in the dock | `secondary-alt-f` |
| Composer | `secondary-shift-space` |
| Attention queue | `secondary-shift-a` |
| Broadcast groups, toggle member | `secondary-shift-m`, `secondary-shift-b` |
| Jump to next waiting agent | `secondary-shift-j` |
| Layout presets | `secondary-alt-1` to `secondary-alt-4` |
| Command palette | `secondary-shift-p` |
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

Shared text inputs use a blue (`#007AFF`) insertion caret, 2 px wide and
vertically centered. Its height is the font size plus 2 px, capped at the
line height, so it does not span the full input row.

Focus is shown by the pane border and the title bar, not by dimming: every
pane keeps full contrast by default, and `unfocused_pane_opacity` below 1.0
opts back into fading the siblings. An agent that needs the user gets the
`vc_conflict` border at 0.7 and a sidebar dot; clicking anywhere in the panel
acknowledges visible completions. The attention queue lists those panes and
`secondary-shift-j` jumps through them. Native OS notifications fire only
while the window is unfocused.

## 7. Platform Materials

| Platform | Backdrop | Sidebar | Terminal | Chrome |
| --- | --- | --- | --- | --- |
| Windows 11 build 22621 and later | Mica by default (`window_backdrop: auto`), transparent by choice; `blurred` and `acrylic` in `paneflow.json` resolve to `auto`, so blur needs `PANEFLOW_WINDOW_BACKDROP=blurred` for one launch | Reveals the backdrop when `windows_chrome_material` is on | Transparent default background when `windows_terminal_material` is on, masked to the panel; the panel inset gutters are repainted opaque only while `windows_chrome_material` is off | Native caption glyphs in light and dark |
| Windows 10 and older 11 | Opaque | Opaque card | Opaque | Same glyphs |
| macOS | Transparent window surface, material dropped in fullscreen | AppKit Sidebar material when `macos_chrome_material` is on | Opaque | Traffic lights with 80 px brand padding |
| Linux | Opaque shell, with no compositor blur | Opaque, tinted from the title bar color | Opaque | Client-side decorations with GPUI's generic glyphs, or server-side when `window_decorations: server` |

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
  Editor Controls menu in 5.4 and the release toast in 5.8. The window, the
  drag ghost, and the dialog card (`modal_card`) retain their shadows.
- Separators between tabs, chips, or toolbar buttons. The floating chip
  language replaced full-height bordered tabs in 0.5.5.
- Identity pills, badges, or logos in the pane header. The sidebar owns
  identity.
- Accent fills on anything larger than a button, or at more than 0.15. A
  button tinted with `accent` at 0.15, like the branch prompt's `Create
  branch`, is the ceiling.
- Hue in a neutral. If a gray reads warm or cool, it is a bug unless the
  preset is Claude, whose paper and graphite are the identity.
- A new radius, a new text size, or a new hover color. Pick from sections
  4.4, 4.6, and 4.3.
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
In `src-app/src/settings/search.rs`: `SettingCopy`, `SearchCard`, `Block`. In `src-app/src/settings/components.rs`:
`setting_card`, `toggle_row`, `setting_text`, `select_trigger`,
`select_menu`, `select_item`, `menu_surface`, `secondary_button`,
`destructive_button`, `hairline`. Add a primitive only when two surfaces
already need the same behavior.

## 10. Known Gaps

- Custom user themes are not loaded. New palettes ship as presets in
  `theme/builtin.rs` with both variants, a `UiColors`, and a syntax palette.
- The `System` theme tile resolves to a concrete variant at click time; it is
  not a persistent follow-the-OS mode.
- `window_decorations` and `window_backdrop` are read once at startup.
- The Linux sidebar cannot reveal a native material; it blends the tint into
  the title bar color instead.
- `reduce_motion` stops hover interpolation, the sidebar slide, the Workspaces
  row motion, menu reveals, and the thinking matrix only; the other animations
  listed in 4.8 ignore it.
