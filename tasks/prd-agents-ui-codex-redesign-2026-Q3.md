[PRD]
# PRD: Refonte UI de l'interface Agents — cockpit Codex (2026-Q3)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-09 | Arthur Jean | Initial draft. Refonte **UI/UX uniquement** de l'interface Agents pour adopter le shell de l'app desktop Codex : rail gauche nav-Codex (Search / Pinned / Projects / Chats), top-bar Agents (thread · projet + menu overflow), polish visuel. Le terminal central reste le cœur (« là où on parle à l'agent »). PAS de panneau Review/diff, PAS de feature git, on ne touche QUE le mode Agents (Cli/Diff figés). Carto codebase `file:line`-sourcée (6 sous-systèmes). |
| 1.1 | 2026-06-09 | Arthur Jean | Hardening après vérification adversariale (4 checkers + critic contre le code réel). Corrigés : recherche = dead code jamais câblé (pas « dans la barre de titre ») ; `bump_id_counters_to` doit être étendu aux chats (collision sinon) ; `TitleBar` est une entity partagée sans accès à l'état agents → US-010 branche sur présence de champ poussé (pas `self.mode`), US-011 dispatche une action GPUI (pas d'appel direct aux handlers) ; `add_terminal_thread` est dans `project_ops` ; filtre `&Project` → nouvelle `chat_visible` ; menus contextuels déjà séparés (refactor) ; citations `path:line` précisées. |

## Problem Statement

Le mode Agents (`AppMode::Agents`) est déjà un cockpit deux-colonnes `rail gauche | terminal central` : le centre (`render_terminal_thread_surface`, `agents_view_actions.rs:458`) est un `Entity<TerminalView>` nu, sans Pane ni tab-strip, et le rail (`render_agents_sidebar`, `agents_sidebar/mod.rs:102`) navigue une hiérarchie `projects → threads`. Mais l'ergonomie du rail et de la top-bar n'a pas le polish ni la structure du shell desktop Codex que vise le repositionnement « cockpit d'agents » ([[project_paneflow_repositioning_2026-06]]).

1. **Le rail n'a pas la structure nav de Codex.** Aujourd'hui : deux lignes d'action (`new_project_row` « New threads », `skills_row` « Skills », `agents_sidebar/mod.rs:152-153`), un eyebrow « Threads » (`threads_section_header`, `:164`), puis une liste plate de projets collapsibles. Codex structure le rail en sections distinctes (New chat, Search, Pinned, Projects, Chats, Settings) qui donnent une carte mentale immédiate. Le « New threads » de Paneflow est mal nommé (il ouvre en réalité le folder picker pour créer un *projet*, `affordances.rs:272`), et « Skills » est un cul-de-sac qui sort vers un browser.
2. **Pas de threads libres (« Chats »).** Le modèle n'a que `projects: Vec<Project>` → `Project { threads: Vec<Thread> }` (`project/mod.rs:185`). Tout thread est rattaché à un projet (un dossier). Il n'existe aucun concept de conversation libre démarrée hors d'un projet, ancrée sur le home dir — ce que Codex appelle « Chats » et qu'Arthur veut explicitement (« le chats s'ouvre au home de l'utilisateur, /home/arthur »).
3. **Pas d'épinglage.** `Thread` (`project/mod.rs:116`) n'a aucun champ `pinned`. Codex ouvre son rail sur une section « Pinned » qui surface les conversations importantes en haut, indépendamment du projet. Aucun équivalent n'existe.
4. **La recherche est du code mort non câblé.** Le filtre de threads est **entièrement implémenté mais n'a aucun call site** : `render_agents_filter_row` (`agents_sidebar/mod.rs:725`) est `pub(crate)` sans appelant (dead code sous `#![allow(dead_code)]`, `:7`), avec `render_agents_filter_input` (`:750`), `handle_filter_key` (`:912`), l'état `agents_filter`/`agents_filter_focus` (`main.rs:328,332`) et le highlight de match. Le filtre n'est donc rendu **nulle part** aujourd'hui (ni barre de titre, ni rail). Codex en fait un item de nav de premier niveau : la tâche est de **câbler** cette infra existante dans le rail, pas de la migrer depuis un emplacement existant (il n'y en a pas).
5. **La top-bar ignore le contexte Agents.** `TitleBar` (`window_chrome/title_bar.rs:11`) affiche le brand `"PaneFlow"` hardcodé (`:164`) + un breadcrumb du *workspace* (répertoire), poussé chaque frame depuis `main.rs:782-790`. En mode Agents, Codex affiche `titre du thread · nom du projet` + un menu overflow `⋯`. Rien de ce contexte n'est présent aujourd'hui.

**Why now:** le centre terminal + le data model `Project`/`Thread` + le cache PTY warm-resume (`agents_terminal_view_cache`, `main.rs:358`) + le picker d'agents (`render_agents_launcher`, `agents_view_actions.rs:173`) + l'infra de filtre/rename/context-menu sont tous **déjà là et stables**. Le mode Agents est à ~90 % du cockpit Codex ; le travail restant est de la structure UI et du data-model léger, pas de la plomberie système. C'est le levier le plus direct du repositionnement « nouvelle gen d'IDE pour l'ère des agents » ([[feedback_paneflow_monetization]]) sur un terrain où cmux est mac-only. La fenêtre est ouverte tant que la base reste petite, avant le durcissement du port macOS/Windows.

## Overview

Ce PRD est **UI/UX only — zéro feature**. En particulier : **pas de panneau Review/diff** (décision Arthur, peut-être plus tard), **pas de staging/commit git** (Paneflow n'a aucun write-back d'index), et **on ne touche QUE le mode Agents** — les modes Cli et Diff, l'ordre/le toggle des modes, et le centre terminal restent figés.

Quatre epics, du socle de données vers le polish.

Le socle (EP-001) ajoute le data-model minimal que la nav Codex exige : un champ `pinned: bool` sur `Thread` (round-trip session), une liste `chats: Vec<Thread>` **séparée** des projets (décision Arthur : Vec séparé, pas de projet implicite) ancrée sur `dirs::home_dir()`, et un **modèle de sélection unifié** qui remplace le couple positionnel `active_project_idx` + `active_thread_idx` par une cible explicite (thread-de-projet | chat-libre | état-picker), pour que `current_thread_view_target`/`select_thread`/`ensure_terminal_view_mounted` (`agents_view_actions.rs:313,337`) adressent les deux sources. Le cache PTY est keyé par `Thread::id` (u64 via `next_thread_id`), commun aux deux sources — aucun changement de cache requis.

Le cœur visuel (EP-002) refond `render_agents_sidebar` en sections Codex : deux actions de tête (`New chat`, `Search`), puis trois sections à eyebrow (`PINNED`, `PROJECTS` avec bouton `+`, `CHATS`), puis le footer Settings existant. `New chat` crée un chat libre dans le home et ouvre le picker ; `Skills` est retiré du rail ; la recherche migre de la barre de titre vers le rail et s'étend aux chats et aux épinglés. Les widgets de ligne existants (`project_header_row`, `thread_row`, `:443,567`) sont réutilisés/restylés — aucun consommateur ne touche directement aux widgets.

La top-bar (EP-003) rend le brand slot mode-conditionnel : en Agents, `titre du thread · projet` au lieu de `"PaneFlow"`, plus un menu overflow `⋯` qui réexpose les actions du thread courant (rename/duplicate/reveal/delete, handlers déjà dans `affordances.rs`). Les modes Cli/Diff gardent la top-bar actuelle au pixel près.

Le polish (EP-004) aligne espacement, typo, eyebrows en petites majuscules muted, hover states, et affine le picker home-state — le tout via les tokens `UiColors` existants (zéro hex neuf hors les accents de marque déjà présents).

Décisions clés : Chats = `Vec<Thread>` séparé (pas de projet implicite) ; Pinned = flag réel persisté (pas de stub) ; top-bar refondue dans ce chantier (rendu mode-conditionnel) ; recherche déplacée dans le rail ; aucun nouveau `AppMode` ; aucune méthode IPC ; aucun changement au centre terminal.

## Goals

| Goal | Phase-1 Target (P0, EP-001/002) | Phase-2 Target (tout) |
|------|---------------------------------|-----------------------|
| Sections de nav dans le rail Agents (baseline : 1 liste plate + 2 lignes d'action) | New chat + Search + Pinned + Projects + Chats + Settings rendues | idem + polish visuel complet |
| Threads libres ancrés home (baseline : 0, tout thread est dans un projet) | `New chat` crée un chat dans `home_dir()` et le rend dans la section Chats | round-trip session (chats survivent au restart) |
| Épinglage de thread (baseline : aucun) | `pinned: bool` persisté + pin/unpin réel + section Pinned alimentée cross-source | idem |
| Top-bar contextuelle en mode Agents (baseline : brand `"PaneFlow"` statique) | `titre thread · projet` + menu `⋯` | menu overflow complet (rename/dup/reveal/delete) |
| Modes Cli/Diff inchangés (régression zéro) | diff visuel nul sur Cli/Diff (golden) | idem |
| Centre terminal inchangé (warm-resume préservé) | PTY survit à la navigation rail comme aujourd'hui | idem |
| CI verte sur les 4 legs de la matrice de release | requis | requis |

## Target Users

### Orchestrateur d'agents (dont Arthur)
- **Role:** développeur qui fait tourner plusieurs agents de code (Claude Code, Codex, opencode, Gemini) en parallèle, chaque thread = un agent dans un terminal, et qui jongle entre projets et sessions rapides.
- **Behaviors:** ouvre beaucoup de threads par jour, certains rattachés à un repo précis (un projet), d'autres jetables/exploratoires (un chat libre dans le home) ; revient sur quelques threads-clés en boucle ; scanne visuellement le rail pour retrouver une session.
- **Pain points:** tout thread doit être créé dans un projet (pas de session rapide hors-repo) ; aucun moyen d'épingler les threads importants ; la recherche est planquée dans la barre de titre ; le rail n'a pas de carte mentale claire (une liste plate).
- **Current workaround:** crée un faux « projet » pour les sessions rapides ; scrolle le rail à la main ; mémorise quel thread est lequel.
- **Success looks like:** `New chat` ouvre une session jetable dans `~` en un clic ; les threads importants sont en haut sous `Pinned` ; `Search` est à portée immédiate ; la top-bar dit toujours sur quoi il est.

### Nouveau venu évaluant Paneflow
- **Role:** développeur qui teste Paneflow après avoir vu Codex/cmux, et juge en 30 secondes si « ça ressemble à un vrai cockpit d'agents ».
- **Behaviors:** ouvre l'app, regarde le rail, lance un agent, juge le polish.
- **Pain points:** un rail plat + un brand statique « ressemble à un multiplexeur », pas à un IDE d'agents ; l'écart de finition avec Codex se voit immédiatement.
- **Current workaround:** aucun — il referme et n'adopte pas.
- **Success looks like:** le shell évoque immédiatement Codex (sections nav familières, top-bar contextuelle, finition) tout en gardant le différenciant Paneflow (vrai terminal central, cross-platform).

## Research Findings

Findings appliqués par ce PRD (carto codebase complète dans le transcript de session : 6 sous-systèmes — shell/layout, sidebar, agent-launcher, diff, terminal-pane, chrome+theme) :

### Competitive Context
- **Codex desktop app** (la cible visuelle) : rail gauche structuré (New chat / Search / Plugins / Automations / Pinned / Projects / Chats / Settings), zone centrale de conversation, top-bar `titre · projet · ⋯` à gauche + contrôles à droite. Paneflow diverge volontairement sur deux points imposés par son archi : (a) le centre est un **vrai terminal** (« là où on parle à l'agent »), pas une zone de chat — donc pas de champ « Ask for follow-up » ni de sélecteurs model/effort/permissions (le modèle se choisit dans la CLI) ; (b) **Plugins/Automations** n'ont aucun équivalent Paneflow et sont omis.
- **cmux** (concurrent direct, mac-only) : a popularisé le cockpit d'agents avec rail de sessions. Pas de build Linux/Windows (le moat de Paneflow). N'a pas la finition de Codex.
- **Zed agent panel** : `ThreadItem` avec hover-action cluster révélé au hover (zéro layout shift via `.invisible()`), `icon_container().when(!visible, invisible)` pour aligner les rows — patterns **déjà répliqués** dans Paneflow (`thread_row`, `hover_actions_cluster`, `agents_sidebar/mod.rs:692,1103`). On les étend, on ne les réinvente pas.

### Best Practices Applied
- **Sélection par cible explicite, pas par index positionnel.** L'ajout d'une seconde source (chats libres) à côté des projets impose un modèle de sélection unifié ; conserver `active_thread_idx: Option<usize>` mènerait au même bug d'index positionnel stale déjà tracé ailleurs ([[project_ep003_identity_review]] : Move-to-Pane menu, [[reference_gpui_scrollhandle_shared_clamp]]). Le cache PTY keyé par `Thread::id` stable (`main.rs:358`) reste la bonne ancre.
- **Round-trip session backward-compatible.** `pinned` et `chats` s'ajoutent à `ThreadSession`/`SessionState` (`paneflow-config/src/schema.rs:932,834`) avec `#[serde(default, skip_serializing_if = …)]` — une `session.json` pré-refonte se recharge sans perte (pattern éprouvé par les annotations `#[serde(default, …)]` sur `ThreadSession::kind`/`terminal_agent`, `schema.rs:932,941` ; le champ `projects` de `SessionState` utilise `skip_serializing_if = "Vec::is_empty"`, `schema.rs:843`).
- **Cross-platform home.** Le home dir vient de `dirs::home_dir()` (le crate `dirs` est déjà la dépendance maison pour les paths) — jamais `$HOME` brut ni un chemin POSIX hardcodé (contrainte cross-platform).
- **Title strip décoration.** Tout titre OSC affiché passe par `clean_sidebar_title` (`project/mod.rs:333`) avant stockage/affichage ([[feedback_claude_code_title_decoration_stripping]]) — déjà fait pour les threads, à appliquer aussi au titre du thread dans la top-bar.
- **Push depuis le render, pas de lecture globale dans TitleBar.** Les nouveaux champs de la top-bar (titre thread, projet) sont poussés via `title_bar.update()` depuis `PaneFlowApp::render` comme `workspace_name` aujourd'hui (`title_bar.rs:11`, push `main.rs:782-790`) — jamais lus depuis l'état global dans `TitleBar::render` (modèle entity GPUI).
- **on_mouse_down (pas on_click) pour les contrôles top-bar** + `stop_propagation`, sinon le clic est avalé par `WindowControlArea::Drag` / la première frame Wayland est perdue (documenté `title_bar.rs:353-367`).

*Full carto sources available in session transcript (workflow wkwjmbp8r).*

## Assumptions & Constraints

### Assumptions (to validate)
- Le modèle de sélection unifié peut remplacer `active_project_idx`/`active_thread_idx` sans casser le warm-resume : le cache `agents_terminal_view_cache` keye par `Thread::id` (stable), pas par index — à valider par un test de navigation (thread projet ↔ chat libre, PTY survit).
- Le rail à N sections reste sous le budget 16 ms même avec Pinned + Chats + Projects rendus simultanément (le canary `RenderTimeCanary` existe déjà, `agents_sidebar/mod.rs:64`). À surveiller, pas à supposer.
- `chats: Vec<Thread>` séparé n'introduit pas de duplication de logique ingérable : rename/delete/duplicate/title-sync sont paramétrables par cible (l'enum de sélection), pas dupliqués par copier-coller. À valider en revue.
- Aucune session existante n'est cassée par l'ajout de `pinned`/`chats` (serde default). À couvrir par un test de round-trip d'une `session.json` pré-refonte.

### Hard Constraints
- **UI/UX only — aucune feature.** Pas de panneau Review/diff, pas de staging/commit, pas de nouvelle méthode IPC, pas de nouvel `AppMode`. Décision Arthur explicite.
- **On ne touche QUE le mode Agents.** Les modes Cli et Diff, le `render_mode_toggle` (`sidebar_actions_menu.rs:126`), l'ordre des modes et le centre terminal (`render_terminal_thread_surface`) restent figés — diff visuel nul attendu sur Cli/Diff. **Exception assumée :** `TitleBar` (`window_chrome/title_bar.rs`) est une entity PARTAGÉE entre tous les modes ; EP-003 l'étend de façon strictement **additive** (nouveaux champs poussés uniquement sur le bras Agents de `PaneFlowApp::render` ; branche de rendu conditionnée à la **présence du champ poussé**, jamais à une lecture de `self.mode` dans `TitleBar`) — le rendu Cli/Diff reste identique au pixel près (AC de non-régression US-010).
- **Rust + GPUI (fork Zed pinné `ArthurDEV44/zed@paneflow/markdown-append-fix`)** — ne jamais remplacer GPUI par une dép crates.io ; ne pas toucher le pin du fork.
- **Cross-platform Linux + macOS + Windows** pour tout nouveau code ; home via `dirs::home_dir()` ; les branches `cfg(windows)` de `paneflow-app` ne compilent pas sur l'hôte Linux (vérifiées par la CI, inspection-only sur le poste dev, [[reference_windows_crosscheck_limit]]).
- **Re-entrancy GPUI** — ne pas re-lire/`.update()` une entity depuis son propre callback ; différer les mutations via `cx.defer` (pattern déjà appliqué au rename, `affordances.rs:200-214`, [[feedback_gpui_entity_reentrancy]]).
- **Clippy lints** : `panic = "deny"`, `unwrap_used`/`expect_used` = `warn` ; nouveau `unwrap()`/`expect()` suit la convention (`?`, `ok_or`, `match`, `expect("invariant documenté")`).
- **`cargo fmt --check` est un gate CI sur les 4 legs** — lancer `cargo fmt` via Bash avant chaque commit/push touchant du Rust (le hook rustfmt du projet réordonne les imports différemment du `cargo fmt` canonique, [[project_rustfmt_hook_divergence]]).
- **Styling via tokens** — `UiColors` (`theme/model.rs:291`) uniquement ; pas de nouveau hex hors les accents de marque déjà présents (`agents_sidebar/mod.rs:554,559`).

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` — gate de formatage (lancer `cargo fmt` d'abord s'il signale un diff ; CI le lance sur les 4 legs).
- `cargo clippy --workspace --all-targets -- -D warnings` — gate de lint (aucun nouveau warning `unwrap`/`expect` ; `panic`/`unimplemented`/`dbg` déniés).
- `cargo test --workspace` — tous les tests, dont les tests de régression ajoutés par chaque story (round-trip session, sélection, filtre).
- `cargo build --workspace` — le build debug compile.
- Vérification manuelle GUI (changements UI) : le rail, la top-bar et le centre rendent correctement en mode Agents ; les modes Cli/Diff sont visuellement inchangés.

## Epics & User Stories

### EP-001: Socle de données — Chats libres, Pinned, sélection unifiée (Phase 1)

Ajouter le data-model minimal que la nav Codex exige, et refondre la sélection pour adresser deux sources de threads (projets + chats libres) sans index positionnel fragile.

**Definition of Done:** un thread peut être épinglé (persisté), un chat libre peut être créé/sélectionné/affiché dans le home dir et survit au restart, et le centre terminal warm-resume fonctionne identiquement pour un thread-de-projet comme pour un chat-libre ; une `session.json` pré-refonte se recharge sans perte.

#### US-001: Champ `pinned` sur Thread + round-trip session
**Description:** As an orchestrator, I want épingler un thread so that mes sessions importantes restent en haut du rail.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `Thread` (`project/mod.rs:116`), when on ajoute `pub pinned: bool`, then `Thread::new`/`new_terminal` (`:141,162`) l'initialisent à `false` et tous les call sites compilent.
- [x] Given `ThreadSession` (`paneflow-config/src/schema.rs`), when on ajoute `pinned` avec `#[serde(default)]`, then `thread_to_session`/`thread_from_session` (`project/mod.rs:228,272`) round-trippent le flag.
- [x] Given une `session.json` écrite avant ce champ (sans clé `pinned`), when rechargée, then chaque thread restaure `pinned = false` sans erreur — couvert par un test de round-trip.
- [x] `cargo clippy` ne montre aucun nouveau warning issu de l'ajout.

#### US-002: Liste `chats: Vec<Thread>` séparée, ancrée home, persistée
**Description:** As an orchestrator, I want des conversations libres hors projet so that je lance une session jetable dans mon home sans créer de faux projet.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `PaneFlowApp` (`main.rs:362`), when on ajoute `chats: Vec<Thread>`, then les chats sont des `Thread` à part entière (ID via `next_thread_id`, `project/mod.rs:49`) non rattachés à un `Project`.
- [x] Given `SessionState` (`schema.rs:834`), when on ajoute `chats: Vec<ThreadSession>` avec `#[serde(default, skip_serializing_if = "Vec::is_empty")]` (cohérent avec le champ `projects`, `schema.rs:843`), then `save_session`/`restore` round-trippent les chats sans toucher la sérialisation des projets ; une session pré-refonte recharge `chats = []`.
- [x] Given un chat créé, when son cwd est résolu, then il vaut `dirs::home_dir()` (le crate `dirs = "5.0"` est déjà une dép de `src-app`, `Cargo.toml:131` ; `home_dir()` renvoie `Option<PathBuf>` → fallback documenté si `None`), jamais `$HOME` brut ni un chemin POSIX hardcodé.
- [x] `bump_id_counters_to` (`project/mod.rs:56`, signature actuelle `(projects: &[Project])` qui n'itère QUE `projects[*].threads`) DOIT être étendue pour couvrir aussi les chats (ex. `(projects: &[Project], chats: &[Thread])` ou un itérateur combiné) — sinon le compteur n'est pas avancé au-delà des IDs de chats restaurés → collision d'ID. Test : une session projets+chats restaurée, le prochain ID == max(tous les IDs)+1.
- [x] Test : round-trip d'une session avec ≥ 1 projet (≥ 1 thread) + ≥ 1 chat ; IDs uniques garantis.

#### US-003: Modèle de sélection unifié (thread-projet | chat | picker)
**Description:** As a maintainer, I want une cible de sélection explicite so that le centre adresse un thread de projet OU un chat libre sans index positionnel ambigu.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** US-002

**Acceptance Criteria:**
- [x] Un type de cible explicite (ex. enum `AgentsTarget { Thread { project_idx, thread_idx }, Chat { chat_idx } }`, forme exacte laissée à l'engineering) remplace l'usage isolé de `active_thread_idx: Option<usize>` pour la résolution du centre.
- [x] `current_thread_view_target` (`agents_view_actions.rs:313`) et `ensure_terminal_view_mounted` (`:337`) résolvent la cible vers le bon `Thread` (projet ou chat) ; le cache PTY reste keyé par `Thread::id` (`agents_terminal_view_cache`, champ de `AgentsViewState` `main.rs:304`, accédé via `self.agents_view.agents_terminal_view_cache`, `:358`) — un chat et un thread de projet ne peuvent pas collisionner (IDs via `next_thread_id`).
- [x] Le sort des champs positionnels existants `active_project_idx`/`active_thread_idx` (`main.rs:518,522`) est tranché explicitement (supprimés OU conservés comme intermédiaires) et tous les write sites sont mis à jour — au minimum `select_thread` (`project_ops/mod.rs:261`) qui écrit les deux champs (`:274-275`) et `remove_thread` (`:285`).
- [x] Given un thread de projet ouvert puis un chat libre ouvert puis retour au thread, when on navigue, then le PTY du premier thread survit (warm-resume préservé) — test de navigation.
- [x] Given `select_thread`/`remove_thread`/`handle_terminal_thread_title_changed` (`project_ops`, `agents_view_actions.rs:417`), when invoqués sur un chat, then ils opèrent sur `chats` ; sur un thread de projet, sur `projects[p].threads` — pas de duplication de logique non paramétrée.
- [x] L'état « picker / home » (aucune cible sélectionnée) reste distinct et porte son contexte de création (dans quel projet, ou « nouveau chat dans le home »).
- [x] Test : sélection chat ↔ thread projet, suppression d'un chat sélectionné (la sélection retombe proprement sur un état valide, pas un index stale).

---

### EP-002: Refonte du rail — nav Codex (Phase 1, cœur)

Restructurer `render_agents_sidebar` en sections Codex et câbler New chat / Search / Pinned / Projects / Chats, en réutilisant les widgets de ligne existants.

**Definition of Done:** le rail Agents présente, de haut en bas, `New chat`, `Search`, puis les sections `PINNED` / `PROJECTS` (avec `+`) / `CHATS`, puis le footer `Settings` ; `New chat` ouvre une session libre dans le home, la recherche vit dans le rail et filtre les trois sources, et `Skills` n'apparaît plus dans le rail.

#### US-004: Squelette du rail en sections + retrait de Skills
**Description:** As a newcomer, I want un rail structuré comme Codex so that je comprends l'app en un coup d'œil.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `render_agents_sidebar` (`agents_sidebar/mod.rs:102`) émet, dans l'ordre : ligne `New chat`, ligne/affordance `Search`, eyebrow `PINNED` + ses rows, eyebrow `PROJECTS` (+ bouton `+`) + ses rows, eyebrow `CHATS` + ses rows, puis `render_sidebar_settings_footer` + `render_mode_toggle` (inchangés, `:293-294`).
- [x] La ligne `skills_row` (définie `:381`, **appelée `:153`** dans `render_agents_sidebar`) est retirée du rail (supprimer l'appel `:153`) ; `show_agents_skills` (`agents_view_actions.rs:58`) peut rester en code mort géré ou être nettoyé, mais aucun point d'entrée dans le rail.
- [x] Les eyebrows sont des labels en petites MAJUSCULES, `ui.muted` : on suit le pattern *structurel* de `threads_section_header` (`:421` — couleur muted `:437`, top margin, px padding) mais en **ajoutant l'uppercase** que cette fonction n'a pas (elle rend `"Threads"` en `FontWeight::NORMAL` non-capitalisé, `:436`).
- [x] Given une section vide (0 pinned, 0 chats), when le rail rend, then la section se masque ou affiche un hint discret (pas d'eyebrow orphelin au-dessus du vide).
- [x] Le `RenderTimeCanary` (`:64`) ne fire pas en usage normal (≤ ~30 projets/threads visibles).
- [x] Aucune régression sur les modes Cli/Diff (ils n'appellent pas `render_agents_sidebar`).

#### US-005: `New chat` → chat libre dans le home + picker
**Description:** As an orchestrator, I want lancer une session libre dans mon home so that je ne crée pas un faux projet pour un test rapide.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** US-002, US-003

**Acceptance Criteria:**
- [x] La ligne `New chat` (remplace `new_project_row` « New threads » en tête de rail, `agents_sidebar/mod.rs:335`) met la sélection en état « picker pour nouveau chat » (cwd cible = `home_dir()`) et affiche le picker d'agents (`render_agents_launcher`, généralisé pour une cible chat — pas seulement `project_idx`).
- [x] Given le clic sur un agent dans le picker en contexte chat, when sélectionné, then un `Thread` est créé dans `chats` (cwd = home), bound au `terminal_agent`, sélectionné, et son PTY auto-lance la commande (réutilise `create_agent_terminal_thread_in`, `affordances.rs:341`, qui appelle `add_terminal_thread`, `project_ops/mod.rs:235` — les deux généralisés à une cible chat).
- [x] Given le chat créé, when il apparaît, then il est rendu dans la section `CHATS`, pas dans `PROJECTS`.
- [x] L'invariant human-in-loop est préservé : seul le launch command est envoyé (`send_command`), aucun prompt utilisateur auto-soumis ([[feedback_human_in_loop_no_headless]]).
- [x] Le picker en contexte chat affiche un titre adapté (« Start a new chat » plutôt que « Start a new thread »).

#### US-006: Section Pinned (cross-source, ★)
**Description:** As an orchestrator, I want voir mes threads épinglés en haut so that j'y reviens sans scroller.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** US-001, US-004

**Acceptance Criteria:**
- [x] La section `PINNED` agrège tous les threads `pinned == true` à travers `projects[*].threads` ET `chats`, rendus avec le widget `thread_row` (`:567`) restylé.
- [x] Given un thread épinglé, when on clique son row dans Pinned, then il devient la cible sélectionnée (même résolution que sa source d'origine) — pas de doublon de cache PTY.
- [x] Une action pin/unpin existe : ★ au hover (cluster `hover_actions_cluster`, `:1103`, étendu) ET dans le context-menu (US-014). Toggle `thread.pinned` + `save_session`.
- [x] Given 0 thread épinglé, when le rail rend, then la section Pinned est masquée (pas d'eyebrow vide).
- [x] Test : pin d'un chat + d'un thread de projet, les deux apparaissent dans Pinned ; unpin retire de la section ; persistance vérifiée.

#### US-007: Section Projects + bouton `+` (folder picker)
**Description:** As an orchestrator, I want ajouter un projet explicitement so that l'action de création de projet n'est plus cachée derrière « New threads ».

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** US-004

**Acceptance Criteria:**
- [x] L'eyebrow `PROJECTS` porte un bouton `+` (trailing) qui appelle `create_agents_project_with_picker` (`affordances.rs:272`) — l'ancien chemin du « New threads ».
- [x] Les projets sont rendus par `project_header_row` (`:443`) inchangé fonctionnellement (collapse, rename inline, badge `+N/-N` git, context-menu) ; seul le style est aligné Codex.
- [x] Les threads d'un projet expand restent rendus par `thread_row` (`:567`), newest-first (ordre actuel préservé, `:235`).
- [x] Given 0 projet, when le rail rend, then un empty-state sous l'eyebrow Projects invite à en créer un (réutilise `empty_state`, `:958`).

#### US-008: Section Chats (rendu des threads libres)
**Description:** As an orchestrator, I want voir mes chats libres groupés so that je distingue les sessions hors-projet.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** US-002, US-003, US-004

**Acceptance Criteria:**
- [x] La section `CHATS` rend `chats` (newest-first) avec `thread_row`, rename inline et context-menu (réutilise les handlers, paramétrés par cible chat).
- [x] Given un chat sélectionné, when on clique son row, then le centre monte son PTY (cwd = home) via la sélection unifiée (US-003).
- [x] Suppression d'un chat : `remove_thread`-équivalent sur `chats` + `save_session` ; la sélection retombe proprement si le chat supprimé était actif.
- [x] Given 0 chat, when le rail rend, then la section Chats est masquée ou affiche un hint discret (« No chats »).
- [x] Le titre OSC d'un chat met à jour son label via `handle_terminal_thread_title_changed` (généralisé aux chats).

#### US-009: Search dans le rail (migration + extension multi-source)
**Description:** As an orchestrator, I want une recherche de premier niveau dans le rail so that je retrouve un thread instantanément.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** US-004, US-008

**Acceptance Criteria:**
- [x] L'input de recherche (`render_agents_filter_input`, `:750` ; wrapper `render_agents_filter_row`, `:725` — aujourd'hui **dead code sans appelant**) est **câblé** dans le rail, sous `New chat` (nouvel appel depuis `render_agents_sidebar`). Aucun emplacement existant à retirer (il n'y en a pas).
- [x] Le filtre est étendu pour matcher aussi les `chats` : `filter::project_visible`/`thread_visible_in_project` (`agents_sidebar/filter.rs`) prennent `&Project` (inapplicable à un chat sans wrapper), donc une nouvelle fonction `chat_visible(thread: &Thread, lowered_needle: &str) -> bool` est ajoutée dans `filter.rs` ; `match_positions` est réutilisable tel quel. Les hits se reflètent dans Pinned/Projects/Chats.
- [x] Given un filtre actif, when il matche des chats, then la section Chats ne montre que les chats correspondants ; un filtre matchant des projets force-expand comme aujourd'hui (`:204`).
- [x] Given un filtre sans aucun match (toutes sources), when actif, then le hint `no_matches_hint` (`:999`) s'affiche.
- [x] Le lowercase-once de la needle (`:177`) est préservé (pas de régression perf) ; le canary 16 ms ne fire pas.
- [x] Escape efface le filtre et rend le focus (parité `handle_filter_key`, `:912`).

---

### EP-003: Top-bar Agents façon Codex (Phase 2)

Rendre le brand slot mode-conditionnel et ajouter le menu overflow du thread courant, sans toucher la top-bar des modes Cli/Diff.

**Definition of Done:** en mode Agents, la top-bar affiche `titre du thread · nom du projet` (ou `· Chat` pour un chat libre) à gauche et un menu `⋯` qui réexpose rename/duplicate/reveal/delete du thread courant ; les modes Cli/Diff gardent la top-bar actuelle au pixel près.

#### US-010: Brand slot mode-conditionnel (thread · projet)
**Description:** As an orchestrator, I want que la top-bar dise sur quel thread je suis so that je garde le contexte sans regarder le rail.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** EP-001

**Acceptance Criteria:**
- [x] `TitleBar` (`title_bar.rs:11`, champs `workspace_name`/`sidebar_width` `:13,14`) gagne deux champs poussés (`agents_thread_title: Option<String>`, `agents_context_label: Option<String>` pour le projet ou « Chat »), écrits via la closure `title_bar.update()` de `PaneFlowApp::render` (parité `workspace_name`, push `main.rs:791-795`) — `TitleBar` n'importe PAS `AppMode` et ne lit jamais l'état global.
- [x] Given `agents_thread_title.is_some()` (poussé uniquement quand `self.mode == AppMode::Agents`, côté `PaneFlowApp::render`), when la top-bar rend, then le brand slot affiche `titre · contexte` au lieu de `"PaneFlow"` (`:164`) ; le titre passe par `clean_sidebar_title`. La branche teste la **présence du champ poussé**, pas `self.mode` dans `TitleBar` (pattern push-only).
- [x] Given `agents_thread_title == None` (modes Cli/Diff — `PaneFlowApp::render` ne pousse les champs Agents que sur le bras Agents), when la top-bar rend, then elle est IDENTIQUE à aujourd'hui (brand `"PaneFlow"` + breadcrumb workspace) — diff visuel nul sur Cli/Diff.
- [x] Given aucune cible sélectionnée en Agents (état picker), when la top-bar rend, then un label neutre s'affiche (ex. nom du projet actif, ou « Agents ») sans casser l'alignement `sidebar_width`.
- [x] `WindowControlArea::Drag` reste sur la racine ; aucun nouvel élément interactif n'avale le drag.

#### US-011: Menu overflow `⋯` du thread courant
**Description:** As an orchestrator, I want les actions du thread courant dans la top-bar so that je renomme/supprime sans aller dans le rail.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** US-010

**Acceptance Criteria:**
- [x] Un bouton `⋯` dans la top-bar (rendu seulement quand `agents_thread_title.is_some()`) **dispatche une action GPUI typée** (ex. `OpenThreadOverflowMenu`) plutôt que d'appeler directement les méthodes agents : `TitleBar` est une `Entity` distincte sans accès à l'état agents de `PaneFlowApp`. Le pattern suit l'update pill qui dispatche `StartSelfUpdate` (`title_bar.rs`). Pousser un `WeakEntity<PaneFlowApp>` dans `TitleBar` est l'anti-pattern à éviter (dépendance inverse).
- [x] `PaneFlowApp` gère l'action, résout le thread courant, puis ouvre un menu déféré réexposant rename/duplicate/reveal/delete via les handlers de `affordances.rs` (`begin_agents_rename` `:95`, `duplicate_agents_thread` `:386`, `reveal_agents_project_in_file_manager` `:454`, `request_agents_confirm_delete` `:68`, `open_agents_thread_menu` `:44`).
- [x] Le bouton utilise `on_mouse_down` + `stop_propagation` (pas `on_click`), sinon perte Wayland / drag (documenté `title_bar.rs:354-367`).
- [x] Given un chat libre comme cible, when le menu s'ouvre, then les actions non pertinentes (reveal projet) sont masquées ou adaptées au cwd home.
- [x] Le menu se ferme au clic extérieur (parité des autres menus déférés, `affordances.rs:60`).

---

### EP-004: Polish visuel Codex (Phase 2)

Aligner finition et états sur Codex via les tokens existants.

**Definition of Done:** espacement, typo, eyebrows, hover states et picker home-state évoquent le shell Codex sans introduire de hex hors tokens, et les empty-states sont soignés.

#### US-012: Espacement, typo, eyebrows, hover states
**Description:** As a newcomer, I want une finition au niveau de Codex so that l'app inspire confiance immédiatement.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** US-004

**Acceptance Criteria:**
- [x] Rows plus aérées (padding/gap alignés Codex), eyebrows en petites majuscules `ui.muted` avec espacement de section cohérent, badges temps (`format_relative_ts` « 1w/2w/1mo » déjà en place, `:1016`) lisibles.
- [x] Hover states cohérents sur toutes les rows (Pinned/Projects/Chats) via `ui.subtle`/`ui.surface` (pattern existant) ; le row actif via `ui.surface`.
- [x] Aucun nouveau hex hors `UiColors` et les accents de marque déjà présents (`:554,559`) ; le thème clair (`paneflow_light`) reste cohérent (les accents Catppuccin hardcodés sont une dette connue, pas aggravée).
- [x] `prefers-reduced-motion` respecté si une transition est ajoutée (a11y).

#### US-013: Picker home-state + empty-states affinés
**Description:** As an orchestrator, I want un état d'accueil net so that créer un thread/chat est évident.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** US-005, US-007

**Acceptance Criteria:**
- [x] `render_agents_launcher` (`agents_view_actions.rs:173`) est affiné (espacement, titre contextuel chat vs projet) et reste centré, cap 640px.
- [x] Les empty-states (`render_agents_no_project`, `agents_view_actions.rs:472` ; `empty_state`, `agents_sidebar/mod.rs:958` ; `empty_project_hint`, `agents_sidebar/mod.rs:986`) sont alignés visuellement et cohérents avec le nouveau rail.
- [x] Given aucun projet ET aucun chat, when le centre rend, then un empty-state d'accueil unique invite à `New chat` ou `+` Projects.

#### US-014: Context-menu pin/unpin + cohérence des actions
**Description:** As an orchestrator, I want pin/unpin dans le menu contextuel so that l'épinglage est découvrable au-delà du hover.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** US-006

**Acceptance Criteria:**
- [x] Le context-menu de thread (`open_agents_thread_menu`, `affordances.rs:44` ; rendu `render_agents_thread_context_menu`, `context_menus.rs:182`, items rename/duplicate/delete `:222-270`) gagne une entrée Pin/Unpin (label dynamique selon `thread.pinned`).
- [x] L'entrée toggle `thread.pinned` + `save_session` + `cx.notify`, cohérente avec le ★ hover (US-006).
- [x] Le menu de thread-projet (`render_agents_thread_context_menu`, `context_menus.rs:182`) et le futur menu de chat partagent un helper interne paramétré par cible — ce qui exige d'extraire un `render_agents_thread_context_menu_inner(target, …)` commun (le menu projet `render_agents_project_context_menu` `:24` reste distinct). Pas trois fonctions dupliquées divergentes.
- [x] Given un chat, when son context-menu s'ouvre, then les entrées non pertinentes sont masquées/adaptées.

## Functional Requirements

- FR-01: En mode Agents, le rail DOIT présenter les sections, de haut en bas : New chat, Search, PINNED, PROJECTS (avec `+`), CHATS, puis le footer Settings + mode-toggle existants.
- FR-02: `New chat` DOIT créer un thread libre dont le cwd est `dirs::home_dir()`, le rendre dans la section CHATS, et ouvrir le picker d'agents pour le lancer.
- FR-03: Un thread (de projet OU chat) DOIT pouvoir être épinglé (`pinned: bool` persisté) ; la section PINNED agrège les épinglés des deux sources.
- FR-04: Les chats libres DOIVENT être une liste séparée des projets, persistée dans `session.json`, et round-tripper sans casser une session pré-refonte (serde default).
- FR-05: La sélection du centre DOIT adresser indifféremment un thread de projet ou un chat libre, sans index positionnel ambigu, en préservant le warm-resume PTY (cache keyé par `Thread::id`).
- FR-06: La recherche DOIT vivre dans le rail (plus dans la barre de titre) et filtrer les trois sources (Pinned/Projects/Chats).
- FR-07: En mode Agents, la top-bar DOIT afficher `titre du thread · contexte (projet|Chat)` et un menu `⋯` réexposant rename/duplicate/reveal/delete du thread courant.
- FR-08: La top-bar des modes Cli et Diff NE DOIT PAS changer (diff visuel nul).
- FR-09: Aucune action de ce PRD NE DOIT soumettre un prompt utilisateur (human-in-loop) ni introduire de feature Review/diff/git/IPC.
- FR-10: `Skills` NE DOIT plus apparaître comme affordance du rail.

## Non-Functional Requirements

- **Performance:** le rail à N sections rend sous le budget 16 ms en usage normal (`RenderTimeCanary`, `agents_sidebar/mod.rs:64`) ; le filtre conserve le lowercase-once (`:177`) — pas de régression vs aujourd'hui.
- **Robustesse:** aucun panic sur sélection/suppression (clippy `panic = "deny"`) ; suppression d'un chat/thread actif retombe sur un état de sélection valide (pas d'index stale) ; re-entrancy GPUI évitée via `cx.defer` pour les drops d'entity (rename).
- **Compatibilité:** une `session.json` pré-refonte se recharge sans perte (chats=[], pinned=false) ; les modes Cli/Diff sont strictement inchangés.
- **Cross-platform:** home via `dirs::home_dir()` (fallback documenté si `None`) ; aucun chemin POSIX hardcodé ; compile et passe sur les 4 legs CI.
- **Accessibilité:** `prefers-reduced-motion` respecté pour toute transition ; contrastes via tokens.
- **Maintenabilité:** réutilisation des widgets de ligne (`project_header_row`/`thread_row`) et des handlers (`affordances.rs`) paramétrés par cible — pas de duplication projet/chat divergente.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | `home_dir()` indisponible | `New chat` sur un env sans home résolu | Fallback documenté (cwd courant ou `/`) + toast, pas de panic | "could not resolve home directory; opened in {fallback}" |
| 2 | Session pré-refonte | Restore d'une `session.json` sans `chats`/`pinned` | `chats = []`, `pinned = false` partout, recharge propre | — |
| 3 | Chat actif supprimé | Delete du chat sélectionné | Sélection retombe sur un état valide (picker home ou autre), pas d'index stale | "Chat deleted" |
| 4 | Section vide | 0 pinned / 0 chat | Section + eyebrow masqués (ou hint discret), pas d'eyebrow orphelin | "No chats" (Chats) |
| 5 | Filtre sans match | Recherche ne matche ni projet ni chat ni pinned | `no_matches_hint` affiché | "No threads match `<query>`. Press Esc to clear." |
| 6 | Collision d'ID au restore | Compteur non avancé au-delà des chats restaurés | `bump_id_counters_to` couvre projets + chats | — |
| 7 | Top-bar en état picker | Mode Agents, aucune cible sélectionnée | Label neutre (projet actif ou « Agents »), alignement préservé | — |
| 8 | Menu `⋯` sur un chat | Overflow ouvert pour un chat libre | Reveal-projet masqué/adapté au home, pas d'action cassée | — |
| 9 | Bascule mode pendant rename | Toggle vers Cli/Diff avec un rename en cours | `commit`/`cancel_agents_rename` via `cx.defer` (pas de re-entry) | — |
| 10 | Pin d'un thread déjà dans Pinned | Re-pin idempotent | Toggle propre (pin↔unpin), pas de doublon de row | — |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Le modèle de sélection unifié casse le warm-resume PTY | Med | High | Cache keyé par `Thread::id` stable (`main.rs:358`), jamais par index ; test de navigation thread↔chat (US-003). |
| 2 | Duplication divergente de logique projet/chat (rename/delete/title) | High | Med | Handlers paramétrés par cible (l'enum US-003), pas de copier-coller ; revue dédiée. |
| 3 | Régression visuelle sur Cli/Diff via la top-bar partagée | Med | High | Rendu strictement mode-conditionnel ; AC de diff visuel nul (US-010) ; les modes Cli/Diff n'appellent pas le rail Agents. |
| 4 | `session.json` cassée par les nouveaux champs | Med | High | `#[serde(default)]` sur `chats`/`pinned` ; test de round-trip d'une session pré-refonte (US-001/002). |
| 5 | Index positionnel stale (chat/thread supprimé sous une sélection) | Med | Med | Sélection par cible explicite + retombée sur état valide (US-003/008) ; leçon [[project_ep003_identity_review]]. |
| 6 | Canary 16 ms qui fire avec 3 sections + filtre | Low | Med | Lowercase-once préservé ; widgets légers réutilisés ; surveillance canary ; `gpui::list` reste l'échappatoire documentée (`:24`). |
| 7 | Re-entrancy GPUI sur les callbacks (rename/menu) | Med | Med | `cx.defer` pour les drops d'entity (pattern `affordances.rs:200`) ; ne jamais re-lire une entity dans son propre callback. |
| 8 | Scope creep vers une vraie feature (Review/git) | Med | Med | Contrainte dure « UI/UX only » ; Non-Goals explicites ; toute feature = nouveau PRD. |

## Non-Goals

Frontières explicites — ce que cette version NE fait PAS :

- **Panneau Review / diff dans le cockpit** — explicitement reporté par Arthur (« peut-être plus tard »). Aucune 3e colonne, aucun montage de `DiffView` dans le mode Agents.
- **Staging / commit / toute écriture git** — Paneflow n'a aucun write-back d'index ; hors scope par design.
- **Sélecteurs model / effort / permissions dans le chrome** — le modèle se choisit dans la CLI (le centre est un terminal, pas un chat). Le champ « Ask for follow-up » de Codex n'a pas d'équivalent : le terminal EST l'input.
- **Plugins / Automations** (items du rail Codex) — aucun équivalent Paneflow ; omis.
- **Toucher les modes Cli et Diff** — figés (rail, toggle, ordre, centre). Diff visuel nul attendu.
- **Nouvel `AppMode` / nouvelle méthode IPC / changement du centre terminal** — hors scope ; on reste dans `AppMode::Agents` et `render_terminal_thread_surface` inchangé.
- **Migration vers `gpui::list`** pour le rail — différée tant que le canary 16 ms ne fire pas (note `agents_sidebar/mod.rs:24`).

## Files NOT to Modify

- `src-app/Cargo.toml` (section GPUI git deps) et le pin du fork Zed — ne jamais toucher.
- `src-app/src/app/sidebar/` (rail CLI) et `src-app/src/app/diff_sidebar/`, `src-app/src/diff/` — modes Cli/Diff figés.
- `src-app/src/app/agents_view_actions.rs:458` (`render_terminal_thread_surface`) — le centre terminal ne change pas (au-delà d'un éventuel polish de fond non comportemental).
- `src-app/src/app/sidebar_actions_menu.rs:126` (`render_mode_toggle`) — l'ordre/le toggle des modes ne bouge pas.
- `src-app/src/terminal/` (PTY/VT) et l'intégration alacritty/`ZedListener`/`FairMutex` — hors scope.
- `src-app/src/ipc.rs` / `ipc_handler.rs` — aucune nouvelle méthode IPC.

## Technical Considerations

Cadré comme questions pour l'engineering, pas comme mandats :

- **Forme du type de sélection (US-003):** enum `AgentsTarget { Thread{p,t}, Chat{c} }` + `Option<AgentsTarget>` pour l'état picker, vs conserver `active_project_idx` et ajouter un `active_chat_idx` parallèle. Recommandé : enum unifié (élimine la classe de bug index-stale ; un seul chemin de résolution). À confirmer.
- **Chats = Vec séparé (tranché par Arthur)** vs projet implicite « ~ ». Décision prise : `Vec<Thread>` séparé. Conséquence : les handlers (rename/delete/duplicate/title-sync) doivent être paramétrés par cible plutôt que de supposer `projects[p].threads`.
- **Généralisation du picker (US-005):** `render_agents_launcher` prend aujourd'hui `project_idx`. Le généraliser à une cible (« projet i » | « nouveau chat home ») vs dupliquer un `render_chat_launcher`. Recommandé : généraliser (un paramètre de contexte de création).
- **Search dans le rail (US-009):** réutiliser `render_agents_filter_input` tel quel déplacé, vs en faire un item « Search » qui révèle la barre au clic (plus proche de Codex). Recommandé : barre inline toujours visible sous New chat (plus simple, l'infra existe), un item « Search » dépliable est une option de polish ultérieure.
- **Champs top-bar + couplage TitleBar (US-010/011):** `TitleBar` est une entity SÉPARÉE sans accès à l'état agents. (a) *Affichage* : deux `Option<String>` poussés (titre, contexte) via `title_bar.update()` ; la branche de rendu teste la présence du champ, pas `self.mode` (push-only, jamais de lecture d'état global dans `TitleBar::render`). (b) *Actions du menu `⋯`* : le bouton dispatche une action GPUI typée que `PaneFlowApp` gère (pattern de l'update pill → `StartSelfUpdate`), il n'appelle PAS directement les méthodes `affordances.rs`. Pousser un `WeakEntity<PaneFlowApp>` dans `TitleBar` est l'anti-pattern à éviter (dépendance inverse).
- **Search = câblage, pas écriture (US-009):** toute l'infra de filtre est déjà implémentée (`render_agents_filter_row`/`render_agents_filter_input`/`handle_filter_key` + état `agents_filter`/`agents_filter_focus`) mais en dead code sans appelant. US-009 est essentiellement du wiring (appeler `render_agents_filter_row` depuis `render_agents_sidebar`) + l'ajout d'une `chat_visible` dans `filter.rs` ; pas de nouvelle logique de filtre à écrire.
- **Empty-states (US-013):** unifier les trois empty-states existants en un composant paramétré vs les garder séparés. Recommandé : composant paramétré léger.

## Success Metrics

Note : pas de télémétrie desktop ([[feedback_no_app_started_metric]]). Métriques observables / revue de code / vérif manuelle.

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Sections de nav dans le rail | 1 liste plate + 2 lignes d'action | 5 sections + footer | Phase-1 | revue de code + capture |
| Threads libres (chats home) | 0 (impossible) | créables, persistés, rendus | Phase-1 | démo + test round-trip |
| Épinglage | aucun | pin/unpin + section alimentée | Phase-1/2 | démo + test |
| Top-bar contextuelle Agents | brand statique | `thread · contexte` + `⋯` | Phase-2 | capture + vérif manuelle |
| Régression Cli/Diff | n/a | diff visuel nul | continu | vérif manuelle + golden tests existants |
| Warm-resume préservé | ok | ok (thread↔chat) | continu | test de navigation |
| Couverture tests nouveaux chemins | n/a | ≥ 1 test par story de données (round-trip, sélection, filtre) | continu | `cargo test --workspace` |
| CI 4 legs verte | requise | requise | continu | matrice de release |

## Open Questions

- **Q1 (US-009):** « Search » = barre inline toujours visible (recommandé, infra existante) ou item dépliable au clic facon Codex ? Polish, tranchable pendant l'implémentation.
- **Q2 (US-005):** un `New chat` lance-t-il toujours le picker, ou peut-il avoir un agent par défaut (dernier utilisé) pour un flux 1-clic ? Recommandé : picker pour rester explicite ; agent-par-défaut = polish ultérieur.
- **Q3 (US-010):** en état picker (aucune cible), la top-bar affiche le nom du projet actif ou un label neutre « Agents » ? À trancher visuellement.
- **Q4 (US-008):** les chats libres doivent-ils être bornés (cap N, éviction des plus anciens) pour éviter une accumulation de PTY en cache, ou illimités pour v1 ? Recommandé : illimités v1, cap = suivi ultérieur (le cache n'a pas d'éviction aujourd'hui, [[project_agent_ui_refactor_status]]).
[/PRD]
