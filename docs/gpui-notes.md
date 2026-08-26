# GPUI notes

Framework-specific knowledge that the code does not make obvious. Read this
before touching rendering, scrolling, focus, or the GPUI dependency pin.
`ARCHITECTURE.md` covers the module layout and thread model; this file only
covers GPUI itself.

## Entity and Element model

- All mutable state lives in an `Entity<T>` and is mutated through
  `Context<Self>`. `cx.new()` creates, `cx.notify()` schedules a repaint,
  `cx.spawn()` runs async work on the main thread.
- Implement `Render` for high-level views (`PaneFlowApp`, `TitleBar`,
  `TerminalView`). It returns a div tree.
- Implement `Element` only for low-level custom painting. `TerminalElement`
  (`src-app/src/terminal/element/mod.rs`) is the sole case. Three phases:
  `request_layout()` then `prepaint()` then `paint()`.
- The `actions!` macro at `src-app/src/app/actions.rs:9` generates zero-sized
  typed action structs in the `paneflow` namespace. Actions dispatch through the
  focus chain.
- Focus: every `TerminalView` owns a `FocusHandle`. The `"Terminal"` key context
  scopes terminal-only keybindings. Navigation is structural (tree traversal),
  not spatial.
- Never reach for `Arc`/`Mutex` for UI state. Use `Rc<Cell<f32>>` for
  single-threaded sharing, for example split ratios captured by render closures.
  The only cross-thread state in the app is the terminal grid.

## Entity re-entrancy

Do not re-read or `.update()` an entity from inside its own callback
(`on_submit`, `on_escape`, `on_change`). Use the value the callback hands you,
and defer mutations with `cx.defer(move |cx| weak.update(cx, ...))`.

## Scroll and wheel

Hard-won from the diff-dock horizontal-scroll work
(`src-app/src/app/diff_dock/mod.rs`), verified against the Zed source. Do not
re-derive these by guessing; three attempts were wrong before these held.

- **Shift+wheel is axis-swapped to X at the platform layer**, before app code
  sees it. X11 (`gpui_linux/.../x11/client.rs::make_scroll_wheel_event`),
  Wayland (`wayland/client.rs`, forces `HorizontalScroll`), and Windows
  (`gpui_windows/events.rs`) all place the value in `delta.x` and zero `delta.y`
  when `modifiers.shift` is set. Read `delta.x` for horizontal and never branch
  on `modifiers.shift`, because `delta.y` reads zero under Shift. On macOS the
  NSEvent delivers horizontal natively, same net effect. The `div.rs`
  `delta_x = delta.y` line is a separate fallback that fires only when
  `delta.x == 0`; it is not the Shift mechanism.
- **`overflow_hidden()` plus `track_scroll()` does not scroll-translate
  children.** It only keeps the handle's bookkeeping (`offset()`, `bounds()`,
  `max_offset()`) live. GPUI pushes the scroll offset onto the element-offset
  stack, which bakes into each child's `bounds.origin`, only when the host
  overflow axis is `Overflow::Scroll`. A custom `Element` that positions content
  off its own `bounds.origin` therefore scrolls only under `overflow_y_scroll`
  or `overflow_scroll`; `set_offset()` under `overflow_hidden` is stored but
  dead. Custom elements pick up the shift automatically through their passed
  `bounds`, with no `window.element_offset()` call.
- **Two-axis recipe** for a vertical list whose items also scroll horizontally,
  the canonical Zed pattern from `data_table.rs`, `thread_view.rs`, and
  `markdown.rs`: host uses `overflow_y_scroll()` plus `track_scroll(&handle)`
  plus `element.style().restrict_scroll_to_axis = Some(true)`. That flag is a
  raw `StyleRefinement` mutation with no builder method, but it compiles because
  non-`#[refineable]` `Style` fields still become `Option<T>`. It stops a
  vertical wheel bleeding into a horizontal child, and stops the native Y
  handler back-filling `delta_y = delta.x` under Shift+wheel. Per-item
  horizontal scrolling stays custom, an `on_scroll_wheel` reading `delta.x`
  only; native owns vertical. See `src-app/src/app/diff_dock/mod.rs:587`.
- A shared `ScrollHandle` is bridled by the shortest scroll container, because
  the clamp writes back. Cross-column scroll sync needs per-column handles plus
  an explicit broadcast.

## Other rendering gotchas

- `uniform_list` masks only the list boundary, not each row. Default text wrap
  makes fixed-height rows overlap, so pair it with `whitespace_nowrap()` and
  `overflow_hidden()`. Use `StyledText::with_highlights` for per-token color.
- `svg()` needs an explicit `text_color`; there is no cascade from the parent,
  so an icon without one renders invisible.
- `drag_over` and `group_drag_over` only fire when a hitbox exists.
  `should_insert_hitbox` ignores drag styles, so a drop overlay needs
  `on_drop`, `.id()`, or `.group()` or it never appears.
- Never register a recursive `notify` watcher on the main thread. A recursive
  `WalkDir` over `target/` blocks the GPUI thread long enough for the compositor
  to mark the window unresponsive. Register watchers and git subprocesses off
  the render thread.

## Styling conventions

- All styling is inline through GPUI's builder API:
  `.bg(rgb(0x181825)).px_3().rounded_md()`. Match the existing builder-chain
  style instead of introducing a separate styling layer.
- Sidebar and title-bar colors are hardcoded dark hex values. They do not follow
  the terminal theme.
- Terminal colors come from `TerminalTheme` (`src-app/src/theme/model.rs:11`),
  resolved through `active_theme()` (`src-app/src/theme/watcher.rs:107`).
- The font defaults to a platform-specific installed monospace fallback at 14px.
  An invalid Linux font name falls back to the first available preferred mono
  family.

## Naming trap

`SplitDirection::Horizontal` (`src-app/src/layout/tree.rs:19`) means a
*horizontal divider bar*, so panes stack top and bottom (`flex_col`).
`Vertical` puts panes side by side (`flex_row`). This is counterintuitive but
consistent throughout the codebase.

## Dependency pin maintenance

`src-app/Cargo.toml` pins `gpui` and `gpui_platform` to an exact revision of
upstream `zed-industries/zed`. There are four `rev` occurrences: the default
dependency (two entries), the Linux `wayland`/`x11` override, and the
`test-support` dev-dependency. There is no Paneflow fork of Zed any more; the
`Markdown::append` patch it carried lost its only consumer when the in-app chat
was removed, and the fork was dropped on 2026-08-26.

`gpui_platform` must keep `features = ["font-kit"]` or macOS renders empty glyph
bitmaps.

To bump: pick an upstream revision, replace every `rev` value with that exact
sha (never a branch, which breaks reproducible builds), run `cargo update`, then
run the full gate set. GPUI's public API changes often enough that a bump is a
real change, not a chore.

Two gates fail for reasons that are not compile errors, so run them explicitly:

- `cargo deny check sources` catches new git sources pulled in through GPUI's own
  dependencies. The 2026-08-26 bump added `zed-industries/scap` and
  `zed-industries/wasm_thread`, both target-gated away from our three platforms.
  Each new source needs a reviewed entry in `deny.toml`'s `allow-git`.
- `cargo test --workspace` catches API breaks in `#[cfg(test)]` code.
  `cargo build` does not compile test targets, so those stay invisible until the
  test build runs. The 2026-08-26 bump broke three test-only call sites this way.

## Settled decisions

- GPUI is not on crates.io and is consumed from the pinned git revision above.
  Never replace it with a crates.io dependency.
- iced was evaluated and rejected: unstable API, and a custom WGPU glyph atlas
  proved too complex. Do not re-propose it.
