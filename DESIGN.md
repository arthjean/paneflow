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
| `docs/user/` | User-facing vocabulary (Agents, Review, Workspaces, Settings pages) |

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

`assets/images/demo-0.9.png` is the reference capture: two agents in pane
cards, the diff dock open on a file, the Files rail alongside. A pull request
that changes a surface ships a capture of that surface in the same theme
family, as `AGENTS.md` already requires.

## 2. The Thesis

### 2.1 What Paneflow is on screen

Paneflow is a cockpit for coding agents, not an IDE and not a terminal
emulator with tabs. The dominant idea of the screen is the grid of live pane
cards, each one a real terminal running a real agent. Everything else is
instrumentation around that grid: a rail of workspaces and tabs on the left,
a title bar that is mostly empty, docks that appear only when review or files
are needed, and a footer switch between the two modes that matter, **Agents**
and **Review**.

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
| Native window systems | Mica on Windows 11, the AppKit sidebar material on macOS, compositor blur on Linux, client-side decorations with platform caption glyphs | `feat(chrome): native compositor blur backdrop on Linux and macOS`, `feat(window-chrome): native Win11 caption glyphs in light and dark`, `feat(macos): add sidebar material setting` |

Paneflow borrows the reasoning of these products, not their pixels. The
bundled Vercel, Claude, and Cursor presets are identity swaps on top of one
structure; the structure is Paneflow's.

### 2.3 Three words

**Quiet.** The shell recedes. Neutrals carry no hue (`feat(theme): make the
shell neutrals hue-free`). The accent appears on links, selected metadata,
focus, one primary action, and nothing larger. Hover and selection are
translucent washes of the text color, not colored fills.

**Continuous.** Rows, cards, menus, and tooltips use a superellipse corner
(exponent 4, `src-app/src/ui_primitives/squircle.rs`), so a hovered row and
the card around it read as one material. Separators are gone between chips
and tabs. Hover, dim, and sidebar slide are interpolated, never stepped.

**Native.** The window is client-decorated on every platform with the
platform's own caption glyphs. The sidebar reveals the OS material where one
exists. Fonts for the terminal come from the user's system, with a bundled
Nerd Font as the default.

### 2.4 Operating principles

1. The pane grid is the content. Chrome is a frame and MUST NOT gain weight,
   color, or motion that competes with a running terminal.
2. Depth comes from the surface ramp (`base`, `surface`, `overlay`, `subtle`)
   and from inset cards with masked corners. Drop shadows were removed in
   0.5.4 and MUST NOT return on chrome; the only shadows are the client-side
   window shadow, the drag ghost, and the About dialog, which is Migration.
3. One highlight material. Hovered, active, and selected states are alpha
   tints of one color per theme lightness, never a per-component fill.
4. Every rounded surface takes its radius from section 4.4. New radii are not
   introduced.
5. Motion explains state: hover, focus dim, sidebar slide, toast lifecycle.
   Nothing animates for decoration except the startup splash shimmer, the
   update banner shimmer, and status spinners. Any new animation MUST read
   `reduce_motion`; section 4.8 lists which existing ones do.
6. Color carries meaning first: added, modified, deleted, conflict, error,
   stalled, and the eight broadcast groups keep their hues across presets.
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
│ 300px        │ ┌ pane card, 20px squircle ─┐ ┌ pane card ─────┐  │ 300px             │
│              │ │ header 40px: title, tools │ │                │  │ or diff dock      │
│ Workspaces   │ │ terminal, inset 10 / 6    │ │                │  │ 880px default     │
│ folder rows  │ └───────────────────────────┘ └────────────────┘  │ 360px min         │
│ tab rows     │              8px gutter, 80px min pane            │                   │
│ update banner│                                                   │                   │
│ Agents|Review│                                                   │                   │
└──────────────┴───────────────────────────────────────────────────┴───────────────────┘
```

| Region | Role | Geometry | Source |
| --- | --- | --- | --- |
| Window | Client-side decorations on every platform | Default 1200 by 800, minimum 800 by 500, corner radius 10, border 1, resize border 10, shadow black 0.4 blurred 5 when floating | `src-app/src/window_state.rs`, `src-app/src/app/constants.rs`, `src-app/src/window_chrome/csd.rs` |
| Title bar | Drag region, sidebar toggle, Files and Help menus, workspace name, caption controls | Height max(1.75 rem, 32 px); control size 20; edge inset 8; control spacing 12; macOS brand padding 80 for the traffic lights | `src-app/src/window_chrome/title_bar.rs` |
| Primary sidebar | Workspaces rail in Agents mode, Changes rail in Review mode, navigation in Settings | Width 300; slides in 280 ms | `src-app/src/app/sidebar/mod.rs`, `src-app/src/main.rs` |
| Main panel | The inset card that holds the pane grid, the Review view, or a Settings page | Inset 4 on right and bottom, and on the left only when the sidebar is hidden; radius 10; four corner masks painted in the shell color | `src-app/src/main.rs` |
| Pane grid | Binary split tree of pane cards | Gutter 8, divider hit area 7, minimum pane 80 | `src-app/src/layout/tree.rs`, `src-app/src/layout/render.rs` |
| Right rail | Sessions rail or Files rail, one at a time | Width 300 | `src-app/src/app/sessions_sidebar.rs`, `src-app/src/app/files_sidebar/mod.rs` |
| Diff dock | Side dock attached to a tab, holding the branch diff and editable file tabs | Width 880 default, 360 minimum, 1400 maximum, clamped to the room the panel has; 8 file tabs maximum | `src-app/src/app/diff_dock/model.rs` |
| Footer | Mode switch Agents / Review plus the Settings gear, with the IPC offline and update banners stacked above it | Persistent primary navigation | `src-app/src/app/sidebar_actions_menu.rs` |

### 3.2 Modes

Paneflow has two modes and one takeover surface.

| Mode | Sidebar | Main panel | Entry |
| --- | --- | --- | --- |
| Agents | Workspaces: folder rows, tab rows with branch, diffstat, and agent icon stack | Pane grid | Footer switch, default |
| Review | Changes: file rows with status letter, path, and diffstat, plus a filter field | Unified or split diff with sticky file headers, a Review button per branch column | Footer switch or `secondary-shift-g` |
| Settings | Back to the app, search field, three nav groups | One page at a time, centered column, 26 px heading | Footer gear |

Settings is not a window. It replaces the main panel and reuses the sidebar
width for its navigation, so the shell never changes shape. The title bar
hides the workspace name while Settings is open.

### 3.3 Overlays

| Overlay | Placement | Shell | Source |
| --- | --- | --- | --- |
| Launch Pad | Centered in the panel | Card 520 wide, radius 10; agent list, branch input, prompt input, footer hint and one accent button | `src-app/src/app/launch_pad.rs` |
| Pane palette | Fills an empty tab, titled `New pane` | A centered 260 px column on a 20 px squircle of the terminal background: 13 px Semibold title, an optional branch row 28 tall, preset rows 34 tall with a 14 px agent mark, gap 2, list capped at 420 tall, inline error at 11 px | `src-app/src/app/pane_palette.rs` |
| Diff dock surface picker | Fills a fresh dock | Three cards 122 by 98, gap 12, radius 10, grid padding 16 | `src-app/src/app/diff_dock/surface_picker.rs` |
| Composer | Scrim over the whole pane, panel docked at its bottom | Black scrim at 0.25 on the 20 px squircle; panel with margin 8, padding 8, gap 6, 1 px border, radius 8; header chips 10 px; input max height 180 | `src-app/src/pane.rs` |
| Menus and selects | Deferred, anchored under the trigger | Squircle 18, list padding 4, item height 28 | `src-app/src/settings/components.rs` |
| Tooltip | After 800 ms | Squircle 14 on the title bar color with a 1 px border | `src-app/src/ui_primitives.rs` |
| Toast | Bottom right, 18 px inset | Radius 8 on `subtle`, minimum width 220, one header row and an optional action row | `src-app/src/app/notifications.rs` |
| System Info dialog | Centered | Squircle 20, 560 wide, padding 20, label column 116 | `src-app/src/app/system_info_dialog.rs` |
| About dialog | Centered | 382 wide, radius 10, 1 px border, large shadow, 32 px header band, 225 px body, hardcoded grays; **Migration**, see section 10 | `src-app/src/app/about_dialog.rs` |

## 4. Foundations

### 4.1 Color architecture

Color resolves in three layers.

1. **Terminal theme**: 36 slots per variant (24 ANSI colors, 5 base colors,
   cursor, selection and its derived foreground, scrollbar thumb, link text,
   two title bar colors) plus a syntax palette for the diff and the editor.
   `src-app/src/theme/model.rs`, values in `src-app/src/theme/builtin.rs`.
2. **UI colors**: the semantic roles the chrome consumes, `UiColors`. A
   preset either ships its own `UiColors` (Vercel, Claude, Cursor) or lets
   Paneflow derive one from the terminal theme's lightness (Paneflow Dark and
   Light).
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
| `overlay` | Shell chrome in dark, popups in light | `#141414` | `#ffffff` |
| `border` | Hairlines, card outlines, pane card border | `#252525` | `#e6e6e6` |
| `subtle` | Pills, inputs, toasts, resting control fill | `#2a2a2a` | `#eeeeee` |
| `muted` | Secondary text, icons at rest, eyebrows | `#a0a0a0` | `#6a6a6a` |
| `text` | Primary text, icons on hover | `#dddddd` | `#262626` |
| `accent` | Links, selected metadata, the one primary action, info callouts | `#57d5c4` | `#4c6fff` |
| `tool_card_header_bg` | Reserved; no surface consumes it today | `#2e2e2e` | `#f1f1f1` |
| `vc_added`, `vc_modified`, `vc_deleted`, `vc_conflict` | Diffstat, status letters, change bars, attention border | `#57d992`, `#ffd166`, `#ff6f6a`, `#ffa657` | `#40a02b`, `#df8e1d`, `#d20f39`, `#fe640b` |
| `vc_*_background` | Row washes in the diff | added, deleted, modified at 0.12 | at 0.16 |
| `group_1` to `group_8` | Broadcast group stripe and picker | blue, green, yellow, red, violet, teal, orange, periwinkle | Catppuccin Latte hues |
| `agent_claude`, `agent_codex` | Identity dots and status glyphs | `#ffa657`, `#7eb6ff` | `#e89271`, `#5b6cff` |
| `agent_error`, `agent_stalled` | Failed and stalled agent states | `#ff6f6a`, `#a0a0a0` | `#d20f39`, `#808080` |

The dark work surface is `#181818` and the dark chrome is `#141414`: the
panel is lighter than the shell around it, which is what makes the inset card
read as a card without a shadow. Light inverts the ramp: pure white work
surface, `#f7f7f7` cards, and a `#f3f4f9` title bar.

Every dark preset that does not ship its own `UiColors` is normalized by
`apply_surface_overrides` to the same chrome `#141414`, terminal `#181818`,
and border `#252525`, so the shell stays identical while the ANSI palette
changes. Light presets keep their own surfaces.

Diff colors on a dark theme fall back to Paneflow's canonical green and red
with opaque row washes unless the preset sets `use_theme_diff_washes`; only
Vercel does. Status hues are functional and MUST NOT be recolored to match a
brand when doing so weakens the meaning.

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
| Unfocused pane dim | terminal background at 0.3 | same | `unfocused_pane_opacity`, default 0.7 |
| Attention border | `vc_conflict` at 0.7 | same | `pane.rs` |
| Icon button hover | the caller's hover color from 0 to 1 | same | `icon_button_sm`, `icon_button_md` |

Fixed values that deliberately do not follow the theme. They read as OS
controls or as system semantics rather than as brand, and are **Contextual**
to the surfaces named:

| Value | Where | Why |
| --- | --- | --- |
| `#339cff` | Toggle track when on | Platform toggle blue |
| `#ff453a` | Destructive button | System red, white label |
| `#007aff` | Pane and sidebar drop target, Paneflow terminal cursor | System blue for drag affordances |
| `#fbbf24` | Sidebar bell when an agent needs input | Amber request signal, identical in every preset |
| `#83c3ff` | Sidebar dot when an agent finished | Light blue completion signal, identical in every preset |
| `hsl(40 85% 55%)`, `hsl(0 62% 56%)` | Callout warning and error | Severity hues independent of preset |
| `#232323` / `#ffffff` | Settings card fill | Card sits one step above `base` in either lightness |

### 4.4 Geometry

| Element | Radius | Corner | Border |
| --- | --- | --- | --- |
| Window | 10 | round | 1 px `border` on free edges |
| Main panel | 10 | round, masked | none |
| Pane card | 20 | squircle | 1 px `border`, or `vc_conflict` at 0.7 with attention |
| Settings card, System Info dialog, pane palette ground | 20 | squircle | none |
| Menu, select popup | 18 | squircle | 1 px `border` at 0.6 |
| Sidebar rows and tab icon cards, footer mode buttons, row skin, secondary button, menu item, tooltip | 14 | squircle | tab icon card and tooltip, 1 px `border` |
| Theme tile | 10 | round | 2 px `text` at 0.12, 0.32 on hover, 0.85 when selected |
| About dialog | 10 | round | 1 px, plus a shadow; **Migration** |
| Sidebar update banner, filter field, settings control, select trigger, title bar menu trigger | 8 | round | none |
| Toast, composer, drop overlay, drop placeholder | 8 | round | drop overlay 2 px blue |
| Toast action button, About close button, theme mockup inner frame | 7 | round | none |
| Toolbar pill, sidebar IPC banner, sidebar hover action button, sidebar branch chip, launch pad field | 6 | round | IPC banner 1 px `border` |
| Title bar sidebar toggle, launch pad primary button | 5 | round | none |
| Icon button, composer chip | 4 | round | none |
| Scrollbar thumb, header chip, filter clear | 3 | round | none |

Squircle means `squircle_fill` and `squircle_border` from
`src-app/src/ui_primitives/squircle.rs`, applied through `squircle_skin`,
`setting_card`, `menu_surface`, or `tooltip_shell`. Plain `rounded()` is for
controls at 10 px or below, where the superellipse is invisible.

### 4.5 Spacing and sizes

| Measure | Value |
| --- | --- |
| Panel inset | 4 |
| Pane gutter | 8 |
| Pane content inset | 10 horizontal, 6 vertical |
| Pane header | 28 content plus the vertical inset twice, 40 total; gap 7 |
| Sidebar row | margin 8, padding 8 by 6, gap 4, line height 18, spacing 4 |
| Sidebar tab icon stack | 16 px icons, cap 4, overlap 11, 24 by 24 icon card |
| Sidebar action button | 20, gap 4; status slot 48; icon slot 20 |
| Sidebar footer | padding 6 top and 8 bottom; mode buttons 30 tall on squircle 14, gap 3, margin 8; gear 30 by 30; banners margin 6 with 2 below, update banner 30 tall with padding 8 |
| Sessions row | height 30; 5 rows per agent group before Show all |
| Settings row | padding 12 by 10, gap 16; section header bottom padding 8 |
| Select trigger | padding 10 by 6, width 190 to 260 |
| Menu | list padding 4, item gap 1, item height 28, width 200 to 280, max height 320 |
| Toggle | track 36 by 22, knob 18 |
| Icon buttons | small 20 outer with 12 icon, medium 24 outer with 13 icon |
| Toolbar pill | height 24, padding 8, gap 5 |
| Filter field | padding 10 by 6, gap 6, 13 px search icon, 16 px clear button with a 10 px glyph |
| Toast | inset 18, padding 12 / 14 by 11, minimum width 220, action buttons 26 tall |
| Scrollbar | width 6, gutter 10, minimum thumb 24, inset 2 |
| Diff | row 18, file header 32, fold row 32, sticky header 24, gutter 36, change bar 4, split divider 3, column header 30, minimum split column 360 |
| Review terminal panel | 520 default, 120 to 1000 |
| Code editor | 12 px mono, caret 2, scrollbar 6, minimum thumb 28 |

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

| Size | Use |
| --- | --- |
| 10 | Filter clear glyph |
| 11 | Sidebar agent state glyphs (bell, error, stalled) and the thinking matrix |
| 12 | Small icon button, select chevron, drag ghost |
| 13 | Medium icon button, filter search, preset logo, menu check mark |
| 14 | Title bar sidebar toggle, editor logos, sidebar folder, sidebar footer banners and gear |
| 15 | Toast icon |
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
| Toast | 180 ms in, 1440 ms hold, 180 ms out | ease-in-out | 8 px lift on entry, 8 px drop on exit |
| Status spinner | 1 s loop | linear rotate | Sidebar update banner while downloading or installing, empty states |
| Sidebar thinking matrix | 720 ms cycle | stepped | 3 by 3 dots of 3 px, gap 1, trailing opacities 0.81, 0.49, 0.26 over a 0.06 base |
| Sidebar update banner shimmer | 2600 ms loop | linear | Five-stop iris ramp sweeping the version label letter by letter, only while an update is available and idle |
| Startup splash | 2600 ms shimmer, 900 ms minimum on screen | linear | Letters at 0.54 alpha, shimmer to 0.82 |
| Tooltip | 800 ms delay | none | `delayed_tooltip` |

`reduce_motion` (Settings, Appearance) is honored in two places today:
`animated_hover` settles instantly and the primary sidebar toggles without
the slide. The dim fade, toasts, spinners, the thinking matrix, and the
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
still carries its own update and IPC pill code, but the cockpit shell never
renders it (`tb.cockpit = true` in `main.rs`); that code is **Migration**,
and the sidebar footer owns both banners. On Windows the caption glyphs are native Windows 11
shapes; on macOS the traffic lights get 80 px of brand padding. The title bar
draws no bottom hairline inside the cockpit shell; the panel inset separates
it from the content.

### 5.2 Primary sidebar, Agents mode

Header row `Workspaces` at label size with two 20 px icon buttons (the
Customize Sidebar menu behind a filter glyph, and new workspace behind a
folder-plus glyph). A workspace is a folder row; its tabs are child rows with
inline rename, hover actions, and reorder by drag. A tab row shows the tab
title, the branch with its glyph, the diffstat in `vc_added` and
`vc_deleted`, and a stack of agent icons capped at four with an 11 px
overlap. Agent status lives in a 48 px slot with 11 px glyphs: an amber bell
when the agent needs input, a light blue 7 px dot when it finished, the
muted dot matrix while it thinks, an `agent_error` circle-x when it failed,
and an `agent_stalled` triangle when it stalled. The bell and the dot use the
fixed colors from 4.3. A `Customize Sidebar` menu on the rail header toggles
branch, diffstat, pull request, and indent guide per value.

Drop placeholder while dragging: margin 6, radius 8, blue at 0.10 with a 0.22
border and a 2 px line.

The footer stacks, top to bottom: the IPC offline banner when the socket is
disabled (margin 6, padding 8 by 6, radius 6, 1 px `border` on `subtle`, a
14 px alert glyph and `IPC offline` at 12 px Medium), the update banner when
a release is available (margin 6, height 30, padding 8, radius 8, the active
row tint, a 14 px download, refresh, tool, or spinning loader glyph, the
label at 12 px with the version shimmering through the iris ramp, a 13 px
bold `×` to dismiss; 0.7 opacity while busy, 0.8 rising to 1.0 on hover for
a package-manager hint), then the mode row: `Agents` and `Review` as two
flexible 30 px squircle buttons at small text Medium with a 3 px gap, and a
30 by 30 gear that opens Settings in one click. The active mode and an open
Settings share the active row tint; the others take the hover tint.

### 5.3 Pane card

A 20 px squircle filled with the terminal background, 1 px `border`. The
header is 40 px: the surface title centered at 14 px, a 6 px status dot, 9 px
chips for progress and worktree, and on the right 22 px action buttons that
stay visible at rest (split vertical, split horizontal, the sessions rail,
the diff dock), a `Z` chip on `accent` when the pane is zoomed, plus a 15 px
close button that appears only while the header is hovered, its glyph fading
from 0.16 to 0.92 under the pointer. The identity
pill was removed in 0.9; the sidebar carries identity, the pane carries
title and state.

State layers, painted in this order: card fill, content, dim layer, drag
overlay, broadcast stripe (3 px of the group color, inset by the radius top
and bottom), border, composer. Unfocused panes in a multi-pane workspace fade
under a 0.3 overlay by default. The drop overlay is blue at 0.10 with a 2 px
blue border, radius 8, margin 8, and a swap variant with its own tint.

### 5.4 Diff dock and Review view

The dock attaches to a tab and opens on a surface picker (three cards, 122 by
98). Tabs are chips with the sidebar rail skin: no separators, active chip on
the active tint, inactive chips wash in on hover. The Review view uses the
same `DiffElement` as the dock: a `Unified | Split` segmented pill, a
`Collapse all` action, a breadcrumb of Project, folder, and base branch, and
a `Review` button that opens an agent under the diff with a staged prompt the
user must send. Change bars are 4 px, dashed for deletions; file headers are
32 px rows that collapse to a 24 px sticky header while scrolling, with the
file-type icon on the left and the diffstat right-aligned.

### 5.5 Settings

Navigation reuses the sidebar width: `Back to the app`, a search field, and
three groups labeled Personal, Terminal, Integrations. Pages are a centered
column with a 26 px heading, eyebrow labels at 11 px muted, and cards
(squircle 20, `#232323` dark or white light). Rows are `toggle_row` or a
`setting_text` plus control: title 12 Medium, description 11 muted, control
right-aligned. Toggles are 36 by 22 with an 18 px white knob. Selects open a
`select_menu` under the trigger. Destructive actions use the fixed red
button. The Appearance page leads with three theme tiles (System, Light,
Dark, 134 tall, radius 10, 2 px border) holding a mockup painted from the
preset, a live diff sample, then the preset select and the preferences.

### 5.6 Menus, selects, tooltips

All popups share `menu_surface`: squircle 18, lifted surface, 0.6 border.
Items are 28 px squircle rows at radius 14 with `text` washes for hover and
selection, 12 px text, a 12 px chevron on triggers, and a 13 px check on the
selected item. Widths run from 200 to
280 and the list scrolls past 320 px. Tooltips are squircle 14 on the title
bar color, padding 8 by 6, small text, shown after 800 ms through
`delayed_tooltip`.

### 5.7 Launch Pad, Composer, palette

Launch Pad is the one modal with a filled accent button: 520 wide, radius
10, an `Agent` list with disabled rows marked `not installed`, a `New
branch` field, an optional `Prompt` field, and a footer hint
`Enter creates · Tab switches fields · Esc cancels`. The Composer dims the
whole pane under a 0.25 black scrim and docks a bordered panel at the bottom:
a `Composer` label at 11 px Medium, then 10 px chips on radius 4 for the
broadcast toggle (`Single pane` on `subtle`, or `Broadcast: group` on
`accent` at 0.15), `agent generating - Enter queues` on `vc_modified` at
0.15 while the agent is busy, and a cancel chip when prompts are queued;
Enter submits, Escape closes. The pane palette fills an empty tab named
`New pane` with a centered 260 px column: a 13 px Semibold title, an
optional branch row, one 34 px row per preset with its 14 px agent mark and
a `not installed` marker at 10 px where the binary is missing, and an inline
error in `vc_deleted` at 11 px.

### 5.8 Feedback

Toasts stack bottom right on `subtle` with a 15 px icon, 12.5 px text, and
26 px action buttons on `text` at 0.08 to 0.12. Error messages are detected
and get the error icon. Callouts (`widgets/callout.rs`) are 16 px icon, 14
Semibold title, 13 muted description, with `accent` for info and the fixed
warning and error hues. Empty states (`panel_empty_state`) center an 18 px
muted glyph, an optional 14 Semibold title, and a 12 px muted message; the
glyph spins while scanning.

## 6. Interaction

### 6.1 Keyboard first

`secondary` maps to Cmd on macOS and Ctrl elsewhere. Every overlay has a
binding, every binding is remappable in Settings, Keyboard Shortcuts, and
every modal answers Enter and Escape; Launch Pad also moves between fields
with Tab.

| Surface | Default |
| --- | --- |
| Split horizontal, vertical | `secondary-shift-d`, `secondary-shift-e` |
| Focus across the grid | `alt-arrow` |
| New tab, next, previous | `secondary-shift-t`, `secondary-]`, `secondary-[` |
| Workspaces 1 to 9 | `secondary-1` to `secondary-9` |
| Diff view | `secondary-shift-g` |
| Files rail | `secondary-alt-f` |
| Composer | `secondary-shift-space` |
| Launch Pad | `secondary-shift-l` |
| Attention queue | `secondary-shift-a` |
| Broadcast groups, toggle member | `secondary-shift-m`, `secondary-shift-b` |
| Jump to next waiting agent | `secondary-shift-j` |
| Layout presets | `secondary-alt-1` to `secondary-alt-4` |

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

Focus is shown by absence of dim: the focused pane stays at full contrast
while its siblings fade. An agent that needs the user gets the
`vc_conflict` border at 0.7 and a sidebar dot; clicking anywhere in the panel
acknowledges visible completions. The attention queue lists those panes and
`secondary-shift-j` jumps through them. Native OS notifications fire only
while the window is unfocused.

## 7. Platform Materials

| Platform | Backdrop | Sidebar | Terminal | Chrome |
| --- | --- | --- | --- | --- |
| Windows 11 build 22621 and later | Mica by default (`window_backdrop: auto`), blur or transparent by choice | Reveals the backdrop when `windows_chrome_material` is on | Transparent default background when `windows_terminal_material` is on, masked to the panel | Native caption glyphs in light and dark |
| Windows 10 and older 11 | Opaque | Opaque card | Opaque | Same glyphs |
| macOS | Transparent window surface, material dropped in fullscreen | AppKit Sidebar material when `macos_chrome_material` is on | Opaque | Traffic lights with 80 px brand padding |
| Linux | Opaque shell; a blur region is requested from the compositor on Wayland through the ext background-effect or KDE protocols, and on X11 under KDE | Opaque, tinted from the title bar color | Opaque | Client-side decorations with GPUI's generic glyphs, or server-side when `window_decorations: server` |

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

- Drop shadows on chrome, cards, rows, menus, or toasts. Only the window and
  the drag ghost cast one; the About dialog's shadow is Migration, not a
  precedent.
- Separators between tabs, chips, or toolbar buttons. The floating chip
  language replaced full-height bordered tabs in 0.5.5.
- Identity pills, badges, or logos in the pane header. The sidebar owns
  identity.
- Accent fills on anything larger than a button. The Launch Pad primary
  button is the ceiling.
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
`toolbar_pill`, `filter_pill`, `section_eyebrow`, `panel_empty_state`,
`text_tooltip`, `delayed_tooltip`. In `src-app/src/settings/components.rs`:
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
- `reduce_motion` stops hover interpolation and the sidebar slide only; the
  other animations listed in 4.8 ignore it.
- The About dialog paints its own grays (`#202020`, `#232323`, `#252525`,
  `#343434`) with a border and a shadow instead of `UiColors`. It is
  **Migration**; the next touch moves it onto the squircle card and the
  semantic roles, as System Info already is.
- `tool_card_header_bg` is defined by every preset but consumed by nothing.
