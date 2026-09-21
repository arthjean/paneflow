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

Re-recording any of these from a live tool is a strict improvement; keep the
grid at 80x16 and the color sources listed above so the goldens stay
comparable.

## Goldens

The corpus feeds two committed goldens in
`src-app/src/terminal/element/golden/`:

- `contrast_corpus_share.txt` - per fixture and preset variant, the number of
  measured text cells, how many are indexed, and how many reach APCA Lc 45 and
  Lc 60, both with the render palette and with the xterm cube formula it
  replaced (`xterm_lc45`, `xterm_lc60`).
- `contrast_corpus_light_indexed_gap.txt` - every indexed cell that still falls
  below Lc 45 on a light preset before any contrast correction runs.

The cube itself is guarded by an invariant rather than a golden: the eight cube
anchors must carry the theme seeds and the grey ramp must run monotonically from
background to foreground on all ten preset variants, asserted in
`src-app/src/theme/palette.rs`.

Regenerate them with `PANEFLOW_BLESS_GOLDEN=1 cargo test -p paneflow-app corpus`
and review the diff: every changed number must map to the palette or preset
change you made.
