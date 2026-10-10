# Third-party notices

Paneflow is GPL-3.0-or-later. Components below carry their own license, which
travels with the code they cover. Native archive components are listed
separately in
[native/libghostty/THIRD_PARTY_NOTICES.md](native/libghostty/THIRD_PARTY_NOTICES.md).

## Unpeel terminal palette and runtime catalog model

The ANSI palette, foreground, background, cursor, and selection values of the
bundled `Tailwind Dark` and `Tailwind Light` themes in
`src-app/src/theme/builtin.rs` are taken from Unpeel
(https://github.com/unpeel-com/unpeel), file
`clients/native/UnpeelNative/Sources/UnpeelNative/Theme.swift`. The chrome
values of those two themes (`UiColors`, scrollbars, title bar, link text,
syntax palettes) are Paneflow's own.

The strict runtime descriptor schema, generated registry model, and provider-neutral screen fixtures under `runtimes/` are adapted from Unpeel's runtime catalog at commit `443877b`. Paneflow's descriptor shape, socket integration, platform policy, and application model are its own.

The background agent markers in
`crates/paneflow-ai-hook/src/background.rs` and the marker paths in
`crates/paneflow-ipc-client/src/ai_hook.rs` are ported from Unpeel's
`crates/unpeel-core/src/hook_assets/background.rs` and
`runtimes/claude-code/assets/hooks/lifecycle.sh::record_subagent_activity`:
the identity validation, the per-generation directory and the atomic create on
start follow that design. Paneflow's launch binding, socket announce and
Windows binary hook vehicle are its own.

The foreground runtime matcher in `crates/paneflow-host/src/runtime_observer.rs`
and the screen change tracker in `crates/paneflow-host/src/viewport_scan.rs`
are ported from Unpeel's `crates/unpeel-core/src/runtime_observer.rs` and the
`ScreenChangeTracker` of `crates/unpeel-core/src/session_host.rs` at commit
`443877b`. The leader-first wrapper-aware runtime matcher follows that design.
Paneflow's manifest fields, scan scheduling, Windows foreground implementation
and Controller projection are its own.

```
MIT License

Copyright (c) 2026 UX Themes AS

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Hugeicons (chrome icons)

These files under `src-app/assets/icons/` are the free Hugeicons stroke set
(https://hugeicons.com), taken from `@hugeicons/core-free-icons`. Their path data
is unchanged; only the wrapping `<svg>` attributes were normalized to the shape
the GPUI asset loader expects.

- `arrow_left.svg`
- `clock.svg`
- `close.svg`
- `detach-pane.svg`
- `dock-pane.svg`
- `file-text.svg`
- `filter-2.svg`
- `filter-circle-outline.svg`
- `folder-open.svg`
- `folder-plus.svg`
- `folder.svg`
- `folders.svg`
- `git-branch.svg`
- `git-merge.svg`
- `git-pull-request.svg`
- `layout-grid.svg`
- `layout-sidebar-right.svg`
- `maximize.svg`
- `minimize.svg`
- `plus.svg`
- `pointer-2.svg`
- `sessions.svg`
- `settings.svg`
- `sidebar.svg`
- `sparkles.svg`
- `split_horizontal.svg`
- `split_vertical.svg`
- `square-slash.svg`
- `tool_search.svg`

```
MIT License

Copyright (c) 2024 Halal Labs

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
