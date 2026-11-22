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
is ported from Unpeel's `crates/unpeel-core/src/runtime_observer.rs` at commit
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
- `chat-bot.svg`
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
- `git-branch.svg`
- `git-merge.svg`
- `git-pull-request.svg`
- `layout-grid.svg`
- `layout-sidebar-right.svg`
- `plus.svg`
- `pointer-2.svg`
- `preference-horizontal.svg`
- `settings.svg`
- `sidebar.svg`
- `split_horizontal.svg`
- `split_vertical.svg`
- `square-slash.svg`
- `tool_search.svg`
- every file under `caption/`, the window caption buttons, taken from
  `@hugeicons/core-free-icons` 4.4.0 and named after their Hugeicons icon
- every file under `menu/` except `webstorm.svg`, the action menu glyphs,
  taken from `@hugeicons/core-free-icons` 4.4.0 and named after the menu action
  they mark
- every file under `dock/`, the right dock glyphs, taken from
  `@hugeicons/core-free-icons` 4.4.0 and named after their Hugeicons icon
- every file under `palette/`, the command palette glyphs, taken from
  `@hugeicons/core-free-icons` 4.4.0 and named after their Hugeicons icon
- every file under `settings/`, the Settings sidebar and page glyphs, taken
  from `@hugeicons/core-free-icons` 4.4.0 and named after their Hugeicons icon

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

## WebStorm marks from T3 Code

`src-app/assets/icons/editor-webstorm.svg` is the WebStorm logo as drawn in
T3 Code (https://github.com/pingdotgg/t3code), file
`apps/web/src/components/JetBrainsIcons.tsx`, under the MIT license below. Its
path data and gradients are unchanged; only the gradient ids were renamed.
`src-app/assets/icons/menu/webstorm.svg` keeps the black square and the
letterforms of the same drawing as a single-color mask for the action menu.
WebStorm is a trademark of JetBrains s.r.o.

```
MIT License

Copyright (c) 2026 T3 Tools Inc.

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

## Diff status icons from Pierre Diffs

The files under `src-app/assets/icons/diff/` are icons from `@pierre/diffs`
1.7.0-beta.3 (https://diffs.com), taken from its icon sprite under the Apache
License 2.0. `file-added.svg`, `file-deleted.svg`, `file-modified.svg` and
`file-moved.svg` are the `symbol-added`, `symbol-deleted`, `symbol-modified`
and `symbol-moved` glyphs, `arrow-right-short.svg` and `expand-all.svg` keep
their names, and `expand-down.svg` is the `expand` glyph. Their path data is
unchanged. `expand-up.svg` is the same `expand` glyph mirrored vertically.

```
Copyright 2025 Pierre Computer Company

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

## File type icons from Pierre Trees and T3 Code

The files under `src-app/assets/icons/files/light/` and
`src-app/assets/icons/files/dark/` are generated by
`scripts/generate-file-icons.ts` from a T3 Code checkout at commit `52f0f8af`,
and the name and extension tables in `src-app/src/file_icons/tables.rs` come
from the same sources. Each icon is written twice, once per theme, with the
colors of its source baked in.

- 53 icons and the built-in name and extension tables are the `complete` set
  of `@pierre/trees` 1.0.0-beta.6 (https://diffs.com), file
  `dist/builtInIcons.js`, with the light and dark palette of `dist/style.js`,
  under the Apache License 2.0 of Pierre Computer Company quoted in the section
  above.
- 22 icons and the added name and extension tables are T3 Code's additions
  (https://github.com/pingdotgg/t3code), file `apps/web/src/pierre-icons.ts`,
  under the T3 Code MIT license quoted above. Within them, the video, diff,
  XML, PDF, Word, spreadsheet, slides and EPUB icons are Lucide icons
  (https://lucide.dev), ISC license, Copyright (c) Lucide Contributors 2022,
  and the C#, Dart, Elixir, Haskell, Java, Kotlin, Lua, PHP, PowerShell, R,
  Scala and TOML logos are Simple Icons (https://simpleicons.org), CC0 1.0.
  The `agents` and `pnpm` icons are T3 Code's own drawings. Each logo names
  the file type it marks and remains the trademark of its owner.
- `angular.svg`, `react-native.svg`, `makefile.svg` and `log.svg` are
  Paneflow's earlier icons, kept from `scripts/file-icons/paneflow/` because
  neither source has an equivalent.

```
ISC License

Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part
of Feather (MIT). All other copyright (c) for Lucide are held by Lucide
Contributors 2022.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

## Workspace folder icons

The files under `src-app/assets/icons/workspace/` come from several sources.

- `chevron-right.svg` and `chevron-down.svg` are the free Hugeicons stroke
  rounded chevrons, under the Hugeicons notice above.
- `metal-folder.svg` and `metal-folder-open.svg` are the glass folder marks of
  Unpeel (https://github.com/unpeel-com/unpeel), file
  `clients/native/UnpeelNative/Sources/UnpeelNative/ChromeIcons.swift`, under
  the Unpeel MIT license above. Their outlines are the Phosphor Icons
  (https://phosphoricons.com) `folder-fill` and `folder-open-fill` glyphs, also
  MIT, Copyright (c) 2023 Phosphor Icons.
- `windows-folder.svg` and `windows-folder-open.svg` are the Icons8
  (https://icons8.com) `Folder` and `Opened Folder` icons. Their path data and
  gradients are unchanged; only the wrapping `<svg>` attributes were
  normalized.
- `windows95-folder*.svg` redraw the folder icon of Microsoft Windows 95 as
  SVG rectangles, one per run of pixels. The 32-cell closed folder follows the
  original pixel for pixel; the 16-cell pair and both open folders are
  Paneflow's drawings in the same 16-color palette.
- `pixel-folder.svg` and `pixel-folder-open.svg` are redrawn on a 16-cell grid
  from a third-party pixel icon pair.

## portable-pty

`vendor/portable-pty/` is portable-pty 0.9.0 by Wez Furlong, as published on
crates.io from the wezterm repository (https://github.com/wezterm/wezterm,
commit `f8921727a11b9f8b073e8c24821d72fd41283500`, directory `pty/`). Paneflow
changed `openpty` in `src/unix.rs` to open both ends of the PTY with
`O_CLOEXEC`, and left out the published examples. It is MIT licensed,
Copyright (c) 2018 Wez Furlong; the full text is in
[vendor/portable-pty/LICENSE.md](vendor/portable-pty/LICENSE.md).

## cosmic-text

`vendor/cosmic-text/` is cosmic-text 0.19.0, Copyright (c) 2022 System76, as
published on crates.io from
https://github.com/pop-os/cosmic-text. Paneflow changed `ShapeWord::build` in
`src/shape.rs` to end a shaping run where the script changes inside a word, and
left out the published tests, benches and dev-dependencies. It is dual licensed
under MIT or Apache-2.0; both texts are in
[vendor/cosmic-text/LICENSE-MIT](vendor/cosmic-text/LICENSE-MIT) and
[vendor/cosmic-text/LICENSE-APACHE](vendor/cosmic-text/LICENSE-APACHE).

## Pierre highlights lexers, the wasm2c runtime and SIMDe

`crates/paneflow-highlights/c/` is written by
`scripts/sync-pierre-highlights.sh` (or `.ps1`) from inputs pinned by hash in
`crates/paneflow-highlights/c/MANIFEST.toml`. The notices of all three parts
are in [crates/paneflow-highlights/c/NOTICE](crates/paneflow-highlights/c/NOTICE).

- `c/generated/highlights.c` and `highlights.h` are the 152 lexers of the npm
  package `@pierre/highlights` 0.5.2 (https://github.com/pierrecomputer/pierre,
  `packages/highlights`), Copyright Pierre Computer Company, translated from its
  `dist/highlights.wasm` to C by wasm2c 1.0.42, and
  `crates/paneflow-highlights/src/tables.rs` holds its language and token
  tables. Apache License 2.0; the full text is in
  [crates/paneflow-highlights/c/generated/LICENSE](crates/paneflow-highlights/c/generated/LICENSE).
- `c/wasm2c/` is the wasm2c runtime of WABT 1.0.42
  (https://github.com/WebAssembly/wabt), Copyright the WebAssembly Community
  Group participants. Apache License 2.0; the full text is in
  [crates/paneflow-highlights/c/wasm2c/LICENSE](crates/paneflow-highlights/c/wasm2c/LICENSE).
- `c/simde/wasm/simd128.h` is the amalgamated header of SIMDe 0.8.2
  (https://github.com/simd-everywhere/simde, commit
  `71fd833d9666141edcd1d3c109a80e228303d8d7`), Copyright Evan Nemerson and
  contributors. MIT License; the full text is in
  [crates/paneflow-highlights/c/simde/LICENSE](crates/paneflow-highlights/c/simde/LICENSE).

The shim under `c/shim/` and the Rust crate around it are Paneflow's own.
