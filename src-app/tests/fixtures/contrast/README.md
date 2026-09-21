# Contrast corpus fixtures

Raw ANSI byte streams replayed through libghostty by the `contrast` tests in
`src-app/src/terminal/element/mod.rs` (module `golden_frame_tests`). Each file is
fed to a real `DisplayTerminal`, published through `CellMirror`, and laid out by
`layout_from_snapshot`, so the corpus exercises the production parser, the cell
mirror and the row layout rather than hand-built `Cell` values.

The tests normalize line endings before feeding (every `\r` dropped, every `\n`
sent as `\r\n`), so the checkout's end-of-line policy cannot change the parsed
grid.

## Provenance

| File | Source |
|---|---|
| `lsd-la.ansi` | `lsd -la` output shape with lsd's default `theme.yaml` color indices: permissions 40 / 192 / 124 / 245, links 13, user 230, group 187, size 229 / 216 / 172, dates 40 / 36, directories 27, symlinks 44. Authored from that documented palette, not captured from a run: `lsd` is not installed on the development host. |
| `btop.ansi` | btop box layout with `graph_symbol = block`, truecolor labels and meters in btop's default theme family (`#eaeaea` text, `#666666` dim, `#5fba7d` / `#f9d198` / `#f55757` gauges, `#427481` box frames). Block graph symbols, not braille: braille graphs land with the decorative exclusion in US-005. |
| `lazygit.ansi` | lazygit panel layout using its default named-ANSI theme (green branch, bold white selection, index 1 / 2 / 3 file status, cyan key hints). All foregrounds are below index 16, which is why this fixture reports `indexed=0` in the share golden. |
| `agent-cli.ansi` | A coding agent transcript shape: brand accent `#d97757`, tool headers in `#8b5cf6` / `#3b82f6` / `#6366f1`, diff lines in `#ef4444` / `#22c55e`, status in `#10b981` / `#f59e0b`, chrome in `#94a3b8` / `#e2e8f0`. Authored from the Tailwind scale those CLIs draw their palettes from, not captured from a run. This is the only fixture whose colors drain enough chroma to fire the US-009 harmony pull, and it fires only on dark presets: without it the pull ships unobserved. |

Re-recording any of these from a live tool is a strict improvement; keep the
grid at 80x16 and the color sources listed above so the goldens stay
comparable.

## Goldens

The corpus feeds two committed goldens in
`src-app/src/terminal/element/golden/`:

- `contrast_corpus_share.txt` - per fixture and preset variant, the number of
  measured text cells, how many are indexed, and how many reach APCA Lc 45 and
  Lc 60, both with the render palette and with the xterm cube formula it
  replaced (`xterm_lc45`, `xterm_lc60`). Since US-006 the same row also records
  the counts with the automatic correction on at its Lc 60 default
  (`acc_lc45`, `acc_lc60`) and how many indexed cells clear Lc 60 once
  corrected (`acc_indexed_lc60`).
- `contrast_corpus_light_indexed_gap.txt` - every indexed cell that still falls
  below Lc 45 on a light preset before any contrast correction runs.
- `contrast_corpus_harmony.txt` - per preset variant and per indexed source, the
  OKLab distance from the corrected color to the nearer of `foreground` and
  `dim_foreground`, the source and realized chroma, and the hue drift.
- `contrast_corpus_pull.txt` - per preset variant and per truecolor source of
  `agent-cli.ansi`, the realized chroma ratio, whether the harmony pull fired,
  and the OKLab distance to its theme target. The pull fires only on dark
  presets, and `#ef4444` straddles the 0.6 threshold across presets, so this
  golden is where a threshold change shows up.

The cube itself is guarded by an invariant rather than a golden: the eight cube
anchors must carry the theme seeds and the grey ramp must run monotonically from
background to foreground on all ten preset variants, asserted in
`src-app/src/theme/palette.rs`.

Regenerate them with `PANEFLOW_BLESS_GOLDEN=1 cargo test -p paneflow-app corpus`
and review the diff: every changed number must map to the palette or preset
change you made.
