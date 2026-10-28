[PRD]
# PRD: Bump de GPUI vers zed@37faef81

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-28 | Arthur Jean | PRD initial : bump de GPUI de `0418cb5b` vers `37faef81`, migration des API clavier, défilement et shaping, requalification Linux et macOS, validation Windows sur matériel réel en dernier epic. |

## Problem Statement

Paneflow consomme `gpui` et `gpui_platform` par révision git exacte de `zed-industries/zed` (`src-app/Cargo.toml:45,46,254,399`), actuellement `0418cb5b60e89278e95af1a7b73382343e122ba8` (2026-10-06). Le crate reste en version `0.2.2` aux deux révisions : il n'existe aucune nouvelle version semver, seulement de la dérive upstream.

1. Paneflow ne profite pas de correctifs upstream qui touchent ses cibles livrées. Zed `main` à `37faef81f74fc1bb08be9939c4cffc8ede6ced2f` (2026-10-27) a 309 commits d'avance, dont 51 sur les crates `gpui*` (132 fichiers, +15 177/-4 821). Parmi eux :
   - **#65649 :** panic Vulkan « SwapchainAcquireSemaphore still in use » à la destruction d'une surface sous Linux.
   - **#65269 :** sous Linux, les polices de repli sans glyphe `m` (emoji, Nerd Font) disparaissent pour le reste de la session.
   - **#66159 :** crash du shaping Windows sur U+2028/U+2029.
   - **#66206 :** fuite d'un `TextSystem` à travers les wrappers de lignes.
2. Le bump n'est pas une simple substitution de révision. L'analyse vérifiée du 2026-10-28 a relevé :
   - **Des ruptures d'API à la compilation :**
     - `Keystroke::parse(&'static str)`, `unparse(os)` et `KeyBinding::load(.., cx: &App)` (#65970) ;
     - `PlatformInput::ScrollWheel` renommé en `Scroll`, avec un nouveau champ `ScrollEvent::source` (#65805) ;
     - suppression de `shape_line_by_hash` (#66152).
   - **Une rupture qui ne se voit qu'au runtime :** `Keystroke::parse` refuse désormais `secondary-` sans OS (`zed/crates/gpui/src/platform/keystroke.rs:159`) et `KeyBinding::new` fait `.unwrap()` dessus (`binding.rs:47`). Les 8 bindings `KeyBinding::new("secondary-…")` de `src-app/src/app/diff_dock/code/view/keymap.rs:67,73-78` et `src-app/src/widgets/text_area.rs:66` feraient paniquer Paneflow au démarrage.
3. Le bump change des comportements que la compilation ne détecte pas :
   - **Mapper clavier Windows :** `KeyBinding::load` utilise désormais le mapper de la plateforme. Sous Windows, c'est `WindowsKeyboardMapper` et non plus `DummyKeyboardMapper` (`src-app/src/keybindings/apply.rs:45`).
   - **Plafond à 30 fps** des frames d'animation en mode économie d'énergie, sans opt-out (#63477, `zed/crates/gpui/src/window.rs:1326`).
   - **Compositing translucide** en source-over (#65571).
   - **Taffy** passe de 0.13 à 0.15 (#65950).
   - **Fermeture native des fenêtres** différée d'une tâche (#66181).
   - **wgpu 30 en pin git** (`gfx-rs/wgpu@cddb2f5d`). Zed #65960, toujours ouverte, signale que Linux n'utilise plus le backend OpenGL.
4. Trois gates CI échoueraient indépendamment de la compilation :
   - **`cargo deny check sources` :** `gfx-rs/wgpu` et `zed-industries/oo7` sont absents de `allow-git` (`deny.toml:144-155`, avec `unknown-git = "deny"` à `deny.toml:132`).
   - **clippy `-D warnings` :** il rejette les alias dépréciés `ScrollWheelEvent` et `on_scroll_wheel`.
   - **Gate d'allocation de l'éditeur :** sa tolérance est de 1 % (`crates/paneflow-host/tests/persistent_baseline/gates.rs:3`).

**Why now:** l'écart n'a que trois semaines (309 commits). Les bumps précédents en couvraient 700 puis 609 ; chaque bump différé cumule les ruptures et rallonge la passe Windows. La rupture clavier #65970 devra être absorbée quel que soit le moment. Les correctifs upstream ci-dessus réduisent des crashes déjà possibles chez les utilisateurs Linux et Windows. Aucune échéance externe n'impose une date ; ce PRD n'invente pas d'urgence commerciale.

## Overview

Paneflow passe de `0418cb5b` à `37faef81` en trois epics. EP-001 bascule les manifestes, le lockfile et la politique de sources, puis migre chaque API rompue dans un seul commit atomique, `chore(deps): bump GPUI to zed@37faef81`, parce qu'aucun état intermédiaire ne compile. C'est le précédent de `8bd1c410`. EP-002 requalifie les performances (baselines, gate d'allocation) et les changements de comportement sur Linux et macOS. EP-003, en dernier comme pour chaque PRD, valide Windows sur matériel réel.

**Décisions de conception :**
- **Résolution de l'OS :** Paneflow résout l'OS hôte à la compilation (`cfg!`), comme le fait déjà `format_keystroke` (`src-app/src/keybindings/display.rs:70`), et appelle `Keystroke::parse_with_os`. Seul `KeyBinding::load` reçoit `cx`, parce que son API l'exige.
- **Bindings `secondary-` des widgets :** ils passent dans des blocs `#[cfg(target_os = "macos")]` (`cmd-`) et `#[cfg(not(target_os = "macos"))]` (`ctrl-`), selon le motif existant (`text_area.rs:71`, `keymap.rs:81`).
- **Éditeur de code :** il réutilise une `SharedString` par ligne visible pour garder les allocations d'une frame tiède au niveau de la baseline.
- **Patchs et sources :** Paneflow recopie les patchs Zed de la closure gpui (async-task, khronos-egl) et autorise chaque nouvelle source git par dépôt exact, jamais par organisation.

**Ce que ce PRD ne fait pas :** il ne forke pas GPUI, ne patche pas localement le plafond à 30 fps et ne change pas de source (`gpui-pre`). Si un risque bloquant se confirme, notamment le repli GL ou la latence en économie d'énergie, le retour arrière consiste à revenir au pin `0418cb5b`, ce qu'un `git revert` des commits du bump suffit à faire.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Paneflow compile et passe tous ses gates sur `37faef81` | 100 % des jobs de `run_tests.yml` verts sur la branche, 4 cibles | Pin toujours sur une révision dont tous les jobs sont verts |
| Aucun crash introduit par le bump | 0 panic au démarrage et pendant les parcours de validation sur Linux, macOS (CI) et Windows (matériel) | 0 rapport de crash attribué au bump |
| Aucune régression de performance non expliquée | 0 dépassement de tolérance non documenté dans les suites terminal, editor, startup et alloc | Baselines Linux à jour sur le commit du bump |
| Correctifs upstream livrés et vérifiés | 3 correctifs vérifiés : polices de repli Linux, U+2028 Windows, teardown Vulkan | Inclus dans la release suivante |

## Target Users

### Développeur sous Linux qui pilote des agents
- **Role:** utilisateur de Paneflow sur Fedora, Ubuntu, Arch ou openSUSE, Wayland ou X11.
- **Behaviors:** plusieurs panes, une police Nerd Font ou emoji en repli, des sessions longues, parfois un portable en profil `power-saver`.
- **Pain points:**
  - des glyphes de repli qui disparaissent après le premier rendu ;
  - un risque de panic Vulkan à la fermeture d'une surface ;
  - les machines sans Vulkan dépendent du repli GL.
- **Current workaround:** retirer les polices de repli ou relancer l'application.
- **Success looks like:** les glyphes de repli restent affichés toute la session, aucun panic à la fermeture, et l'application démarre aussi sans Vulkan.

### Développeur sous Windows
- **Role:** utilisateur Windows 10/11 x64, éventuellement avec une disposition non US (AZERTY).
- **Behaviors:** raccourcis par défaut et surcharges dans `paneflow.json`, sortie d'agents contenant du texte Unicode arbitraire.
- **Pain points:**
  - un crash possible sur U+2028/U+2029 ;
  - des raccourcis à symboles shiftés qui ne suivent pas la disposition active.
- **Current workaround:** aucun pour le crash ; pour les raccourcis, des combinaisons sans symbole.
- **Success looks like:** aucun crash sur ces caractères, et les raccourcis listés dans les réglages se déclenchent sur US comme sur AZERTY.

### Mainteneur de Paneflow
- **Role:** Arthur, qui bumpe GPUI, maintient la CI à quatre cibles et qualifie les releases.
- **Behaviors:** bump par révision exacte, gates `--locked`, Windows en dual-boot, macOS via la CI.
- **Pain points:**
  - des ruptures découvertes job par job en CI ;
  - du code `cfg(windows)` qui n'est linté que sur le leg Windows ;
  - des ruptures runtime que `cargo build` ne voit pas.
- **Current workaround:** relire le diff Zed à la main, puis corriger au fil des échecs CI.
- **Success looks like:** un bump planifié, un commit atomique, des baselines expliquées, une passe Windows unique.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Zed :** suit `main`. Les PR de ce bump y sont toutes mergées entre le 2026-10-07 et le 2026-10-27. #65960 (le repli GL Linux) est ouverte et absente de `37faef81`.
- **longbridge/gpui-component :** a quitté le pin git de Zed pour des snapshots crates.io `gpui-pre` pinnés exactement (`=0.3.10`), sans `[patch]` ([Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-component/main/Cargo.toml)). Paneflow reste sur la source git Zed : changer de source est une décision stratégique hors de ce PRD.
- **Forks aval de Zed :** ils ont intégré ces PR en quelques jours. L'un a dû retirer le pin git wgpu faute de pouvoir le récupérer ([#65649](https://github.com/zed-industries/zed/pull/65649)).
- **crates.io :** `gpui` y est toujours en 0.2.2, publiée le 2025-10-22 ([crates.io](https://crates.io/api/v1/crates/gpui)).
- **Market gap :** sans objet pour un bump. L'enjeu est de ne pas laisser la dérive upstream rendre le prochain bump plus coûteux que celui-ci.

### Best Practices Applied
- **Pins et patchs :** pinner chaque source git par SHA de 40 caractères et recopier les `[patch.crates-io]` de la closure de dépendances. Les patchs d'une dépendance ne se propagent pas au consommateur.
- **`allow-git` :** cargo-deny compare les URL exactement, au suffixe `.git` près (doc cargo-deny, `docs/src/checks/sources/cfg.md`). `allow-org.github` accorde la confiance à une organisation entière, d'où le choix de dépôts exacts.
- **Lockfile :** mise à jour ciblée (`cargo update -p gpui -p async-task`) plutôt que `generate-lockfile`, qui bumperait toutes les dépendances crates.io.
- **wgpu :** 30.0.1 est la dernière version crates.io et ne contient pas wgpu#10230 (fusionnée sur trunk le 2026-09-12). Le pin git ne peut donc pas être évité aujourd'hui ([wgpu releases](https://github.com/gfx-rs/wgpu/releases)).
- **Taffy 0.15 ([CHANGELOG](https://github.com/DioxusLabs/taffy/blob/main/CHANGELOG.md)) :**
  - `Position::Static` devient la valeur par défaut, mais GPUI garde `Relative` explicitement (`zed/crates/gpui/src/style.rs:783`).
  - Les boîtes absolues sont rattachées à leur bloc contenant.
  - `align_items`, `align_content` et `justify_content` ne sont plus des `Option`.
  - Leur valeur `Normal` n'est pas documentée pour flexbox.

*Sources complètes : PR Zed #65970, #65805, #63477, #65571, #65649, #65950, #66152 ; wgpu #10230, #10348 ; CHANGELOG Taffy ; docs cargo-deny.*

## Assumptions & Constraints

### Assumptions (to validate)
- **A1 :** `37faef81` compile avec la toolchain 1.98.0 de Paneflow. Zed utilise 1.98.1 aux deux révisions, et wgpu 30 déclare une MSRV de 1.87 sur crates.io ; la MSRV du pin git n'est pas vérifiée.
- **A2 :** la valeur `Normal` de Taffy 0.15 se résout comme l'ancien `None` pour les conteneurs flex, c'est-à-dire `stretch` et `flex-start`. C'est une inférence tirée de CSS, non documentée. Validation : US-010.
- **A3 :** `shape_line` sur une `SharedString` réutilisée atteint le cache de layout de GPUI sans allocation par ligne. Validation : US-005 et US-007, via le gate d'allocation.
- **A4 :** le plafond à 30 fps ne s'applique qu'aux frames portées par des callbacks d'animation ou de next-frame (`zed/crates/gpui/src/window.rs:1326-1356`), donc l'écho clavier n'est retardé que si une animation tourne. Validation : US-008.
- **A5 :** le repli OpenGL de wgpu 30 démarre toujours sous Linux. Zed #65960 laisse penser le contraire. Validation : US-009.
- **A6 :** le runner `macos-14` compile les shaders Metal de `gpui_apple`. `build.rs:142` n'impose que `-mmacosx-version-min=12.0`, et le SDK 26 n'est exigé que par `script/bundle-mac` de Zed. Validation : US-012.
- **A7 :** une entrée `allow-git` qui ne correspond plus à aucun crate ne produit qu'un avertissement. Non vérifié ; US-001 retire de toute façon les entrées mortes.
- **A8 :** la révision `gfx-rs/wgpu@cddb2f5d` reste récupérable depuis les runners GitHub. Validation : US-001.

### Hard Constraints
- **Cibles :** quatre cibles livrées (Linux x86_64 et aarch64, macOS aarch64, Windows x86_64), sans régression sur aucune (`AGENTS.md`, « Cross-platform compatibility is mandatory »).
- **Pas de fork :** ni fork de Zed, ni patch local de GPUI. Le fork a été supprimé le 2026-08-26 et GPUI reste pinné sur une révision upstream exacte.
- **Commandes cargo :** toujours en `--locked`. Le lockfile régénéré est commité avec le bump.
- **Toolchain :** pinnée en 1.98.0 (`rust-toolchain.toml`), sans `+stable`.
- **Code Windows :** le code `#[cfg(windows)]` n'est linté que par le job `Windows x86_64 libghostty check`. Tout nouvel item se déclare avant un `mod tests` existant (`clippy::items_after_test_module`).
- **Pas de commentaires** dans le Rust, le shell ou le PowerShell.
- **Binaires auxiliaires :** les plafonds de taille de `paneflow-shim`, `paneflow-ai-hook` et `paneflow-mcp` sont inchangés.
- **Baselines :** une baseline de bench refuse un arbre sale. On commite d'abord, on enregistre ensuite (`bench/README.md`).
- **Windows :** la validation sur matériel réel se fait dans le dernier epic. Chaque epic se construit sur Fedora.
- **Fusion :** le bump n'est fusionné sur `main` qu'après les stories P0 des trois epics. Aucun tag de release avant que les stories P1 soient certifiées.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre legs
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, y compris les tests et les avertissements de dépréciation
- `cargo test --workspace --locked` - tests unitaires et d'intégration, y compris le code `#[cfg(test)]` que `cargo build` ignore
- `cargo deny check advisories licenses sources` - sources git, licences et advisories, puisque ce PRD change les dépendances

For the merge commit and the stories that touch performance:
- `scripts/perf-gates.sh` - gates de frames et d'allocation Linux, identiques au job `Performance gates (Linux x86_64)`
- Le workflow `run_tests.yml` est vert sur la PR, y compris `macOS aarch64 smoke build`, `macOS aarch64 render smoke (visual)`, `Windows x86_64 libghostty check` et `Windows x86_64 render smoke (visual)`

For UI stories, additional gates:
- Captures avant (`0418cb5b`) et après (`37faef81`) des vues listées dans la story, comparées et jointes au rapport de la story
- Revue visuelle par Arthur des paires de captures en mode translucide

## Epics & User Stories

### EP-001: Basculer Paneflow sur zed@37faef81

Paneflow compile, démarre sans panic et passe ses gates Linux sur la nouvelle révision GPUI. Les stories de cet epic forment un seul commit atomique, parce qu'aucun état intermédiaire ne compile.

**Definition of Done:**
- Un commit `chore(deps): bump GPUI to zed@37faef81` est poussé sur la branche `chore/gpui-37faef81`.
- Les quatre gates de qualité passent sous Linux.
- Les jobs `Style (fmt + clippy)`, `Tests (Linux x86_64)`, `Release build + binary-size budget (Linux x86_64)` et `Security Audit (cargo-deny)` sont verts.
- Le binaire de release démarre et enregistre toutes ses keymaps sans panic.

#### US-001: Aligner manifestes, lockfile et sources autorisées
**Description:** As a mainteneur de Paneflow, I want que les manifestes, le lockfile et `deny.toml` pointent tous sur `37faef81` et sur ses patchs so that le build `--locked` et `cargo deny check sources` acceptent la nouvelle closure de dépendances sans dérive hors de gpui.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `src-app/Cargo.toml`, when on lit les lignes 45, 46, 254 et 399, then les quatre valeurs `rev` valent `37faef81f74fc1bb08be9939c4cffc8ede6ced2f`, et `gpui_platform` garde `features = ["font-kit"]` et `features = ["wayland", "x11"]`
- [ ] Given le `[patch.crates-io]` racine, when on le compare à celui de Zed à `37faef81`, then `async-task` pointe sur `https://github.com/zed-industries/async-task.git` rev `015845a19fc31752c02367567d3b8900a406f9da`, `khronos-egl` sur `https://github.com/zed-industries/khronos-egl` rev `42214b36347b108fd9630ca6254b96dde59e04fe`, et les revs de `calloop` et `async-process` sont identiques à celles de Zed
- [ ] Given le `[patch.crates-io]` de Zed, when on choisit les entrées à recopier, then aucune entrée hors de la closure de dépendances de gpui n'est ajoutée, en particulier `tree-sitter-language`
- [ ] Given `Cargo.lock` régénéré par une mise à jour ciblée sur `gpui` et `async-task`, when on compare les versions résolues des dépendances directes de Paneflow hors de la closure gpui, then aucune ne change
- [ ] Given `deny.toml`, when on lit `allow-git`, then il contient exactement les dépôts git présents dans `Cargo.lock` : ajout de `https://github.com/gfx-rs/wgpu`, `https://github.com/zed-industries/oo7`, `https://github.com/zed-industries/async-task.git` et `https://github.com/zed-industries/khronos-egl`, retrait de toute entrée qui ne correspond plus à un crate verrouillé, et aucune clé `allow-org`
- [ ] Given `src-app/tests/dependency_source_policy.rs`, when il lit le nouveau `Cargo.lock`, then chaque source git est une révision complète de 40 caractères et le test passe
- [ ] Given un cache cargo vide sur un runner GitHub Linux, when le job CI récupère les dépendances, then la révision `gfx-rs/wgpu@cddb2f5d80dc0009b2ef6767635140cacf90a007` est récupérée. En cas d'échec, la story passe BLOCKED avec le log de récupération, sans contournement par un autre dépôt.

#### US-002: Migrer l'analyse des raccourcis vers l'API clavier OS-aware
**Description:** As a utilisateur de Paneflow, I want que mes raccourcis par défaut et personnalisés, y compris `secondary-`, se résolvent pour mon OS so that chaque raccourci des réglages se déclenche et que les conflits restent détectés après le bump.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un build macOS, when le helper d'OS hôte de `keybindings` est appelé, then il renvoie `OperatingSystem::Mac` ; il renvoie `OperatingSystem::Windows` sur un build Windows et `OperatingSystem::Linux` sur tout autre build
- [ ] Given `canonical_keystroke` (`keybindings/apply.rs:12`), when il reçoit une chaîne dynamique, then il l'analyse avec `Keystroke::parse_with_os` et l'OS hôte
- [ ] Given un build Linux ou Windows, when on appelle `keystrokes_conflict("secondary-shift-d", "ctrl-shift-d")`, then le résultat est `true` ; sur un build macOS, `keystrokes_conflict("secondary-shift-d", "cmd-shift-d")` renvoie `true`
- [ ] Given `make_binding` (`apply.rs:23`), when il construit un binding, then il appelle `KeyBinding::load(.., cx)` avec l'`&App` reçu de `apply_keybindings` (`apply.rs:55`), et ses tests (`apply.rs:159,168,182,247,324`) s'exécutent avec un contexte de test GPUI
- [ ] Given les appels `unparse()` de `app/settings.rs:444,489` et `keybindings/display.rs:533`, when ils sont compilés, then ils passent l'OS hôte à `unparse`
- [ ] Given le test `every_default_chord_round_trips_through_parse` (`display.rs:529`), when il parcourt `DEFAULTS` et les defaults de la plateforme, then chaque chord, y compris les 48 `secondary-` de `DEFAULTS` (`defaults.rs:7-418`), s'analyse et fait l'aller-retour sans erreur
- [ ] Given la méthode IPC qui atteint `send_keystroke` (`terminal/view.rs:916`, via `app/ipc_handler/surface_methods.rs:1244`), when elle reçoit `"secondary-c"`, then la touche est résolue en `cmd-c` sur macOS et en `ctrl-c` ailleurs
- [ ] Given la même méthode IPC, when elle reçoit une chaîne invalide comme `"ctrl-"`, then l'appelant reçoit une erreur JSON-RPC qui nomme la chaîne, sans panic
- [ ] Given un `paneflow.json` contenant un raccourci invalide, when `apply_keybindings` s'exécute, then un avertissement nomme ce raccourci, il est ignoré, et tous les autres bindings sont appliqués

#### US-003: Lier les raccourcis secondaires des widgets par plateforme
**Description:** As a utilisateur de l'éditeur de code et des zones de texte, I want que copier, coller, annuler, rétablir, tout sélectionner, enregistrer et envoyer immédiatement restent liés à Cmd sur macOS et à Ctrl ailleurs so that Paneflow démarre sans panic et ces commandes fonctionnent comme avant.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `app/diff_dock/code/view/keymap.rs:67,73-78` et `widgets/text_area.rs:66`, when le code est compilé, then aucun `KeyBinding::new` ne reçoit de chaîne commençant par `secondary-`
- [ ] Given ces 8 bindings, when ils sont réécrits, then ils sont répartis entre un bloc `#[cfg(target_os = "macos")]` (`cmd-`) et un bloc `#[cfg(not(target_os = "macos"))]` (`ctrl-`), avec les mêmes actions et les mêmes contextes, selon le motif de `text_area.rs:71` et `keymap.rs:81`
- [ ] Given un contexte de test GPUI, when on enregistre les keymaps de l'éditeur de code et de la zone de texte, then l'enregistrement se termine sans panic
- [ ] Given ce même contexte, when on lit les bindings enregistrés, then `CeCopy` est lié à `cmd-c` sur macOS et à `ctrl-c` sur Linux et Windows
- [ ] Given un build Linux ou Windows, when l'utilisateur presse Super+C dans l'éditeur de code, then `CeCopy` ne se déclenche pas, et aucun binding `cmd-` de macOS ne s'y applique
- [ ] Given `app/diff_dock/new_tab_menu.rs:56,69`, when le menu d'onglet affiche `secondary-g` et `secondary-j`, then l'affichage passe toujours par `format_keystroke` (`display.rs:69`), et il est identique avant et après le bump : `⌘` sur macOS, `Ctrl` ailleurs

#### US-004: Migrer le défilement vers ScrollEvent
**Description:** As a mainteneur de Paneflow, I want que tout le code de défilement utilise `ScrollEvent`, `on_scroll` et `PlatformInput::Scroll` so that clippy `-D warnings` passe et que le défilement du terminal, du diff et des modales se comporte comme avant.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `src-app` et `crates`, when le code est compilé, then aucun usage de `ScrollWheelEvent`, `on_scroll_wheel` ou `PlatformInput::ScrollWheel` ne subsiste
- [ ] Given les gestionnaires de production de `terminal/view.rs:1762`, `terminal/input.rs:1307,1419`, `app/custom_buttons_modal.rs:716,927`, `app/diff_dock/mod.rs:695,726`, `app/diff_dock/code/view/render.rs:213` et `app/diff_dock/code/view/pointer.rs:17`, when ils sont migrés, then ils utilisent les nouveaux noms sans changement de logique
- [ ] Given les littéraux de test de `layout/render.rs:935,1042`, `terminal/view.rs:2401,2501` et `pointer.rs:231-236`, when ils construisent un `ScrollEvent`, then ils complètent les champs par `..Default::default()`, ce qui donne `ScrollSource::Pointer`
- [ ] Given le test de `pointer.rs:322`, when il construit l'entrée, then il utilise `PlatformInput::Scroll`
- [ ] Given un `ScrollEvent` avec `source: ScrollSource::Touch`, when il atteint un gestionnaire de Paneflow, then il est traité comme un défilement de pointeur, sans panic ni double défilement
- [ ] Given un défilement en pixels de précision, émis par un trackpad ou une molette à défilement lissé, when il atteint le terminal, then la logique de `terminal/input.rs:40` (seuil sur les deltas en pixels) et de `terminal/input.rs:1361` (`touch_phase`) produit le même nombre de lignes défilées qu'avant le bump, et un test existant ou ajouté le vérifie

#### US-005: Remplacer le shaping par hash de l'éditeur de code
**Description:** As a utilisateur du dock de code, I want qu'une frame tiède de l'éditeur reste à 6 allocations et 384 octets au plus, à 1 % près, so that la suppression de `shape_line_by_hash` n'ajoute ni allocation ni latence de frame.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `app/diff_dock/code/element.rs:799,858` et `app/diff_dock/code/perf_bench.rs:638`, when le code est compilé, then aucun appel à `shape_line_by_hash`, `layout_line_by_hash` ou `try_layout_line_by_hash` ne subsiste
- [ ] Given une frame tiède, sans changement du buffer ni du viewport, when l'éditeur prépeint 60 lignes, then les compteurs `materialized_lines` et `materialized_numbers` restent à 0
- [ ] Given une modification d'une seule ligne visible, when la frame suivante est prépeinte, then `materialized_lines` vaut 1 et la ligne affichée reflète le nouveau texte, sans contenu périmé
- [ ] Given le passage de 999 à 1000 lignes, ce qui élargit la gouttière, when la frame suivante est prépeinte, then chaque numéro visible est rematérialisé une fois puis plus jamais sur les frames tièdes suivantes
- [ ] Given les tests de `app/diff_dock/code/view/render.rs:343-416`, when ils s'exécutent, then ils vérifient les compteurs avec cette nouvelle définition (matérialisations de chaînes)
- [ ] Given `perf_bench.rs:567-580`, when on construit le `WindowTextSystem`, then il est détenu par un `Rc` et non plus par un `Arc`
- [ ] Given la métrique `prepaint_60_rows_warm` de `bench/baselines/linux-x86_64/editor-alloc.json`, when le gate d'allocation s'exécute, then elle ne dépasse pas la baseline (6 allocations et 384 octets par itération) de plus de 1 %

#### US-006: Aligner les références à gpui_apple et consigner le bump
**Description:** As a mainteneur de Paneflow, I want que le filtre de log, les notes de manifeste et le CHANGELOG reflètent la nouvelle révision so that les avertissements de police macOS restent filtrés et que les correctifs hérités soient annoncés.

**Priority:** P1
**Size:** XS (1 pt)
**Dependencies:** Blocked by US-002, US-003, US-004, US-005

**Acceptance Criteria:**
- [ ] Given `src-app/src/launch.rs:255`, when on lit le filtre par défaut, then il nomme `gpui_apple::text_system=error` au lieu de `gpui_macos::text_system=error`
- [ ] Given la note de `src-app/Cargo.toml:34-42` sur la feature `font-kit`, when on la relit, then elle nomme la chaîne `gpui_platform` → `gpui_macos` → `gpui_apple`, qui existe à `37faef81`
- [ ] Given `CHANGELOG.md`, section `[Unreleased]`, when on la lit, then elle annonce sous « Fixed » :
  - le maintien des polices de repli sous Linux ;
  - la fin du panic Vulkan à la destruction d'une surface ;
  - la fin du crash Windows sur U+2028/U+2029 ;
  - la fin d'une fuite mémoire du système de texte.
- [ ] Given cette même section, when on la lit, then elle annonce sous « Changed » :
  - des raccourcis Windows qui suivent la disposition clavier active ;
  - des animations limitées à 30 images par seconde en mode économie d'énergie.
- [ ] Given une mesure d'EP-002 ou d'EP-003 qui infirme une entrée du CHANGELOG, when la story concernée est terminée, then cette entrée est corrigée dans la même story

---

### EP-002: Requalifier performances et comportements sur Linux et macOS

Le bump ne dégrade ni la latence, ni les allocations, ni le rendu sous Linux, et il démarre aussi sans Vulkan. macOS est qualifié par la CI, faute de matériel garanti.

**Definition of Done:**
- Les baselines Linux sont enregistrées sur le commit du bump, chaque écart étant expliqué.
- Les spikes US-008 et US-009 ont un verdict écrit, GO ou NO-GO, assorti de mesures.
- La passe visuelle Linux est approuvée.
- Les deux jobs macOS de `run_tests.yml` sont verts.

#### US-007: Re-baseliner les suites de performance Linux
**Description:** As a mainteneur de Paneflow, I want des baselines Linux enregistrées sur le commit du bump, avec la cause de chaque écart so that les gates de performance comparent les futures modifications à un état connu.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given le commit du bump sur un arbre propre, when on exécute `scripts/bench-terminal.sh`, `scripts/bench-editor.sh` et `scripts/bench-startup.sh` sur Linux x86_64, then les résultats sont archivés dans `bench/results/` avec le SHA du commit
- [ ] Given chaque métrique comparée à l'ancienne baseline, when elle dépasse sa tolérance, then la cause est écrite (commit upstream ou changement Paneflow) avant tout rafraîchissement de la baseline
- [ ] Given une régression au-delà de la tolérance sans cause identifiée, when elle est constatée, then la baseline n'est pas rafraîchie et la story passe BLOCKED avec la mesure
- [ ] Given les baselines d'allocation `terminal-alloc.json` et `editor-alloc.json`, when un rafraîchissement est justifié, then il passe par `scripts/perf-gates.sh --refresh-alloc-baselines` sur un arbre propre, dans un commit séparé du bump
- [ ] Given les tests `every_committed_baseline_is_clean_current_and_on_its_platform` (crate `paneflow-app`) et `every_committed_persistent_baseline_is_clean_current_and_on_its_platform` (crate `paneflow-host`), when ils lisent `bench/baselines/`, then ils passent
- [ ] Given la PR, when le job `Performance gates (Linux x86_64)` s'exécute, then il est vert

#### US-008: Valider l'hypothèse que le plafond à 30 fps ne dégrade pas l'écho clavier
**Description:** As a utilisateur Linux sur portable en économie d'énergie, I want que le p95 de latence de saisie dans un terminal ne dépasse pas de plus de 16 ms celui du mode normal pendant qu'un agent tourne so that le plafond à 30 fps de GPUI ne se voie qu'aux animations décoratives.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given une machine Linux avec power-profiles-daemon, when on exécute le même scénario en profil `balanced` puis `power-saver` (spinner d'agent actif dans la sidebar, 200 frappes à 10 Hz dans un shell), then les deux exécutions sont mesurées avec `PANEFLOW_LATENCY_PROBE=1` (build debug) et avec `scripts/perf-hardware.sh --frame-log` (build release)
- [ ] Given la mesure en `power-saver`, when on lit les intervalles de frame pendant le spinner, then au moins 90 % sont de 33 ms ou plus, ce qui prouve que le plafond est actif et que la mesure le voit
- [ ] Given les deux exécutions, when on compare le p95 du temps entre le gestionnaire d'entrée et la fin de la peinture CPU, then l'écart `power-saver` moins `balanced` est de 16 ms au plus, et le verdict GO est écrit
- [ ] Given un écart supérieur à 16 ms, when le verdict est rendu, then il est NO-GO, avec un brouillon d'issue upstream Zed et une story de suivi Paneflow, et aucun contournement n'est codé dans ce PRD
- [ ] Given une machine sans power-profiles-daemon, when le scénario s'exécute, then la mesure est consignée `not_measured` avec sa raison, jamais à 0
- [ ] Given les fichiers de résultats, when la story est terminée, then ils sont archivés dans `bench/results/` selon le protocole matériel de `bench/README.md`

#### US-009: Valider l'hypothèse que le repli OpenGL démarre toujours sous Linux
**Description:** As a utilisateur Linux sans pilote Vulkan, par exemple en VM ou sur un vieux GPU, I want que Paneflow démarre sur le backend GL so that le passage à wgpu 30 ne m'empêche pas de lancer l'application.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given le build release de `0418cb5b` et `VK_ICD_FILENAMES` pointant vers un fichier inexistant, when on lance Paneflow, then on consigne si la fenêtre principale s'affiche en moins de 10 s et quel backend le log nomme, ce qui sert de référence
- [ ] Given le build release de `37faef81` et le même environnement, when on lance Paneflow, then la fenêtre principale s'affiche en moins de 10 s, le log nomme le backend GL et aucun panic n'est émis
- [ ] Given que le build de référence démarrait en GL et que le nouveau échoue, when le verdict est rendu, then il est NO-GO et la story passe BLOCKED avec le log, en citant les deux options : attendre Zed #65960, ou rester sur `0418cb5b`
- [ ] Given un démarrage GL réussi, when on ouvre deux panes et on ferme une fenêtre détachée, then aucun panic EGL n'est émis, ce qui couvre le patch `khronos-egl`

#### US-010: Passe visuelle Linux sur la translucidité, Taffy 0.15 et les polices de repli
**Description:** As a utilisateur Linux, I want que la mise en page et les surfaces translucides restent lisibles et à leur place après le bump so that le passage au compositing source-over et à Taffy 0.15 ne casse aucune vue.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given les builds de `0418cb5b` et `37faef81`, when on capture la fenêtre principale (sidebar et deux panes), les réglages, le dock de diff avec l'éditeur de code, la modale des boutons personnalisés et un menu contextuel, en thèmes sombre et clair, then on obtient 10 paires de captures en mode opaque
- [ ] Given chaque paire opaque, when on la compare avec `compare -metric AE` d'ImageMagick, then aucune zone ne change de géométrie, et toute différence de pixels est attribuée à une cause écrite
- [ ] Given les mêmes vues en mode translucide, when on capture les paires, then Arthur approuve chacune, et toute surface dont un texte passe sous un contraste de 4.5:1 fait l'objet d'un ajustement des alphas de `app/constants.rs`, justifié par les captures
- [ ] Given les sites `paint_path` à alpha partiel (`pane.rs:332`, `ui_primitives/squircle.rs:190,220`, `window_chrome/mod.rs:91`, `diff/element.rs:1478`, `terminal/element/paint/decorations.rs:131`, `settings/components.rs:372,377`), when on inspecte leurs captures translucides, then chacun figure dans au moins une capture revue
- [ ] Given `font_fallbacks` contenant une police de symboles Nerd Font et une police emoji, when on affiche un glyphe de chacune, qu'on ouvre un nouveau pane, puis qu'on y affiche les mêmes glyphes, then ils s'affichent dans les deux panes
- [ ] Given une police de repli configurée mais non installée, when Paneflow démarre, then un avertissement la nomme une seule fois, sans panic, et le rendu se poursuit avec la police principale

#### US-011: Vérifier le cycle de fermeture des fenêtres
**Description:** As a utilisateur qui détache des panes, I want que fermer une fenêtre ou quitter l'application sauve et restaure ma session comme avant so that le nettoyage différé introduit par #66181 ne perde aucun état.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given une fenêtre de pane détachée, when on la ferme, then `detached_window_closed` et le nettoyage des notifications (`app/window.rs:341-345`) s'exécutent, et un test GPUI le vérifie après que les tâches en attente ont été exécutées
- [ ] Given trois fenêtres ouvertes, when on quitte puis qu'on relance, then la session restaure les trois fenêtres et leurs panes
- [ ] Given un spinner d'agent actif, when on ferme sa fenêtre, then aucun panic d'emprunt de l'`App` n'est émis
- [ ] Given une fermeture suivie d'un arrêt immédiat du processus, when on relance, then `session.json` n'est ni tronqué ni invalide

#### US-012: Qualifier macOS via la CI
**Description:** As a utilisateur macOS, I want que le build macOS compile, rende du texte et filtre ses avertissements de police so that la migration objc2 et le déplacement vers `gpui_apple` n'affectent pas la release macOS.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given la PR, when le job `macOS aarch64 smoke build` s'exécute sur `macos-14`, then fmt, clippy `aarch64-apple-darwin`, test, check et build release sont verts, shaders Metal compris
- [ ] Given l'artefact du job `macOS aarch64 render smoke (visual)`, when Arthur ouvre la capture, then le texte du terminal y est rendu, et le contrôle OCR souple compte au moins 10 caractères alphanumériques
- [ ] Given le log de ce smoke, when on y cherche `gpui_apple::text_system`, then aucune ligne d'avertissement n'apparaît
- [ ] Given un échec de compilation des shaders lié au SDK du runner, when il est constaté, then la story passe BLOCKED avec le log, sans changement d'image de runner décidé dans ce PRD
- [ ] Given l'absence de matériel macOS, when la story est terminée, then le rapport indique que le Low Power Mode, le flou sous macOS 26/27 et le Display P3 n'ont pas été vérifiés sur matériel

---

### EP-003: Valider Windows sur matériel réel

Le leg Windows compile et linte le code `cfg(windows)` sur la nouvelle révision. Les raccourcis, la translucidité et les correctifs hérités sont vérifiés sur Windows 11, sur le dual-boot d'Arthur.

**Definition of Done:**
- Les jobs `Windows x86_64 libghostty check` et `Windows x86_64 render smoke (visual)` sont verts.
- La passe matérielle couvre les dispositions US et AZERTY, Mica, l'économiseur de batterie et U+2028.
- Le rapport précise ce qui a été vérifié par inspection et ce qui l'a été sur matériel.

#### US-013: Faire passer le leg Windows de la CI
**Description:** As a mainteneur de Paneflow, I want que le code `#[cfg(windows)]` compile et passe clippy sur la nouvelle révision so that les ruptures invisibles sous Linux soient corrigées avant la passe matérielle.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given la PR, when le job `Windows x86_64 libghostty check` s'exécute, then clippy `--target x86_64-pc-windows-msvc -D warnings`, test, check et build release sont verts
- [ ] Given tout item ajouté dans un fichier qui contient un `mod tests`, when on lit l'ordre des items, then l'item est déclaré avant ce module
- [ ] Given le code `#[cfg(windows)]` qui touche le clavier, le défilement ou le texte, when la story est terminée, then le rapport liste chaque fichier relu par inspection
- [ ] Given l'artefact du job `Windows x86_64 render smoke (visual)`, when Arthur ouvre la capture, then le texte du terminal y est rendu
- [ ] Given une erreur propre à Windows découverte par le job, when elle est corrigée, then le correctif reste dans le périmètre de cette story et n'ajoute aucun `#[allow]` sans raison écrite

#### US-014: Valider les raccourcis avec le mapper clavier Windows
**Description:** As a utilisateur Windows avec une disposition US ou AZERTY, I want que chaque raccourci affiché dans les réglages se déclenche avec la disposition active so that le passage au mapper de la plateforme améliore l'usage au lieu de casser des combinaisons.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given Windows 11 sur matériel réel en disposition US, when on déclenche chaque chord de `DEFAULTS` et `OTHER_PLATFORM_DEFAULTS`, then chaque action attendue s'exécute, et le rapport liste les chords testés
- [ ] Given la disposition AZERTY (France), when on refait le même parcours, then chaque action s'exécute. Tout chord dont la touche physique diffère de la disposition US est listé avec la combinaison effective.
- [ ] Given une surcharge `"ctrl-shift-d": "close_pane"` dans `paneflow.json`, when on l'utilise dans les deux dispositions, then le pane se ferme
- [ ] Given Paneflow en cours d'exécution, when on passe de US à AZERTY avec Win+Espace puis qu'on déclenche un chord à symbole shifté, then l'action suit la nouvelle disposition sans redémarrage
- [ ] Given un chord par défaut qui ne peut plus être produit dans une disposition, when il est constaté, then il est consigné comme finding avec la disposition et la touche, et la story passe IN_REVIEW et non DONE tant qu'il n'est pas résolu
- [ ] Given les raccourcis de l'éditeur de code et de la zone de texte (US-003), when on les utilise dans les deux dispositions, then copier, coller, annuler, rétablir, tout sélectionner, enregistrer et envoyer immédiatement fonctionnent avec Ctrl

#### US-015: Passe matérielle Windows sur Mica, l'économiseur de batterie, U+2028 et la fermeture
**Description:** As a utilisateur Windows, I want que le texte sur fond Mica ou flou garde un contraste d'au moins 4.5:1, que le p95 de latence de saisie sur batterie reste à 16 ms au plus de celui sur secteur, et que l'application survive aux séparateurs Unicode so that le bump n'introduise aucune régression propre à Windows.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given les backdrops `mica`, `blurred` et `acrylic` (`app/constants.rs:46,59-60`), when on capture la fenêtre principale et les réglages avant et après le bump, then Arthur approuve chaque paire, et toute surface dont un texte passe sous un contraste de 4.5:1 est corrigée par ses alphas
- [ ] Given un terminal qui affiche `printf '  '` puis la sortie d'un agent qui contient ces caractères, when ils sont rendus, then Paneflow ne plante pas et le texte suivant s'affiche
- [ ] Given l'économiseur de batterie activé et un spinner d'agent actif, when on reprend le scénario de saisie de US-008, then l'écart de p95 avec l'économiseur désactivé est de 16 ms au plus, ou un verdict NO-GO est écrit avec ses mesures
- [ ] Given une fenêtre de pane détachée, when on la ferme puis qu'on quitte et relance, then la session est restaurée sans panic
- [ ] Given la fermeture de session Windows (WM_ENDSESSION) avec Paneflow ouvert, when on se reconnecte et relance, then `session.json` est valide et restaure les fenêtres

## Functional Requirements

- **FR-01 :** Paneflow doit consommer `gpui` et `gpui_platform` à la révision `37faef81f74fc1bb08be9939c4cffc8ede6ced2f` dans les quatre déclarations de `src-app/Cargo.toml`.
- **FR-02 :** Le `[patch.crates-io]` racine doit reprendre les révisions Zed de `async-task`, `async-process`, `calloop` et `khronos-egl`, et aucun autre patch Zed.
- **FR-03 :** `deny.toml` doit autoriser chaque dépôt git présent dans `Cargo.lock` par son URL exacte, et seulement ceux-là.
- **FR-04 :** Toute chaîne de raccourci analysée à l'exécution (configuration, IPC, détection de conflit) doit l'être avec l'OS hôte, et `secondary-` doit donner `cmd-` sur macOS et `ctrl-` ailleurs.
- **FR-05 :** Le système ne doit jamais passer à `KeyBinding::new` une chaîne qui commence par `secondary-`.
- **FR-06 :** Lorsqu'un raccourci de configuration est invalide, le système doit l'ignorer, journaliser un avertissement qui le nomme et appliquer tous les autres.
- **FR-07 :** Lorsqu'un appel IPC transmet une touche invalide, le système doit renvoyer une erreur à l'appelant, sans panic.
- **FR-08 :** Le défilement doit produire, pour un même événement d'entrée, le même déplacement qu'avant le bump.
- **FR-09 :** Une frame tiède de l'éditeur de code ne doit matérialiser aucune chaîne de ligne ou de numéro.
- **FR-10 :** Le filtre de log par défaut doit réduire `gpui_apple::text_system` au niveau `error`.
- **FR-11 :** Les raccourcis Windows doivent se déclencher selon la disposition clavier active, y compris après un changement de disposition à chaud.
- **FR-12 :** Le système ne doit pas désactiver ni contourner le plafond à 30 fps de GPUI dans ce PRD.

## Non-Functional Requirements

- **Performance :**
  - `prepaint_60_rows_warm` reste à 6 allocations et 384 octets par itération au plus, à 1 % près (tolérance du gate d'allocation, `gates.rs:3`).
  - L'écart de p95 de latence entre le gestionnaire d'entrée et la fin de la peinture, mode économie d'énergie contre mode normal avec spinner actif, est de 16 ms au plus.
  - La médiane du temps de première frame (`startup.json`) reste à moins de 5 % au-dessus de la baseline Linux ; au-delà, l'écart doit être expliqué avant la fusion.
- **Fiabilité :**
  - 0 panic au démarrage sur les quatre cibles.
  - 0 panic pendant les parcours de US-009, US-011, US-014 et US-015.
  - Démarrage en moins de 10 s sur le backend GL quand Vulkan est absent.
- **Sécurité :**
  - 100 % des sources git du lockfile sont des révisions de 40 caractères (`dependency_source_policy.rs`).
  - 0 erreur `cargo deny` sur les advisories, licences et sources.
  - 0 clé `allow-org` ajoutée.
- **Accessibilité :**
  - 100 % des chords par défaut s'analysent sur chacun des trois OS.
  - Le texte des surfaces translucides garde un contraste d'au moins 4.5:1 sur les captures de US-010 et US-015.
- **Taille :**
  - Les plafonds de taille des binaires auxiliaires restent inchangés : `paneflow-shim` 512 KiB, `paneflow-ai-hook` environ 375 KB, `paneflow-mcp` 512 KiB.
  - Le job de budget de taille reste vert.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Raccourci `secondary-` personnalisé | `paneflow.json` contient `"secondary-shift-d": "close_pane"` | Résolu en `cmd-shift-d` sur macOS et `ctrl-shift-d` ailleurs, binding appliqué | Aucun |
| 2 | Raccourci invalide en configuration | `paneflow.json` contient `"ctrl-": "close_pane"` | Binding ignoré, autres bindings appliqués, avertissement dans le log | Log : `shortcuts: invalid keystroke 'ctrl-'` |
| 3 | Touche IPC invalide | `paneflow` CLI ou MCP envoie `"ctrl-"` à `send_keystroke` | Erreur JSON-RPC renvoyée, aucun effet sur le pane | « invalid keystroke: ctrl- » |
| 4 | Conflit masqué par `secondary-` | Surcharge `ctrl-shift-d` alors qu'un défaut `secondary-shift-d` existe sous Linux | Conflit détecté comme avant le bump | Message de conflit existant des réglages |
| 5 | Disposition non US sous Windows | AZERTY active, chord à symbole shifté | Le mapper Windows traduit selon la disposition active | Aucun |
| 6 | Changement de disposition à chaud | Win+Espace pendant l'exécution | Les bindings suivent la nouvelle disposition sans redémarrage | Aucun |
| 7 | Vulkan absent sous Linux | VM sans pilote Vulkan, `VK_ICD_FILENAMES` vide | Démarrage sur GL en moins de 10 s, ou NO-GO documenté (US-009) | Aucun si GL fonctionne |
| 8 | Échec EGL sans erreur rapportée | Pilote GL défaillant | Pas de panic grâce au patch `khronos-egl` ; échec journalisé | Log d'erreur du renderer |
| 9 | Mode économie d'énergie | Profil `power-saver`, économiseur de batterie ou Low Power Mode, avec spinner actif | Animations à 30 images par seconde, écho clavier dans le budget de 16 ms | Aucun |
| 10 | Séparateurs Unicode sous Windows | Sortie terminal avec U+2028/U+2029 | Rendu sans crash | Aucun |
| 11 | Police de repli sans glyphe `m` | `font_fallbacks` avec Noto Color Emoji ou une Nerd Font de symboles | La police reste disponible toute la session | Aucun |
| 12 | Police de repli absente | Police configurée non installée | Un avertissement unique, rendu avec la police principale | Log : avertissement GPUI nommant la police |
| 13 | Fermeture pendant une animation | Fermer une fenêtre dont un spinner tourne | Nettoyage différé d'une tâche, sans panic d'emprunt | Aucun |
| 14 | Défilement de précision rapporté comme pointeur | Souris à défilement lissé, trackpad | Même déplacement qu'avant le bump, `source` ignoré | Aucun |
| 15 | Révision wgpu introuvable | Runner sans accès à `gfx-rs/wgpu@cddb2f5d` | Build en échec explicite, story BLOCKED, pas de dépôt de remplacement | Log cargo de récupération |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Un binding `secondary-` oublié fait paniquer Paneflow au démarrage | Med | High | US-003 réécrit les 8 sites et un test enregistre les keymaps ; FR-05 |
| 2 | Le mapper Windows change des raccourcis à symboles shiftés ou casse un chord en AZERTY | Med | High | US-014 sur matériel, deux dispositions et changement à chaud |
| 3 | Le repli GL de wgpu 30 ne démarre plus (Zed #65960) | Med | High | Spike US-009 avec référence `0418cb5b` ; NO-GO = attendre #65960 ou rester sur l'ancien pin |
| 4 | Le gate d'allocation à 1 % échoue sur l'éditeur | High | Med | US-005 conserve une `SharedString` par ligne ; US-007 n'autorise un rafraîchissement que si la cause est écrite |
| 5 | Taffy 0.15 déplace des vues (valeur `Normal` non documentée, 95 `.absolute()` dans 45 fichiers) | Med | Med | US-010 : comparaison pixel des captures opaques avant/après |
| 6 | Le compositing source-over éclaircit les voiles translucides (Mica, flou, Linux translucide) | High | Low | US-010 et US-015 : revue visuelle, ajustement des alphas de `constants.rs` |
| 7 | Le plafond à 30 fps retarde l'écho clavier pendant un spinner | Med | Med | Spike US-008, seuil de 16 ms ; NO-GO = issue upstream et story de suivi, pas de patch local |
| 8 | La révision git de wgpu n'est pas récupérable en CI | Low | High | AC de US-001 sur runner à cache vide ; BLOCKED plutôt qu'un contournement |
| 9 | Zed `main` avance encore avant l'implémentation | High | Low | Le PRD fixe `37faef81` ; Open Question 1 pour un éventuel recalage |
| 10 | macOS non vérifiable sur matériel (Low Power Mode, flou macOS 26/27) | Med | Med | US-012 qualifie par la CI et l'OCR ; l'écart est consigné comme question ouverte |
| 11 | Une rupture `cfg(windows)` n'apparaît qu'au leg Windows | Med | Med | US-013 avant la passe matérielle ; règle d'ordre des items |

## Non-Goals

- **`ScrollSource::Touch` :** aucune exploitation du champ `source` ni des gestes tactiles. Paneflow ne cible pas d'écran tactile ; à revoir si une demande d'usage tactile apparaît.
- **Hot-reload :** aucune activation de la feature `hot-reload` de gpui (Subsecond, #65738). Elle est opt-in et inutile pour une release.
- **Display P3 :** `WindowOptions::use_display_p3` reste à `false`. Un changement de rendu des couleurs relève d'une décision de design séparée.
- **`gpui-pre` :** pas de passage aux snapshots crates.io comme gpui-component. Changer de source est une décision stratégique, à instruire dans un document dédié.
- **Opt-out du plafond à 30 fps :** pas de patch local de GPUI, puisque le fork a été supprimé le 2026-08-26. Toute suite passe par l'upstream ou par une story Paneflow future.
- **Dédoublonnage de cocoa et objc :** pas de migration des dépendances `cocoa` et `objc` de Paneflow (`src-app/Cargo.toml:280-281`) vers objc2, même si elles ne sont plus partagées avec gpui. À traiter en suivi.
- **Révisions ultérieures :** pas de bump au-delà de `37faef81` dans ce PRD.

## Files NOT to Modify

- `native/libghostty/**`, `src-app/src/terminal/pty_session/session_backend.rs`, `src-app/src/terminal/types.rs` : frontière du moteur de terminal, sans rapport avec GPUI.
- `crates/paneflow-shim/**`, `crates/paneflow-ai-hook/**`, `crates/paneflow-mcp/**` : binaires auxiliaires à taille plafonnée, que gpui n'atteint pas.
- `src-app/src/keybindings/defaults.rs` : les chords par défaut restent identiques ; seuls les points d'entrée d'analyse changent.
- `src-app/src/bench_harness.rs`, `scripts/perf-gates.sh`, `crates/paneflow-host/tests/persistent_baseline/gates.rs` : on rafraîchit une baseline, on n'élargit jamais une tolérance.
- `rust-toolchain.toml` : 1.98.0 reste compatible.
- `src-app/src/window_chrome/linux_backdrop.rs`, `src-app/src/window_chrome/macos_backdrop.rs` : la logique de sélection des backdrops ne change pas ; seuls les alphas de `app/constants.rs` peuvent changer, et uniquement sur preuve visuelle.
- `.github/workflows/**` : aucun changement de runner ni de job n'est requis.

## Technical Considerations

- **Architecture :** faut-il résoudre l'OS via `cx.platform_info().os` plutôt qu'un helper `cfg!` ? Recommandé : le helper. L'OS d'une cible native est figé à la compilation, `display.rs:70` le fait déjà, et passer `cx` à travers les appelants de `canonical_keystroke` (`config_writer.rs:108,179,216`, `settings.rs:493,504`) agrandirait le diff. L'ingénierie confirme qu'aucune cible web n'est envisagée.
- **Structure des commits :** un seul commit pour EP-001, comme `8bd1c410`, ou un commit par story ? Recommandé : un seul, parce qu'aucun état intermédiaire ne compile, puis un commit par story pour EP-002 et EP-003. C'est un écart assumé à la règle « un commit atomique par story » d'`AGENTS.md`. L'ingénierie valide.
- **Data Model :** sans objet. `paneflow.json` et `session.json` ne changent pas de schéma, et les chaînes `secondary-` des utilisateurs restent acceptées.
- **Shaping de l'éditeur :** comment conserver les `SharedString` par ligne ? Options : un champ par ligne du layout existant, invalidé au changement de texte ou de largeur de gouttière, ou un cache dédié indexé par numéro de ligne. Compromis : le premier ne crée pas de structure, le second isole l'invalidation. Recommandé : le premier, si l'élément conserve déjà un état par ligne visible.
- **Dependencies :** recopier le patch `khronos-egl` ? Recommandé : oui. `Cargo.lock:3270` résout déjà `khronos-egl` 6.0.0, et le patch corrige un panic du chemin GLES de wgpu-hal. Alternative : l'omettre et accepter ce panic sur le repli GL.
- **Sources :** dépôts exacts ou `allow-org.github = ["zed-industries", "gfx-rs"]` ? Recommandé : dépôts exacts, pour une confiance plus étroite au prix d'une ligne par dépôt.
- **Migration et rollback :** pas de migration de données. Le retour arrière consiste à revenir sur le commit du bump et, s'il existe, sur le commit de baselines ; on retrouve alors `0418cb5b` sans autre action.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Jobs CI verts sur la révision cible | 0 % (le bump ne compile pas tel quel) | 100 % des jobs de `run_tests.yml` | Avant la fusion (Month-1) | Run CI de la PR |
| Panics au démarrage | 8 bindings qui paniqueraient | 0 sur 4 cibles | Avant la fusion | Test d'enregistrement des keymaps, lancements macOS (CI) et Windows (matériel) |
| `prepaint_60_rows_warm` | 6 allocations, 384 octets par itération | ≤ baseline à 1 % près | Avant la fusion | `scripts/perf-gates.sh` |
| Écart de p95 de latence en économie d'énergie | N/A (nouveau) | ≤ 16 ms | Month-1 | US-008 et US-015, `PANEFLOW_LATENCY_PROBE` |
| Démarrage sans Vulkan | Mesuré en US-009 sur `0418cb5b` | Fenêtre en moins de 10 s sur GL | Month-1 | US-009 |
| Chords par défaut fonctionnels sous Windows | Non mesuré en AZERTY | 100 % en US et AZERTY | Month-1 | US-014 |
| Correctifs upstream vérifiés | 0 sur 3 | 3 sur 3 | Month-1 | US-010 (polices), US-015 (U+2028), US-009 et US-011 (surfaces Vulkan/GL) |
| Rapports de crash attribués au bump | N/A | 0 | Month-6 | Issues GitHub étiquetées après la release |

## Open Questions

- **Recalage de la cible :** faut-il viser une révision plus récente si Zed #65960 (repli GL) est fusionnée avant le début de US-001 ? À trancher par Arthur avant US-001. Le choix conditionne la validité de l'analyse d'API de ce PRD.
- **Matériel macOS :** une machine macOS est-elle disponible pour le Low Power Mode, le flou sous macOS 26/27 et l'apparence du lien avec un SDK antérieur ? À confirmer par Arthur avant US-012. Sans machine, ces points restent non vérifiés et sont mentionnés comme tels dans la release.
- **Suite d'un NO-GO en latence :** si US-008 ou US-015 rend un NO-GO, faut-il une issue upstream seule ou aussi une story Paneflow qui suspend les animations décoratives en économie d'énergie ? À décider par Arthur à la clôture de US-008.
- **Source de GPUI :** faut-il, à terme, passer à `gpui-pre` comme gpui-component ? C'est une décision stratégique séparée, à instruire avant le prochain bump.
[/PRD]
