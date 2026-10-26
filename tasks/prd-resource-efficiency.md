[PRD]
# PRD: Réduire la consommation CPU, GPU et RAM du desktop, de la sonde git aux réveils inutiles

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-14 | Arthur Jean | PRD initial : réparer le gate de réflexion aveugle depuis `328a8fe1` et mesurer le cas réel (TUI d'agent animée, 30 workspaces), puis supprimer les coûts mesurés le 2026-10-14 (sonde git sur tous les workspaces, paint du terminal hors layer, sidebar reconstruite à chaque frame), les frames d'animation inutiles, la rétention mémoire des arènes glibc, le travail du pipeline terminal au scroll et dans les panes cachés, et quatre boucles à intervalle fixe. Vérification Windows réel en dernier epic. |
| 1.1 | 2026-10-26 | Arthur Jean | US-007 : le critère « au moins 15 % de CPU en moins au thread principal » devient « `BoundsTree::insert` absent du paint du terminal » et « au moins 10 % en moins, fenêtre maximisée ». Le profil du 2026-10-26 donne `BoundsTree::insert` à 12,5 % du thread sur `main` avec 18 workspaces : même supprimé en entier, il ne peut pas rendre 15 %. Les deux layers le ramènent à 0,5 %, sans appel depuis le terminal. |
| 1.2 | 2026-10-26 | Arthur Jean | US-011 : la mesure de 4 h de `RssAnon` contre un desktop sans plafond et la comparaison CPU du thread principal sont retirées. Le critère devient le nombre d'arènes lu dans `/proc/<pid>/maps` (2 avec le plafond, 63 avec `MALLOC_ARENA_MAX=64`, mesuré le 2026-10-26) ; le RSS et le CPU du thread principal restent relevés sur matériel réel par US-019. |

## Problem Statement

Un seul agent dont l'interface s'anime dans un pane occupe le thread principal du desktop à 10 à 25 % d'un cœur, et la sonde git consomme en plus environ 10 % d'un cœur dans des processus enfants que le CPU de Paneflow n'affiche pas. Les gates de performance livrés par `prd-performance-gates.md` ne voient ni l'un ni l'autre.

1. **La sonde git de 30 s interroge tous les workspaces ouverts.** `spawn_git_poll` (`src-app/src/app/workspace_ops/git_watch.rs:190`) sonde `git_probe_cwds()` (`src-app/src/app/tab_worktree.rs:94`), soit chaque workspace et chaque worktree lié. Sur l'instance réelle (0.17.7, 33 workspaces), 186 processus git en 62 s au repos, aucune écriture de `.git/index` pendant la mesure, et un tour complet mesuré à 2,9 s de CPU : environ 10 % d'un cœur en continu, 58 297 processus git en 5,8 h.
2. **Le paint du terminal ordonne chaque glyphe un par un.** `TerminalElement::paint` (`src-app/src/terminal/element/mod.rs:1778`) peint hors de tout `paint_layer`, si bien que chaque primitive passe par `BoundsTree::insert` : 22,5 % du thread principal dans un profil plat. Un A/B sur le même build, la grille enveloppée dans un layer, fait passer le thread principal de 12,8 % à 9,9 % (5,0 à 3,9 ms par frame).
3. **La sidebar est reconstruite et remise en page à chaque frame du terminal.** `render_sidebar` (`src-app/src/app/sidebar/mod.rs:263`) est une méthode de la vue racine, et toute notification redessine la racine (`gpui/src/window.rs:176` au rev épinglé). Mesure : 3,9 ms par frame avec 18 workspaces contre 1,9 ms avec un seul, soit environ 0,11 ms par workspace et par frame, 3,7 ms avec 33 workspaces.
4. **Le gate de réflexion est aveugle depuis le 2026-10-10.** `desktop.thinking.root_renders_per_s` valait 11,13 jusqu'à `195a9f5f` et vaut 1,866, la valeur du seul clignotement, depuis `513f504a`. Entre les deux, `328a8fe1` prend l'état de tour des agents uniquement dans OSC 7501, alors que le harnais injecte `ai.prompt_submit` (`crates/paneflow-host/tests/persistent_baseline/headless.rs:160`). Le shimmer ajouté par `3a4ca9e1`, une frame toutes les 33 ms (`src-app/src/ui_primitives/shimmer.rs:10`), passe donc sans contrôle.
5. **La mémoire et les réveils dérivent sans borne.** Le desktop garde 126 arènes glibc pour 84 Mo de RSS après 5,8 h (24,5 Mo après 4 min sur une instance neuve), alors que le host plafonne les siennes (`crates/paneflow-host/src/main.rs:176`). La boucle IPC se réveille 105 fois par seconde, le suivi du worker 70 fois, chaque session au repos 10,6 fois, mesurés par changements de contexte sur 10 s.
6. **Le pipeline terminal refait tout au scroll et pour les panes que personne ne voit.** Le cache de lignes est indexé par position dans le viewport (`convert.rs:214`, `element/mod.rs:1353`) : `layout_scroll_cached` vaut 552 µs, autant que sans cache. `PublishGate` (`publish.rs:182`) n'a aucune entrée de visibilité : un pane caché convertit et publie jusqu'à 125 frames par seconde.

**Why now:** Paneflow se vend comme cockpit multi-agents (mémoire projet, repositionnement 2026-06), donc avec beaucoup de workspaces et d'agents en arrière-plan : exactement la configuration qui multiplie les coûts 1 et 3. Le shimmer de `3a4ca9e1` n'est pas encore publié et le gate qui devait l'arrêter ne mesure plus rien : c'est le dernier moment pour le régler avant qu'il atteigne les utilisateurs.

## Overview

Le PRD commence par rendre la mesure fidèle. Il répare le scénario de réflexion de `perf_gates` en faisant émettre à la session un OSC 7501, donne une borne basse aux budgets de frames pour qu'un scénario qui ne dessine rien ne passe plus, et ajoute un scénario « TUI animée, 30 workspaces » au protocole matériel et aux gates, rejoué par un script de spinner versionné qui reproduit la charge d'un agent réel (25,6 rendus par seconde et 12,4 % de CPU, contre 25 et 12 % mesurés sur Claude Code).

Il supprime ensuite les trois coûts mesurés. La sonde périodique ne vise plus que le workspace actif ; les autres se sondent à l'activation, sur leurs événements `.git` et toutes les 5 min, et rien ne se sonde quand aucune fenêtre n'est active. La grille du terminal se peint dans deux layers homogènes, l'un pour les fonds et l'autre pour le texte, avec curseur et overlays au-dessus : l'ordre de dessin reste identique et l'ordonnancement par glyphe disparaît. La sidebar passe par une vue GPUI en cache qui observe l'app et porte sa propre horloge de spinner, de sorte qu'une frame de terminal la rejoue au lieu de la reconstruire.

Viennent ensuite les frames, la mémoire et le pipeline : shimmer calé sur la grille de 90 ms, curseur fixe quand la fenêtre est inactive, arènes glibc plafonnées à 2 comme dans le host, lignes réutilisées après un scroll, panes cachés limités à une publication par seconde. Enfin, les quatre boucles à intervalle fixe attendent sur leur descripteur ou leur canal. Aucune dépendance n'est ajoutée ; tout repose sur GPUI au rev épinglé, `libc` et les compteurs existants.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Processus git par minute au repos, 33 workspaces, fenêtre active | ≤ 30 (180 aujourd'hui) | ≤ 30, budget `perf_gates` tenu |
| CPU du thread principal, TUI animée, 18 workspaces, machine de référence | ≤ 6 % (12,8 % aujourd'hui) | ≤ 6 % sur Linux et Windows |
| Rendus de la sidebar pendant 30 s de sortie terminal sans agent | ≤ 2 (environ 750 aujourd'hui) | ≤ 2, budget tenu |
| Rendus racine par seconde, un agent en réflexion, shimmer visible | ≤ 12 | ≤ 12, scénario qui échoue s'il ne voit pas le spinner |
| Réveils par seconde au repos : IPC, worker, session inactive | ≤ 1 chacun (105, 70, 10,6) | ≤ 1 chacun |

## Target Users

### Utilisateur multi-agents
- **Role:** Développeur qui pilote plusieurs agents CLI dans Paneflow, avec des dizaines de workspaces ouverts (33 sur l'instance mesurée).
- **Behaviors:** Laisse des agents travailler dans des onglets et workspaces en arrière-plan, garde la fenêtre ouverte toute la journée, passe d'un workspace à l'autre.
- **Pain points:** 10 à 25 % d'un cœur pour un seul agent animé, plus 10 % en processus git ; ventilateur et batterie sollicités sans raison visible.
- **Current workaround:** Fermer des workspaces ou quitter Paneflow pour retrouver un repos, au prix du contexte perdu.
- **Success looks like:** Un agent d'arrière-plan coûte à peu près une session tmux, et le nombre de workspaces ouverts ne change plus le coût d'une frame.

### Utilisateur sur portable
- **Role:** Utilisateur Linux, macOS ou Windows sur batterie.
- **Behaviors:** Fenêtre Paneflow souvent inactive derrière un navigateur ou un éditeur.
- **Pain points:** Réveils permanents (plus de 190 par seconde au repos, toutes boucles confondues) qui empêchent le processeur de rester en veille profonde.
- **Current workaround:** Aucun, à part quitter l'app.
- **Success looks like:** Une fenêtre inactive sans sortie terminal ne réveille presque plus le processeur.

### Mainteneur (Arthur)
- **Role:** Mainteneur unique, qui décide sur des mesures.
- **Behaviors:** S'appuie sur `perf_gates`, l'A/B et le protocole matériel pour accepter un changement.
- **Pain points:** Un gate qui passe en ne mesurant plus rien (réflexion à 1,866 rendus par seconde depuis le 2026-10-10) et un protocole matériel qui rate le cas d'un agent dont l'interface s'anime.
- **Current workaround:** Profils et A/B à la main, comme le 2026-10-14.
- **Success looks like:** Chaque coût de ce PRD a un compteur et un budget qui échoue s'il régresse ou s'il cesse de mesurer.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- Ghostty : arrête le renderer d'une fenêtre occultée, met une frame en file au retour à l'écran et affiche un curseur fixe hors focus ([HN Ghostty 1.0](https://hn.nuxt.dev/item/42517447), [discussion #13286](https://github.com/ghostty-org/ghostty/discussions/13286)). Paneflow reprend le principe pour les panes cachés et la fenêtre inactive, sans détection d'occultation (Non-Goals).
- Alacritty : boucle d'événements sans polling, référence communautaire du « 0 % au repos ». Paneflow vise la même chose pour ses boucles d'arrière-plan.
- WezTerm : rapports de CPU bloqué à 100 % sous forte sortie ([#1594](https://github.com/wez/wezterm/issues/1594)), le reproche que ce PRD évite.
- VS Code et JetBrains : rafraîchissent git sur événements du système de fichiers, et proposent de couper ce rafraîchissement ou de limiter la détection de dépôts quand il y en a beaucoup ([GitLens #583](https://github.com/gitkraken/vscode-gitlens/issues/583)). Paneflow garde son watcher `.git` et restreint le poll au workspace actif, sans surveillance récursive des arbres de travail.
- Zed : l'éditeur peint son texte dans `paint_layer` et réutilise ses vues `.cached()` tant qu'elles ne sont pas notifiées (source au rev épinglé, `crates/gpui/src/window.rs:4447`, `view.rs`).
- **Market gap:** les multiplexeurs comparables sont soit mono-plateforme, soit non conçus pour des dizaines de workspaces d'agents ; un cockpit qui coûte le même prix avec 1 ou 33 workspaces est différenciant.

### Best Practices Applied
- Atteindre un vrai repos, sans timer en attente, et regrouper les échéances ([Apple Energy Efficiency Guide](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/power_efficiency_guidelines_osx/)) : boucles bloquantes sur descripteur ou canal, animations sur la grille de 90 ms existante.
- Plafonner les arènes glibc à 2 dans un processus long à nombreux threads ([Heroku](https://devcenter.heroku.com/articles/tuning-glibc-memory-behavior), [glibc tunables](https://www.gnu.org/software/libc/manual/2.30/html_node/Memory-Allocation-Tunables.html)) : déjà appliqué au host.
- Mettre une frame en file au retour à l'écran d'une surface masquée (Ghostty), pour qu'aucun rendu ne soit perdu.
- Mettre en cache par identité stable de ligne, jamais par index de viewport.
- Un compteur ne vaut que s'il échoue quand il cesse de mesurer : borne basse sur les scénarios censés produire des frames.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- libghostty marque toutes les lignes du viewport comme sales quand le contenu défile d'une ligne, ce qui explique `layout_scroll_cached` égal à la version sans cache. Validé par US-012 avant US-013.
- Un plafond de 2 arènes ne coûte pas plus de 3 % de CPU au thread principal sous la charge de référence, d'après l'expérience du host et le défaut de Heroku. Vérifié par US-019 sur matériel réel.
- Le survol des éléments de la sidebar reste vivant dans une vue en cache, car GPUI notifie la vue courante quand l'état de survol change (`crates/gpui/src/elements/div.rs:2703` au rev épinglé). Vérifié par les critères d'US-008.
- Des diffstats de workspaces inactifs vieilles d'au plus 5 min ne gênent pas l'utilisateur, puisque l'activation et les événements `.git` les rafraîchissent aussitôt.
- Un shimmer à 90 ms reste lisible ; repli sur 45 ms si la passe visuelle le refuse (Open Questions).
- Une publication par seconde suffit à la détection de services et à l'activité des panes cachés.
- Le script de spinner versionné reproduit la charge d'un agent réel : 25,6 rendus par seconde et 12,4 % de CPU contre 25 et 12 % mesurés sur Claude Code le 2026-10-14.

### Hard Constraints
- Linux, macOS et Windows : chaque changement a un chemin sur les trois OS, ou un repli documenté ; le code `#[cfg(windows)]` n'est linté que par le job Windows.
- Le thread de rendu ne bloque jamais ; aucune surveillance récursive de fichiers.
- Aucune nouvelle dépendance ; GPUI reste au rev `0418cb5b`.
- Aucun commentaire dans le code source.
- La sémantique de durabilité des hooks et les garanties de suivi des descendants restent inchangées.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration
- `cargo deny check advisories licenses sources` - uniquement quand une dépendance change

Gates additionnels :
- Le job CI `Performance gates (Linux x86_64)` passe.
- Stories qui changent un coût (EP-001 à EP-005) : mesure avant/après jointe à la PR, prise avec `system.counters`, `host.status`, `scripts/bench-terminal.sh` ou le scénario d'US-002, avec la commande qui la reproduit.
- Stories UI (US-005, US-007, US-008, US-009, US-010) : passe visuelle d'Arthur sous Linux sur un build debug, capture ou enregistrement court joint à la PR. L'agent livre le changement sans lancer l'app pour la vérifier.
- Stories qui touchent du code `#[cfg(windows)]` : le job « Windows x86_64 libghostty check » passe, et la PR dit si Windows a été vérifié par inspection ou sur le matériel réel.

## Epics & User Stories

### EP-001: Mesurer le cas réel et couper la sonde git

Rendre les gates fidèles au cas d'un agent dont l'interface s'anime parmi des dizaines de workspaces, puis supprimer les 10 % d'un cœur dépensés en processus git.

**Definition of Done:** le scénario de réflexion de `perf_gates` voit de nouveau le spinner et échoue s'il ne le voit pas ; le scénario « TUI animée, 30 workspaces » tourne dans `perf_gates` et dans `scripts/perf-hardware.sh` ; au repos avec 30 workspaces, `perf_gates` borne les processus git à 30 par minute fenêtre active et à 0 fenêtre inactive.

#### US-001: Rendre au scénario de réflexion un agent qui réfléchit
**Description:** As a mainteneur, I want que le scénario `desktop_thinking` de `perf_gates` mette l'agent en réflexion par le même canal que les agents réels so that le budget de rendus en réflexion mesure de nouveau le spinner au lieu du seul clignotement.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le harnais (`crates/paneflow-host/tests/persistent_baseline/headless.rs`) met la session en réflexion en faisant écrire au programme du pane la séquence OSC 7501 de début de tour, au lieu d'injecter `ai.prompt_submit`.
- [ ] Given le scénario `desktop_thinking` sur `main`, when `perf_gates` mesure, then `desktop.thinking.root_renders_per_s` vaut entre 10 et 12 (11,1 attendu), et la PR cite les deux valeurs de la bisection (11,13 sur `195a9f5f`, 1,866 sur `513f504a`).
- [ ] Le budget passe de `Limit::AtMost(12.0)` à `Limit::Within` entre 10 et 12, avec une marge qui explique la borne basse.
- [ ] Échec : given une session dont l'agent ne passe pas en réflexion, when le gate mesure, then le budget est en échec avec la raison « aucun spinner dessiné », jamais satisfait.
- [ ] Le test unitaire du calcul de budget (`gate_runs.rs`, à côté de `a_thinking_window_that_was_never_focused_never_satisfies_its_budget`) couvre la valeur 1,866 et la rejette.

#### US-002: Mesurer une TUI animée parmi 30 workspaces
**Description:** As a mainteneur, I want un scénario reproductible où un pane affiche une interface d'agent animée pendant que la sidebar liste 30 workspaces so that les gates et le protocole matériel mesurent le cas qui coûte 10 à 25 % d'un cœur en usage réel.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un script de fixture versionné réécrit trois lignes en bas d'écran toutes les 50 ms dans un bloc de sortie synchronisée (DEC 2026), au-dessus de 40 lignes de texte, et change le titre OSC toutes les 100 ms ; il prend sa durée en argument et existe en version shell POSIX et PowerShell.
- [ ] `perf_gates` ajoute un scénario `desktop_tui_30_workspaces` : home amorcé avec 30 workspaces sur des dépôts git temporaires, fenêtre focalisée, script lancé dans le pane actif ; il enregistre `root_renders`, `git_spawns` et, après US-006, `sidebar_renders` sur une fenêtre de 30 s.
- [ ] `scripts/perf-hardware.sh` et `scripts/perf-hardware.ps1` acceptent un état `tui-30-workspaces` qui rejoue le même script et note le CPU et le RSS des trois processus.
- [ ] Given le scénario sur `main` avant tout correctif de ce PRD, when il tourne, then la PR enregistre les valeurs de référence : rendus racine par seconde (environ 25), processus git par minute, et le CPU du thread principal sur matériel réel.
- [ ] Échec : given un script qui ne démarre pas ou un pane qui ne reçoit aucune sortie, when le scénario mesure, then chaque budget du scénario est manquant avec sa raison, jamais satisfait.

#### US-003: Ne sonder périodiquement que le workspace actif
**Description:** As a utilisateur multi-agents, I want que la sonde de 30 s ne vise que le workspace affiché so that 33 workspaces ouverts ne coûtent plus 10 % d'un cœur en processus git.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `spawn_git_poll` sonde toutes les 30 s les répertoires du workspace actif de chaque fenêtre : son `cwd` et ses worktrees liés.
- [ ] Les autres workspaces sont sondés au plus une fois toutes les 5 min, et immédiatement quand ils deviennent actifs (`select_workspace`, `activate_workspace_at`, `activate_workspace_without_window` dans `src-app/src/app/workspace_ops/mod.rs`), le résultat appliqué en moins de 3 s après l'activation sur un dépôt de taille moyenne.
- [ ] Le chemin des événements `.git` (`spawn_git_event_refresh`) reste inchangé : un `git commit` ou un `git switch` dans un workspace inactif rafraîchit sa diffstat sans attendre les 5 min.
- [ ] Given 30 workspaces sur des dépôts immobiles et une fenêtre active, when on mesure 60 s, then `git_spawns.total` augmente d'au plus 30 (180 aujourd'hui avec 33 workspaces).
- [ ] Échec : given une sonde qui échoue (git absent, timeout), when elle revient, then la diffstat du workspace garde son état « indisponible » actuel et la sonde suivante suit la cadence normale, sans relance immédiate.

#### US-004: Suspendre la sonde périodique sans fenêtre active
**Description:** As a utilisateur sur portable, I want qu'aucune sonde périodique ne tourne quand aucune fenêtre Paneflow n'est active so that une fenêtre laissée derrière d'autres apps ne lance plus de git.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Quand aucune fenêtre n'est active selon le registre `agents::notifications::is_window_active`, la boucle de 30 s et la cadence de 5 min ne lancent aucune sonde.
- [ ] À la réactivation d'une fenêtre (`observe_window_activation`, `src-app/src/app/window.rs:314`, et son pendant pour les fenêtres détachées), le workspace actif est sondé une fois immédiatement, puis la cadence normale reprend.
- [ ] Given une fenêtre inactive pendant 5 min sur 30 workspaces, when on mesure, then `git_spawns.total` n'augmente pas, sauf sur événement `.git`.
- [ ] Échec : given une fenêtre fermée pendant qu'une sonde tourne, when la sonde revient, then son résultat est ignoré sans erreur et sans relance.

#### US-005: Borner les processus git au repos dans `perf_gates`
**Description:** As a mainteneur, I want un budget qui borne les processus git du scénario à 30 workspaces so that une sonde qui revient à tous les workspaces échoue en CI.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002, US-003, US-004

**Acceptance Criteria:**
- [ ] `gates.rs` ajoute `desktop.tui_30_workspaces.git_spawns_per_min` (`Limit::AtMost(30.0)`) et `desktop.tui_30_workspaces_inactive.git_spawns_per_window` (`Limit::Exactly(0.0)` sur 60 s, fenêtre sans focus), avec leur marge écrite.
- [ ] Given `main` avant US-003, when le budget est évalué, then il échoue ; la PR cite la valeur mesurée.
- [ ] `desktop.diff_stat.git_spawns_per_probe` reste à `Limit::Exactly(3.0)`.
- [ ] Échec : given un scénario dont le home n'a pas pu amorcer ses 30 dépôts, when le budget est évalué, then il est manquant avec sa raison, jamais satisfait par un compteur à zéro.

---

### EP-002: Réduire le coût d'une frame

Enlever l'ordonnancement par glyphe et la reconstruction de la sidebar, les deux tiers du coût d'une frame mesurés le 2026-10-14.

**Definition of Done:** sous le scénario d'US-002, le CPU du thread principal est au plus 6 % sur la machine de référence (12,8 % avant), `sidebar_renders` reste à 2 au plus pendant 30 s de sortie terminal sans agent, et l'ordre de dessin du terminal est identique à aujourd'hui sur la fixture de z-order.

#### US-006: Compter les rendus de la sidebar
**Description:** As a mainteneur, I want un compteur `sidebar_renders` exposé par `system.counters` so that la mise en cache de la sidebar se vérifie par un nombre déterministe plutôt que par un temps.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] `src-app/src/work_counters.rs` ajoute `SIDEBAR_RENDERS`, incrémenté à chaque exécution de `render_sidebar`, et `system.counters` l'expose sous `sidebar_renders`.
- [ ] `perf_gates` ajoute `desktop.tui_30_workspaces.sidebar_renders`, lu sur les 30 s du scénario d'US-002, dont le script tourne sans agent ; le budget est créé en mode consultatif et passe à `Limit::AtMost(2.0)` dans US-008.
- [ ] Given `main` avant US-008, when le scénario mesure, then `sidebar_renders` vaut à peu près le nombre de rendus racine (environ 750 sur 30 s), valeur citée dans la PR.
- [ ] Échec : given un scénario dont le script ne produit aucune frame, when il mesure, then le budget est manquant avec sa raison.

#### US-007: Peindre la grille du terminal en deux layers homogènes
**Description:** As a utilisateur multi-agents, I want que la grille du terminal se peigne sans ordonner chaque glyphe so that une frame ne paie plus `BoundsTree::insert` pour chaque primitive de la grille, environ 12 % du thread principal avec 18 workspaces.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Dans `TerminalElement::paint` (`src-app/src/terminal/element/mod.rs:1778`), un premier `window.paint_layer(bounds, …)` enveloppe le fond de base, la ligne d'overscan, les fonds de cellules, la sélection, les surlignages de recherche et les blocs pleins.
- [ ] Un second `paint_layer` enveloppe les sprites, le texte et les symboles. Les images kitty sous le texte se peignent entre les deux layers, et les décorations, les images kitty au-dessus du texte, le soulignement d'hyperlien, le curseur, le curseur d'ancre, la scrollbar, les marques de recherche, l'IME et l'overlay de sortie se peignent après, dans cet ordre.
- [ ] L'écriture de `scrollbar_metrics` dans son mutex reste hors des layers et inchangée.
- [ ] Un script de fixture versionné affiche fonds colorés, sélection, texte barré et souligné, hyperlien, image kitty au-dessus et au-dessous du texte et curseur bloc ; la passe visuelle Linux confirme un rendu identique à `main` sur ce script.
- [ ] Given le scénario d'US-002 sur la machine de référence, when on profile le thread principal, then aucun appel à `BoundsTree::insert` ne part du paint du terminal et la fonction pèse moins de 1 % du thread (12,5 % sur `main`), profil joint.
- [ ] Given le même scénario en fenêtre maximisée, when on compare à `main`, then le CPU du thread principal baisse d'au moins 10 %, mesure jointe.
- [ ] Échec : given des bornes de layer vides (pane de taille nulle pendant un redimensionnement), when la frame se peint, then rien n'est poussé et aucune primitive ne fuit hors du masque de contenu.

#### US-008: Rendre la sidebar par une vue en cache
**Description:** As a utilisateur multi-agents, I want que la sidebar ne se reconstruise que quand son contenu change so that le coût d'une frame de terminal ne dépende plus du nombre de workspaces ouverts.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Une entité de vue sidebar, rendue par `.cached(…)` à la place de l'appel de `render.rs:514`, appelle `render_sidebar` de l'app et observe l'entité app : toute notification de l'app la redessine, une notification d'un seul `TerminalView` non.
- [ ] L'horloge du spinner et du shimmer de la sidebar (`SpinnerClock`, `src-app/src/app/sidebar/lane.rs`) appartient à cette vue et la notifie elle seule ; les drapeaux `SPINNER_DRAWN` et `SHIMMER_DRAWN` ne sont plus lus par la racine (`render.rs:763`).
- [ ] Given un agent en réflexion, when le terminal publie des frames et que la sidebar est servie depuis le cache, then le spinner et le shimmer de la ligne de l'agent continuent d'avancer.
- [ ] Given le scénario d'US-002, sans agent, when on mesure 30 s, then `sidebar_renders` vaut au plus 2, et le budget `desktop.tui_30_workspaces.sidebar_renders` passe à `Limit::AtMost(2.0)`.
- [ ] Le survol, les menus contextuels, le renommage, le glisser-déposer, l'ajout et la suppression de workspaces et le changement de thème mettent la sidebar à jour dans la frame suivante ; la passe visuelle Linux couvre chacun.
- [ ] Toute donnée affichée par la sidebar vient de l'état de l'app ou d'une entité que la vue observe ; la PR liste les `read(cx)` d'entités autres que l'app trouvés dans `render_sidebar` et la façon dont chacun invalide la vue.
- [ ] Given le scénario d'US-002 sur la machine de référence, when on compare à US-007 seul, then le coût d'une frame baisse d'au moins 1,5 ms avec 18 workspaces, mesure jointe.
- [ ] Échec : given un changement d'état d'agent dans un pane d'un workspace inactif, when le snapshot arrive, then sa ligne de sidebar change dans la frame suivante, jamais à la prochaine notification sans rapport.

---

### EP-003: Couper les frames d'animation inutiles et la rétention mémoire

Ramener les animations sur la grille de 90 ms, arrêter le curseur d'une fenêtre inactive et rendre au système la mémoire que les arènes glibc retiennent.

**Definition of Done:** le budget de réflexion d'US-001 tient avec le shimmer visible ; une fenêtre inactive ne fait plus clignoter son curseur ; le desktop lancé sous Linux gnu ne compte pas plus de 2 arènes glibc.

#### US-009: Caler le shimmer sur la grille de 90 ms
**Description:** As a utilisateur multi-agents, I want que le shimmer des agents en réflexion avance sur les pas du spinner so that il n'ajoute aucune frame aux 11,1 par seconde du spinner.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `SHIMMER_FRAME` (`src-app/src/ui_primitives/shimmer.rs:10`) vaut `ANIMATION_STEP_MS` (90 ms), et un test d'alignement, sur le modèle de `src-app/src/terminal/blink.rs:42-54`, prouve que chaque réveil du shimmer tombe sur un pas du spinner.
- [ ] Given un agent en réflexion avec le shimmer visible, when `perf_gates` mesure, then `desktop.thinking.root_renders_per_s` reste entre 10 et 12.
- [ ] La passe visuelle Linux juge le mouvement de la bande ; si Arthur le refuse, `SHIMMER_FRAME` passe à 45 ms et la PR enregistre la mesure de rendus par seconde correspondante.
- [ ] Échec : given `reduce_motion` actif, when un agent réfléchit, then aucun réveil de shimmer n'est programmé.

#### US-010: Arrêter le clignotement quand la fenêtre est inactive
**Description:** As a utilisateur sur portable, I want que le curseur reste fixe quand la fenêtre Paneflow n'est pas active so that une fenêtre en arrière-plan ne redessine plus 1,85 fois par seconde.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Quand aucune fenêtre n'est active, `spawn_cursor_blink` (`src-app/src/app/bootstrap.rs:481`) ne bascule plus `BlinkPhase` et laisse le curseur visible.
- [ ] À la réactivation, le curseur est visible tout de suite et le clignotement reprend au pas suivant de la grille de 90 ms.
- [ ] Given une fenêtre inactive sans sortie terminal pendant 30 s, when on lit `root_renders`, then il augmente d'au plus 1.
- [ ] Échec : given une fenêtre détachée active pendant que la fenêtre principale est inactive, when le curseur du pane détaché est focalisé, then il continue de clignoter.

#### US-011: Plafonner les arènes glibc du desktop
**Description:** As a utilisateur multi-agents, I want que le desktop plafonne ses arènes malloc comme le host so that une longue session ne retienne plus 84 Mo dans 126 arènes.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `src-app/src/main.rs` appelle, avant de créer le moindre thread, une copie de `cap_malloc_arenas` (`crates/paneflow-host/src/main.rs:176-186`) avec la même paire de `cfg` (`target_os = "linux"`, `target_env = "gnu"`), 2 arènes et un repli sans effet ailleurs.
- [ ] Une variable `MALLOC_ARENA_MAX` fournie par l'utilisateur est respectée et n'est pas écrasée.
- [ ] Given le desktop lancé sous Linux gnu sans `MALLOC_ARENA_MAX`, when on compte les tas d'arène de 64 Mio dans `/proc/<pid>/maps` après le démarrage, then il en compte au plus 1 en plus de l'arène principale ; avec `MALLOC_ARENA_MAX=64`, le compte dépasse 2, ce qui prouve que la variable est respectée.
- [ ] Le gain de `RssAnon` et l'absence de régression CPU du thread principal ne sont pas mesurés par cette story : US-019 les relève sur matériel réel. Si US-019 mesure une régression du thread principal imputable au plafond, le plafond passe à 4.
- [ ] Échec : given un `mallopt` refusé, when le desktop démarre, then il continue et écrit un avertissement, comme le host.

---

### EP-004: Moins de travail dans le pipeline terminal

Réutiliser au scroll ce qui est déjà converti et mis en page, et ne plus convertir des frames que personne ne voit.

**Definition of Done:** `layout_scroll_cached_220x60` vaut au plus 150 µs et `publish_scroll_220x60` au plus 100 µs dans `scripts/bench-terminal.sh` ; un pane caché publie au plus une frame par seconde et se met à jour dès qu'il redevient visible.

#### US-012: Valider l'hypothèse : libghostty marque toutes les lignes au scroll d'une ligne
**Description:** As a mainteneur, I want savoir quelles lignes libghostty marque sales quand le contenu défile d'une ligne so that la réutilisation des lignes repose sur un comportement mesuré.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un test dans `src-app/src/terminal/ghostty_session/` alimente un terminal de 220x60 plein, ajoute une ligne en bas, et enregistre `dirty_rows`, `history_size` et `display_offset` du snapshot suivant.
- [ ] Le résultat est consigné dans la PR : nombre de lignes marquées, et si le décalage se déduit de la variation de `history_size` et `display_offset`.
- [ ] Échec : given un résultat où seules les nouvelles lignes sont marquées, when le spike conclut, then US-013 est annulée et la cause de `layout_scroll_cached` est recherchée ailleurs, dans un ajout au PRD.

#### US-013: Réutiliser les lignes converties et mises en page après un scroll
**Description:** As a utilisateur multi-agents, I want qu'une ligne qui défile garde sa conversion et sa mise en page so that un agent qui imprime du texte en continu ne refasse pas 13 200 cellules à chaque ligne.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] `CellMirror::publish` (`convert.rs:209`) déduit le décalage du viewport de `history_size` et `display_offset`, fait pivoter `row_versions` et le buffer de cellules de ce décalage, et ne reconvertit que les lignes nouvelles ou réellement modifiées.
- [ ] Le cache de lignes de l'élément (`element/mod.rs:1353`) pivote du même décalage, et `RowLayout` ne contient plus de position verticale absolue : elle s'ajoute au paint, comme le fait déjà `overscan_row_layout`.
- [ ] Given `scripts/bench-terminal.sh`, when on compare à la baseline, then `layout_scroll_cached_220x60` vaut au plus 150 µs (552 µs avant) et `publish_scroll_220x60` au plus 100 µs (359 µs avant), et les allocations par passe de scroll baissent d'au moins 80 % dans le gate d'allocations.
- [ ] Le contenu affiché reste identique à `main` pour un scroll d'une ligne, de plusieurs lignes, un scroll en arrière (`display_offset` non nul), l'écran alternatif, un effacement d'écran et un redimensionnement pendant la sortie ; un test couvre chaque cas.
- [ ] Échec : given un décalage incohérent (variation de `history_size` au-delà du nombre de lignes, ou historique tronqué à sa limite), when la frame se publie, then toutes les lignes sont reconverties comme aujourd'hui.

#### US-014: Limiter les panes cachés à une publication par seconde
**Description:** As a utilisateur multi-agents, I want qu'un pane que personne ne voit ne convertisse plus ses frames à 125 par seconde so that des agents en arrière-plan ne coûtent plus de CPU de conversion ni de réveils du thread principal.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un message de contrôle `RuntimeMessage::Visibility(bool)` (`src-app/src/terminal/ghostty_session/mailbox.rs:3`, envoyé par `try_send_control`) informe le runtime ; l'app l'envoie quand un pane entre dans ou sort de l'onglet actif du workspace actif d'une fenêtre, et quand une fenêtre détachée s'ouvre ou se ferme.
- [ ] Tant que le pane est caché, `PublishGate` garde le contenu en attente et publie au plus une fois par seconde ; libghostty continue de recevoir toute la sortie.
- [ ] Au retour à l'écran, le runtime publie immédiatement, dans le respect du maintien de rendu (DEC 2026) existant, et le pane affiche un contenu à jour dans la frame suivante.
- [ ] Given un pane caché qui imprime une URL de serveur de dev, when la ligne arrive, then le service est détecté en moins de 1,5 s.
- [ ] Given un pane caché qui reçoit le script d'US-002, when on mesure 10 s, then il publie au plus 10 frames (environ 200 aujourd'hui), mesure jointe.
- [ ] Échec : given une file de contrôle pleine ou fermée, when `Visibility` ne peut pas être envoyé, then le pane reste en mode visible, jamais bloqué en mode caché.

---

### EP-005: Supprimer les réveils à intervalle fixe

Faire attendre les quatre boucles qui se réveillent sans rien avoir à faire sur leur descripteur ou leur canal.

**Definition of Done:** au repos, le thread IPC, le thread de suivi du worker et chaque session inactive se réveillent au plus une fois par seconde, et le thread principal n'a plus de timer de 200 ms permanent.

#### US-015: Attendre les connexions IPC sans polling de 10 ms
**Description:** As a utilisateur sur portable, I want que le serveur IPC dorme jusqu'à une connexion so that il ne réveille plus le processeur 100 fois par seconde.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Sous Unix, la boucle d'acceptation (`src-app/src/ipc.rs:212-248`) attend sur le descripteur du listener avec `libc::poll` et un timeout de 5 s, au lieu de `sleep(10 ms)` après `WouldBlock`.
- [ ] Le contrôle d'inode du socket tourne toujours toutes les 5 s, et un socket écrasé est toujours relié de nouveau.
- [ ] Windows garde son listener bloquant, inchangé.
- [ ] Given un desktop au repos, when on compte pendant 10 s les changements de contexte du thread `paneflow-ipc`, then il y en a au plus 10 (1 050 avant).
- [ ] `ARCHITECTURE.md` remplace la « 50 ms app poll loop » (lignes 73 et 91) par le dispatch piloté par événements de `serve_ipc_requests`.
- [ ] Échec : given `poll` qui renvoie une erreur `EINTR`, when la boucle reprend, then elle relance l'attente sans fermer le serveur ; toute autre erreur garde le comportement actuel de `IPC accept error`.

#### US-016: Faire attendre le suivi du worker jusqu'à l'échéance du keepalive
**Description:** As a utilisateur sur portable, I want que le suivi des sessions du worker dorme entre deux frames so that il ne se réveille plus 66 fois par seconde.

**Priority:** P2
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `stream_follow` (`crates/paneflow-serve/src/server.rs:433`) attend avec `recv_timeout(min(FOLLOW_KEEPALIVE - écoulé, 500 ms))` au lieu de `FOLLOW_POLL` (15 ms).
- [ ] Le keepalive part toujours au plus 2 s après la dernière frame, et `controller_conformance.rs` passe sans modification.
- [ ] Given un worker au repos avec un abonné, when on compte pendant 10 s ses changements de contexte, then il y en a au plus 25 (700 avant).
- [ ] Échec : given un arrêt du worker pendant l'attente, when `shutdown` passe à vrai, then la connexion envoie son `end` en moins de 600 ms.

#### US-017: Remplacer le timer git de 200 ms par une attente sur canal
**Description:** As a utilisateur sur portable, I want que le watcher git ne réveille le thread principal que quand un événement arrive so that le thread de rendu n'ait plus de timer permanent.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] `spawn_git_event_refresh` (`src-app/src/app/workspace_ops/git_watch.rs:104`) attend le premier événement sur un canal asynchrone de `smol` au lieu d'un `Timer::after(200 ms)` en boucle ; le timer ne sert qu'à l'anti-rebond d'une rafale en cours.
- [ ] Les règles de `git_refresh_due` (300 ms de calme, plafond de 2 s) et leurs tests restent identiques.
- [ ] Given aucun événement `.git` pendant 30 s, when on observe le thread principal, then cette boucle ne s'est réveillée aucune fois.
- [ ] Échec : given un watcher qui renvoie une erreur, when elle arrive sur le canal, then elle est journalisée comme aujourd'hui et la boucle continue d'attendre.

#### US-018: Laisser une session au repos bloquer sur sa mailbox
**Description:** As a utilisateur multi-agents, I want qu'une session sans sortie ni geste en cours dorme jusqu'au prochain message so that dix sessions ouvertes ne réveillent plus le processeur cent fois par seconde.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Dans `run_attached_runtime` (`src-app/src/terminal/ghostty_session/attached_runtime.rs:536-562`), quand aucune publication n'est en attente, qu'aucune sortie n'est récente, qu'aucun glisser n'est actif et qu'aucune recompression n'est due, la boucle bloque sur la mailbox sans timeout.
- [ ] La boucle du runtime en processus (`pty_runtime.rs:529-588`), que mesure le bench terminal, suit la même règle, l'attente d'un enfant en fin de vie gardant son tick de 10 ms.
- [ ] Chaque écriture de `shutdown_sent` faite hors du thread du runtime (parmi `mod.rs:348`, `mod.rs:428`, `commands.rs:776`, `pty_runtime.rs:842`, `:905`, `:1352`) poste aussi un message de contrôle, pour qu'une session bloquée s'arrête ; la PR dit pour chacune si elle s'exécute sur le thread du runtime.
- [ ] Given une session au repos, when on compte pendant 10 s les changements de contexte de son thread, then il y en a au plus 10 (106 avant), et `idle_wakeups_shell_per_s` du bench terminal passe d'environ 10 à au plus 1.
- [ ] Échec : given une fermeture de pane pendant que la session bloque, when la fermeture arrive, then le thread du runtime se termine en moins de 200 ms ; un test le vérifie.

---

### EP-006: Vérifier sur le matériel réel

Mesurer les gains sur la machine de référence et vérifier les changements de rendu et de boucles sur Windows réel, en un seul passage à la fin.

**Definition of Done:** le protocole matériel Linux enregistre l'état `tui-30-workspaces` avant et après le PRD ; la passe Windows réel couvre chaque story d'EP-002 à EP-005 et corrige ce qu'elle trouve.

#### US-019: Mesurer les gains sur matériel réel Linux
**Description:** As a mainteneur, I want rejouer le protocole matériel sur le build qui contient EP-001 à EP-005 so that les gains publiés reposent sur une mesure réelle et pas sur l'A/B du 2026-10-14.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005, US-008, US-011, US-014, US-018

**Acceptance Criteria:**
- [ ] `scripts/perf-hardware.sh --state tui-30-workspaces` et `--state idle-4-panes` tournent sur la machine de référence pour le dernier tag publié et pour `main` ; les quatre résultats sont archivés sous `bench/results/` avec un résumé daté.
- [ ] Le résumé compare le CPU du thread principal, les processus git par minute, le RSS des trois processus et les réveils par thread aux objectifs de Goals, et marque chaque objectif tenu ou manqué.
- [ ] Échec : given un objectif manqué, when le résumé est écrit, then il nomme la story concernée et propose un ajout au PRD au lieu d'arrondir la valeur.

#### US-020: Vérifier les changements sur Windows réel
**Description:** As a mainteneur, I want une passe sur le Windows du dual boot so that le rendu en layers, la sidebar en cache, le shimmer, le curseur et les boucles se comportent sous Windows comme sous Linux.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007, US-008, US-009, US-010, US-016, US-018

**Acceptance Criteria:**
- [ ] La fixture de z-order d'US-007 rend sous Windows comme sous Linux, capture jointe.
- [ ] La sidebar en cache, le shimmer et l'arrêt du clignotement à la désactivation de la fenêtre se comportent comme sous Linux.
- [ ] `scripts/perf-hardware.ps1 --state tui-30-workspaces` enregistre le CPU, le RSS et les rendus racine du build Windows, archivés sous `bench/results/`.
- [ ] Le suivi du worker (US-016) et la session au repos (US-018) gardent sous Windows leurs bornes de réveils, mesurées avec les outils Windows nommés dans la PR.
- [ ] Échec : given un écart propre à Windows, when la passe le trouve, then il est corrigé dans cette story ou consigné comme nouvelle story, jamais laissé sans trace.

---

## Functional Requirements

- FR-01: Le desktop ne doit sonder périodiquement que le workspace actif de chaque fenêtre active, toutes les 30 s ; les autres workspaces au plus toutes les 5 min, et dès leur activation.
- FR-02: Le desktop ne doit lancer aucune sonde git périodique quand aucune fenêtre n'est active, et doit sonder le workspace actif dès la réactivation.
- FR-03: Le système doit garder le rafraîchissement git sur événements `HEAD` et `index`, inchangé.
- FR-04: La grille du terminal doit se peindre en deux layers homogènes, avec un ordre de dessin identique à celui d'aujourd'hui pour chaque catégorie de primitive.
- FR-05: Une frame déclenchée par la seule sortie d'un terminal ne doit pas reconstruire la sidebar.
- FR-06: Le spinner et le shimmer de la sidebar doivent continuer d'avancer quand la sidebar est servie depuis le cache.
- FR-07: Aucune animation de la sidebar ne doit programmer un réveil hors de la grille de 90 ms.
- FR-08: Le curseur ne doit pas clignoter quand aucune fenêtre n'est active, et doit rester visible.
- FR-09: Le desktop doit plafonner ses arènes glibc sous Linux gnu, sauf si `MALLOC_ARENA_MAX` est fourni.
- FR-10: Un pane caché doit publier au plus une frame par seconde, et immédiatement quand il redevient visible.
- FR-11: Après un scroll, seules les lignes nouvelles ou modifiées doivent être reconverties et remises en page.
- FR-12: Les boucles IPC, de suivi du worker, du watcher git et des sessions au repos ne doivent pas se réveiller sans message, événement ou échéance réelle.
- FR-13: Chaque budget de `perf_gates` qui mesure des frames doit échouer quand son scénario n'en produit pas.
- FR-14: Le système ne doit pas surveiller récursivement les arbres de travail.

## Non-Functional Requirements

- **Performance:** CPU du thread principal ≤ 6 % d'un cœur sous le scénario d'US-002 avec 18 workspaces sur Ryzen 7 7800X3D (12,8 % avant) ; coût d'une frame ≤ 2,5 ms dans ce scénario (5,0 ms avant) ; `layout_scroll_cached_220x60` ≤ 150 µs ; `publish_scroll_220x60` ≤ 100 µs.
- **Performance:** ≤ 30 processus git par minute au repos avec 33 workspaces et une fenêtre active, 0 sans fenêtre active hors événement `.git`.
- **Performance:** au repos, ≤ 1 réveil par seconde pour le thread IPC, le thread de suivi du worker et chaque thread de session inactive.
- **Performance:** ≤ 12 rendus racine par seconde avec un agent en réflexion et le shimmer visible ; ≤ 2 rendus de sidebar en 30 s de sortie terminal sans agent.
- **Memory:** au plus 2 arènes glibc dans le desktop sous Linux gnu ; le RSS des trois processus est relevé par US-019.
- **Reliability:** un pane qui redevient visible affiche un contenu à jour dans la frame suivante ; une session bloquée s'arrête en moins de 200 ms à la fermeture ; un abonné du worker reçoit son `end` en moins de 600 ms après l'arrêt.
- **Freshness:** la diffstat d'un workspace devenu actif se met à jour en moins de 3 s ; celle d'un workspace inactif a au plus 5 min d'âge, sauf sans fenêtre active.
- **Accessibility:** avec `reduce_motion`, aucun réveil de shimmer ou de spinner n'est programmé.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Aucun dépôt git | Workspaces sur des dossiers sans `.git` | Une seule `rev-parse` par sonde, aucune diffstat | None |
| 2 | Sonde en échec | git absent, timeout, dépôt corrompu | Diffstat « indisponible » comme aujourd'hui, cadence normale | None |
| 3 | 100 workspaces | Session restaurée avec 100 workspaces | Coût de frame inchangé par rapport à 1 workspace (US-008) ; au plus une sonde inactive toutes les 5 min par workspace | None |
| 4 | Fenêtre réactivée | L'utilisateur revient sur Paneflow | Sonde du workspace actif immédiate, curseur visible, clignotement repris | None |
| 5 | Workspace activé avec une diffstat périmée | Changement de workspace | Sonde immédiate, résultat en moins de 3 s | None |
| 6 | Pane caché qui imprime une URL | Serveur de dev lancé dans un onglet en arrière-plan | Service détecté en moins de 1,5 s | None |
| 7 | Pane rendu visible pendant un maintien de rendu | Programme en milieu de frame synchronisée (DEC 2026) | La frame capturée s'affiche, jamais une frame à moitié dessinée | None |
| 8 | État d'agent qui change dans un workspace inactif | Snapshot du worker | Ligne de sidebar à jour dans la frame suivante malgré le cache | None |
| 9 | Changement de thème | Réglage ou suivi du thème système | `Window::refresh` contourne le cache ; sidebar recolorée | None |
| 10 | Scroll en arrière pendant la sortie | `display_offset` non nul | Lignes réutilisées ou reconverties sans contenu erroné | None |
| 11 | Historique tronqué | Limite de scrollback atteinte pendant un scroll | Repli sur la reconversion complète | None |
| 12 | `MALLOC_ARENA_MAX` fourni | Variable d'environnement de l'utilisateur | Valeur respectée, aucun `mallopt` | None |
| 13 | Socket IPC écrasé | Un autre processus remplace le socket | Contrôle d'inode toutes les 5 s, nouveau bind comme aujourd'hui | None |
| 14 | Fermeture pendant une attente bloquante | Pane fermé, worker arrêté | Arrêt en moins de 200 ms (session) et de 600 ms (worker) | None |
| 15 | Pane de taille nulle | Redimensionnement extrême | Layers non poussés, aucune primitive hors masque | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | La sidebar en cache affiche un état périmé, parce qu'une donnée vient d'une entité qu'elle n'observe pas | Med | High | Critère d'US-008 qui liste chaque `read(cx)` hors app ; compteur `sidebar_renders` ; passe visuelle par interaction ; edge case 8 |
| 2 | Les deux layers changent l'ordre de dessin (images kitty, barré, curseur) | Med | Med | Découpage fixé dans US-007 d'après l'ordre de lots de GPUI ; fixture de z-order rendue sous Linux et Windows |
| 3 | La réutilisation au scroll affiche du contenu faux | Med | High | Spike US-012 avant tout code ; un test par cas de scroll ; repli sur la reconversion complète en cas de doute |
| 4 | Le plafond d'arènes crée de la contention sur le thread de rendu | Low | Med | Mesure du thread principal par US-019 ; repli sur 4 arènes |
| 5 | Le shimmer à 90 ms est jugé saccadé | Med | Low | Passe visuelle ; repli sur 45 ms avec mesure |
| 6 | Un pane caché manque une publication et reste périmé à l'affichage | Low | High | Publication immédiate à la visibilité ; mode visible par défaut si `Visibility` échoue |
| 7 | Les gains Linux ne se transposent pas sous Windows ou macOS | Med | Med | EP-006 sur Windows réel ; macOS par inspection, faute de machine (Open Questions) |
| 8 | Les nouveaux scénarios rendent `perf_gates` instable sur des runners partagés | Med | Med | Budgets en compteurs d'événements, pas en temps ; marges écrites ; mode consultatif avant d'être bloquant (US-006) |

## Non-Goals

What this version explicitly does NOT include:

- **Changer la cadence du listing système du host** (500 ms dès qu'une session imprime, `crates/paneflow-host/src/runtime.rs:1310`). L'espacer affaiblirait la garantie de suivi des descendants ; à revoir seulement si une mesure sur matériel réel en fait un coût dominant.
- **Réduire le scrollback du miroir** (10 000 lignes, comme le host, `attached_runtime.rs:465`). Le scroll profond demanderait de rapatrier l'historique depuis le host ; décision après mesure du RSS d'une longue session (Open Questions).
- **Changer d'allocateur** (mimalloc, jemalloc) : nouvelle dépendance sans mesure, seulement si US-011 ne suffit pas.
- **Mettre en cache chaque Pane** (`.cached()` autour des enfants de `layout/render.rs:140`) : non mesuré, risque de chrome périmé ; à revoir après US-008 avec 4 splits.
- **Détecter l'occultation ou la minimisation des fenêtres** : GPUI n'expose pas de signal fiable sous Wayland ; ce PRD ne traite que les panes cachés par l'app et la fenêtre inactive.
- **Réécrire le double parse host puis desktop ou le transport base64 dans JSON**, déjà hors périmètre de `prd-performance-gates.md`.
- **Gater en CI le temps de frame absolu ou la charge GPU** : runners sans GPU fiable ; le protocole matériel s'en charge (US-019).

## Files NOT to Modify

- `src-app/Cargo.toml` : les quatre valeurs `rev` de GPUI (`0418cb5b`) et la feature `font-kit` de `gpui_platform`.
- `native/libghostty/manifest.toml`, les archives sous `native/libghostty/prebuilt/` et les plafonds de `src-app/build.rs`.
- `src-app/src/terminal/pty_session/session_backend.rs` et `src-app/src/terminal/types.rs`, avec son test de garde `alacritty_is_absent_from_the_app_crate`.
- `src-app/src/git_command.rs` : `PROBE_CONFIG`, `PROBE_ENV` et `NEUTRALIZED_FILTER_SETTINGS`.
- `crates/paneflow-shim`, `crates/paneflow-ai-hook`, `crates/paneflow-mcp` : aucun changement ni dépendance.
- `crates/paneflow-host/src/process_listing.rs` et la cadence `LISTING_INTERVAL` (Non-Goals).
- `.github/workflows/release.yml` : aucun gate ajouté aux jambes de release.
- `bench/baselines/**`, sauf par `--set-baseline` ou `--refresh-alloc-baselines` sur un arbre propre et commité.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Sidebar en cache :** entité mince qui garde une référence faible vers l'app et appelle `render_sidebar` dans son propre `render` ; les listeners créés par `cx.listener` restent attachés à l'app. Recommandé : cette entité, plutôt qu'une liste virtualisée, parce qu'elle supprime le coût sur toutes les frames de terminal sans réécrire les lignes. L'ingénierie confirme que l'app n'est plus empruntée quand la vue enfant se rend (le rendu des vues enfants suit le retour du `render` racine).
- **Horloge du spinner :** déplacer `SpinnerClock` dans la vue sidebar, ou garder l'horloge à la racine et stocker la demande dans un champ mis à jour au rendu de la sidebar ? Recommandé : la déplacer, pour que ses ticks n'invalident plus que la sidebar.
- **Découpage des layers :** deux layers suffisent-ils, ou les décorations (soulignés nombreux sur une page de liens) méritent-elles un troisième layer ? Recommandé : deux, puis un profil plat sur une page dense en hyperliens.
- **Visibilité des panes :** calculer l'ensemble visible aux points de bascule existants (activation de workspace et d'onglet, fenêtres détachées) ou le déduire d'un « peint récemment » dans `TerminalView` ? Recommandé : les points de bascule, déterministes et testables.
- **Cadence des workspaces inactifs :** 5 min suffisent-elles, ou faut-il viser 10 min pour descendre sous 1 % d'un cœur avec 33 workspaces ? Recommandé : 5 min, à revoir après US-019.
- **Plafond d'arènes :** 2 comme le host, ou 4 pour un processus de rendu à plus de 90 threads ? Recommandé : 2, avec le repli d'US-011.
- **Dépendances :** aucune ; `libc` est déjà présent (`src-app/Cargo.toml:122`), `smol` fournit le canal asynchrone d'US-017.
- **Migration :** aucune donnée utilisateur ; les nouveaux compteurs s'ajoutent à `system.counters` sans changer les existants.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Processus git par minute au repos, 33 workspaces, fenêtre active | 180 | ≤ 30 | Month-1 | `system.counters`, `git_spawns.total` sur 60 s |
| CPU de git en processus enfants, même cas | ~10 % d'un cœur | ≤ 1,5 % | Month-1 | Tour de sondes chronométré, `getrusage` des enfants |
| CPU du thread principal, scénario d'US-002, 18 workspaces | 12,8 % | ≤ 6 % | Month-1 | `/proc/<pid>/task/<pid>/stat` sur 8 s, machine de référence |
| Coût d'une frame, même scénario | 5,0 ms | ≤ 2,5 ms | Month-1 | CPU du thread principal divisé par `root_renders` |
| Rendus de sidebar, 30 s de sortie sans agent | ~750 | ≤ 2 | Month-1 | `sidebar_renders`, budget `perf_gates` |
| Rendus racine par seconde, agent en réflexion | 1,866 mesuré, faux (gate aveugle) ; ~30 attendus avec le shimmer à 33 ms | entre 10 et 12 | Month-1 | `desktop.thinking.root_renders_per_s` |
| Réveils par seconde au repos : IPC, worker, session | 105, 70, 10,6 | ≤ 1 chacun | Month-1 | Changements de contexte par thread sur 10 s ; `idle_wakeups_shell_per_s` |
| Arènes glibc du desktop, Linux gnu | 126 à 5,8 h, non plafonné | ≤ 2 | Month-1 | Tas d'arène de 64 Mio dans `/proc/<pid>/maps` |
| `layout_scroll_cached_220x60` et `publish_scroll_220x60` | 552 µs et 359 µs | ≤ 150 µs et ≤ 100 µs | Month-1 | `scripts/bench-terminal.sh` |
| Publications d'un pane caché par seconde, sortie continue | jusqu'à 125 | ≤ 1 | Month-1 | Compteur de publications du runtime, scénario d'US-014 |
| Objectifs tenus sur matériel réel Linux et Windows | N/A (new) | tous, ou un ajout au PRD par écart | Month-6 | US-019, US-020 |

## Open Questions

- Le shimmer à 90 ms est-il acceptable visuellement, ou faut-il 45 ms ? Arthur, à la passe visuelle d'US-009 ; la borne de `desktop.thinking.root_renders_per_s` en dépend.
- Une machine macOS est-elle disponible pour EP-006 ? Arthur, avant US-020 ; sinon macOS reste vérifié par inspection.
- Le scrollback du miroir mérite-t-il un plafond plus bas que celui du host ? Arthur, après une mesure du RSS d'une session de 4 h à forte sortie ; décide d'un PRD de suivi.
- Le budget `desktop.thinking.root_renders_per_s` a laissé passer 1,866 pendant quatre jours : faut-il une borne basse pour tous les budgets de frames existants (`desktop.focused_idle.root_renders_per_s` compris) ? Arthur, pendant US-001.
[/PRD]
