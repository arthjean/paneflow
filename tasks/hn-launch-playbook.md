# Playbook lancement Hacker News — Paneflow

Préparé le 2026-06-12. Le repo est prêt côté contenu (voir §0). Ce qui reste
avant de poster est dans §1 — le GIF démo est le seul vrai bloquant.

## 0. Fait le 2026-06-12 (référence)

- Release notes v0.4.4 backfillées (étaient vides — mauvais signal pour le trafic /releases).
- Social preview régénérée avec le logo v0.4.2 (même composition) ET uploadée sur GitHub
  (vérifiée côté serveur — usesCustomOpenGraphImage, nouveau hash). Asset committé :
  `assets/images/social-preview.png`. → §1.2 social preview : FAIT.
- 4 issues communautaires : #7 Catppuccin (good first issue), #9 DECSCUSR cursor shapes,
  #10 Windows testers, #11 macOS feedback (help wanted — macOS ship signé depuis
  plusieurs versions mais Arthur développe sur Linux ; l'issue sonde aussi la demande
  Intel via 👍). → §4 : FAIT, reste à y répondre en < 24 h quand quelqu'un mord.
  (#8 audible bell créée puis SUPPRIMÉE le 2026-06-12 — veto Arthur : redondante avec
  le routage d'attention qui est le cœur du produit ; ne pas re-proposer.)
- `tasks/record-demo.sh` : chorégraphie IPC du scénario GIF (workspace démo + Claude/Codex
  préremplis via surface.split prompt). Tu lances Kooha, tu exécutes, tu presses Enter on camera.
- README : barre de navigation, nudge star post-Quickstart, section Architecture, FAQ anti-objections HN (tmux, Electron, télémétrie, fork Zed/cmux, headless, GPL, Windows, qualité du terminal).
- `ARCHITECTURE.md` public : thread model, pipeline keystroke→pixel, boundary alacritty, lifecycle agents (shim+hooks), IPC/MCP, self-update fail-closed, télémétrie opt-in, stratégie cross-platform. C'est l'aimant pour le lectorat HN technique.
- Description GitHub réécrite (bénéfice + stack + plateformes).
- `devto-v0.3.4-hardening.md` sorti de la racine trackée (cruft public).
- Déjà en place avant : social preview custom, 20 topics, discussions activées, templates issues/PR, CONTRIBUTING, SECURITY.

## 1. Bloquants avant de poster

> **MAJ 2026-06-12 (round 4) - hero = vidéo finale d'Arthur :**
> Le hero du README est désormais `assets/images/demo.gif` (8,39 MiB, 960x540, accéléré 2,8x)
> dérivé de la vraie démo d'Arthur (`tasks/demo-source-2026-06-12.mp4`, 105s 1080p+audio).
> Social preview remplacée par l'export Figma d'Arthur (`assets/images/social-preview.png`,
> redéployée sur GitHub settings). Avant publication, vérifier visuellement que le pill
> de l'asset dit bien "Open source" (ne pas bricoler l'asset de marque à la main).
> **Lecteur vidéo natif (son + contrôles) :** GitHub n'embarque une vraie balise `<video>`
> que via une URL `user-attachments/assets/...`, générée uniquement par drag-drop dans
> l'UI web (éditeur d'issue/README), authentifiée par cookie de session — donc PAS faisable
> via `gh`/CLI ni via une URL raw (Content-Type octet-stream → pas de lecture). Pour upgrader
> le GIF en lecteur : ouvrir un éditeur markdown GitHub, glisser `tasks/demo-source-2026-06-12.mp4`,
> récupérer l'URL générée, et la mettre dans `<video src=... controls muted width="100%">`.
> Un essai a marché (URL ec5a7789) mais portait l'ancienne mauvaise vidéo, donc remplacé par le GIF.
>
> **MAJ 2026-06-12 (round 3 historique, remplacé par round 4) :**
> - **GIF démo : ancienne itération remplacée** (`assets/images/demo.gif` pointe maintenant
>   vers le GIF round 4 décrit ci-dessus ; l'ancien GIF n'est plus le fichier courant,
>   commit 788819b). Tourné via un harnais headless : sway+pixman dans un conteneur podman
>   (XDG_RUNTIME_DIR partagé), Paneflow 0.4.4 hôte en lavapipe, chorégraphie IPC
>   (surface.split command+env, soumission par `\r` séparé — le submit:true est avalé par
>   le bracketed paste des TUI agents), capture wf-recorder, ffmpeg palettegen.
>   Contenu : session réelle, Claude Code implémente le thème Catppuccin (issue #7) pendant
>   que Pi explique le shim — statuts sidebar live, zéro erreur à l'écran.
>   Source MP4 : `tasks/demo-source-2026-06-12.mp4` (pour le site/X/LinkedIn).
>   Si tu veux re-tourner une version à la main : `tasks/record-demo.sh`.
> - **Quickstart testé sur conteneurs frais** : Ubuntu 24.04 (deb install ✅, apt repo wiré ✅,
>   AppImage --appimage-extract-and-run ✅, résolution VER ✅) et Fedora 42 (rpm --import ✅,
>   checksig "digests signatures OK" ✅, dnf install ✅, repo wiré ✅).
>   **Bug trouvé et corrigé** : dpkg-sig n'existe plus dans Ubuntu 24.04 → README et
>   linux-signing.md basculés sur le fallback `ar x … && gpg --verify` (commit 788819b).
> - Social preview : FAIT (round 2). Issues seedées #7-#10 : FAIT (round 2).
>
> **Il ne reste plus que le post HN lui-même (§2) — ton compte, ton timing.**

### 1.1 GIF démo (asset n°1, non négociable)

Un PNG dense ne convertit pas ; un GIF de 20s qui raconte une histoire, oui.

**Spec :**
- 1600×1000 (ou 1280×800), 15-25 s, < 10 Mo pour le README (GitHub limite 10 Mo).
- Scénario (une seule prise, rythme soutenu) :
  1. Launch Pad (`Ctrl+Shift+L`) → worktree + split + Claude Code lancé avec premier prompt pré-rempli.
  2. Deuxième agent (Codex) lancé dans un pane voisin → les deux spinners tournent en parallèle.
  3. Un agent pose une question → dot ambre sur le tab + notification desktop avec la question.
  4. `Ctrl+Shift+J` → warp direct sur l'agent qui attend, réponse.
  5. Fin : vue d'ensemble des panes + diff viewer 2 s.
- Thème One Dark, fonte par défaut, fenêtre propre (pas de workspaces persos visibles).
- Outils Fedora/Wayland : Kooha ou `wl-screenrec` → `gifski --fps 12 --quality 80` (meilleur ratio qualité/poids que ffmpeg palettegen).
- Garde aussi le MP4 source : site, X, LinkedIn.

**Intégration :** le README pointe déjà vers `assets/images/demo.gif` (garder
`assets/images/hero-paneflow.png` comme image statique / fallback social preview).
Le GIF reste dans le repo pour survivre aux forks/mirrors.

### 1.2 Vérifs 24 h avant

- `paneflow.dev` : page d'accueil à jour avec la version courante, le lien GitHub au-dessus de la ligne de flottaison (le trafic HN rebondit vite).
- Social preview GitHub : régénérée avec le logo v0.4.2 ? (Settings → Social preview ; `assets/images/social-preview.png` date du 1er juin, le logo a changé le 10 juin → re-uploader si besoin.)
- Tester le Quickstart AppImage ET le .deb sur une VM Ubuntu 24.04 fraîche — le commentaire HN le plus dommageable est « the install command in the README doesn't work ».
- Latest release avec notes complètes (fait pour v0.4.4 ; si tu release d'ici là, soigner les Highlights).

## 2. Le post

**Type :** Show HN. **URL : le repo GitHub**, pas paneflow.dev — pour un projet
OSS, HN convertit mieux sur le code, et le README fait le travail de la landing.

**Titre recommandé :**

> Show HN: Paneflow – A native terminal workspace for running coding agents in parallel

Variantes si tu préfères ancrer la stack (HN aime "in Rust") :

> Show HN: Paneflow – Run Claude Code, Codex and other CLI agents in parallel (Rust, GPUI)

À éviter : superlatifs ("blazing fast", "the best"), le mot "multiplexer" en
titre (positionnement = cockpit d'agents), toute mention d'IA générique type
"AI-powered".

**Timing :** mardi, mercredi ou jeudi, 14:00-15:30 UTC (16:00-17:30 chez toi =
matin côte Est + lève-tôt côte Ouest). Jamais le weekend, jamais un jour de
grosse actu (keynote Apple, release OpenAI…).

**Premier commentaire (à poster dans les 2 minutes, depuis ton compte) — draft :**

> Hi HN, author here.
>
> Paneflow started because I was running Claude Code and Codex side by side in
> tmux and kept missing the moment an agent stopped to ask me a question. The
> sessions were fine; my attention routing wasn't. So I built a terminal
> workspace whose whole job is knowing what the agents in its panes are doing:
> thinking, waiting on input (and what the question is), finished, errored, or
> stalled — surfaced as tab dots, an attention queue, and desktop
> notifications that carry the actual question.
>
> Technical bits HN might care about: it's native Rust on Zed's GPUI (Vulkan /
> Metal / DirectX — no Electron), terminal emulation is upstream
> alacritty_terminal behind a neutral-type boundary, one PTY thread per pane,
> and a JSON-RPC IPC layer that powers a CLI (`paneflow up` builds an agent
> workspace from a declarative spec), an MCP bridge (agents can read other
> panes' output, read-only), and the lifecycle tracking. Updates are
> minisign-signed and verification fails closed. Telemetry is opt-in and off
> by default. More in ARCHITECTURE.md.
>
> Design constraint I won't compromise on: agents run as real CLI processes
> in real PTY panes, and a human presses Enter. Paneflow pre-fills prompts
> (composer, broadcast groups, launch pad) but never submits on your behalf.
>
> Honest limitations: Linux, macOS Apple Silicon, and Windows x64 today; Windows
> ARM64 and Intel macOS are currently out of the build matrix. GPL-3.0.
>
> I'd especially like feedback from people running 3+ agents in parallel: what
> does your attention routing look like, and what's missing here?

**Pendant les 8 premières heures :**
- Réponds à TOUT, vite, factuel, zéro défensive. Sur une critique fondée :
  « you're right, tracking issue here: <lien> » vaut 50 upvotes.
- Questions prévisibles : pourquoi pas tmux (réponse = la FAQ), télémétrie
  (opt-in, off par défaut, 5 events, kill switch), GPL vs MIT, "yet another
  AI terminal", perfs/RAM vs Electron, Wayland vs X11, pourquoi GPUI et pas
  iced/egui (réponse honnête : iced évalué et rejeté, atlas de glyphes custom
  trop complexe).
- Ne JAMAIS demander d'upvote à qui que ce soit, nulle part (ni LinkedIn, ni
  DM). Le vote-ring detection de HN flag le post et c'est terminal. Tu peux
  partager le lien « je suis sur HN aujourd'hui » SANS demander de vote.

**Si flop (< 10 points en 3-4 h) :** pas grave, c'est courant. Droit moral de
re-poster sous un autre angle dans 3-4 semaines ; tu peux aussi écrire à
hn@ycombinator.com pour demander le second-chance pool (ils le font
volontiers pour les Show HN de qualité).

## 3. Distribution autour du lancement

Échelonner — pas tout le même jour (chaque canal mérite son pic).

| Canal | Quand | Note |
|---|---|---|
| X/Twitter | jour J, après le post HN | Thread technique : GIF + 3-4 tweets de chirurgie technique (grille Aiden Bai). Lien HN en réponse, pas en tweet 1 |
| LinkedIn | jour J+1 | EN (launch post). Format feature-post : direct, zéro storytelling |
| r/rust | J+3 | Angle "built with GPUI outside Zed" — les Rustacés s'intéressent au framework. Lire les règles du sub avant |
| r/commandline, r/ClaudeAI | J+7, un seul à la fois | Adapter le post au sub |
| This Week in Rust | semaine du lancement | PR sur la section project updates |
| awesome-claude-code, awesome-ai-agents, awesome-rust | J+7 | PRs one-liner, lire les CONTRIBUTING de chaque liste |
| alternativeto.net | quand tu veux | Créer la fiche (alternative à tmux, Warp, iTerm2, cmux) |
| Product Hunt | optionnel, J+30 | Seulement si tu as l'énergie d'animer une 2e journée de lancement |

## 4. Issues à seeder (signal de vitalité)

Un repo à 0 issue ouverte ressemble à un repo mort. Avant le lancement, ouvre
3-4 issues `good first issue` / `help wanted` réelles (drafts ci-dessous, à
poster sous ton compte) :

1. **`good first issue` — New bundled theme: Catppuccin Mocha** — "The theme
   system supports hot-reload and ships One Dark + PaneFlow Light
   (`src-app/src/theme/builtin.rs`). Catppuccin is the most-requested palette
   family in terminal land; adding it is a contained, well-documented change."
2. **`help wanted` — Cursor shape escape sequences (DECSCUSR)** — "Underline /
   bar cursor shapes requested by vim/neovim users."
3. **`help wanted, windows` - Windows runtime reports wanted** - "The signed
   MSI ships for Windows 10/11 x64; we still want real-world reports around
   PTY behavior, fonts, IME, and named-pipe IPC."

Réponds à toute issue entrante en < 24 h pendant le mois post-lancement.

## 5. Mesure

- **Cibles réalistes :** front page HN = +200-500 stars sur 48 h ; page 2 =
  +50-150 ; flop = +10-30 (et tu re-postes plus tard).
- GitHub Insights → Traffic : fenêtre glissante de 14 jours — screenshote à
  J+2 et J+7 sinon tu perds les données.
- PostHog (paneflow.dev) : segment referrer `news.ycombinator.com` ;
  conversion = clic vers /releases ou GitHub. Pas de funnel avec events
  desktop (cf. règle no-app_started-metric).
- Stars : https://star-history.com/#ArthurDEV44/paneflow&Date pour le graph.

## 6. Après le pic

- Le trafic HN meurt en 48 h ; ce qui reste = le README + les releases. Chaque
  release future avec des Highlights soignés est un micro-événement
  (les watchers reçoivent un mail).
- Les commentaires HN les plus critiques sont une roadmap gratuite : ouvre une
  issue par critique fondée, lien vers le commentaire, et réponds sur HN avec
  le lien. Boucle de confiance.
