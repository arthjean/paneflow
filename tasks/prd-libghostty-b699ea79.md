[PRD]
# PRD: Re-pin libghostty-vt sur Ghostty b699ea79 et intégration de toute la nouvelle surface C

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-07 | Arthur Jean | PRD initial : re-pin de libghostty-vt de `0c2a290d` (2026-09-14) vers `b699ea79` (2026-10-06), 322 commits dont 57 touchent libghostty-vt. Intègre tous les correctifs et toutes les nouvelles API C (render hold du mode 2026, forme de pointeur OSC 22, overscan et identité de ligne, statut de programme OSC 7501, invites OSC 133, RIS, DECRQCRA/XTCHECKSUM, OSC inconnus, rappel du scrollback au redimensionnement, mémoire, compression de l'historique restauré). 5 epics, 16 stories. |
| 1.1 | 2026-10-08 | Arthur Jean | La vérification sur le matériel Windows réel quitte US-014 pour un epic final, EP-006, qui regroupe les tests et correctifs Windows. Le développement des epics se fait sous Linux, sans aller-retour par epic. 6 epics, 17 stories. |
| 1.2 | 2026-10-09 | Arthur Jean | Clôture. L'audit du 2026-10-09 a ouvert `tasks/prd-libghostty-b699ea79-remediation.md`, qui corrige les écarts trouvés après la livraison. Cette version consigne les réponses aux Open Questions, les amendements aux critères (`stub.rs`, OSC 22 et RIS, config d'`xt_checksum_extension`), la liste des terminaux alimentés par ConPTY, la commande de mesure d'US-016 et l'inspection du cas « compression non supportée ». Le PRD passe `DONE`. |

## Problem Statement

Paneflow lie statiquement libghostty-vt au commit Ghostty `0c2a290d3a3e2a599be3a43435d778a5896667ee` (`native/libghostty/manifest.toml:3`). La tête de `main` upstream est `b699ea79f4b881421b4b3055abc16a0957d76beb` ; le clone local `/home/arthur/dev/ghostty` est à jour avec `origin` au 2026-10-07. Le commit figé en est un ancêtre, à 322 commits de distance. 57 de ces commits touchent `src/terminal`, `include/ghostty/vt*` ou `src/lib_vt.zig`.

**Convention de référence.** Dans tout ce document, `ghostty/` désigne `/home/arthur/dev/ghostty` au commit `b699ea79` : tous les numéros de ligne qui suivent ce préfixe sont pris à ce commit. Les chemins sans préfixe désignent Paneflow au commit `198f202e`.

1. **Paneflow embarque des plantages et des défauts déjà corrigés upstream.** Le plus grave : réduire le nombre de colonnes sans reflow laisse la tête d'un caractère large dans la dernière colonne, puis `insertBlanks`, `eraseLine(.left)` et `rowWillBeShifted` écrivent au-delà de la ligne (`e6db5b633`, test `ghostty/src/terminal/Terminal.zig:16072`). Le redimensionnement en deux phases de Paneflow (`crates/paneflow-terminal-ghostty/src/engine.rs:142-160`) passe exactement par une réduction de colonnes. Viennent ensuite :
   - un saut du curseur au reverse wrap (`d4f45bee3`) ;
   - une sélection de mot fausse sur les caractères larges et aux sauts de ligne durs (`a3e80a685`, `c4f15c884`), alors que Paneflow utilise les gestes de sélection upstream ;
   - des OSC que CAN et SUB n'annulaient pas (`520d8f55a`) ;
   - une palette non réinitialisée par RIS (`bb20f8e45`) ;
   - un `ghostty_search_tick` accepté après la libération du terminal (`b1c264163`), alors que Paneflow s'appuie sur la recherche native ;
   - un collage qui continue après une écriture refusée (`2b0ceff7d`), alors que Paneflow appelle `ghostty_terminal_paste`.
2. **Le mode 2026 de Paneflow a les deux défauts que l'API render hold a été créée pour corriger.** Paneflow sonde le mode à chaque décision de publication (`src-app/src/terminal/ghostty_session/publish.rs:140-178`, plafond de 150 ms en `mod.rs:79`). Upstream documente pourquoi ce sondage échoue (`ghostty/include/ghostty/vt/terminal.h:1717-1739`) :
   - la frame laissée à l'écran est la dernière dessinée, parfois à moitié ;
   - une hold relâchée puis reprise entre deux publications fait perdre la frame finie, ce qui donne l'impression d'un pane figé.

   Claude Code, Neovim et fzf utilisent le mode 2026.
3. **Le signal d'état d'agent le plus fiable reste inutilisé.** OSC 7501 (« program status protocol », `ghostty/include/ghostty/vt/terminal.h:1162-1370`) laisse un programme dire `idle`, `working`, `done`, `blocked` (permission, question, auth) ou `error`. C'est exactement le vocabulaire de la sidebar de Paneflow (`AgentState`, `crates/paneflow-ipc-client/src/agent.rs:5-11`). Aujourd'hui, ce vocabulaire est déduit de règles textuelles sur l'écran (`crates/paneflow-host/src/viewport_scan.rs:76-104`) ou des hooks.
4. **Plusieurs comportements de terminal restent sans réponse :**
   - un programme qui demande une forme de pointeur par OSC 22 n'obtient rien (`src-app/src/terminal/view.rs:1632-1642`) ;
   - sous Windows, ConPTY ne peut pas rappeler le scrollback après un redimensionnement, et Paneflow le désynchronise de l'écran réel (`c55f213aa`) ;
   - les OSC inconnus sont jetés sans trace (`crates/paneflow-terminal-ghostty/src/callback_ffi.rs:213-216`).

**Why now:** le dernier re-pin date du 2026-09-14 (PR #85, puis #86, commit `a0104737`). Les re-pins hebdomadaires sont coupés depuis `ae27f5ed`. L'écart grossit et chaque bump coûte plus cher à relire. Upstream vient de figer des API qu'il documente comme des correctifs de conception pour les embarqueurs (render hold, overscan). libghostty n'a pas de version taguée et pas de promesse de stabilité ; seul le diff d'en-tête entre deux commits fait foi. Plus l'écart est court, plus la relecture est fiable.

## Overview

Le travail se fait en deux temps, sur le modèle du pin précédent.

1. Le workflow `.github/workflows/libghostty-bump.yml`, lancé en `workflow_dispatch` avec `source_sha=b699ea79f4b881421b4b3055abc16a0957d76beb`, reconstruit et atteste les quatre archives. Il les publie en pré-release `libghostty-vt-b699ea79...` et ouvre une PR bot avec le manifeste, les bindings et les en-têtes.
2. Une branche de feature reprend ce contenu. Elle complète ce que le workflow ne fait pas :
   - `GHOSTTY_SHA` dans les workflows de lane et de release ;
   - l'inventaire de licences, car uucode passe à `9d555245` pour Unicode 18 (`ghostty/build.zig.zon:43-46`) ;
   - les notices et le SBOM.

   Elle étend ensuite la validation ABI aux nouveaux discriminants et structs, et pose un filet de tests de non-régression pour les correctifs upstream que Paneflow traverse.

Chaque nouvelle API C reçoit ensuite un consommateur réel dans Paneflow :

- **render hold** remplace le sondage du mode 2026. Le callback capture la frame voulue dans un render state dédié, et le plafond anti-gel de 150 ms est conservé.
- **OSC 22** pilote le curseur GPUI, sauf quand l'UI impose sa propre forme (barre de défilement, lien survolé).
- **L'overscan et l'identité de ligne** entrent dans le snapshot du wrapper. L'overscan sert au défilement fluide au pixel, en P2.
- **OSC 7501, OSC 133 et RIS** remontent au host. Le host tient les enregistrements selon la spécification et les donne au détecteur d'état d'agent avant les règles textuelles. Les hooks gardent la précédence.
- **DECRQCRA/XTCHECKSUM** devient une option opt-in, désactivée par défaut.
- **Les OSC inconnus** sont journalisés comme les APC aujourd'hui.
- **`RESIZE_PULL_SCROLLBACK`** passe à faux pour les terminaux alimentés par ConPTY.
- **La mémoire par terminal** est exposée par le statut du host.
- **La compression de l'historique** s'active à la restauration d'un snapshot à l'attache.

Décisions structurantes, avec leur preuve. Le host est le seul terminal qui répond au PTY (`crates/paneflow-host/src/runtime.rs:1207-1211`) : OSC 7501 et DECRQCRA y sont configurés, et les réponses ne sortent qu'une fois. Les variantes wire d'`AgentStateSource` ne bougent pas : OSC 7501 passe par la source écran existante (`ActivitySource::Screen`, `crates/paneflow-serve/src/state.rs:31-36`). Les API sans consommateur arrivent par les bindings seulement. C'est le cas du parseur OSC autonome (`ghostty_osc_set`), du mode contrôle tmux et de CSI 8 t (ignoré par lib-vt, `ghostty/src/terminal/stream_terminal.zig:779-783`).

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Écart au `main` upstream de libghostty-vt | 0 commit au merge (pin `b699ea79`) | Re-pin au plus 4 semaines derrière `main` |
| Nouvelles API C de l'intervalle avec un consommateur Paneflow ou une non-adoption justifiée dans ce PRD | 100 % (16 options, données ou structs listées en US-003) | 100 % à chaque re-pin |
| Frames perdues sous mode 2026 sur le scénario « relâche et reprend dans le même write » | 0 sur 1 000 itérations du test d'US-005 | 0 |
| Sessions dont l'état d'agent vient d'OSC 7501 quand le programme l'émet | 100 % des sessions sans hooks qui émettent OSC 7501 | 100 % |
| Régressions terminal signalées après le release qui embarque le re-pin | 0 plantage, 0 régression de rendu reproductible | 0 |

## Target Users

### Développeur qui fait tourner des agents CLI dans Paneflow
- **Role:** utilisateur de Paneflow sous Linux, macOS ou Windows, avec plusieurs agents (Claude Code, Codex, OpenCode) et des outils longs (cargo, terraform, déploiements) côte à côte.
- **Behaviors:** suit l'état de chaque pane depuis la sidebar, redimensionne souvent les panes, sélectionne et recherche dans le scrollback.
- **Pain points:** un TUI en mode 2026 qui scintille ou semble figé ; un plantage au redimensionnement avec des emoji ou du CJK ; un statut « working » faux quand les règles textuelles se trompent ; sous Windows, une sortie qui atterrit au mauvais endroit après un redimensionnement.
- **Current workaround:** réduire la fréquence des redimensionnements, relancer le pane, regarder chaque pane au lieu de la sidebar.
- **Success looks like:** des panes qui ne plantent pas, des TUI qui se dessinent en une frame, un statut exact quand le programme le déclare.

### Mainteneur de Paneflow
- **Role:** Arthur, seul contributeur, et les agents qui implémentent les stories.
- **Behaviors:** re-pin libghostty par le workflow de bump, relit le diff d'en-tête, vérifie Windows sur le matériel réel (dual boot).
- **Pain points:** un écart upstream qui grossit, des correctifs upstream invisibles sans test côté Paneflow, une validation ABI qui ne couvre pas les nouvelles structs.
- **Current workaround:** relire les 322 commits à la main.
- **Success looks like:** un re-pin dont chaque correctif traversé et chaque nouvelle API a un test côté Paneflow, et un procédé reproductible au prochain bump.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Consommateurs de libghostty-vt** (dont l'app Ghostty elle-même) : tous figent un commit, faute de version taguée. Mitchell Hashimoto annonce une première version taguée sous environ 6 mois après l'annonce de mi-2026 ([wikidocs, analyse libghostty](https://wikidocs.net/blog/@jaehong/8549/)). Paneflow suit déjà ce modèle avec le contrôle d'intégrité le plus strict (hash d'archive, attestation, ABI à l'exécution).
- **alacritty_terminal et wezterm-term** : crates semver, mises à jour par version. Pas d'équivalent render hold exposé à l'embarqueur.
- **xterm** : DECRQCRA désactivé par défaut derrière `allowWindowOps`, parce qu'il permet de relire l'écran ([vttest checksums](https://invisible-island.net/vttest/vttest-checksums.html), [VTE #24](https://gitlab.gnome.org/GNOME/vte/-/issues/24)). Ghostty fait le même choix (`ghostty/include/ghostty/vt/terminal.h:2368-2380`).
- **Mode 2026** : Kitty, foot, WezTerm, iTerm2, Windows Terminal et Ghostty l'implémentent. Upstream recommande un délai d'une seconde (`terminal.h:1717-1725`) ; Paneflow garde ses 150 ms, plus stricts.
- **Market gap:** un cockpit d'agents qui lit l'état déclaré par le programme (OSC 7501) au lieu de le deviner à l'écran. La spécification vit chez son auteur ([superlogical.com, program-status](https://www.superlogical.com/rex/docs/build/program-status)), citée par l'en-tête.

### Best Practices Applied
- Bindings régénérés depuis l'en-tête figé par `scripts/generate-libghostty-bindings.sh`, jamais édités à la main (`native/libghostty/README.md`).
- Une struct dimensionnée (`size`) se lit seulement si `size` couvre les champs lus (`terminal.h:1245-1247`).
- Les chaînes passées aux callbacks sont empruntées : copie avant retour (`terminal.h:1241-1243`).
- Le texte OSC 7501 est non fiable : retirer les caractères de mise en forme invisibles et nommer le terminal d'origine avant de l'afficher hors du terminal (`terminal.h:1322-1327`).
- Un seul terminal répond au PTY, sinon les réponses sont doublées (déduit de `terminal.h:2406-2414` et de `runtime.rs:1207`).

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- Le workflow de bump construit `b699ea79` sur les quatre cibles avec la recette actuelle. Raison : Zig ne bouge pas (`ghostty/build.zig.zon:6`, `minimum_zig_version = "0.16.0"`). Doute : `fe9cf6a26` réécrit `src/build/SharedDeps.zig`, et uucode se télécharge désormais depuis `github.com` au lieu de `deps.files.ghostty.org` (`ghostty/build.zig.zon:43-46`). US-001 valide.
- Un callback render hold peut appeler `ghostty_render_state_update` sur un render state distinct sans violer l'aliasing Rust. Raison : le callback reçoit la poignée `GhosttyTerminal` en argument (`terminal.h:1754-1756`), et `CallbackState` vit derrière le pointeur `userdata`, hors de l'emprunt `&mut DisplayTerminal` (`crates/paneflow-terminal-ghostty/src/callbacks.rs:213-218`). US-005 le prouve ou bascule sur la solution de repli.
- Le paquet uucode `9d555245` embarque l'Unicode Character Database 18.0.0. Raison : le titre du commit `12542b392` (« Update uucode for Unicode 18 »). US-002 le vérifie dans le paquet téléchargé avant d'écrire la licence.
- `GhosttyTerminalUnknownSequence` garde sa taille. Raison : l'union reste bornée par `uint64_t _padding[16]` (`terminal.h:612-619`). US-003 le vérifie par `ghostty_type_json`.
- Les shells instrumentés par Paneflow émettent OSC 133 A et C (`src-app/src/terminal/shell.rs:37-41`). Le callback semantic prompt reçoit donc `PROMPT_START` au retour du shell, ce qui suffit à purger les enregistrements `working` et `blocked`, comme l'exige la spécification.

### Hard Constraints
- Aucun commentaire dans le code source (AGENTS.md). L'intention passe par les noms, les types et les tests.
- Les items `#[cfg(windows)]` se déclarent avant tout `mod tests`. Ils ne sont lintés que par le job « Windows x86_64 libghostty check ».
- Le thread de rendu ne bloque jamais. Les callbacks s'exécutent sur les threads de session Ghostty (`src-app/src/terminal/ghostty_session/mod.rs:328-436`), jamais sur le thread GPUI.
- Les types moteur restent dans `paneflow-terminal-ghostty` ; l'app ne voit que les miroirs neutres de `src-app/src/terminal/types.rs`.
- Chaque nouvelle méthode du wrapper a son pendant dans `crates/paneflow-terminal-ghostty/src/stub.rs`, construit sans la feature `native`.
- Toute invocation cargo reste `--locked`. Aucune dépendance nouvelle dans `paneflow-shim`, `paneflow-ai-hook` ou `paneflow-mcp`, qui ont des plafonds de taille.
- Humain dans la boucle : un statut OSC 7501 s'affiche et peut notifier, mais ne déclenche jamais d'action sur un agent.
- La publication d'une pré-release et l'ouverture d'une PR bot (US-001) sont des actions externes : l'agent demande l'accord d'Arthur avant de les lancer.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration
- `cargo test -p paneflow-libghostty-sys --locked` - intégrité du manifeste, des bindings et de l'ABI

Gates additionnels :
- Stories qui touchent du code `#[cfg(windows)]` ou un chemin ConPTY (US-002, US-014) : le job « Windows x86_64 libghostty check » passe dès que la branche est poussée, et la PR dit que Windows a été vérifié par inspection. La vérification sur le matériel réel est regroupée en EP-006.
- Stories qui modifient un workflow (US-002) : `scripts/check-workflow-action-pins.sh` passe.
- Stories du chemin de rendu (US-005, US-007, US-008) : `scripts/bench-terminal.sh` comparé à `bench/baselines/linux-x86_64/terminal.json`, résultat joint à la PR. Le job CI `perf-gates` passe.
- Stories UI (US-005, US-006, US-008, US-011) : passe visuelle d'Arthur sous Linux sur un build debug, avec une capture ou un enregistrement court joint à la PR. L'agent livre le changement sans lancer l'app pour la vérifier.
- `cargo deny check advisories licenses sources` : seulement si une dépendance Rust change. Aucune story n'en prévoit.

## Epics & User Stories

### EP-001: Re-pin libghostty-vt sur b699ea79

Faire passer les quatre archives, les bindings, les en-têtes, les métadonnées de licence et la validation ABI au commit `b699ea79`, et prouver par des tests côté Paneflow que les correctifs upstream traversés tiennent.

**Definition of Done:** `native/libghostty/manifest.toml` fige `b699ea79`. Les quatre lanes libghostty et `run_tests.yml` sont vertes sur la branche. La validation ABI couvre chaque nouveau discriminant et chaque nouvelle struct listés en US-003. Les tests d'US-004 passent sous Linux, et le job Windows les compile.

#### US-001: Publier les archives b699ea79 par le workflow de bump
**Description:** As a mainteneur, I want reconstruire, attester et publier les quatre archives libghostty-vt au commit `b699ea79` par le workflow existant so that la provenance soit établie dans le run de bump, comme pour chaque pin précédent.

Le workflow fait déjà : contrôle de Zig (`.github/workflows/libghostty-bump.yml:150-160`), manifeste mis en scène (`:201-209`), bindings régénérés, quatre builds (Linux et macOS avec `--verify-reproducible`), attestation, pré-release et PR bot. Le README décrit le procédé (`native/libghostty/README.md`, section « Bumping the pinned source »).

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Avant tout lancement, l'agent obtient l'accord explicite d'Arthur : le run publie une pré-release et ouvre une PR.
- [x] Un premier run `workflow_dispatch` avec `source_sha=b699ea79f4b881421b4b3055abc16a0957d76beb` et `dry_run=true` passe sur les quatre cibles. Son identifiant est consigné dans la PR d'US-002.
- [x] Un second run sans `dry_run` publie la pré-release `libghostty-vt-b699ea79f4b881421b4b3055abc16a0957d76beb` avec quatre assets attestés, et ouvre la PR bot `bot/libghostty-b699ea79`.
- [x] `gh attestation verify` passe sur chacune des quatre archives téléchargées par `scripts/fetch-libghostty.sh`.
- [x] Échec : given un build de cible qui échoue (par exemple, uucode ne se télécharge pas depuis `github.com`, ou la transition translate-c de `fe9cf6a26` casse la recette Windows), when le run se termine, then aucune pré-release n'est publiée. La story passe `BLOCKED` avec l'étape et l'extrait de log en cause, sans contournement local de la recette.
- [x] Échec : given un `minimum_zig_version` différent de `0.16.0` à la cible, when le job `resolve` démarre, then le workflow refuse avant tout build (`libghostty-bump.yml:150-160`) et la story passe `BLOCKED` avec le renvoi vers un re-pin de Zig manuel.

#### US-002: Intégrer le re-pin et ses métadonnées dans une branche de feature
**Description:** As a mainteneur, I want reprendre le contenu de la PR bot dans `feat/libghostty-b699ea79` et compléter ce que le workflow ne réécrit pas so that le build, les lanes de release et l'inventaire de licences désignent tous le même commit.

Le workflow ne réécrit que les clés du manifeste listées en `libghostty-bump.yml:201-209` et la version de licence de libghostty-vt. Le reste se met à jour à la main, comme dans `a0104737` :
- `GHOSTTY_SHA` : `.github/workflows/libghostty-linux.yml:30`, `libghostty-macos.yml:21`, `libghostty-windows.yml` et `release.yml:63` ;
- la licence uucode, désormais à `9d55524551411b493cca41ca06363625d90aff1e` (`ghostty/build.zig.zon:43-46`), et l'Unicode Character Database 18.0.0 :
  - `native/libghostty/manifest.toml:100-110` ;
  - `THIRD_PARTY_NOTICES.md:34,49` ;
  - `sbom.cdx.json:80-96,185` ;
- les mentions du pin dans `native/libghostty/README.md:31,184` et `docs/release/macos-libghostty.md:228,237` ;
- une entrée de `CHANGELOG.md`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] `git grep 0c2a290d` ne renvoie plus que l'historique : `CHANGELOG.md` antérieur, `bench/results/`, `bench/baselines/` et `docs/release/qualification/`.
- [x] La version de l'UCD écrite dans le manifeste, les notices et le SBOM est celle lue dans le paquet uucode réellement téléchargé par le build. La valeur et le fichier lu sont cités dans la PR.
- [x] `notice_sha256` et `sbom_sha256` sont réécrits par `scripts/repin-libghostty-manifest.sh`, et `paneflow-libghostty-sys/build.rs` accepte le résultat.
- [x] `native/libghostty/windows-smoke.c` envoie une séquence UTF-8 multi-octets (par exemple `é` et `漢`) et vérifie qu'elle s'imprime. C'est le chemin simdutf que `a141f9bdb` protège pour les DLL ; Paneflow lie l'archive statique.
- [x] `CHANGELOG.md`, section `[Unreleased]`, annonce le re-pin et ses effets visibles, chacun en une phrase.
- [x] Échec : given une notice ou un SBOM modifié sans hash à jour, when `cargo build` s'exécute, then `build.rs` refuse avec le nom du fichier (test existant gardé vert).
- [x] Échec : given un host lancé par une version à `0c2a290d` et un desktop à `b699ea79`, when le desktop s'attache, then la session finit `Incompatible` avec « Retry » et « Stop host and restart » (`src-app/src/terminal/host_link.rs:252-254,365`). C'est le comportement existant de chaque re-pin. Les Upgrade notes du CHANGELOG le disent.
- [x] Si le gate d'allocations de `perf-gates` bouge à cause du moteur, la nouvelle baseline est un commit séparé produit par `scripts/perf-gates.sh --refresh-alloc-baselines` depuis un arbre propre. Le message nomme les commits upstream en cause, par exemple `a4f0d9f4a` ou `88e66cbc6`.

#### US-003: Étendre la validation ABI aux nouveaux discriminants et structs
**Description:** As a mainteneur, I want que la validation ABI à la construction d'un terminal couvre toute la nouvelle surface so that une dérive de layout au prochain re-pin échoue à la construction plutôt que de corrompre la mémoire.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [x] `validate_discriminants` (`crates/paneflow-terminal-ghostty/src/abi.rs:79-129`) ajoute les ancres suivantes :
  - options du terminal (`ghostty/include/ghostty/vt/terminal.h:2337-2417`) : `GHOSTTY_TERMINAL_OPT_RESIZE_PULL_SCROLLBACK = 40`, `OPT_RENDER_HOLD = 41`, `OPT_SEMANTIC_PROMPT = 42`, `OPT_RESET = 43`, `OPT_XT_CHECKSUM_REPORT = 44`, `OPT_XT_CHECKSUM_EXTENSION = 45`, `OPT_PROGRAM_STATUS = 46` ;
  - données : `GHOSTTY_TERMINAL_DATA_MOUSE_SHAPE = 41` et `DATA_MEMORY_USAGE = 42` (`:2832,2847`) ;
  - `GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_OSC = 1` (`:538`) ;
  - `GHOSTTY_PROGRAM_STATUS_STATE_CLEAR = 5` (`:1180-1185`) ;
  - `GHOSTTY_SEMANTIC_PROMPT_COMMAND_END = 4` (`:1379-1397`) ;
  - `GHOSTTY_MOUSE_SHAPE_ZOOM_OUT = 33` (`ghostty/include/ghostty/vt/mouse.h:76-112`) ;
  - render state : `GHOSTTY_RENDER_STATE_OPTION_OVERSCAN = 1`, `GHOSTTY_RENDER_STATE_DATA_OVERSCAN = 20` et `GHOSTTY_RENDER_STATE_ROW_DATA_ID = 7` (`ghostty/include/ghostty/vt/render.h:392,418,472`) ;
  - `GHOSTTY_SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY = 2` (`ghostty/include/ghostty/vt/snapshot.h:194`).
- [x] Les huit ancres existantes, dont `OPT_DEVICE_ATTRIBUTES = 8`, tiennent toujours.
- [x] `abi_layout.rs` (`crates/paneflow-terminal-ghostty/src/abi_layout.rs:21-101`) valide taille, alignement, offsets et tailles de champs, contre `ghostty_type_json()`, pour :
  - `GhosttyTerminalProgramStatus` ;
  - `GhosttyTerminalSemanticPrompt` ;
  - `GhosttyTerminalMemoryUsage` ;
  - `GhosttyTerminalUnknownOscSequence` ;
  - `GhosttyTerminalUnknownSequence` ;
  - `GhosttyRenderStateOverscan` ;
  - `GhosttyRenderStateRowId`.
- [x] Un test affirme que `GhosttyTerminalUnknownSequence` garde au pin `b699ea79` la taille qu'il avait au pin `0c2a290d`. La valeur est relevée dans les bindings des deux pins et écrite en dur dans le test.
- [x] Les signatures des quatre nouveaux callbacks (`GhosttyTerminalRenderHoldFn`, `ProgramStatusFn`, `SemanticPromptFn`, `ResetFn`, `terminal.h:1366,1513,1544,1754`) sont figées par des assignations `const _:` typées, à côté des quatorze existantes (`callbacks.rs:18-31`).
- [x] Échec : given un JSON de layout altéré qui décale un champ de `GhosttyTerminalProgramStatus`, when la validation tourne, then elle renvoie `AbiMismatch` nommant la struct et le champ (test sur une copie altérée).

#### US-004: Prouver côté Paneflow les correctifs upstream traversés
**Description:** As a mainteneur, I want un test Paneflow par correctif upstream qui passe par une API que Paneflow appelle so that un re-pin futur qui casserait l'un d'eux échoue dans notre CI et pas chez un utilisateur.

Chaque test vit dans `crates/paneflow-terminal-ghostty/tests/`. Il pilote le terminal par l'API du wrapper (`feed`, `resize`, gestes de sélection, recherche, collage) et cite en nom de test le commit upstream qu'il couvre.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Caractère large coupé (`e6db5b633`, oracle `ghostty/src/terminal/Terminal.zig:16072`) : given une ligne qui finit par un caractère large en dernière colonne, when `DisplayTerminal::resize` réduit les colonnes d'une unité, puis le test envoie `CSI @` et `CSI 1 K`, then rien ne panique et la dernière colonne est vide.
- [x] Wrap en attente (`f9ab34f10`, `Terminal.zig:16313`) et curseur sauvegardé (`afded91df`, `Terminal.zig:16296`) : après un redimensionnement, le curseur vivant et le curseur sauvegardé gardent leur wrap en attente et leur position, y compris après des élargissements répétés.
- [x] Reverse wrap (`d4f45bee3`, `Terminal.zig:11134`) : un `CUB` avec reverse wrap et un wrap en attente au-dessus de la marge haute ne fait pas sauter le curseur.
- [x] Jeux de caractères :
  - un point de code au-delà de 0xFF s'imprime non converti dans un jeu de caractères (`6220a3617`, `Terminal.zig:7002`) ;
  - un single shift ne s'applique qu'à un seul caractère imprimé (`c706451fe`, `Terminal.zig:7023`) ;
  - l'impression groupée du jeu graphique spécial donne le même résultat que l'impression caractère par caractère (`a4f0d9f4a`).
- [x] Sélection par les gestes que Paneflow utilise :
  - le mot s'arrête correctement sur les caractères larges (`a3e80a685`, `ghostty/src/terminal/Screen.zig:10345`) ;
  - le mot s'arrête aux sauts de ligne durs (`c4f15c884`, `Screen.zig:10273`) ;
  - la ligne s'arrête aux frontières d'invite à travers des cellules vides (`13b5ab204`, `Screen.zig:9737`).
- [x] Parseur :
  - CAN puis SUB au milieu d'un `OSC 0` laissent le titre inchangé (`520d8f55a`, `ghostty/src/terminal/stream_terminal.zig:5566`) ;
  - un entier OSC avec séparateur de chiffres est rejeté (`73768913b`) ;
  - `OSC 105` passe par le parseur de couleurs (`36953bca8`) ;
  - DECRQM répond aux modes ANSI et ne tronque pas un mode sur 16 bits (`9dc0d974e`, `3beb6d717`, `ghostty/src/terminal/c/terminal.zig:4306`).
- [x] RIS (`bb20f8e45`) : given le thème Paneflow appliqué et un `OSC 4` qui change la couleur 1, when le programme envoie `ESC c`, then la couleur 1 revient à celle du thème Paneflow, pas à la palette par défaut de Ghostty.
- [x] Kitty : un `EL 2` efface le drapeau de placeholder Unicode de la ligne (`a573781c6`, `Terminal.zig:14280`). Le PNG d'une image kitty se décode avec la sortie remise à zéro avant le callback (`aca9bf031`) : le trampoline de `crates/paneflow-terminal-ghostty/src/sys.rs:54-59` (`decode_png_trampoline`) remplit les quatre champs, ou renvoie faux.
- [x] Échec : given un handle de recherche dont le terminal a été libéré, when `ghostty_search_tick` est appelé, then l'appel renvoie `GHOSTTY_INVALID_VALUE` et le wrapper le traduit en erreur sans plantage (`b1c264163`, `ghostty/src/terminal/c/search.zig:949-1013`).
- [x] Échec : given un lecteur de collage qui voit une écriture refusée, when `ghostty_terminal_paste` continue, then le collage échoue et rien après l'écriture refusée n'atteint le PTY (`2b0ceff7d`).
- [x] Les corpus et golden existants (`src-app/src/terminal/bench_corpus.rs` et les tests golden du wrapper) passent sans changement. Si les largeurs Unicode 18 de `12542b392` en font bouger un, le diff est relu, puis consigné dans la PR avec le point de code en cause.

---

### EP-002: Rendu : sortie synchronisée, forme du pointeur, overscan

Consommer les trois nouvelles API de rendu : render hold pour le mode 2026, OSC 22 pour le curseur, overscan et identité de ligne pour le défilement fluide.

**Definition of Done:** le mode 2026 ne passe plus par un sondage du mode. Un programme qui demande une forme de pointeur l'obtient dans le pane. Le snapshot du wrapper porte l'overscan et l'identité de ligne. Le défilement fluide est livré derrière `reduce_motion`, ou la story P2 est explicitement reportée.

#### US-005: Remplacer le sondage du mode 2026 par l'effet render hold
**Description:** As a développeur qui fait tourner un TUI en mode 2026, I want que Paneflow affiche exactement la frame que le programme a terminée so that le pane ne montre ni frame à moitié dessinée ni frame périmée quand le programme relâche et reprend la hold entre deux publications.

Le modèle upstream est donné en entier dans `ghostty/include/ghostty/vt/terminal.h:1639-1756`, résumé dans `ghostty/include/ghostty/vt/render.h:76-96` :
- au début de la hold, capturer la frame par `ghostty_render_state_update` depuis le callback ;
- pendant la hold, ne plus mettre à jour le render state ;
- à l'expiration du délai, remettre le mode à zéro par `GHOSTTY_TERMINAL_OPT_MODE` (`:2229`).

La hold se termine à la remise à zéro du mode 2026, à un RIS ou à un `ghostty_terminal_resize`. Côté Zig, voir `ghostty/src/terminal/stream_terminal.zig:1917` et `:700-704`. Oracles upstream : `ghostty/src/terminal/c/terminal.zig:4746` (« set render_hold callback ») et `:6710` (« resize disables synchronized output »).

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] `DisplayTerminal` installe `GHOSTTY_TERMINAL_OPT_RENDER_HOLD` sur les terminaux du desktop qui publient des frames (`src-app/src/terminal/ghostty_session/`). Les terminaux du host, qui ne rendent pas (`crates/paneflow-host/src/runtime.rs:906-935`), ne l'installent pas.
- [x] Au début d'une hold, le trampoline met à jour un render state dédié, détenu par `CallbackState`. Il ne passe jamais par `&mut DisplayTerminal`. Tant que la hold dure, `snapshot()` publie ce render state capturé.
- [x] `publish.rs` ne lit plus le mode 2026 (`publish.rs:140-178`). La décision de publication vient de l'état de hold tenu par les événements.
- [x] Given un seul `feed` qui contient la frame A, puis `CSI ? 2026 h`, la frame B complète, `CSI ? 2026 l`, `CSI ? 2026 h` et une frame C partielle, when la publication suivante a lieu, then le contenu publié est la frame B, ni A ni C partielle. Ce test échoue avec le sondage actuel ; il passe sur 1 000 itérations.
- [x] Given une hold jamais relâchée, when 150 ms s'écoulent (`SYNC_OUTPUT_MAX_HOLD`, `mod.rs:79`), then le desktop remet le mode 2026 à zéro par `OPT_MODE` et republie le contenu vivant. Un programme qui renvoie `CSI ? 2026 h` pendant la hold ne repousse pas l'échéance.
- [x] Un redimensionnement ou un RIS pendant une hold la termine et republie le contenu vivant (test).
- [x] Pendant une hold, un défilement ou une sélection de l'utilisateur affiche le contenu vivant (`terminal.h:1740-1741`).
- [x] Échec : given un callback render hold qui panique, when l'événement est traité, then la session remonte `CallbackPanicked` (`callbacks.rs:315-330`) comme les autres callbacks.
- [x] Échec : given que la capture dans le callback ne peut pas se faire sans aliasing, when la conception est revue, then la solution de repli est retenue et documentée dans la PR. Elle consiste à couper `feed` à chaque transition de hold par `ghostty_terminal_vt_write_until_ground`, déjà appelé par le wrapper, et à capturer entre deux appels. Le test « relâche et reprend » ci-dessus reste exigé.
- [x] Les tests existants de `publish.rs:377-443` et de `crates/paneflow-terminal-ghostty/tests/display_terminal.rs:88-104` sont adaptés, pas supprimés.

#### US-006: Honorer la forme de pointeur demandée par OSC 22
**Description:** As a développeur qui utilise un TUI qui change le pointeur, I want que le curseur de la souris prenne la forme demandée par le programme so that les zones cliquables, redimensionnables ou de texte se reconnaissent comme dans Ghostty.

Upstream expose la forme par `GHOSTTY_TERMINAL_DATA_MOUSE_SHAPE` (`ghostty/include/ghostty/vt/terminal.h:2825-2832`), avec 34 valeurs (`ghostty/include/ghostty/vt/mouse.h:76-112`). La valeur initiale est `TEXT`, et un `OSC 22` vide la remet à `TEXT` (`b1bfd1d9c`, `ghostty/src/terminal/stream_terminal.zig:719-720`). Oracle : `ghostty/src/terminal/c/terminal.zig:7136`. GPUI au pin `0418cb5` offre 21 variantes de `CursorStyle` (`crates/gpui/src/platform.rs`).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Le snapshot du wrapper lit `DATA_MOUSE_SHAPE` à chaque publication et la porte dans un miroir neutre `MouseShape` (`src-app/src/terminal/types.rs`). L'app ne voit aucun type moteur.
- [x] `view.rs:1632-1642` garde ses priorités : `Arrow` sur la barre de défilement ou pendant son déplacement, puis `PointingHand` sur un lien survolé avec Ctrl. Sinon, la forme demandée par le programme s'applique.
- [x] La correspondance est une table exhaustive, testée valeur par valeur :
  - `TEXT` vers `IBeam`, `VERTICAL_TEXT` vers `IBeamCursorForVerticalLayout` ;
  - `DEFAULT` vers `Arrow`, `POINTER` vers `PointingHand`, `CONTEXT_MENU` vers `ContextualMenu` ;
  - `CROSSHAIR` et `CELL` vers `Crosshair`, `ALIAS` vers `DragLink`, `COPY` vers `DragCopy` ;
  - `NO_DROP` et `NOT_ALLOWED` vers `OperationNotAllowed`, `GRAB` vers `OpenHand`, `GRABBING` vers `ClosedHand` ;
  - `COL_RESIZE` vers `ResizeColumn`, `ROW_RESIZE` vers `ResizeRow` ;
  - `N`, `S`, `E`, `W` vers `ResizeUp`, `ResizeDown`, `ResizeRight`, `ResizeLeft` ;
  - `NS` vers `ResizeUpDown`, `EW` vers `ResizeLeftRight` ;
  - `NE`, `SW`, `NESW` vers `ResizeUpRightDownLeft`, et `NW`, `SE`, `NWSE` vers `ResizeUpLeftDownRight` ;
  - `HELP`, `PROGRESS`, `WAIT`, `MOVE`, `ALL_SCROLL`, `ZOOM_IN` et `ZOOM_OUT` vers `Arrow`.
- [x] Given un programme qui envoie `OSC 22 ; pointer ST` puis `OSC 22 ; ST`, when la souris survole le pane, then le curseur passe à `PointingHand`, puis revient à `IBeam`.
- [x] Échec : given un discriminant inconnu renvoyé par une version future de la bibliothèque, when la table le reçoit, then le curseur vaut `IBeam` et rien ne panique (test sur une valeur hors plage).
- [x] Échec : given un pane non focalisé ou un programme sorti, when le shell reprend la main, then la forme reste celle du dernier `OSC 22` reçu jusqu'au prochain `OSC 22` ou RIS, comme upstream. Le test le fixe. *(Amendé en 1.2, voir « Amendements de clôture ».)*

#### US-007: Exposer l'overscan et l'identité de ligne dans le snapshot du wrapper
**Description:** As a mainteneur, I want que le snapshot du wrapper puisse capturer des lignes au-dessus et au-dessous du viewport, et que chaque ligne porte sa position et son identité so that le défilement fluide (US-008) et un futur cache par ligne disposent des données upstream sans nouveau passage FFI.

Référence upstream :
- vue d'ensemble « Overscan » et « Row Identity » : `ghostty/include/ghostty/vt/render.h:97-204` ;
- types `GhosttyRenderStateOverscan` (`:266-281`) et `GhosttyRenderStateRowId` (`:284-302`) ;
- `DATA_OVERSCAN` (`:392`), `DATA_OVERSCAN_REQUEST` (`:398`), `OPTION_OVERSCAN` (`:418`), `ROW_DATA_VIEWPORT_Y` (`:467`) et `ROW_DATA_ID` (`:472`) ;
- implémentation : `ghostty/src/terminal/c/render.zig:26-71` ;
- oracles : `render.zig:2691-3033`.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] `DisplayTerminal::set_overscan(above, below)` règle `GHOSTTY_RENDER_STATE_OPTION_OVERSCAN`. Le snapshot rapporte l'overscan réellement capturé (`DATA_OVERSCAN`), jamais la demande.
- [x] Chaque ligne du snapshot porte `viewport_y` (`i32`) et `row_id` (deux `u64`), lus dans le même `ghostty_render_state_row_get_multi` que la dirtiness et les cellules (`crates/paneflow-terminal-ghostty/src/snapshot_ffi.rs:263-300`).
- [x] Les lignes d'overscan sont séparées des lignes du viewport dans le contenu publié. `CellMirror` (`src-app/src/terminal/ghostty_session/convert.rs:194-250`) continue d'indexer `0..rows` sur le seul viewport.
- [x] Given un overscan à zéro, la valeur par défaut, when un corpus du bench est publié, then le contenu publié est identique à celui d'avant la story (test golden).
- [x] Given `{above: 1, below: 1}` et un viewport défilé au milieu du scrollback, when la publication a lieu, then une ligne est capturée de chaque côté, avec les `viewport_y` -1 et `rows`. Les identifiants restent stables après un défilement d'une ligne (miroir de `render.zig:2940`).
- [x] Échec : given un viewport collé en bas, when la publication a lieu, then l'overscan bas capturé vaut 0 et aucune ligne fictive n'est inventée (miroir de `render.zig:2918`).
- [x] Le surcoût de publication avec `{1, 1}` reste sous 5 % sur `scripts/bench-terminal.sh`, chiffre joint à la PR.

#### US-008: Défiler au pixel par l'overscan
**Description:** As a développeur sur trackpad, I want que le contenu du terminal suive le geste au pixel près so that le défilement du scrollback ne saute plus ligne par ligne.

Modèle upstream : `ghostty/include/ghostty/vt/render.h:134-163`. Un overscan bas d'une ligne suffit pour dessiner la ligne partiellement visible ; `offset_px` revient à 0 quand rien n'est capturé en bas. La demande d'overscan se pose une fois, car tout changement de demande provoque un rendu complet (`render.h:409-417`).

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Les deltas de défilement précis du trackpad déplacent la grille d'un décalage en pixels, strictement inférieur à une hauteur de cellule, et franchissent une ligne quand le décalage atteint une hauteur de cellule.
- [x] Les deltas en lignes de la molette gardent le comportement actuel, ligne par ligne, avec `scroll_multiplier` (`src-app/src/terminal/view.rs:217,578`).
- [x] La demande d'overscan `{1, 1}` se pose une fois par session, pas à chaque geste.
- [x] `reduce_motion: true` désactive le décalage en pixels ; le défilement reste ligne par ligne.
- [x] Given un viewport en bas du scrollback, when l'utilisateur continue de défiler vers le bas, then le décalage reste à 0 (pas de rebond dans le vide).
- [x] Échec : given une sortie qui arrive pendant un geste, when une frame est publiée, then le décalage en cours est conservé et le contenu ne saute pas d'une ligne (passe visuelle d'Arthur, enregistrement joint).
- [x] Le rendu à 60 Hz pendant un geste ne dépasse pas le budget de `perf-gates` du scénario de défilement, chiffre joint à la PR.

---

### EP-003: Statut de programme OSC 7501, invites OSC 133 et RIS

Faire remonter l'état déclaré par le programme jusqu'à la sidebar, en tenant les enregistrements côté host selon la spécification.

**Definition of Done:** un programme qui émet OSC 7501 dans un pane sans hooks voit son état dans la sidebar. Les règles de la spécification (remplacement, effacement de sous-arbre, purge à l'invite et à la sortie, 256 enregistrements au plus) sont prouvées par des tests du host. Le RIS d'un programme et les invites OSC 133 sont des événements du wrapper avec un consommateur.

#### US-009: Brancher PROGRAM_STATUS, SEMANTIC_PROMPT et RESET dans le wrapper
**Description:** As a mainteneur, I want trois nouveaux événements typés du wrapper pour OSC 7501, OSC 133 et RIS so that le host et le desktop les consomment sans toucher à un type moteur.

Références upstream :
- types : `GhosttyTerminalProgramStatus` (`ghostty/include/ghostty/vt/terminal.h:1211-1291`), `GhosttyTerminalSemanticPrompt` (`:1430-1487`) et `GhosttyTerminalResetFn` (`:1519-1546`) ;
- trampolines C : `ghostty/src/terminal/c/terminal.zig:706-760` ;
- parseur OSC 7501 : `ghostty/src/terminal/osc/parsers/program_status.zig`, avec les limites `max_msg_bytes = 2048` et `max_title_bytes = 192` (`:69-73`) et les tests `:406-721` ;
- oracles : `c/terminal.zig:4978` (program_status), `:5097` (semantic_prompt) et `:5192` (reset).

À un RIS, upstream envoie un `CLEAR` d'id vide, puis un retrait de progression, puis le callback reset (`ghostty/src/terminal/stream_terminal.zig:700-714`).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] `BackendEvent` (`crates/paneflow-terminal-ghostty/src/model.rs:277-300`) gagne trois variantes :
  - `ProgramStatus`, avec `state`, `kind`, `progress: Option<u8>`, et `id`, `app`, `title`, `message` copiés ;
  - `SemanticPrompt`, avec `kind`, `prompt_kind` et `exit_code: Option<i32>` ;
  - `Reset`.
- [x] Chaque trampoline lit `size` avant tout champ et ignore un rapport dont `size` est inférieur à la taille de la struct au pin. Il passe par `with_state` (garde de panique), copie les chaînes empruntées avant de rendre la main, et respecte `MAX_CALLBACK_BYTES` (`callback_ffi.rs:9`).
- [x] Les événements suivent la file plafonnée existante (`callbacks.rs:90-184`). Un plafond `MAX_PENDING_PROGRAM_STATUS_EVENTS` est fixé à 64, le minimum de la spécification ; un dépassement produit `EffectsOverflow`.
- [x] Une valeur d'état inconnue est ignorée. Un `kind` inconnu devient `None`. Un `progress` à -1 devient `None`. Chaque cas a son test.
- [x] `PROGRAM_STATUS` ne s'installe que sur les terminaux du host, qui répondent au PTY. Given `OSC 7501 ; ? ST` reçu par une session, when la réponse est écrite, then exactement une réponse atteint le PTY (test au niveau du host). Le terminal du desktop n'installe pas le callback et ne répond pas.
- [x] Le desktop consomme `Reset` en publiant immédiatement (`urgent`) au lieu d'attendre `MIN_PUBLISH_INTERVAL` (`mod.rs:76`).
- [x] Les méthodes d'installation ont leur pendant dans `stub.rs`. *(Amendé en 1.2, voir « Amendements de clôture ».)*
- [x] Échec : given un message OSC 7501 dont le base64 se décode en UTF-8 invalide, when le programme l'émet, then aucun événement n'est produit, puisque upstream le rejette (`program_status.zig:624`). Le test le vérifie au niveau du wrapper.

#### US-010: Tenir les enregistrements de statut dans le host selon la spécification
**Description:** As a mainteneur, I want que le host tienne pour chaque session les enregistrements OSC 7501 selon les règles de la spécification so that l'état exposé soit celui que le programme a déclaré, sans fuite d'enregistrements périmés.

Règles : `ghostty/include/ghostty/vt/terminal.h:1299-1327`. Un rapport remplace entièrement son enregistrement. `CLEAR` retire l'id et tout son sous-arbre (`/`), et `CLEAR` sans id retire tout. `PROMPT_START` et la sortie du programme retirent `working` et `blocked`. Le host garde au plus 256 enregistrements et en permet au moins 64, avec éviction du moins récemment mis à jour.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Le runtime du host (`crates/paneflow-host/src/runtime.rs:1201-1233`) consomme `ProgramStatus`, `SemanticPrompt` et `Reset` dans `drain_engine_events`, à côté de `Progress` (`:1222-1224`).
- [x] Un type `ProgramStatusRecords` applique les règles ci-dessus. Il a un test par règle, dont l'effacement de `build` qui retire `build/test` et l'éviction au 257e enregistrement.
- [x] `ViewportScan` (`runtime.rs:106-113`) gagne `program_status` : l'enregistrement racine (id vide), ou à défaut le plus récemment mis à jour. La capture JSON de `crates/paneflow-host/src/control.rs:689` et de `viewport_scan.rs:302` l'expose à côté de `progress`.
- [x] Given une session dont le programme sort, when le host constate la sortie, then les enregistrements `working` et `blocked` disparaissent, et `done` et `error` restent jusqu'au prochain `PROMPT_START` ou `CLEAR`.
- [x] Given un RIS du programme, when le host le traite, then les enregistrements et la progression (`self.progress`) sont vides (test).
- [x] Échec : given un programme qui émet 10 000 rapports avec des ids distincts, when le host les traite, then la mémoire des enregistrements reste bornée à 256 entrées et le host reste réactif (test qui mesure la taille).

#### US-011: Faire de l'état OSC 7501 une source d'état d'agent
**Description:** As a développeur qui suit ses agents et ses builds dans la sidebar, I want que l'état déclaré par OSC 7501 l'emporte sur les règles textuelles d'écran so that la sidebar affiche « working », « waiting » ou « finished » quand le programme le dit, au lieu de le deviner.

Le détecteur d'écran reçoit `ScreenView { screen, title, progress }` (`crates/paneflow-host/src/viewport_scan.rs:76-104`) et classe en états d'écran, dont `idle` et `blocked` (`crates/paneflow-serve/src/state.rs:25-26`). La source reste `ActivitySource::Screen`, émise sur le fil comme `terminal` (`state.rs:954`). Les hooks gardent la précédence (`AgentStateSource`, `crates/paneflow-ipc-client/src/agent.rs:53-57,71-79`).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] `ScreenView` et `ScreenInput` gagnent `program_status`. Quand un enregistrement est présent, il décide l'état d'écran avant toute règle textuelle :
  - `working` vers occupé ;
  - `blocked` vers `blocked`, quel que soit le `kind` ;
  - `idle` et `done` vers `idle` ;
  - `error` vers l'état d'erreur si la couche d'écran le porte, sinon `idle`, avec la raison consignée dans la PR.
- [x] `classification_hash` (`viewport_scan.rs:97-103`) inclut `program_status`, si bien qu'un changement d'état seul déclenche une reclassification.
- [x] Given une session avec hooks actifs, when le programme émet aussi OSC 7501, then l'état affiché reste celui des hooks, car la précédence `Hook > Terminal` est inchangée (test).
- [x] Le `message` d'un enregistrement `blocked` alimente le corps de la notification existante, derrière la même porte `NotifyWhenAgentWaiting` (`src-app/src/agents/notifications.rs:181-186`). Il passe par `sanitize_notification_message` (`src-app/src/agents/notifications.rs:188`, qui appelle `strip_bidi_zero_width`, `src-app/src/markdown/parser.rs:462-477`), comme les notifications de programme aujourd'hui (`events.rs:210-215`), et le titre nomme le pane d'origine.
- [x] Aucun état OSC 7501 ne déclenche d'action sur un agent : envoi, soumission ou relance.
- [x] `CHANGELOG.md` annonce qu'un programme qui émet OSC 7501 voit son état dans la sidebar, avec un exemple de séquence.
- [x] Échec : given un `title` ou un `message` qui contient U+202E ou U+2066, when il est affiché dans la sidebar ou une notification, then ces caractères sont retirés (test).

---

### EP-004: Protocoles et options de plateforme

Consommer les options qui changent le comportement protocolaire : DECRQCRA/XTCHECKSUM en opt-in, OSC inconnus, rappel du scrollback sous ConPTY.

**Definition of Done:** DECRQCRA est désactivé par défaut et activable par la config. Les OSC inconnus suivent le même chemin que les APC. Les terminaux alimentés par ConPTY ne rappellent plus le scrollback au redimensionnement. La vérification sur le matériel Windows réel est en EP-006.

#### US-012: Rendre DECRQCRA et XTCHECKSUM activables, désactivés par défaut
**Description:** As a développeur qui teste une application de terminal avec vttest ou esctest, I want activer les réponses de checksum DECRQCRA par la config so that ces suites de conformité passent, sans exposer par défaut le contenu de l'écran aux programmes.

Références upstream :
- options : `ghostty/include/ghostty/vt/terminal.h:2368-2403`. `XT_CHECKSUM_REPORT` est désactivé par défaut, parce qu'un programme peut relire l'écran cellule par cellule. Les bits d'`XT_CHECKSUM_EXTENSION` vont de 1 à 16, une valeur au-delà de 31 est refusée, et la valeur survit à un RIS ;
- implémentation : `ghostty/src/terminal/xt_checksum.zig` et `ghostty/src/terminal/stream_terminal.zig:683-684,1750-1760` ;
- oracles : `ghostty/src/terminal/c/terminal.zig:6556`, « checksum report requires explicit opt in », et `:6612`, « checksum extension survives resets ».

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] `TerminalConfig` (`crates/paneflow-config/src/schema/terminal.rs:166-192`) gagne `xt_checksum_report: Option<bool>`, faux par défaut, et `xt_checksum_extension: Option<u8>`, 0 par défaut, sur le modèle de l'opt-in OSC 52 (`Osc52ClipboardConfig`, `:69`).
- [x] La config est appliquée aux terminaux du host, qui répondent au PTY, à la création de session (`runtime.rs:906-935`), et prise en compte par le watcher de config pour les nouvelles sessions.
- [x] Given la config par défaut, when un programme envoie `CSI 1 ; 1 ; 1 ; 1 ; 24 ; 80 * y`, then aucune réponse n'atteint le PTY (test au niveau du host).
- [x] Given `xt_checksum_report: true`, when le même DECRQCRA arrive sur un écran connu, then la réponse est `DCS 1 ! ~ <hex> ST`, avec la valeur de l'oracle upstream pour le même contenu.
- [x] Échec : given `xt_checksum_extension: 32`, when la config se charge, then le loader la rejette avec un message qui nomme la clé et la plage 0-31. La valeur précédente reste en vigueur. *(Amendé en 1.2, voir « Amendements de clôture ».)*
- [x] `docs/user/` documente les deux clés, avec la phrase de risque reprise de l'en-tête upstream.

#### US-013: Faire passer les OSC inconnus par le chemin des séquences non gérées
**Description:** As a mainteneur, I want que les OSC que libghostty-vt n'implémente pas soient rapportés comme les APC so that un protocole OSC non supporté se diagnostique dans les logs au lieu de disparaître.

Références upstream :
- types : `GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_OSC` et `GhosttyTerminalUnknownOscSequence` (`ghostty/include/ghostty/vt/terminal.h:533-630`) ;
- conditions de rapport : `:635-660`. Rien n'est rapporté si `UNKNOWN_MAX_BYTES` vaut 0, si la séquence a été annulée par CAN ou SUB, ou si le protocole est supporté ;
- oracle : `ghostty/src/terminal/c/terminal.zig:5233`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Le trampoline `unknown_sequence` (`crates/paneflow-terminal-ghostty/src/callback_ffi.rs:203-226`) accepte l'étiquette OSC. `BackendEvent::UnknownSequence` porte le type (APC ou OSC) et, pour un OSC, le terminateur.
- [x] Le desktop journalise un OSC inconnu au niveau `debug`, échappé, dans la limite de `MAX_UNKNOWN_SEQUENCE_BYTES` (`crates/paneflow-terminal-ghostty/src/options.rs:11`), comme un APC aujourd'hui (`events.rs:309-315`). Le host continue de l'ignorer (`runtime.rs:1227-1232`).
- [x] Given `ESC ] 7400 ; status=busy BEL`, when le terminal le traite, then un événement OSC est produit avec le contenu `7400;status=busy` et le terminateur BEL.
- [x] Échec : given `ESC ] 7400 ; abc CAN`, when le terminal le traite, then aucun événement n'est produit (`520d8f55a`).
- [x] Échec : given un OSC de 10 000 octets, when il est rapporté, then le contenu est tronqué à la limite et `truncated` vaut vrai.

#### US-014: Ne plus rappeler le scrollback au redimensionnement sous ConPTY
**Description:** As a développeur sous Windows, I want qu'agrandir un pane n'aspire plus de lignes du scrollback dans la zone active so that la sortie suivante de ConPTY atterrisse au bon endroit.

Références upstream :
- option : `ghostty/include/ghostty/vt/terminal.h:2313-2337`. La valeur par défaut est vrai et elle est préservée à travers un RIS ;
- commit `c55f213aa`. Il renvoie aux implémentations de Windows Terminal, xterm.js et WezTerm, pour la même raison ;
- oracle : `ghostty/src/terminal/c/terminal.zig:3771`.

ConPTY vit dans `crates/paneflow-host/src/pty/windows.rs`, et son comportement de redessin est testé dans `crates/paneflow-host/tests/conpty.rs`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Le wrapper expose `set_resize_pull_scrollback(bool)`, avec son pendant dans `stub.rs`. *(Amendé en 1.2, voir « Amendements de clôture ».)*
- [x] Tout terminal alimenté par un flux ConPTY règle l'option à faux : celui du host, créé par `new_terminal` (`runtime.rs:906-935`), et tout terminal du desktop qui rejoue ce flux. La liste est établie par l'agent et consignée dans la PR. Le choix passe par `cfg!(windows)` en expression, pas par un item `#[cfg(windows)]`.
- [x] Sous Linux et macOS, l'option garde sa valeur par défaut (test qui lit la valeur appliquée).
- [x] Given l'option à faux et un écran dont la première ligne est passée en scrollback, when les lignes augmentent, then des lignes vides s'ajoutent en bas et la ligne passée en scrollback y reste (test sous Linux avec l'option forcée).
- [x] Échec : given un RIS du programme, when l'option était à faux, then elle reste à faux (test).
- [x] La vérification sur le matériel Windows réel est portée par US-017 (EP-006).

---

### EP-005: Mémoire et restauration

Exposer la mémoire de chaque terminal et réduire celle d'un historique restauré.

**Definition of Done:** le statut du host rapporte la mémoire de chaque session. L'attache d'une session à long scrollback consomme moins de mémoire résidente, mesure à l'appui.

#### US-015: Rapporter la mémoire de chaque terminal dans le statut du host
**Description:** As a mainteneur, I want lire la mémoire résidente, virtuelle et compressée de chaque terminal so that un budget mémoire se mesure par session plutôt que par processus.

Références upstream :
- struct : `GhosttyTerminalMemoryUsage` (`ghostty/include/ghostty/vt/terminal.h:446-509`). Elle est dimensionnée : il faut poser `size`, sinon l'appel renvoie `GHOSTTY_INVALID_VALUE` ;
- donnée : `DATA_MEMORY_USAGE` (`:2834-2847`). Elle parcourt toutes les pages, donc il ne faut pas la lire après chaque écriture ;
- oracles : `ghostty/src/terminal/c/terminal.zig:2667,2763`.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] `DisplayTerminal::memory_usage()` renvoie un miroir de la struct, ou une erreur si `size` est refusé.
- [x] Le statut du host (`host.status`) expose par session `resident_bytes`, `virtual_bytes`, `compressed_bytes` et `image_bytes` (écrans principal et alternatif sommés), plus `compression_supported`. La lecture se fait à la demande, jamais à chaque `feed`.
- [x] Un test vérifie qu'après l'écriture de 10 000 lignes, `primary_resident_bytes` dépasse celui d'un terminal neuf, et que `primary_virtual_bytes` est au moins égal à `primary_resident_bytes`.
- [x] Échec : given une session dont le terminal n'est pas vivant, when le statut est demandé, then la session rapporte `memory: null` avec une raison, jamais 0.
- [x] `bench/README.md`, section « Compteurs de travail », documente les champs et leur unité.

#### US-016: Compresser l'historique restauré à l'attache d'une session
**Description:** As a développeur qui rattache une session à long scrollback, I want que l'historique restauré dans le desktop soit stocké compressé so that le desktop ne double pas la mémoire du scrollback du host.

Références upstream :
- option : `GHOSTTY_SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY` (`ghostty/include/ghostty/vt/snapshot.h:164-194`). L'historique se décompresse à l'accès (défilement, recherche) ; sur une plateforme sans compression, l'option est acceptée sans effet ;
- oracle : `ghostty/src/terminal/c/snapshot.zig:1386`.

Le desktop décode le snapshot du host dans `src-app/src/terminal/ghostty_session/attached_runtime.rs:200`, par `SnapshotDecoder` (`crates/paneflow-terminal-ghostty/src/snapshot_codec.rs:70-82`).

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-015

**Acceptance Criteria:**
- [x] `SnapshotDecoder` expose l'option, et `attached_runtime.rs` l'active à l'attache.
- [x] Mesure jointe à la PR, prise avec `memory_usage()` d'US-015 : un snapshot de 50 000 lignes restauré avec l'option donne un `primary_resident_bytes` inférieur à celui obtenu sans l'option. Le rapport et la commande qui le reproduit sont cités.
- [x] Given un historique restauré compressé, when l'utilisateur défile jusqu'en haut ou lance une recherche, then le contenu est identique à celui d'une restauration sans compression (test).
- [x] Échec : given une plateforme où `compression_supported` vaut faux, when l'option est activée, then la restauration réussit sans erreur et sans changement de comportement (test qui force le cas, ou inspection consignée si non forçable).

---

### EP-006: Vérification et correctifs Windows

Regrouper en fin de PRD toute la vérification sur le matériel Windows réel, pour que les epics précédents se développent et se valident sous Linux sans aller-retour par epic.

**Definition of Done:** le job « Windows x86_64 libghostty check » est vert sur le dernier commit de la branche. Chaque vérification matérielle listée ici est faite sur le dual boot d'Arthur, et chaque défaut trouvé est corrigé avec un test de non-régression quand il est testable sous Linux.

#### US-017: Vérifier le PRD sur le matériel Windows réel et corriger les écarts
**Description:** As a développeur sous Windows, I want que le re-pin et ses nouvelles options se comportent sous Windows comme sous Linux so that la release Windows n'embarque pas de régression vue seulement après publication.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-014, US-016

**Acceptance Criteria:**
- [x] Le job « Windows x86_64 libghostty check » passe sur le dernier commit de la branche qui porte EP-001 à EP-005.
- [x] Vérifié sur le matériel Windows réel (dual boot d'Arthur), sur un build debug lancé par `scripts/dev.ps1` : agrandir puis réduire un pane qui exécute `dir /s` ne décale plus la sortie suivante (US-014). La PR le dit.
- [x] Échec : given un défaut constaté sous Windows, when il est corrigé, then le correctif reste dans cet epic, un test de non-régression l'accompagne s'il est reproductible sous Linux, sinon la PR décrit la vérification manuelle refaite.

## Functional Requirements

- FR-01: Le manifeste, les bindings, les en-têtes, les quatre archives et `GHOSTTY_SHA` désignent tous `b699ea79f4b881421b4b3055abc16a0957d76beb`.
- FR-02: La construction d'un terminal échoue avec `AbiMismatch` si un discriminant ou un layout listé en US-003 diffère de `ghostty_type_json()`.
- FR-03: Sous mode 2026, le desktop publie la frame capturée au début de chaque hold, et jamais une frame partielle tant que la hold dure moins de 150 ms.
- FR-04: Quand une hold dépasse 150 ms, le desktop remet le mode 2026 à zéro et publie le contenu vivant.
- FR-05: Le curseur de la souris prend la forme demandée par OSC 22, sauf sur la barre de défilement et sur un lien survolé avec Ctrl.
- FR-06: Le host tient au plus 256 enregistrements OSC 7501 par session, selon les règles de la spécification.
- FR-07: Un enregistrement OSC 7501 décide l'état d'écran d'une session avant les règles textuelles. Il ne l'emporte jamais sur les hooks.
- FR-08: Exactement un terminal par session répond au PTY à `OSC 7501 ; ?` et à DECRQCRA.
- FR-09: Le système ne doit PAS répondre à DECRQCRA tant que `terminal.xt_checksum_report` n'est pas vrai.
- FR-10: Le système ne doit PAS déclencher d'action sur un agent à partir d'un état OSC 7501.
- FR-11: Tout terminal alimenté par ConPTY a `RESIZE_PULL_SCROLLBACK` à faux ; tous les autres gardent la valeur par défaut.
- FR-12: Un OSC inconnu est journalisé au niveau `debug`, échappé et tronqué à 4 096 octets.

## Non-Functional Requirements

- **Performance :**
  - publication d'un snapshot sans overscan : écart de 0 % au-delà du bruit de `perf-gates` par rapport au pin `0c2a290d` ;
  - avec overscan `{1, 1}` : au plus 5 % de surcoût sur `scripts/bench-terminal.sh` ;
  - le callback render hold capture une frame en moins de 2 ms au p95 sur un écran de 200 x 60 en release, mesuré par le harnais de `src-app/src/terminal/perf_bench.rs`.
- **Security :**
  - DECRQCRA est désactivé par défaut, et 0 réponse de checksum sort sans opt-in (test de FR-09) ;
  - le texte OSC 7501 affiché hors du terminal est débarrassé des caractères U+202A-202E, U+2066-2069 et U+200B-200F ;
  - chaque chaîne copiée depuis un callback est bornée : 64 Kio par chaîne, 2 048 octets par message OSC 7501 côté upstream.
- **Reliability :**
  - 0 panique sur les scénarios de redimensionnement d'US-004 répétés 1 000 fois ;
  - une hold jamais relâchée ne gèle pas un pane plus de 150 ms ;
  - un callback qui panique termine la session avec `CallbackPanicked`, sans abort du processus.
- **Scalability :** 256 enregistrements OSC 7501 au plus par session ; 64 événements de statut en attente au plus par terminal avant `EffectsOverflow`.
- **Compatibility :** les quatre cibles livrées (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`) construisent et passent leur lane libghostty. Windows est vérifié sur le matériel réel en EP-006 (US-017).
- **Accessibility :** `reduce_motion: true` désactive 100 % du décalage en pixels d'US-008.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Programme sans OSC 7501 (état vide) | Session ordinaire, aucun rapport | Le détecteur d'écran garde ses règles textuelles actuelles, sans changement | Aucun |
| 2 | Hold jamais relâchée | TUI qui plante pendant `CSI ? 2026 h` | Mode remis à zéro à 150 ms, contenu vivant publié | Aucun |
| 3 | Hold relâchée et reprise dans le même write | Programme qui redessine en continu | La frame complète entre les deux holds est publiée | Aucun |
| 4 | Redimensionnement ou RIS pendant une hold | Glisser un séparateur pendant un redessin | La hold se termine, le contenu vivant est publié | Aucun |
| 5 | Réduction de colonnes qui coupe un caractère large | Pane réduit sur une ligne d'emoji | La tête orpheline est effacée, aucune panique | Aucun |
| 6 | Host à l'ancien pin, desktop au nouveau | Mise à jour avec des sessions vivantes | Session `Incompatible` avec Retry et Stop host and restart (`host_link.rs:252`) | Libellés existants de la carte de fin de session |
| 7 | Rafale d'enregistrements OSC 7501 | Programme qui émet 10 000 ids | Éviction du plus ancien au-delà de 256 | Aucun |
| 8 | Programme qui sort en `blocked` | Agent tué pendant une demande de permission | `working` et `blocked` purgés à la sortie | Aucun |
| 9 | Texte OSC 7501 piégé | `msg` avec U+202E | Caractères retirés avant la sidebar et la notification | Notification nettoyée, titre qui nomme le pane |
| 10 | DECRQCRA sans opt-in | Programme qui sonde l'écran | Aucune réponse | Aucun |
| 11 | `xt_checksum_extension` hors plage | `32` dans `paneflow.json` | Valeur rejetée, valeur précédente gardée | Erreur de config qui nomme la clé et la plage 0-31 |
| 12 | Forme de pointeur inconnue | Version future de la bibliothèque | `IBeam` | Aucun |
| 13 | Viewport en bas pendant un défilement fluide | Trackpad vers le bas en fin de scrollback | Décalage à 0, pas de rebond | Aucun |
| 14 | uucode indisponible sur `github.com` | Panne réseau pendant le bump | Pas de pré-release, story `BLOCKED` avec le log | Aucun (CI) |
| 15 | Compression non supportée | Plateforme sans compression de scrollback | Option acceptée sans effet | Aucun |

Catégories écartées :
- états de chargement : aucune opération asynchrone visible n'est ajoutée ;
- dégradation réseau côté utilisateur : rien ne passe par le réseau à l'exécution ;
- changement de permissions en cours de session : aucune notion d'accès n'est touchée ;
- modifications concurrentes entre utilisateurs : Paneflow est mono-utilisateur. La concurrence interne (hold contre redimensionnement) est couverte par le cas 4.

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Le build `b699ea79` échoue sur une cible (transition translate-c `fe9cf6a26`, uucode sur `github.com`) | Med | High | `dry_run` d'abord (US-001), story `BLOCKED` avec le log, pas de contournement local |
| 2 | La capture dans le callback render hold crée un aliasing ou un deadlock | Med | High | Render state dédié dans `CallbackState`, solution de repli par `vt_write_until_ground` (US-005), garde de panique existante |
| 3 | Régression de rendu sous mode 2026 (pane figé) | Low | High | Plafond de 150 ms conservé, tests de fin de hold sur redimensionnement et RIS, passe visuelle |
| 4 | Les largeurs Unicode 18 déplacent des golden ou l'alignement de cellules | Med | Med | Golden relus en US-004, point de code consigné, aucun golden réécrit sans relecture |
| 5 | Réponses doublées au PTY (OSC 7501, DECRQCRA) | Med | Med | Callbacks installés sur le seul terminal du host, test « exactement une réponse » |
| 6 | OSC 7501 contredit les hooks et fait clignoter l'état | Low | Med | Précédence `Hook > Terminal` inchangée, test dédié en US-011 |
| 7 | Fuite d'écran par DECRQCRA activé | Low | High | Désactivé par défaut, avertissement documenté, opt-in par clé explicite |
| 8 | Les baselines d'allocations de `perf-gates` cassent avec le moteur | Med | Low | Rebaseline en commit séparé depuis un arbre propre, commits upstream nommés |
| 9 | Ghostty quitte GitHub et le dépôt source bouge (signalé par la recherche, non confirmé pour ce dépôt) | Low | Med | `ls-remote` sur `github.com/ghostty-org/ghostty` vérifié le 2026-10-07. `source_repository` du manifeste à revoir au prochain bump si le dépôt déménage |
| 10 | Le défilement fluide coûte trop de frames | Med | Low | P2, mesure exigée, `reduce_motion`, report possible sans bloquer l'epic |

## Non-Goals

- **Pas de wrapper pour le parseur OSC autonome** (`ghostty_osc_set`, `ghostty/include/ghostty/vt/osc.h:180-206,330`). Paneflow n'appelle aucune fonction `ghostty_osc_*`. La capture des OSC inconnus passe par le terminal (US-013).
- **Pas d'interface OSC 133.** Les marques de commande ont été supprimées le 2026-06-11 et ne reviennent pas. OSC 133 sert seulement de donnée interne (purge des enregistrements OSC 7501).
- **Pas de redimensionnement de fenêtre par CSI 8 t.** lib-vt l'ignore (`ghostty/src/terminal/stream_terminal.zig:779-783`) ; seule l'app Ghostty l'implémente (`714fe9b90`).
- **Pas de correctifs du mode contrôle tmux** (`e8b858cc6`, `1ca2858d0`, `a41248013`) : la surface C n'expose pas tmux. Ils arrivent dans l'archive sans consommateur.
- **Pas de cache de lignes indexé par identifiant de ligne.** Upstream marque toutes les lignes dirty après un défilement (`ghostty/include/ghostty/vt/render.h:178-182`), donc le cache ne gagnerait rien aujourd'hui. À revoir quand upstream affinera la dirtiness.
- **Pas d'interprétation du `title` et du `message` OSC 7501 au-delà de l'affichage** (`terminal.h:1286-1290`).
- **Pas de reprise automatique des re-pins hebdomadaires.** Ils restent coupés (`ae27f5ed`) ; réactiver le planning est une décision séparée.

## Files NOT to Modify

- `native/libghostty/bindings.rs` et `native/libghostty/prebuilt/*/bindings.rs` : générés par `scripts/generate-libghostty-bindings.sh`, jamais édités.
- Les hashes de `native/libghostty/manifest.toml` : réécrits par `scripts/repin-libghostty-manifest.sh`, jamais à la main. Seules les entrées `[[licenses]]` de uucode et de l'UCD sont éditées (US-002).
- `native/libghostty/prebuilt/*/lib/` : assets de release, hors git.
- `rust-toolchain.toml` et les quatre `rev` GPUI de `src-app/Cargo.toml` : hors du périmètre.
- `bench/baselines/**` : seulement par `--set-baseline` ou `--refresh-alloc-baselines` depuis un arbre propre (US-002).
- `crates/paneflow-ipc-client/src/agent.rs` : les variantes wire d'`AgentStateSource` et d'`AgentState` ne changent pas.
- `src-app/src/terminal/types.rs::alacritty_is_absent_from_the_app_crate` : garde existante, à laisser verte.
- `/home/arthur/dev/ghostty` : clone de référence en lecture seule, jamais patché.

## Technical Considerations

- **Render hold et propriété du render state :** le render state capturé vit-il dans `CallbackState` (recommandé), ou dans un objet partagé entre `SnapshotCache` et le callback ? À confirmer en lisant `crates/paneflow-terminal-ghostty/src/snapshot.rs`, `callbacks.rs:213-218` et l'ordre de destruction dans `Drop`. La solution de repli par `ghostty_terminal_vt_write_until_ground` reste ouverte si la capture dans le callback s'avère impossible.
- **Mapping OSC 7501 vers les états d'écran :** l'état d'écran sait-il porter une erreur, ou `error` doit-il retomber sur `idle` ? Recommandé : réutiliser l'état d'erreur s'il existe dans le classifieur (`crates/paneflow-serve/src/state.rs`), sans nouvelle variante wire.
- **Terminaux du desktop qui rejouent ConPTY :** quels terminaux du desktop rejouent le flux brut plutôt qu'un snapshot (`display_runtime.rs`, `attached_runtime.rs`, `pty_runtime.rs`) ? Le réglage d'US-014 doit couvrir exactement ceux-là.
- **Overscan dans le contenu publié :** les lignes d'overscan vont-elles dans des vecteurs séparés du `Content` (recommandé, pour ne pas toucher l'indexation de `CellMirror`), ou dans le même tableau avec un décalage ?
- **Consommateurs supplémentaires de `Reset` :** la recherche native et la sélection ont-elles besoin d'une invalidation explicite après le RIS d'un programme, ou le marquage dirty upstream suffit-il ? À vérifier par un test avant d'ajouter du code.
- **Dépendances :** aucune nouvelle crate. La version de bindgen reste celle de `scripts/generate-libghostty-bindings.sh`.
- **Migration :** aucune donnée persistée ne change de format, car le format de snapshot est inchangé (`snapshot.h:186`). Le retour arrière revert la branche ; la pré-release `libghostty-vt-b699ea79...` peut rester publiée ou être supprimée à la main.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Commits libghostty-vt de retard sur `main` | 57 (322 au total) | 0 au merge | Month-1 | `git rev-list --count <pin>..origin/main` dans le clone Ghostty |
| Nouvelles API de l'intervalle couvertes par un test Paneflow | 0 / 16 | 16 / 16 | Month-1 | Ancres d'US-003 et tests d'US-004 à US-016 |
| Frames perdues, scénario « relâche et reprend » | Frame B jamais publiée avec le sondage | 0 perte sur 1 000 itérations | Month-1 | Test d'US-005 |
| Plantages au redimensionnement signalés | Exposition au défaut `e6db5b633` | 0 | Month-6 | Issues GitHub et logs de crash |
| Sessions sans hooks dont l'état vient d'OSC 7501 quand émis | N/A (new) | 100 % | Month-1 | Test d'US-011 et champ `program_status` du statut du host |
| Réponses DECRQCRA sans opt-in | N/A (new) | 0 | Month-1 | Test d'US-012 |

## Open Questions

Toutes les questions sont tranchées (version 1.2).

- La mise à jour de l'app arrête-t-elle le host avant de relancer le desktop, ou l'utilisateur voit-il « Stop host and restart » sur chaque session après la mise à jour ? Réponse attendue d'Arthur avant la release qui embarque le re-pin ; la note d'Upgrade d'US-002 en dépend.
  - **Réponse :** la mise à jour depuis l'app arrête le host. `update_restart_plan` (`src-app/src/app/quit_dialog.rs:63-85`) choisit `StopEverything` quand aucune session ne tourne ou que la politique de sortie est `Stop`, et sinon pose la question de sortie habituelle. Seuls une mise à jour hors de l'app (paquet, Homebrew, MSI lancé à la main) ou le choix « Keep sessions running » laissent un host de l'ancien pin, dont les sessions finissent `Incompatible`. La note d'Upgrade du CHANGELOG le dit (US-017 de la remédiation).
- Garde-t-on le plafond de hold à 150 ms, ou passe-t-on à la seconde recommandée par upstream (`terminal.h:1717-1725`) ? Décision d'Arthur pendant la revue d'US-005. Le PRD garde 150 ms par défaut.
  - **Réponse :** 150 ms est gardé (`SYNC_OUTPUT_MAX_HOLD`, `src-app/src/terminal/ghostty_session/mod.rs:80`), mesuré depuis la capture du début de la hold.
- Les états `idle` et `done` d'OSC 7501 doivent-ils déclencher la notification de fin de tour existante, ou seulement `blocked` ? À trancher par Arthur à la passe visuelle d'US-011. Le PRD ne notifie que sur `blocked`, derrière la porte existante.
  - **Réponse :** seul `blocked` notifie. `idle` et `done` ne notifient jamais ; `error` ne notifie pas non plus, il entre seulement dans l'Attention Queue pour un programme sans agent et donne `Errored` pour un agent (US-010 et US-011 de la remédiation).
- Le défilement fluide (US-008) est-il voulu dans cette release, ou reporté après mesure ? Décision d'Arthur après US-007.
  - **Réponse :** le défilement fluide est livré, et `reduce_motion` le désactive : le décalage au pixel vaut 0 dès la frame suivante, et il vaut aussi 0 tant que le programme lit la souris ou que l'écran alternatif défile en alterné (US-005 de la remédiation).

## Amendements de clôture (1.2)

- **`stub.rs` caduc.** Les critères qui demandent un pendant de chaque méthode du wrapper dans `crates/paneflow-terminal-ghostty/src/stub.rs` (Quality Gates, US-009, US-014) ne s'appliquent plus : le fichier a été supprimé en `4752a664` (2026-09-25), avant ce PRD, et libghostty est l'unique moteur, lié statiquement sur chaque cible livrée.
- **OSC 22 à travers un RIS.** La forme de pointeur demandée par OSC 22 survit à un RIS (`ESC c`), comme upstream : `fullReset` (`ghostty/src/terminal/Terminal.zig:4983-5026`) ne touche pas `mouse_shape` (`:94`). Seul un reset manuel par `DisplayTerminal::reset` la ramène à `text`. Le test `osc_22_sets_the_mouse_shape_until_the_next_request_or_a_reset` (`crates/paneflow-terminal-ghostty/tests/display_terminal.rs:338`) fixe les deux cas.
- **Config d'`xt_checksum_extension` (amende US-012).** Le critère d'échec d'US-012 (« le loader la rejette … La valeur précédente reste en vigueur ») est remplacé par la règle d'US-014 de la remédiation : une valeur hors de 0 à 31 (par exemple `32`, `-1`, `1.5` ou `"4"`) ne retire que cette clé, qui retombe à 0, avec un avertissement qui nomme la clé et la plage 0-31. Le reste de `paneflow.json`, `xt_checksum_report` compris, s'applique, au démarrage comme au rechargement par le watcher. Le host suit la même règle pour les nouvelles sessions.
- **Terminaux alimentés par ConPTY (US-014).** `RESIZE_PULLS_SCROLLBACK` vaut `!cfg!(windows)` (`crates/paneflow-host/src/pty.rs:4`) et s'applique à exactement deux terminaux :
  - celui du host, créé par `new_terminal` (`crates/paneflow-host/src/runtime.rs:1045`) ;
  - ceux du desktop qui rejouent un flux PTY, par `follow_pty_scrollback_policy` (`src-app/src/terminal/ghostty_session/mod.rs:1289`) : le runtime PTY local (`pty_runtime.rs:376`) et le runtime attaché à une session du host (`attached_runtime.rs:232`).

  Le runtime d'affichage seul (`display_runtime.rs`, pane d'échec de lancement et surfaces sans PTY) n'est alimenté par aucun PTY et garde la valeur par défaut.
- **Mesure d'US-016.** Commande qui la reproduit :

  ```bash
  cargo test -p paneflow-terminal-ghostty --features native --locked --test display_terminal a_compressed_restore_of_fifty_thousand_lines_holds_less_resident_memory -- --nocapture
  ```

  Sur Linux x86_64 le 2026-10-09, pour 50 000 lignes restaurées, `primary_resident_bytes` passe de 34 922 496 octets sans l'option à 1 339 578 avec, 86 pages sur 87 compressées. Le fichier de test ne compile que sous Linux et Windows x86_64 MSVC, donc la mesure n'existe pas pour macOS.
- **Compression non supportée (inspection).** Le cas n'est pas forçable par le wrapper à la restauration : `SnapshotDecoder::set_compress_history` (`crates/paneflow-terminal-ghostty/src/snapshot_codec.rs:133-142`) transmet l'option à upstream, dont l'en-tête (`ghostty/include/ghostty/vt/snapshot.h:186-188`) dit qu'elle est acceptée sans effet sur une plateforme sans compression. La restauration (`src-app/src/terminal/ghostty_session/attached_runtime.rs:217-219`) ne change donc pas de comportement. Le test de mémoire ci-dessus exige alors une restauration identique à celle sans l'option (`assert_eq!(compressed, plain)`). La recompression au repos, ajoutée par la remédiation, s'arrête sans erreur sur `CompressionProgress::Unsupported` (`src-app/src/terminal/ghostty_session/recompression.rs:68`), cas forcé par `an_unsupported_compression_stops_the_idle_step_without_an_error`.
[/PRD]
