[PRD]
# PRD: Hiérarchie à onglets pour l'interface CLI - dossier workspace, onglet, layout de panes - 2026-Q3

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-08-24 | Arthur Jean | PRD initial: un niveau `Tab` s'insère entre le workspace et l'arbre de layout, la tab strip interne du pane disparaît, la sidebar CLI devient un arbre dossier/onglets et le badge d'activité descend au niveau de l'onglet |

## Problem Statement

1. **Les onglets sont au mauvais niveau.** `Pane` possède aujourd'hui son propre `tabs: Vec<TabContent>` avec `selected_idx` (`src-app/src/pane.rs:321-322`). Un workspace n'a qu'un seul arbre de layout (`Workspace::root: Option<LayoutTree>`, `src-app/src/workspace/mod.rs:74`), donc la seule façon de tenir deux compositions de travail distinctes dans un même projet est de créer deux workspaces, ce qui duplique le cwd, la détection git, le scan de ports et les worktrees gérés. L'utilisateur qui veut « un fil claude sur le refactor » et « un fil codex sur les tests » dans le même dépôt doit choisir entre deux workspaces redondants ou empiler ses terminaux dans la strip d'un seul pane, où ils sont invisibles depuis la sidebar.

2. **Deux niveaux d'onglets imbriqués coexistent déjà et se contredisent.** Le mode Agents rend un arbre projet > thread (`src-app/src/app/agents_sidebar/mod.rs:461` et `:588`), tandis que le mode CLI rend une liste plate de workspaces (`src-app/src/app/sidebar/mod.rs:494-514`) dont les tabs internes ne sont visibles nulle part hors du pane. Le seul produit du marché à assumer deux niveaux d'onglets homonymes est Emacs (`tab-bar` de frame contre `tab-line` de window), dont la documentation doit ouvrir sur une désambiguïsation. Zellij, WezTerm, kitty, Ghostty et Warp modèlent tous session > onglet > pane sans onglets par pane.

3. **Le badge d'activité agrège trop haut et perd l'information.** `sidebar_agent_summary` (`src-app/src/app/sidebar/mod.rs:231-269`) réduit toutes les sessions d'un workspace à un seul état par précédence `NeedsInput > Errored > Stalled > Finished > Thinking`. Un workspace avec trois agents dont un attend une entrée n'affiche qu'une cloche et un compteur: rien ne dit lequel, ni où. L'information existe pourtant déjà: `AgentSession.surface_id` porte l'entity id du terminal concerné (`src-app/src/ai_types.rs:112`, résolu par `bind_or_resolve_session_surface`, `src-app/src/app/ipc_handler.rs:2176-2187`), et `sync_attention` (`:2350`) la pousse déjà vers les panes. Elle est jetée à l'affichage sidebar.

4. **Ouvrir un agent demande de connaître sa ligne de commande.** Créer un terminal ouvre un shell; lancer `claude`, `codex --yolo` ou `grok --always-approve` suppose de la taper. Les deux catalogues qui contiennent déjà ces commandes sont dispersés et invisibles au moment de la création: `TerminalAgent::visible(config)` (`src-app/src/agent_launcher.rs:394`) alimente des boutons de la tab bar, et `ButtonCommand` (`crates/paneflow-config/src/schema/session.rs:239-249`) vit par workspace dans `session.json`, édité par une modale séparée (`src-app/src/app/custom_buttons_modal.rs`). Aucun point d'entrée unique au moment où l'utilisateur décide quoi lancer.

5. **Le geste « déplacer un tab vers un autre pane » est déjà défectueux.** Le menu contextuel Move to Pane utilise un `tab_idx` positionnel devenu obsolète après réordonnancement, défaut suivi depuis la revue EP-003 identity de 2026-06-05 et jamais corrigé. La mécanique associée (`take_tab_for_move` `:1456`, `insert_moved_tab` `:1481`, `insert_duplicated_tab` `:1511`, `PaneEvent::DropSplit{source_tab_id}` `:241`, `PaneEvent::DuplicateTabInto` `:256`) représente une surface de bug entretenue pour un geste que le split couvre déjà.

**Why now:** deux mesures lèvent le seul risque qui bloquait la décision. D'abord, la persistance est déjà prête: `LayoutNode::Pane { surfaces: Vec<SurfaceDefinition> }` (`crates/paneflow-config/src/schema/layout.rs:146-152`) sérialise les tabs actuels comme un simple tableau, donc la migration v1 vers v2 est mécanique et sans perte. Ensuite, la remontée d'état est déjà résolue: chaque `AgentSession` porte son `surface_id` et chaque terminal porte son `detected_agent` (`src-app/src/terminal/pty_session.rs:1521-1530`), scan PID-authoritative déposé par `apply_pane_scan` (`src-app/src/app/event_handlers.rs:1381`). Le badge par onglet et le cluster d'icônes par pane se calculent donc par filtrage d'un état existant, sans un seul nouveau message IPC. Ce qui restait une refonte spéculative est devenu un déplacement de niveau dans le modèle.

## Overview

Un niveau `Tab` s'insère entre `Workspace` et `LayoutTree`. Un workspace cesse de posséder un arbre de layout et possède une liste d'onglets; chaque onglet possède l'arbre que le workspace possédait, avec sa mécanique de split, de zoom et de navigation inchangée. Symétriquement, le pane cesse de posséder une liste de surfaces et n'en possède plus qu'une: la tab strip du pane disparaît et laisse la place à un header de carte qui garde le nom de surface, la pilule d'agent et le cluster d'actions. Le nombre de niveaux d'onglets dans le produit passe donc de deux à un, ce qui aligne Paneflow sur Zellij, kitty, Ghostty et WezTerm.

La sidebar CLI devient un arbre à deux étages: la ligne workspace devient un dossier repliable, et chaque onglet est une ligne enfant. Ce rendu n'est pas à inventer, il existe déjà dans le mode Agents, où `project_header_row` (`src-app/src/app/agents_sidebar/mod.rs:461`) porte le chevron et l'état déplié et `thread_row` (`:588`) porte l'indentation par placeholder invisible de 14 px (`:691`), le cluster d'actions au survol (`:1175`) et le renommage inline (`:951`). Le mode CLI reprend cette grammaire. Il n'y a pas de barre d'onglets horizontale au-dessus du layout: la liste latérale est le seul emplacement, comme les terminal tabs de VS Code depuis la 1.57, où les splits d'un même onglet sont groupés sous une entrée d'arbre unique.

Le badge d'activité descend d'un cran. Un onglet calcule son propre `SidebarAgentSummary` en filtrant les sessions du workspace sur l'ensemble des entity ids de terminaux de son arbre, exactement la forme du calcul existant appliquée à un sous-ensemble. La ligne d'onglet porte en plus un cluster d'icônes, une par pane, lue directement dans `terminal.detected_agent`. La ligne dossier ne devient pas muette pour autant: repliée, elle réagrège l'état de ses onglets pour ne rien cacher; dépliée, elle n'affiche plus que les sessions non attribuables à un terminal (`surface_id` resté à `None`), cas résiduel des shims anciens.

La création d'un onglet passe par une palette de presets. Elle ne crée pas un quatrième catalogue: elle est une vue unifiée en lecture sur les trois sources qui existent, le shell par défaut, les agents visibles de `TerminalAgent::visible(config)` et les `custom_buttons` du workspace. Chaque ligne porte un chevron ouvrant les variantes déclarées de la commande, plus le placement voulu (nouvel onglet, split à droite, split en bas), et le pied de liste renvoie vers les écrans Settings qui éditent déjà ces sources. Aucun champ de configuration n'est ajouté à `paneflow.json` par cette version, donc aucune migration de configuration.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Un seul niveau d'onglets dans le produit | 0 `Vec<TabContent>` restant dans `pane.rs`, 0 geste de tab inter-panes | Idem, plus parité des gestes onglet sur les trois OS |
| Rendre visible ce qui tourne, sans ouvrir le pane | 100 % des sessions à `surface_id` résolu attribuées à leur onglet; icône par pane pour les 16 agents détectés | Idem, plus l'état par pane au survol |
| Ne perdre aucune session existante à la mise à jour | 100 % des surfaces d'un `session.json` v1 restaurées après migration, 0 workspace perdu | Idem, avec fixture v1 gelée en CI |
| Lancer un agent sans taper sa commande | Palette atteignable au clavier, 3 sources de presets fusionnées, 0 nouveau champ de config | Variantes déclarables par l'utilisateur |
| Ne pas régresser le terminal | 0 modification de `src-app/src/terminal/*`, gates verts à chaque story | Idem après la promotion Windows de libghostty |

## Target Users

### Développeur pilotant plusieurs agents sur un même dépôt

- **Role:** utilisateur principal de Paneflow, un projet ouvert, plusieurs fils de travail simultanés, sous Linux, macOS ou Windows.
- **Behaviors:** ouvre un agent sur un refactor, un autre sur les tests, un shell pour les commandes git, un serveur de dev; passe de l'un à l'autre toute la journée.
- **Pain points:** un seul arbre de layout par workspace le force à empiler les panes ou à dupliquer le workspace; les terminaux empilés dans la strip d'un pane sont invisibles depuis la sidebar; le badge du workspace lui dit qu'un agent attend, pas lequel.
- **Current workaround:** deux workspaces sur le même dossier, ou un pane à quatre tabs qu'il faut ouvrir pour savoir ce qu'il contient.
- **Success looks like:** une ligne par fil de travail sous le dossier du projet, avec l'icône de ce qui tourne dans chaque pane et le loader sur le bon fil.

### Développeur qui revient sur un projet après une pause

- **Role:** même personne, quelques heures plus tard, ou après un redémarrage de l'application.
- **Behaviors:** rouvre Paneflow, cherche le fil sur lequel il travaillait, vérifie ce que les agents ont produit.
- **Pain points:** rien dans la sidebar ne nomme les compositions de travail; il faut cliquer dans chaque workspace pour retrouver l'état.
- **Current workaround:** renommer les workspaces à la main pour encoder le fil de travail dans le titre.
- **Success looks like:** les onglets nommés survivent au redémarrage avec leur layout, et une mise à jour de Paneflow ne lui coûte aucune surface.

## Research Findings

### Competitive Context

- **Zellij** (session > tab > pane) et **WezTerm** (workspace > tab > pane), **kitty** (os-window > tab > window), **Ghostty** (window > tab > split): tous exposent un niveau d'onglets et aucun n'a d'onglets par pane. C'est la structure cible, elle est standard.
- **VS Code**: les terminal tabs livrées en 1.57 (mai 2021) sont une liste latérale, pas une strip, et les splits d'un même onglet sont groupés sous une seule entrée d'arbre. Précédent direct du rendu sidebar retenu ici, y compris la cohabitation sans friction avec les onglets d'éditeur parce que les régions sont séparées.
- **Windows Terminal**: `newTabMenu` accepte des entrées `profile`, `folder` (sous-menus imbriqués), `separator`, `matchProfiles` et `remainingProfiles`, avec une page Settings dédiée. Modèle du picker et du chevron.
- **Warp**: les Tab Configs TOML décrivent un arbre de panes avec des paramètres typés référencés par interpolation, et les anciennes Launch Configurations restent lisibles indéfiniment après avoir été marquées legacy. Leçon de migration: renommer le concept sans casser l'ancien format.
- **cmux**: sidebar de tabs verticaux riches (branche, PR, cwd, ports, dernière notification) mais conserve aussi des tabs horizontaux par pane. Sa convention clavier, un modificateur par niveau, est la plus propre relevée.
- **Emacs**: seul cas assumé de deux niveaux d'onglets, `tab-bar` contre `tab-line`, dont la documentation doit s'ouvrir sur une désambiguïsation. Confirme le coût cognitif que cette version supprime.
- **Market gap:** aucun produit ne rend, sur une ligne d'onglet, un cluster d'icônes identifiant l'agent de chaque pane. C'est le différenciateur visuel de cette version.

### Best Practices Applied

- Adressage par identifiant stable et jamais par index positionnel: tmux a dû introduire `%pane_id` et `@window_id` parce que `session:window.pane` cassait les scripts dès que `base-index` changeait. Paneflow conserve `surface_id` comme unité d'adressage et n'expose aucun index d'onglet dans l'IPC.
- Versionner le schéma de session et écrire la branche de migration en même temps que le bump, plutôt que de compter sur un fallback.
- Ne pas réaffecter silencieusement un raccourci existant: `Ctrl+Tab` garde son sens actuel et le niveau onglet reçoit ses propres bindings.

*Sources: documentation Warp Tab Configs, documentation Zellij Layouts, PR microsoft/terminal newTabMenu, manuels Emacs Tab Bars et Tab Line, notes de version VS Code 1.57.*

## Assumptions & Constraints

### Assumptions (to validate)

- **A1:** réduire un pane à une seule surface ne supprime aucun usage réel, parce que le split couvre le besoin de juxtaposition et que la sidebar couvre le besoin d'empilement. Fondé sur le fait que Zellij, kitty, Ghostty et WezTerm n'ont jamais eu d'onglets par pane. Non validé sur les utilisateurs de Paneflow.
- **A2:** un workspace dépasse rarement une dizaine d'onglets, donc la liste latérale reste lisible et n'a pas besoin de virtualisation par `uniform_list`. Si faux, la sidebar devra virtualiser.
- **A3:** la migration v1, qui promeut les surfaces non actives d'un pane en onglets à pane unique, produit un bruit acceptable à la première ouverture. Non validé.
- **A4:** les trois sources de presets (shell, agents, `custom_buttons`) suffisent au premier jet et l'utilisateur ne réclamera pas de variantes personnalisées avant plusieurs semaines.

### Hard Constraints

- Compatibilité Linux, macOS et Windows obligatoire pour chaque story, sans chemin exclusif à un OS.
- Aucune modification de `src-app/src/terminal/*`: le backend libghostty est en cours de promotion Windows (`tasks/prd-windows-libghostty-backend-2026-Q3.md`, `IN_PROGRESS`) et toute collision y serait coûteuse.
- Aucune collision avec `tasks/prd-agents-chat-transcript-2026-Q3.md` (`READY`): ce PRD ne touche pas le mode Agents ni `agents_sidebar/`, il en emprunte seulement les patrons de rendu.
- Aucun événement de télémétrie ajouté: les métriques de ce PRD se mesurent localement.
- `surface_id` reste l'unité d'adressage externe: aucun index positionnel d'onglet dans l'IPC ou le pont MCP.

## Quality Gates

Ces commandes doivent passer pour chaque user story:

- `cargo fmt --check` - formatage canonique, gate du pipeline de release sur les quatre legs de la matrice
- `cargo clippy --workspace -- -D warnings` - lints workspace
- `cargo test --workspace` - suite complète

Pour toute story marquée UI (US-006, US-008 à US-013, US-014 à US-017), une vérification visuelle manuelle est requise en plus des gates: lancer `cargo run`, exercer le geste décrit dans les critères et confirmer le rendu. Les gates automatisés ne couvrent pas le rendu GPUI.

## Epics & User Stories

### EP-001: Socle - l'onglet porte le layout

Insérer le niveau `Tab` entre `Workspace` et `LayoutTree` sans rien changer à ce que voit l'utilisateur. À la fin de cet épic chaque workspace possède exactement un onglet implicite, le pane garde encore sa tab strip, et l'application est indiscernable de la version précédente.

**Definition of Done:** `Workspace` ne possède plus `root` ni `saved_layout`; tous les appelants passent par l'onglet actif; les gates passent et le comportement observable est inchangé.

#### US-001: Le workspace possède une liste d'onglets
**Description:** En tant que développeur, je veux que le modèle de données porte une liste d'onglets par workspace, afin que plusieurs compositions de travail puissent coexister dans un même projet.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Une structure `Tab` porte un identifiant stable, un titre, un `LayoutTree` et un `saved_layout` de zoom; `Workspace` porte `tabs: Vec<Tab>` et `active_tab_idx: usize`, en remplacement de `root` et `saved_layout` (`src-app/src/workspace/mod.rs:74-77`).
- [x] `pane_count`, `contains_pane`, `any_pane`, `collect_panes`, `focus_first`, `propagate_custom_buttons` et `propagate_config` (`src-app/src/workspace/mod.rs:244-343`) parcourent tous les onglets et non le seul onglet actif, en incluant les `saved_layout` de chacun comme le code actuel inclut le `saved_layout` du workspace.
- [x] `MAX_PANES` (`src-app/src/layout/mod.rs:36`) devient une borne par onglet et non par workspace; une nouvelle constante `MAX_TABS_PER_WORKSPACE = 32` est déclarée à côté de `MAX_WORKSPACES` (`src-app/src/workspace/mod.rs:24`) et exportée par le même chemin.
- [x] Given un workspace dont tous les onglets sont fermés, when le dernier onglet est retiré, then le workspace conserve exactement un onglet vide plutôt que zéro, et aucun code appelant ne peut observer `tabs.is_empty()`.
- [x] Given une tentative de création au-delà de `MAX_TABS_PER_WORKSPACE`, when elle est soumise, then elle est refusée sans panique et sans mutation partielle, et le refus est journalisé au niveau `warn`.
- [x] Tests unitaires dans `src-app/src/workspace/mod.rs` couvrant l'invariant du dernier onglet et le refus au plafond.

#### US-002: Zoom et sérialisation par onglet
**Description:** En tant que développeur, je veux que le zoom et la sérialisation de layout appartiennent à l'onglet, afin qu'un onglet zoomé n'affecte pas les autres.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] `is_zoomed` et `exit_zoom` (`src-app/src/workspace/mod.rs:228-242`) opèrent sur l'onglet actif; deux onglets peuvent être zoomés indépendamment.
- [x] `serialize_layout` et `serialize_layout_without_scrollback` (`:297-307`) produisent un `LayoutNode` par onglet et conservent la préférence actuelle pour `saved_layout` quand l'onglet est zoomé.
- [x] Given un onglet zoomé, when l'utilisateur bascule vers un autre onglet puis revient, then l'état zoomé et le layout sauvegardé sont intacts.
- [x] Given un onglet zoomé dont le pane zoomé est fermé, when la fermeture aboutit, then l'onglet sort du zoom sans perdre les panes du `saved_layout`.
- [x] Test de non-régression dans `src-app/src/layout/` vérifiant qu'un aller-retour sérialisation d'un workspace à deux onglets, dont un zoomé, est idempotent.

#### US-003: Les opérations de layout sont routées vers l'onglet actif
**Description:** En tant que développeur, je veux que split, fermeture de pane et navigation de focus s'appliquent à l'onglet actif, afin que le comportement observable reste identique après l'insertion du niveau.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Les opérations de `src-app/src/app/workspace_ops/` (création, fermeture, sélection, focus) et les presets de `src-app/src/layout/presets.rs` s'appliquent à l'onglet actif du workspace actif.
- [x] Les cinq sites qui appliquent `MAX_PANES` (`src-app/src/cli/flow_spec.rs:369`, `src-app/src/cli/workspace_spec.rs:134`, `src-app/src/settings/tabs/workspaces.rs:1330`, `src-app/src/app/session.rs:711`, `src-app/src/ipc.rs:49`) comptent les feuilles de l'onglet visé et non du workspace.
- [x] `find_pane_by_surface_id` (`src-app/src/app/ipc_handler.rs:824-861`) retourne l'onglet propriétaire en plus du pane, et tous ses appelants consomment la nouvelle forme.
- [x] Given un `surface_id` appartenant à un onglet non actif, when un appelant le résout, then il obtient le bon onglet et aucun appelant ne suppose que le pane trouvé est visible.
- [x] Given un split demandé dans un onglet déjà à 32 feuilles, when il est soumis, then il est refusé avec le même message que le refus actuel et l'arbre reste inchangé.

---

### EP-002: Le pane devient mono-surface

Supprimer la tab strip du pane et tout ce qui en dépend. Le pane ne possède plus qu'un terminal, garde son header, son cluster d'actions et sa carte flottante introduite par le commit `30e26c5`.

**Definition of Done:** `pane.rs` ne contient plus ni `tabs` ni `selected_idx`; aucun geste de déplacement de tab entre panes ne subsiste; les gates passent.

#### US-004: `Pane` abandonne son `Vec<TabContent>`
**Description:** En tant que développeur, je veux que `Pane` ne possède qu'une surface, afin que le seul niveau d'onglets du produit soit celui du workspace.

**Priority:** P0
**Size:** XL (8 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] `tabs: Vec<TabContent>` et `selected_idx` (`src-app/src/pane.rs:321-322`) sont remplacés par une surface unique; les états aujourd'hui indexés par `EntityId` (attention, errored, hits de recherche, prefill en attente) dégénèrent en `Option`/`bool` sans map.
- [x] Les méthodes multi-tab de `pane.rs` (`new_with_tabs:487`, `add_tab:894`, `close_tab_at:1370`, `reorder_tab:1427`, `take_tab_for_move:1456`, `insert_moved_tab:1481`, `insert_duplicated_tab:1511`, `render_tab_bar:1684`, `render_tab:1805`) sont supprimées ou réduites à leur équivalent mono-surface; `active_terminal_opt` (`:1579`) devient infaillible. Écart acté à la revue EP-002: `active_terminal_opt` reste `Option` parce que `PaneSurface` porte aussi `Markdown` et `Diff`; l'infaillibilité demandée n'est atteignable que si le pane est terminal-only, ce que la PRD ne prévoit pas.
- [x] Les variantes de `PaneEvent` qui portent un index ou un id de tab (`NewTerminalTab:207`, `TabsChanged:230`, `OpenTabMenu:233`, `DropSplit{source_tab_id}:241`, `DuplicateTabInto{dest_idx}:256`) sont supprimées ou reformulées sans notion d'index.
- [x] Given un pane dont le terminal se termine, when le processus sort, then le pane se ferme comme aujourd'hui se fermait le dernier tab d'un pane, sans laisser de pane vide dans l'arbre.
- [x] Les tests existants de `src-app/src/pane.rs:3124` sont conservés ou remplacés par leur équivalent mono-surface, jamais supprimés en silence.

#### US-005: Les appelants externes passent à l'API mono-surface
**Description:** En tant que développeur, je veux que les dix-neuf fichiers qui manipulent les tabs d'un pane soient migrés, afin que le workspace compile et se comporte comme avant.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [x] Les 93 références aux API de tab hors `pane.rs` sont migrées, en priorité `src-app/src/app/workspace_ops/mod.rs` (17), `src-app/src/app/event_handlers.rs` (16), `src-app/src/app/theme_picker.rs` (10), `src-app/src/app/sidebar/context_menu.rs` (7), `src-app/src/app/pane_drag.rs` (6), `src-app/src/app/ipc_handler.rs` (5), `src-app/src/app/workspace_ops/tab.rs` (4), `src-app/src/layout/serde.rs` (3).
- [x] `src-app/src/app/workspace_ops/tab.rs` est réorienté: `handle_new_tab` crée un onglet de workspace, `handle_close_tab` ferme un onglet de workspace.
- [x] Given un `grep` sur les identifiants de tab de pane après la story, when il est exécuté sur `src-app/`, then il ne retourne que des occurrences relatives au niveau workspace.
- [x] Given une compilation de la matrice cross-platform, when les trois cibles sont vérifiées, then aucune branche `cfg(target_os)` ne conserve un appel à une API supprimée; les branches Windows non compilables sur hôte Linux sont vérifiées par inspection et le fait est déclaré dans la PR.

#### US-006: Le header de pane remplace la tab strip
**Description:** En tant que développeur, je veux que la bande supérieure du pane devienne un header de carte, afin que le pane garde son identité visuelle et ses actions sans onglets.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [x] La bande rendue par `render_tab_bar` devient un header portant le nom de surface, la pilule d'agent (`render_agent_pill`, `src-app/src/pane.rs:2343`) et le cluster d'actions existant, sans liste d'onglets.
- [x] Le header reste transparent et laisse la carte peindre, conformément au modèle posé par le commit `30e26c5`: la carte est la seule surface qui peint dans le sous-arbre du pane.
- [x] Les boutons de commandes personnalisées (`src-app/src/pane.rs:2618-2632`) et l'indicateur de zoom restent atteignables depuis le header.
- [x] Given un pane non focalisé, when il est rendu, then l'atténuation introduite par `f2bdd9d` s'applique au header comme au contenu.
- [x] Given un nom de surface plus long que la largeur disponible, when il est rendu, then il est tronqué sans faire déborder le header ni pousser le cluster d'actions hors de la carte.
- [x] Vérification visuelle manuelle requise.

#### US-007: Retrait des gestes de tab inter-panes
**Description:** En tant que développeur, je veux que les gestes de déplacement et duplication de tab entre panes soient retirés, afin de supprimer une surface de bug que le split couvre déjà.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [x] L'entrée Move to Pane du menu contextuel est supprimée, avec le défaut de `tab_idx` positionnel qu'elle traînait depuis la revue EP-003 identity.
- [x] Le glisser-déposer d'un tab sur un pane cible (`src-app/src/app/pane_drag.rs`) est retiré; le glisser-déposer de pane à l'intérieur d'un onglet est conservé.
- [x] `PaneEvent::DropSessionSplit` dont le bord `None` signifiait « ajouter en tab » (`src-app/src/pane.rs:268`) exige désormais un bord de split explicite, ou crée un nouvel onglet quand aucun bord n'est fourni.
- [x] Given un dépôt de session ou de markdown sans bord (`DropMarkdownSplit`, `:281`), when il aboutit, then le comportement retenu est documenté dans le code et couvert par un test.
- [x] Given un utilisateur qui cherche l'ancien geste, when il ouvre le menu contextuel du pane, then aucune entrée morte ni désactivée ne subsiste.

---

### EP-003: Sidebar CLI - dossier workspace et lignes d'onglet

Transposer dans la sidebar CLI la grammaire d'arbre déjà en service dans la sidebar Agents.

**Definition of Done:** un workspace se replie, ses onglets sont des lignes enfants sélectionnables, renommables, fermables et déplaçables; les gates passent et la vérification visuelle est faite.

#### US-008: La ligne workspace devient un dossier repliable
**Description:** En tant que développeur, je veux replier un projet dans la sidebar, afin de garder une liste lisible quand plusieurs projets sont ouverts.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] `render_workspace_row` (`src-app/src/app/sidebar/mod.rs:514`) porte un chevron et un état déplié, sur le patron de `project_header_row` (`src-app/src/app/agents_sidebar/mod.rs:461`, bascule `:535`), avec l'icône de dossier de la référence visuelle.
- [x] L'état déplié de chaque workspace est mémorisé pour la durée de la session applicative et n'est pas persisté, comme l'état d'ouverture de la sidebar Files.
- [x] Le glisser-déposer de réordonnancement de workspace (`WorkspaceDrag`, bords de dépôt `:58`) continue de fonctionner sur la ligne dossier.
- [x] Given un workspace replié dont un onglet reçoit une demande d'entrée agent, when l'état change, then la ligne dossier affiche l'agrégat, conformément à US-012.
- [x] Given un double-clic sur la ligne dossier, when il est reçu, then il déclenche le renommage du workspace comme aujourd'hui (`:558`) et non le repliement.
- [x] Vérification visuelle manuelle: faite le 2026-08-25 (sélecteur plein pane, boutons centrés, split-bas sur le pane de droite rendu au bon emplacement).

#### US-009: Les onglets sont des lignes enfants
**Description:** En tant que développeur, je veux voir mes onglets listés sous le dossier du projet, afin de savoir ce qui existe sans ouvrir chaque workspace.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Chaque onglet d'un workspace déplié est rendu comme une ligne enfant sur le patron de `thread_row` (`src-app/src/app/agents_sidebar/mod.rs:588`), avec le placeholder invisible de 14 px pour l'indentation (`:691`) et sans icône par onglet, conformément à la règle établie pour les lignes de thread.
- [x] Un clic sur une ligne d'onglet active le workspace et l'onglet et donne le focus au premier pane de cet onglet.
- [x] L'onglet actif du workspace actif porte l'état visuel actif; l'onglet actif d'un workspace non actif porte un état distinct de l'inactif.
- [x] Given un titre d'onglet plus long que la largeur de la sidebar, when il est rendu, then il est tronqué avec ellipse sans faire déborder ni chevaucher le badge d'activité.
- [x] Given un workspace à `MAX_TABS_PER_WORKSPACE` onglets, when il est déplié, then la liste reste scrollable et la hauteur des lignes de workspace n'est pas écrasée, comme le vérifie déjà `sidebar_workspace_rows_keep_height_when_list_overflows` (`src-app/src/app/sidebar/mod.rs:1281`).
- [x] Vérification visuelle manuelle: faite le 2026-08-25 (sélecteur plein pane, boutons centrés, split-bas sur le pane de droite rendu au bon emplacement).

#### US-010: Cycle de vie d'un onglet depuis la sidebar
**Description:** En tant que développeur, je veux créer, renommer et fermer un onglet depuis la sidebar, afin de gérer mes fils de travail sans passer par le pane.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] La ligne dossier expose au survol une action de création d'onglet, sur le patron de `hover_actions_cluster` (`src-app/src/app/agents_sidebar/mod.rs:1175`); elle ouvre la palette de US-014.
- [x] Le renommage inline d'un onglet reprend le mécanisme de `src-app/src/app/agents_sidebar/mod.rs:951` et le titre par défaut d'un onglet est le nom du preset qui l'a créé, passé par `clean_sidebar_title`.
- [x] Un menu contextuel sur une ligne d'onglet offre au minimum renommer et fermer, et suit les conventions de `src-app/src/app/sidebar/context_menu.rs`.
- [x] Given la fermeture du dernier onglet d'un workspace, when elle est confirmée, then le workspace conserve un onglet vide conformément à l'invariant de US-001, et ne se ferme pas.
- [x] Given la fermeture d'un onglet contenant un agent en cours, when elle est déclenchée, then le comportement est identique à la fermeture actuelle d'un pane portant un agent, sans processus orphelin.
- [x] Vérification visuelle manuelle: faite le 2026-08-25 (sélecteur plein pane, boutons centrés, split-bas sur le pane de droite rendu au bon emplacement).

#### US-011: Réordonner un onglet et le déplacer entre projets
**Description:** En tant que développeur, je veux glisser un onglet dans la liste, afin de le ranger ou de le rattacher à un autre projet.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Un onglet peut être glissé pour être réordonné à l'intérieur de son workspace, en réutilisant le mécanisme de bords de dépôt existant (`src-app/src/app/sidebar/mod.rs:58`).
- [x] Un onglet peut être déposé sur un autre dossier workspace, ce qui le rattache à ce workspace en conservant son arbre de panes et ses terminaux vivants.
- [x] Given un dépôt sur un workspace déjà à `MAX_TABS_PER_WORKSPACE`, when il est relâché, then le déplacement est refusé, l'onglet retourne à sa position d'origine et aucun terminal n'est tué.
- [x] Given un onglet déplacé vers un workspace de cwd différent, when le déplacement aboutit, then les terminaux gardent leur cwd réel et le PRD ne prétend pas les relocaliser.
- [x] Vérification visuelle manuelle: faite le 2026-08-25 (sélecteur plein pane, boutons centrés, split-bas sur le pane de droite rendu au bon emplacement).

---

### EP-004: Statut au niveau de l'onglet

Descendre le badge d'activité du workspace vers l'onglet et rendre visible, par onglet, ce qui tourne dans chaque pane.

**Definition of Done:** chaque ligne d'onglet porte son propre état agent et le cluster d'icônes de ses panes; aucune session résolue n'est attribuée au mauvais onglet.

#### US-012: Badge d'activité par onglet et repli du dossier
**Description:** En tant que développeur, je veux que le loader s'affiche sur l'onglet concerné, afin de savoir immédiatement lequel de mes fils attend une entrée.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Un onglet calcule son `SidebarAgentSummary` en filtrant `workspace.agent_sessions` sur l'ensemble des entity ids de terminaux de son arbre, en réutilisant la fonction de précédence existante (`sidebar_agent_summary`, `src-app/src/app/sidebar/mod.rs:231-269`) plutôt qu'en la dupliquant.
- [x] `render_workspace_agent_summary` (`:1019`) et le loader comet (`:1114`) sont rendus sur la ligne d'onglet, avec la même table d'états et les mêmes largeurs de slot (`slot_width`, `:104`).
- [x] Une ligne dossier dépliée n'affiche que l'agrégat des sessions dont le `surface_id` est resté `None`, cas des shims anciens et des chaînes de résolution non abouties; elle n'affiche rien quand cet ensemble est vide.
- [x] Une ligne dossier repliée affiche l'agrégat complet de ses onglets, plus les sessions non attribuées, de sorte qu'aucun état ne soit caché par le repli.
- [x] Given une session dont `surface_id` se résout après coup par la marche d'ancêtres (`src-app/src/app/ipc_handler.rs:2265-2319`), when la résolution aboutit, then le badge migre du dossier vers l'onglet propriétaire au rendu suivant, sans double comptage.
- [x] Given une session dont le PID est balayé par `sweep_stale_pids` (`src-app/src/app/event_handlers.rs:1126-1246`), when le balayage passe, then le badge de l'onglet disparaît au même rythme qu'aujourd'hui celui du workspace.
- [x] `AgentCompletionNotification` (`src-app/src/workspace/mod.rs:123`) est acquittée par le clic sur l'onglet concerné et non plus seulement par le clic sur le workspace.
- [x] Tests unitaires dans le module de tests de la sidebar (`src-app/src/app/sidebar/mod.rs:1240+`) couvrant: filtrage par onglet, session non attribuée, et repli du dossier.
- [x] Vérification visuelle manuelle requise.

#### US-013: Cluster d'icônes par pane sur la ligne d'onglet
**Description:** En tant que développeur, je veux voir sur la ligne d'onglet une icône par pane identifiant ce qui y tourne, afin de reconnaître un fil de travail sans l'ouvrir.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] La ligne d'onglet rend une icône par pane, dans l'ordre de parcours des feuilles de son arbre, lue depuis `terminal.detected_agent` (`src-app/src/terminal/pty_session.rs:1521-1530`) sans déclencher de scan supplémentaire au render.
- [x] Le chemin d'icône vient de `TerminalAgent::icon_path` (`src-app/src/agent_launcher.rs:96-115`); un pane sans agent détecté rend une icône de terminal générique. Arbitrage visuel du 2026-08-24: le cluster est monochrome, une seule teinte pour tout le rail, donc la distinction multicolore contre teinté (`icon_multicolor`, `:154`) ne s'applique pas ici - tous les glyphes passent par `svg()`.
- [x] Given un onglet à plus de quatre panes, when il est rendu, then le cluster est plafonné visuellement et le dépassement est indiqué sans faire déborder la ligne ni écraser le badge d'activité.
- [x] Given un pane dont l'agent n'est pas encore détecté au premier rendu, when le scan aboutit (debounce 500 ms, `src-app/src/app/event_handlers.rs:1256-1279`), then l'icône apparaît sans provoquer de reflow de la ligne.
- [x] Given un onglet vide, when il est rendu, then aucun cluster n'est peint et la ligne garde la même hauteur que les autres.
- [x] Vérification visuelle manuelle requise.

---

### EP-005: Palette de presets « New pane »

Un point d'entrée unique au moment où l'utilisateur décide quoi lancer, construit comme une vue sur les catalogues existants.

**Definition of Done:** le sélecteur occupe la place de la surface à créer, onglet ou moitié de split, se parcourt au clavier et y lance le preset choisi; aucun champ n'a été ajouté à `paneflow.json`.

**Réduction de périmètre (2026-08-25, décision produit):** le sélecteur est une colonne de boutons simples, centrée dans le pane. Le champ de recherche, le sous-menu de chevron et le pied « Manage presets... » sont retirés. US-016 et US-017 sont annulées, et les critères de US-014 qui les portaient le sont avec elles.

**Extension du point d'entrée (2026-08-25, décision produit):** les boutons de split du header de pane ouvrent le même sélecteur à l'emplacement exact du pane cible, dans la moitié que le split va y carver, au lieu d'y déposer un shell nu. Les raccourcis clavier `Ctrl+Shift+D` et `Ctrl+Shift+E` restent directs.

#### US-014: Coquille de la palette et filtrage
**Description:** En tant que développeur, je veux une palette de recherche au clavier, afin de choisir quoi lancer sans quitter le clavier.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Le sélecteur est la *surface à créer*, pas un overlay: `New tab` et le `+` de la sidebar ouvrent un onglet « New pane » qui occupe tout l'espace d'un pane réel (même carte, même rayon, même fond que `Pane::render`), sans bordure ni pied de liste. Choisir un preset remplit ce même onglet.
- [x] Les boutons de split du header de pane (`PaneEvent::Split`) ouvrent le même sélecteur *à l'emplacement du pane cible*: il est injecté dans l'arbre de layout (`LayoutTree::render_with_preview`) et occupe la moitié que le split va carver dans ce pane précis, quelle que soit sa position; les autres panes ne bougent pas, et le preset choisi est lancé dans le nouveau pane via `split_with_target` avec son profil et sa commande. Les refus (zoom actif, plafond `MAX_PANES`) sont rendus avant l'ouverture ou dans le sélecteur, jamais derrière lui.
- [x] Le contenu est centré verticalement et horizontalement: le titre « New pane », puis une colonne de boutons simples. Aucun champ de recherche, aucun élément qui se plie ou se déplie.
- [x] Navigation clavier: flèches haut et bas pour la sélection, Entrée pour lancer, Échap pour fermer, sur le patron de `theme_picker.rs:145-156`.
- [x] Given la fermeture du sélecteur par Échap, when elle est reçue, then aucun onglet ni pane n'est créé, l'arbre de l'onglet est inchangé, et le focus retourne à l'élément qui l'avait avant l'ouverture (le pane cible pour un sélecteur de split).
- [x] Vérification visuelle manuelle: faite le 2026-08-25 (sélecteur plein pane, boutons centrés, split-bas sur le pane de droite rendu au bon emplacement).
- ~~Un matcher par sous-séquence insensible à la casse est écrit en repo, sans nouvelle dépendance, et classe les correspondances de préfixe avant les correspondances internes; il est couvert par des tests unitaires.~~ (annulé: le filtrage est retiré)
- ~~Given une requête sans correspondance, when elle est saisie, then la palette affiche un état vide explicite et Entrée ne lance rien.~~ (annulé: le filtrage est retiré)

#### US-015: Sources de presets unifiées
**Description:** En tant que développeur, je veux voir dans une seule liste le shell, mes agents et mes commandes personnalisées, afin de ne plus avoir à me souvenir où chaque chose est configurée.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [x] Un type `Preset` à trois variantes projette les sources existantes: le shell par défaut, `TerminalAgent::visible(config)` (`src-app/src/agent_launcher.rs:394`) et les `custom_buttons` du workspace (`crates/paneflow-config/src/schema/session.rs:239-249`); aucun nouveau champ n'est ajouté à `paneflow.json`.
- [x] Chaque bouton rend l'icône de sa source et son libellé; l'ordre est Terminal, puis les agents visibles dans l'ordre de `TerminalAgent::ALL`, puis les commandes personnalisées.
- [x] La commande lancée pour un agent provient de `TerminalAgent::launch_command(config)` (`:366`), donc le réglage de bypass Claude reste honoré sans être réimplémenté.
- [x] Given un agent visible mais non installé (`is_installed`, `:283`), when la liste est rendue, then il apparaît distinctement comme non installé et son lancement échoue avec un message lisible plutôt qu'un terminal vide.
- [x] Given un workspace sans commande personnalisée, when le sélecteur s'ouvre, then seules les deux premières sources sont listées, sans section vide.

#### US-016: Sous-menu de chevron: variantes et placement - ANNULÉE

**Statut:** CANCELLED (2026-08-25). Le sélecteur ne contient que des boutons simples: aucun chevron, aucun sous-menu, aucune variante. Le placement est porté par le point d'entrée (onglet ou bouton de split), pas par un sous-menu. Le code correspondant a été retiré.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-015

#### US-017: « Manage presets... » renvoie vers les Settings - ANNULÉE

**Statut:** CANCELLED (2026-08-25). Le pied de liste est retiré avec le reste du chrome; les Settings restent atteignables par leur entrée habituelle. La liste continue de refléter la visibilité des agents sans redémarrage, puisqu'elle est reconstruite à chaque rendu depuis `cached_config`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-015

---

### EP-006: Persistance et surfaces externes

Faire survivre les onglets au redémarrage sans perdre une seule surface existante, et rendre le nouveau niveau visible depuis l'IPC, le pont MCP et le clavier.

**Definition of Done:** un `session.json` v1 se migre sans perte, `surface.list` expose l'onglet, les raccourcis de niveau onglet existent et sont documentés.

#### US-018: Schéma de session v2 et migration depuis v1
**Description:** En tant que développeur, je veux que mes onglets survivent au redémarrage et que la mise à jour ne me coûte aucune surface, afin de pouvoir adopter la version sans risque.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001, US-004

**Acceptance Criteria:**
- [ ] `WorkspaceSession` (`crates/paneflow-config/src/schema/session.rs:190-216`) porte un tableau d'onglets, chacun avec un titre et un `LayoutNode`, et `SESSION_SCHEMA_VERSION` (`:5`) passe à 2.
- [ ] Une branche de lecture v1 explicite est ajoutée avant le rejet pour version non supportée (`src-app/src/app/session.rs:294-316`): un `session.json` v1 est migré, jamais remplacé par une session vide.
- [ ] La règle de migration est: l'arbre du workspace devient le premier onglet, chaque pane y étant réduit à sa surface focalisée, puis chaque surface restante de chaque pane devient un onglet supplémentaire à pane unique, dans l'ordre de parcours; le nom de l'onglet reprend le nom de la surface.
- [ ] Given un `session.json` v1 dont un pane porte les 64 surfaces autorisées (`MAX_PANE_SURFACES`, `crates/paneflow-config/src/schema/layout.rs:226`), when il est migré, then le plafond `MAX_TABS_PER_WORKSPACE` est appliqué, le surplus est journalisé au niveau `warn` avec son décompte, et aucune surface n'est perdue en silence.
- [ ] Given un `session.json` v2 lu par une version antérieure de Paneflow, when elle démarre, then elle emprunte le chemin de version non supportée existant, écrit sa sauvegarde de corruption et démarre propre plutôt que d'écraser silencieusement.
- [ ] Given un `session.json` v1 dont le champ d'onglets est absent et le layout est `None`, when il est migré, then le workspace obtient un onglet unique avec un pane par défaut.
- [ ] Une fixture v1 gelée est ajoutée aux tests de `crates/paneflow-config/src/loader_tests/session.rs` et vérifie que le nombre de surfaces avant et après migration est identique.

#### US-019: IPC et pont MCP conscients de l'onglet
**Description:** En tant qu'agent CLI, je veux savoir à quel onglet appartient une surface, afin de raisonner sur la composition de travail sans deviner.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] `surface.list` (`src-app/src/app/ipc_handler.rs:1589`) expose pour chaque surface l'identifiant et le titre de son onglet, en ajout et sans retirer ni renommer un champ existant.
- [ ] `surface.focus` active l'onglet propriétaire avant de donner le focus, et `surface.split` mute l'arbre de l'onglet propriétaire, y compris quand cet onglet n'est pas l'onglet actif.
- [ ] Le nommage des surfaces (`src-app/src/workspace/surface_naming.rs`) et la déduplication par cwd restent inchangés: l'onglet n'entre pas dans la dérivation du nom.
- [ ] Aucun index positionnel d'onglet n'est exposé par l'IPC ni par le pont MCP; l'adressage reste `surface_id` ou nom de surface.
- [ ] `crates/paneflow-mcp` reporte le champ d'onglet dans la sortie de `list_panes` et sa documentation (`docs/mcp-bridge.md`) est mise à jour.
- [ ] Given un client IPC antérieur qui ignore les nouveaux champs, when il appelle `surface.list`, then il continue de fonctionner sans erreur de désérialisation.

#### US-020: Raccourcis de niveau onglet
**Description:** En tant que développeur, je veux des raccourcis dédiés aux onglets, afin de naviguer sans souris et sans perdre mes réflexes existants.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] `NewTab` et `CloseTab` (`src-app/src/app/actions.rs:15-16`) portent désormais sur l'onglet de workspace, en gardant leurs bindings actuels `secondary-alt-t` et `secondary-w` (`src-app/src/keybindings/defaults.rs:154`, `:159`).
- [ ] Deux actions `NextTab` et `PreviousTab` sont ajoutées et liées à `secondary-]` et `secondary-[`, qui figurent parmi les slots libres du taken-set documenté (`src-app/src/keybindings/defaults.rs:98`).
- [ ] `secondary-tab` conserve son sens actuel de workspace suivant; aucun binding existant n'est réaffecté à un autre niveau.
- [ ] Les nouveaux bindings sont surchargeables par la clé `shortcuts` de `paneflow.json` comme les autres et apparaissent dans l'onglet Shortcuts des Settings via `keybindings::display`.
- [ ] Given un utilisateur ayant déjà surchargé `secondary-]` dans sa configuration, when l'application démarre, then sa surcharge gagne et aucun conflit n'est journalisé en erreur.
- [ ] La table de raccourcis de `CLAUDE.md` est mise à jour dans la même story.

## Functional Requirements

- FR-01: Un workspace doit posséder au moins un onglet à tout instant; aucune opération ne doit pouvoir le laisser à zéro.
- FR-02: Un pane doit posséder exactement un terminal; le produit ne doit plus offrir aucun moyen d'en empiler plusieurs.
- FR-03: Le système doit calculer l'état agent d'un onglet à partir des seules sessions dont le `surface_id` appartient à un terminal de cet onglet.
- FR-04: Le système doit attribuer à la ligne workspace, et non à un onglet, toute session dont le `surface_id` n'est pas résolu.
- FR-05: Une ligne workspace repliée doit rendre l'agrégat de ses onglets, de sorte que le repli ne cache aucun état.
- FR-06: La palette doit lancer un agent via `TerminalAgent::launch_command` et ne doit jamais reconstruire une ligne de commande d'agent en propre.
- FR-07: Le système ne doit exposer aucun index positionnel d'onglet dans l'IPC ni dans le pont MCP.
- FR-08: La lecture d'un `session.json` v1 doit produire une session migrée, jamais une session vide.
- FR-09: Le système ne doit pas réaffecter un raccourci existant à un niveau différent de la hiérarchie.
- FR-10: Le système ne doit émettre aucun événement de télémétrie pour les gestes introduits par ce PRD.

## Non-Functional Requirements

- **Performance de rendu:** le rendu d'une image de la sidebar affichant 20 workspaces dépliés totalisant 60 onglets doit tenir sous 4 ms sur la machine de référence; le basculement d'onglet doit produire sa première image en moins de 16 ms pour un onglet de 8 panes.
- **Coût par image:** le calcul du résumé agent d'un onglet doit être linéaire dans le nombre de sessions du workspace, sans allocation par image autre que le cluster d'icônes, mesuré par absence de régression sur les benchs existants de `paneflow-threads`.
- **Mémoire:** un onglet inactif ne doit pas coûter plus de 512 octets d'état propre hors terminaux; le RSS additionnel d'un workspace à 10 onglets à un pane doit rester sous 5 Mo par rapport à un workspace à 10 panes équivalent, mesuré par diff heaptrack selon `tasks/heaptrack-runbook.md`.
- **Migration:** 100 % des surfaces d'un `session.json` v1 doivent être présentes après migration, vérifié par égalité de décompte sur une fixture gelée; 0 workspace perdu.
- **Cross-platform:** chaque story doit compiler sur Linux et macOS et être vérifiée par inspection sur Windows quand la compilation hôte n'est pas possible, ce fait étant déclaré explicitement dans la PR.
- **Clavier:** 100 % des gestes d'onglet (créer, changer, renommer, fermer) doivent être atteignables sans souris.
- **Robustesse:** aucune panique sur les bornes; les refus de plafond (`MAX_PANES`, `MAX_TABS_PER_WORKSPACE`, `MAX_WORKSPACES`) doivent laisser l'état inchangé et être journalisés au niveau `warn`.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Dernier onglet fermé | L'utilisateur ferme le seul onglet d'un workspace | Le workspace conserve un onglet vide, il ne se ferme pas | Aucun |
| 2 | Onglet vide | Un onglet dont tous les panes sont fermés | Rendu de l'état vide du workspace actuel, invite à ouvrir la palette | « Ouvrir un pane » |
| 3 | Plafond d'onglets | Création au-delà de 32 onglets | Refus sans mutation, journalisé `warn` | « Limite de 32 onglets atteinte pour ce projet » |
| 4 | Plafond de panes | Split au-delà de 32 feuilles dans un onglet | Refus, arbre inchangé, message dans la palette si l'origine est la palette | Message de refus existant conservé |
| 5 | Session non attribuée | `AgentSession.surface_id` reste `None` | Badge rendu sur la ligne workspace, jamais sur un onglet arbitraire | Infobulle indiquant l'attribution au projet |
| 6 | Résolution tardive | `surface_id` résolu après la marche d'ancêtres | Le badge migre du dossier vers l'onglet au rendu suivant, sans double comptage | Aucun |
| 7 | Migration v1 volumineuse | Un pane v1 portant 64 surfaces | Migration jusqu'au plafond, surplus journalisé avec décompte, aucune perte silencieuse | Bandeau de session au démarrage |
| 8 | Downgrade applicatif | Une version antérieure lit un `session.json` v2 | Chemin de version non supportée existant, sauvegarde écrite, démarrage propre | Message de corruption existant |
| 9 | Palette sans correspondance | Requête ne filtrant aucun preset | État vide explicite, Entrée inerte | « Aucun preset » |
| 10 | Agent non installé | Preset d'agent dont le binaire est absent | Ligne marquée non installée, lancement refusé avec message lisible | « Binaire introuvable dans le PATH » |
| 11 | Dépôt d'onglet sur workspace saturé | Glisser un onglet vers un projet à 32 onglets | Refus, retour à la position d'origine, aucun terminal tué | Aucun, retour visuel du refus |
| 12 | Cluster d'icônes saturé | Onglet à plus de quatre panes | Cluster plafonné avec indication de dépassement, ligne non débordée | Infobulle listant les panes |
| 13 | Onglet zoomé quitté | Bascule d'onglet alors qu'un pane est zoomé | Le zoom appartient à l'onglet et est intact au retour | Aucun |
| 14 | Agent en cours à la fermeture | Fermeture d'un onglet portant un agent actif | Même comportement que la fermeture d'un pane actuel, sans processus orphelin | Confirmation existante conservée |
| 15 | Détection d'agent tardive | Icône inconnue au premier rendu | Icône générique puis remplacement au scan, sans reflow de ligne | Aucun |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Le refactor de `pane.rs` (112 occurrences internes, 93 externes réparties sur 19 fichiers) casse le terminal, cœur du produit | High | High | EP-001 insère le niveau `Tab` sans toucher au pane et reste livrable seul; EP-002 ne démarre qu'ensuite; `src-app/src/terminal/*` est interdit de modification; les tests de layout et de sérialisation servent de golden |
| 2 | Le bump de schéma de session détruit les sessions des utilisateurs, le chemin actuel de version non supportée retombant sur une session vide (`src-app/src/app/session.rs:294`) | Medium | High | US-018 impose une branche de lecture v1 explicite avant le rejet, une fixture v1 gelée et une égalité de décompte de surfaces |
| 3 | 20 stories et un blast radius XL: dérive de périmètre et epic bloquant | Medium | Medium | Les six épics sont ordonnés pour être livrables séparément; EP-005 ne dépend que d'EP-001 et peut être livré avant EP-002 si nécessaire |
| 4 | Coexistence temporaire de deux niveaux d'onglets pendant EP-001 et EP-002 | High | Low | EP-001 n'a aucune conséquence visible: la sidebar ne change qu'en EP-003, donc l'utilisateur ne voit jamais les deux niveaux à la fois |
| 5 | La perte du geste de déplacement de tab entre panes est vécue comme une régression | Low | Medium | Le geste est déjà défectueux (index positionnel obsolète) et le split couvre le besoin; le retrait est explicite dans les non-goals et dans les notes de version |
| 6 | La migration v1 produit une liste d'onglets bruyante sur les workspaces à panes multi-surfaces | Medium | Low | Règle de migration déterministe et documentée, noms d'onglets repris des noms de surface, plafond appliqué avec journalisation |
| 7 | Collision avec la promotion Windows de libghostty en cours | Medium | Medium | Interdiction de modifier `src-app/src/terminal/*`; toute story touchant au PTY est hors périmètre |

## Non-Goals

- **Pas de barre d'onglets horizontale** au-dessus du layout: la sidebar est le seul emplacement, comme les terminal tabs de VS Code. Réexaminable si l'usage montre que la sidebar est trop souvent repliée.
- **Pas de nouveau tableau `presets` dans `paneflow.json`** dans cette version: la palette est une vue en lecture sur trois sources existantes. Un catalogue unifié éditable est un candidat pour la suite, une fois A4 validée.
- **Pas de variantes de preset ni de sous-menu:** le sélecteur n'expose qu'un bouton par preset (décision du 2026-08-25); le placement est porté par le point d'entrée, onglet ou bouton de split, pas par un choix dans la liste.
- **Pas de nouvelle méthode IPC** de type `tab.list` ou `tab.create`: seuls des champs additifs sont ajoutés aux méthodes de surface existantes.
- **Pas de dépendance de fuzzy matching** (`nucleo`, `fzf` et équivalents): un matcher par sous-séquence en repo suffit au catalogue attendu.
- **Aucune modification du mode Agents** ni de `src-app/src/app/agents_sidebar/`: ce PRD emprunte ses patrons de rendu sans les modifier, pour ne pas entrer en collision avec `tasks/prd-agents-chat-transcript-2026-Q3.md`.
- **Le geste de déplacement de tab entre panes n'est pas remplacé:** il est retiré, et le split le couvre.
- **Aucun événement de télémétrie** n'est ajouté; les métriques de succès se mesurent localement.

## Files NOT to Modify

- `src-app/src/terminal/*` - émulation VT et sessions PTY; la promotion Windows de libghostty est en cours et toute collision y serait coûteuse.
- `src-app/src/layout/tree.rs` - la mécanique d'arbre est réutilisée telle quelle par l'onglet; seuls ses appelants changent.
- `src-app/src/workspace/surface_naming.rs` - la dérivation et la déduplication des noms de surface sont indépendantes du niveau onglet.
- `src-app/src/agent_launcher.rs` - le cœur de `TerminalAgent` est consommé, pas modifié; la palette n'y ajoute rien.
- `src-app/src/keys.rs`, `src-app/src/mouse.rs` - traduction d'entrée bas niveau.
- `src-app/src/theme/*`, `src-app/src/update/*` - hors périmètre.
- `src-app/src/ipc.rs` couche de transport - seules les charges utiles des méthodes de surface changent, pas le transport.
- `src-app/src/widgets/*` - réutilisés par la palette, pas modifiés.

## Technical Considerations

- **Modèle:** `Tab` est-il une struct simple possédée par `Workspace`, ou une `Entity<Tab>` GPUI? Recommandation: une struct simple, parce que rien dans l'onglet n'a besoin d'être observé indépendamment et que le workspace est déjà un état plat mutable. À confirmer si le rendu de la sidebar exige des abonnements par onglet.
- **Identité d'onglet:** identifiant monotone par processus sur le modèle de `next_workspace_id` (`src-app/src/workspace/mod.rs:36-41`), ou UUID persisté? Recommandation: identifiant monotone en mémoire plus titre persisté, puisque l'IPC n'expose aucun index et que `surface_id` reste l'unité d'adressage.
- **Ordre de livraison:** EP-001 puis EP-002 est l'ordre sûr, mais EP-005 ne dépend que d'EP-001. Faut-il livrer la palette tôt pour obtenir un retour d'usage sur A4 avant d'engager le refactor XL de `pane.rs`? Arbitrage à faire à la fin d'EP-001.
- **Sérialisation:** le tableau d'onglets remplace-t-il `layout` dans `WorkspaceSession`, ou coexiste-t-il avec lui en lecture seule? Recommandation: remplacement avec bump de version, parce qu'une coexistence silencieuse ferait démarrer une version antérieure sur une session apparemment valide mais vide.
- **Virtualisation:** faut-il rendre la liste d'onglets avec `uniform_list`? Recommandation: non dans cette version, sous réserve d'A2; noter que `uniform_list` n'applique aucun clip par ligne, ce qui impose `whitespace_nowrap` et `overflow_hidden` si l'on s'y résout.
- **Résumé agent:** le calcul par onglet doit-il être mémoïsé par génération de scan, comme l'ordre de sidebar l'est déjà par `SidebarOrderCache`? Recommandation: mesurer d'abord; le calcul est linéaire dans le nombre de sessions, qui est petit.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Niveaux d'onglets dans le produit | 2 (workspace absent, pane présent) | 1 | Month-1 | `grep` sur les API de tab de pane dans `src-app/`, doit être vide hors niveau workspace |
| Occurrences d'API de tab de pane | 205 dans 20 fichiers | 0 | Month-1 | Même `grep`, décompte avant et après |
| Gestes pour lancer un agent dans un nouveau fil | 3 (créer un workspace ou un tab, cliquer dans le pane, taper la commande) | 2 (ouvrir la palette, choisir le preset) | Month-1 | Comptage manuel du parcours sur la machine de référence |
| Sessions agent attribuées à leur onglet | 0 % (agrégat workspace uniquement) | 100 % des sessions à `surface_id` résolu | Month-1 | Test unitaire de filtrage plus vérification visuelle avec trois agents actifs |
| Surfaces perdues à la migration v1 vers v2 | N/A (nouveau) | 0 | Month-1 | Fixture v1 gelée dans `crates/paneflow-config/src/loader_tests/session.rs`, égalité de décompte |
| Workspaces redondants sur un même dépôt | Mesure à relever sur le `session.json` de référence avant EP-003 | Réduction d'au moins la moitié | Month-6 | Comptage manuel des workspaces partageant un même `cwd` dans `session.json` |

## Open Questions

- `Ctrl+Tab` doit-il rester « workspace suivant » ou devenir « onglet suivant »? Ce PRD tranche pour le statu quo afin de ne réaffecter aucun binding existant, mais l'onglet devient le niveau de bascule le plus fréquent. Arthur, avant US-020; la surcharge par `shortcuts` reste possible sans code.
- La migration v1 doit-elle promouvoir les surfaces non actives en onglets, ou les abandonner avec un rapport? Ce PRD tranche pour la promotion sans perte. Arthur, avant US-018; A3 est non validée.
- Faut-il livrer EP-005 avant EP-002 pour obtenir un retour d'usage sur la palette avant le refactor XL? Arthur, à la fin d'EP-001.
- Le cluster d'icônes doit-il être plafonné à quatre panes comme la référence visuelle le suggère, ou s'adapter à la largeur de la sidebar? Arthur, avant US-013. Tranché pendant US-013: plafond fixe à quatre icônes, le dépassement étant rendu par un suffixe `+N`, conformément au critère US-013 AC3 qui suppose déjà un plafond. Une largeur adaptative aurait fait dépendre le cluster de la largeur de la sidebar, donc rendu la ligne réflowable au moment où le scan d'agent aboutit, ce que US-013 AC4 interdit.
[/PRD]
