# Baseline Alacritty EP-001

Capture du 2026-07-14 sur Linux x86_64, en profil Cargo `release`, avec le seed déterministe `0x50414e45464c4f57`.

```sh
cargo test -p paneflow-app terminal::backend_corpus::alacritty_eight_pane_baseline --release --locked --no-default-features -- --ignored --nocapture
```

| Mesure | Valeur |
|---|---:|
| Panes persistantes | 8 |
| Streams par pane | 100 |
| Entrée totale | 62 400 octets |
| Débit parser | 2,964 MiB/s |
| Entrée vers snapshot p50 | 8 us |
| Entrée vers snapshot p95 | 107 us |
| Verrou p95 | 17 us |
| Durée murale | 20 ms |
| Temps CPU | 10 ms |
| RSS initiale | 8 286 208 octets |
| RSS finale | 22 740 992 octets |
| CPU | AMD Ryzen 7 7800X3D 8-Core Processor |

Sortie brute:

```json
{"seed":"0x50414e45464c4f57","panes":8,"streams_per_pane":100,"bytes":62400,"throughput_mib_s":2.964,"input_to_snapshot_p50_us":8,"input_to_snapshot_p95_us":107,"lock_p95_us":17,"wall_ms":20,"cpu_ms":10,"rss_start_bytes":8286208,"rss_end_bytes":22740992,"cpu_model":"AMD Ryzen 7 7800X3D 8-Core Processor","profile":"release","measurement_scope":"persistent-eight-pane-parser-to-neutral-snapshot"}
```

Cette baseline mesure le chemin parser Alacritty vers snapshot neutre avec huit terminaux persistants. Elle ne mesure pas encore le délai jusqu'à la présentation d'une frame GPUI, donc elle ne satisfait pas à elle seule la gate `input-to-frame` de US-003.

## Scénario end-to-end GPUI

Le scénario suivant conserve huit `Pane` actives dans un `LayoutTree` de production de deux lignes par quatre colonnes. Chaque échantillon injecte le même stream synthétique dans les huit `TerminalView`, invalide les entités, puis attend que le dispatcher GPUI soit parqué. Le chrono s'arrête donc après la construction de la scène, lorsque les huit appels `TerminalElement::paint` ont fini.

```sh
cargo test -p paneflow-app --release --locked layout::render::tests::alacritty_eight_pane_gpui_input_to_paint_baseline -- --ignored --nocapture --test-threads=1
```

| Mesure | Valeur |
|---|---:|
| Panes actives | 8 |
| Streams par pane | 100 |
| Entrée totale | 62 400 octets |
| Débit injection vers paint | 0,047 MiB/s |
| Input-to-frame p50 | 11 962 us |
| Input-to-frame p95 | 15 902 us |
| Acquisitions `render_content` mesurées | 6 720 |
| Durée de verrou `render_content` p50 | 7 us |
| Durée de verrou `render_content` p95 | 7 us |
| Durée murale | 1 257 ms |
| Temps CPU | 1 250 ms |
| RSS initiale | 17 682 432 octets |
| RSS maximale | 26 447 872 octets |
| RSS finale | 26 447 872 octets |
| CPU | AMD Ryzen 7 7800X3D 8-Core Processor |
| Plateforme | Linux x86_64 |
| Profil | Cargo `release` |
| Seed | `0x50414e45464c4f57` |

Sortie brute:

```json
{"seed":"0x50414e45464c4f57","panes":8,"streams_per_pane":100,"bytes":62400,"throughput_mib_s":0.047,"input_to_frame_p50_us":11962,"input_to_frame_p95_us":15902,"render_content_lock_samples":6720,"render_content_lock_held_p50_us":7,"render_content_lock_held_p95_us":7,"wall_ms":1257,"cpu_ms":1250,"rss_start_bytes":17682432,"rss_peak_bytes":26447872,"rss_end_bytes":26447872,"hardware":"AMD Ryzen 7 7800X3D 8-Core Processor","platform":"linux-x86_64","profile":"release","measurement_boundary":"byte injection through GPUI dispatcher parked after TerminalElement::paint","lock_measurement":"all render_content terminal-lock hold durations from the measured GPUI paints","presentation_scope":"GPUI test-platform scene generation; excludes Window::present, GPU submission, compositor, and display scanout"}
```

Cette mesure satisfait la frontière demandée pour US-003: injection des bytes, parsing Alacritty, snapshot neutre, layout GPUI et fin du paint terminal. La sonde démarre après le paint de chauffe et capture les 6 720 durées de possession du verrou produites par les passes `render_content` réelles pendant les 100 cycles mesurés. GPUI exécute plusieurs passes complètes de huit panes avant de parquer son dispatcher; aucune boucle de snapshots artificielle n'est ajoutée après les frames. La mesure ne prétend pas couvrir `Window::present`, la soumission GPU, le compositeur Wayland/X11 ou le scanout écran. La métrique parser-vers-snapshot précédente reste distincte et conserve son nom `input_to_snapshot`.
