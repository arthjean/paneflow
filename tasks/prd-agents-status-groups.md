[PRD]
# PRD: Section Agents groupée par statut

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-03 | Arthur Jean | PRD initial, issu de la revue du 2026-11-02 : section Agents groupée par statut dans la DA Paneflow, statuts de l'arbre masquables par `sidebar_show.status`, retrait de `sidebar_show.agent_status`, une seule projection d'état pour l'arbre et la section. 4 epics, 11 stories. |
| 1.1 | 2026-11-11 | Arthur Jean | US-005 : le libellé d'en-tête de groupe commence à 15 px du bord, et non 14. La maquette utilise un padding de ligne de 6 px, l'app un padding de 7 px (`SIDEBAR_ROW_PADDING_X`) ; l'en-tête s'aligne sur le contenu des lignes. Décision d'Arthur à la revue d'EP-002. |
| 1.2 | 2026-11-11 | Arthur Jean | Ajout d'EP-005 (US-012 à US-014), epic d'exploration : montrer qu'un agent qui attend ses sous-agents n'est pas au repos, en restant sur OSC 7501. Aucun code produit. 5 epics, 14 stories. |

## Problem Statement

**Convention de référence.** Les chemins désignent Paneflow au commit `aa82df043`. La maquette validée est la section « Direction retenue » de l'artifact https://claude.ai/artifact/KYUjBTzv2LfPeLbd2Rupf2 (version 4, privé). Le protocole d'état est OSC 7501 révision 0.4 ([spec](https://www.superlogical.com/rex/docs/build/program-status)).

1. **La section Agents répète l'arbre.** Des utilisateurs signalent le doublon. Les lignes Agents appellent les mêmes fonctions de rendu que les lignes de l'arbre (`render_lane` et `render_status_badge`, `src-app/src/app/sidebar/lane.rs:76-204`, appelées depuis `src-app/src/app/sidebar/agents_section.rs:495-520`), et DESIGN.md:573 prescrit « the trailing status from the tree rows ». Dans le scénario de référence de six agents de la maquette, l'arbre affiche cinq statuts et la section les répète tous les cinq ; un agent au travail fait miroiter deux titres à la fois.
2. **La liste se lit badge par badge.** Chaque agent occupe une ligne de 55 px avec une ligne méta (`agents_section.rs:31-33`, `agents_section.rs:613-639`), et l'état n'est porté que par le badge en fin de ligne. Sous le plafond de 330 px, quatre agents répartis sur deux rangs tiennent sans défilement.
3. **L'arbre et la section ne calculent pas l'état de la même façon.** L'arbre agrège avec `sidebar_agent_summary` (`src-app/src/app/sidebar/meta.rs:43-85`, quatre états), la section classe avec `AgentRank` (`src-app/src/app/sidebar/agents.rs:9-33`, six rangs). Deux écarts sont visibles : un agent sans OSC 7501 affiche « No status » dans la section et rien dans l'arbre ; un agent reparti au travail alors que son résultat précédent n'a pas été vu affiche « Done » dans l'arbre et la matrice dans la section. La matrice de l'arbre reste `muted` (`tint: None` en dur, `meta.rs:67-83`) alors que DESIGN.md:541-542 demande l'accent du runtime, que la section applique déjà (`agents_section.rs:495-500`).

**Why now:** la section Agents est sortie en 0.19.0 le 2026-11-01 et les premiers retours portent sur ce doublon. Le réglage `sidebar_show.agent_status`, que ce PRD retire, n'a jamais été publié : il n'apparaît que dans la section Unreleased du changelog (CHANGELOG.md:6-15). Le retirer maintenant ne touche aucune configuration d'utilisateur publiée.

## Overview

La section Agents garde sa place (entre l'arbre des workspaces et le footer), son périmètre (les agents de l'espace actif), son tri (`order_agents`, `agents.rs:58-78`) et ses interactions, mais présente ses agents en groupes par statut, dans l'ordre des rangs existants : Need you, Error, Done, Working, Idle, No status. Chaque groupe non vide commence par un en-tête de 24 px : son libellé en 12 px Medium suivi du nombre d'agents en `muted`. L'en-tête prend la couleur du statut pour les trois groupes qui attendent l'utilisateur (ambre, `agent_error`, bleu clair) et reste `muted` pour Working, Idle et No status. Chaque agent tient sur une ligne de 32 px : logo du runtime, titre de l'onglet suivi de « · » et du nom du workspace en `muted`, puis son statut actuel (badge Need you, Error, Done ou No status, matrice en comète dans la teinte du runtime avec le shimmer du titre pour Working, rien pour Idle). La ligne méta et l'écart de 10 px entre rangs disparaissent. Il n'y a ni durée ni point à gauche : la DA reste celle de Paneflow. La présentation vient d'une capture de Herdr fournie par Arthur ; elle ne figure pas dans le code Herdr consulté, ce PRD reprend un design, pas du code.

L'arbre garde ses statuts par défaut. Un nouvel item « Status » dans Customize sidebar › Show, placé à côté de PR (`sidebar_show.status`, vrai par défaut), les retire des lignes de workspace et d'onglet : badge, matrice, shimmer du titre et infobulle de statut disparaissent, et la PR reprend la place selon la règle existante (`lane.rs:20-30`). Les lignes Agents affichent toujours leur statut, puisque masquer le statut d'une section groupée par statut reviendrait à masquer la section. Le réglage « Status » de Customize agent rows (`sidebar_show.agent_status`) est donc retiré ; il ne reste que Logo et Workspace. Une configuration qui le définit encore se charge comme s'il était absent.

Dans le même lot, l'arbre calcule l'état de chaque session avec `AgentRank::of`, la fonction de la section et de ⌘⇧J, teinte sa matrice à l'accent du runtime, et le code mort `ToolAggregate.dominant`, `ToolAggregate.active_tool_name` et `state_rank` (`src-app/src/ai_types.rs:69-175`) disparaît. Aucune donnée nouvelle ne traverse l'hôte, le worker ou l'IPC.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Supprimer le doublon pour qui le souhaite | 0 statut d'agent répété dans la sidebar avec `sidebar_show.status` éteint, scénario de référence de 6 agents | Inchangé |
| Densifier la section | Au moins 6 agents répartis en 2 groupes visibles sans défilement sous 330 px, contre 4 aujourd'hui | Inchangé |
| Une seule projection d'état | 0 combinaison d'état et de non-lu pour laquelle l'arbre et la section divergent, vérifié par test | Inchangé |
| Tenir le coût de rendu | `render_sidebar` sous 16 ms (pire construction et pire frame complète) avec 50 agents sur 30 workspaces, build release | Inchangé |

## Target Users

### Développeur qui fait tourner plusieurs agents en parallèle
- **Role:** Arthur et les utilisateurs de Paneflow qui lancent 3 à 10 agents (Claude Code, Codex, OpenCode, Pi) dans plusieurs workspaces.
- **Behaviors:** répartit les tâches par workspace, revient vers un agent quand il attend une permission ou a fini, garde la sidebar ouverte pendant la journée.
- **Pain points:** voit le même statut deux fois (onglet et section), lit l'état ligne par ligne, perd de la hauteur avec des lignes de 55 px.
- **Current workaround:** masquer la section Agents (`sidebar_show.agents`) et perdre le tri par urgence, ou supporter le doublon.
- **Success looks like:** les en-têtes disent combien d'agents attendent, échouent, ont fini ou travaillent ; un réglage retire les statuts de l'arbre.

### Utilisateur venant de Herdr
- **Role:** développeur habitué au panneau d'agents d'un multiplexeur TUI.
- **Behaviors:** suit ses agents par état et passe de l'un à l'autre au clavier.
- **Pain points:** la section actuelle ne regroupe pas les agents ; l'état se lit badge par badge.
- **Current workaround:** ⌘⇧J et les notifications système.
- **Success looks like:** retrouver des groupes par état, sans quitter la DA de Paneflow.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Herdr** (dépôt local à jour avec origin/master du 2026-10-29) : deux sections empilées, « spaces » avec un glyphe d'état agrégé par workspace et « agents » en liste à plat triée `grouped` (par workspace) ou `priority` (`src/config/model.rs:97-102`, `src/client/shell/agent_sidebar.rs:37-45`), avec des points de couleur statiques. La présentation par groupes de la capture d'Arthur n'apparaît ni dans ce code ni dans sa documentation non publiée.
- **cmux** : anneau bleu sur le pane en attente, onglet allumé, panneau de notifications et saut à la plus récente ([manaflow-ai/cmux](https://github.com/manaflow-ai/cmux)).
- **Superset** : indicateur de travail dans la sidebar, indicateur orange sur les workspaces qui attendent, carillon en fin de tour ([README](https://raw.githubusercontent.com/superset-sh/superset/HEAD/README.md)).
- **VS Code, sessions d'agents** : une pastille « N sessions require input » dans la barre de titre quand la sidebar est masquée ([vscode #324203](https://github.com/microsoft/vscode/issues/324203)).
- **Market gap:** aucun des produits consultés ne groupe les agents par statut sous des en-têtes avec compteur, dans le code ou la documentation vérifiés.

### Best Practices Applied
- Un état ne repose pas sur la couleur seule (WCAG 2.2, critère 1.4.1) : l'en-tête porte le mot, les badges aussi, et la matrice est une forme distincte.
- Une animation en boucle doit pouvoir s'arrêter (WCAG 2.2, critère 2.2.2) : matrice et shimmer restent immobiles sous `reduce_motion`, comme aujourd'hui.
- `done` signifie « résultat pas encore vu » et c'est au terminal de l'acquitter (spec OSC 7501 r0.4, section lifetime) : le groupe Done suit l'acquittement existant (`acknowledge_visible_completions`).
- Les champs déclarés sont non fiables (spec r0.4, section security) : aucun texte déclaré n'entre dans un en-tête ou une ligne, et l'identité affichée vient du runtime détecté.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- Le rôle d'accessibilité `Heading` existe dans la révision de GPUI épinglée et s'expose sur Linux, macOS et Windows. Aucun usage n'existe dans le code ; `Role::Group` est déjà utilisé (`src-app/src/app/usage/page.rs:427`) et sert de repli. Validé dans US-006.
- Le tri par récence à l'intérieur d'un groupe convient : la capture Herdr trie les agents au repos du plus récent au plus ancien, et ⌘⇧J dépend de cet ordre (`agents.rs:58-66`).
- Un titre d'onglet suivi de « · workspace » tient dans les 270 px utiles d'une ligne (`src-app/src/app/sidebar/style.rs:21-22`) pour les titres courants ; au-delà, l'ellipse coupe d'abord le suffixe et l'infobulle existante donne le texte complet.
- Laisser les statuts de l'arbre activés par défaut suffit : les utilisateurs gênés par le doublon trouvent l'item « Status » dans Customize sidebar › Show. Non vérifié, suivi par les retours du premier mois.

### Hard Constraints
- OSC 7501 reste la seule source d'état de tour (`ARCHITECTURE.md:363-366`) : pas de lecture d'écran, pas de hook, pas d'heuristique.
- Aucune donnée nouvelle ne traverse l'hôte, le worker ou l'IPC : pas de durée, pas d'horodatage de changement d'état.
- Aucune I/O synchrone ni sous-processus sur le thread de rendu (`AGENTS.md`, section Boundaries).
- Aucun commentaire dans le code source. Tout nouvel item se déclare avant le `mod tests` du fichier (piège `clippy::items_after_test_module`, `AGENTS.md`).
- La DA suit DESIGN.md : primitives existantes (`squircle_skin`, `ROW_RADIUS` = 9 de la sidebar, `sidebar_row_shell`), couleurs de statut fixes dans tous les presets (DESIGN.md:277-278), aucune nouvelle taille de texte ni nouveau rayon (DESIGN.md:113-114).
- Linux, macOS et Windows sont des cibles ; ce PRD n'ajoute aucun code derrière un `cfg` de plateforme.
- La sidebar est une vue en cache (`src-app/src/app/sidebar/view.rs:28`) : l'état de la section vit sur `PaneFlowApp`.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration

Gates additionnels :
- Stories qui touchent le rendu de la section Agents (US-004, US-005, US-006) : `cargo test -p paneflow-app --release --locked fifty_agents_across_thirty_workspaces_fit_the_sidebar_frame_budget` - budget de 16 ms, test ignoré en debug (`agents_section.rs:2409-2414`).
- Stories qui touchent la configuration (US-007, US-008) : `cargo test -p paneflow-config --locked` - schéma et chargement.
- Stories d'UX/UI (US-002, US-005, US-006, US-009, US-010) : revue par le skill `/better-interface` sur la surface modifiée, menée en appliquant les skills `better-*` à DESIGN.md et à la maquette validée. La story ne passe en IN_REVIEW qu'avec un verdict `Approve`, sans finding `HIGH` restant. Les vérifications qui exigent l'app rendue sont marquées `Not verified` dans le rapport.
- Stories d'interface (EP-002, EP-003) : passe visuelle manuelle par Arthur sur macOS et Windows, comparée à la maquette validée. L'agent d'implémentation ne lance pas l'application et ne fait pas de capture.
- Stories d'exploration (EP-005) : elles ne modifient aucun code produit, donc les trois commandes ci-dessus ne s'appliquent pas ; chaque fait consigné cite sa source (capture datée, fichier et ligne, ou section de la spec).

## Epics & User Stories

### EP-001: Une seule projection d'état

L'arbre, la section Agents et ⌘⇧J lisent l'état d'un agent avec la même fonction, et la matrice de l'arbre prend l'accent du runtime. Aucun changement de mise en page.

**Definition of Done:** `sidebar_agent_summary` calcule l'état de chaque session avec `AgentRank::of` ; pour chaque combinaison d'état et de non-lu, l'arbre et la section donnent le même état, prouvé par test ; la matrice de l'arbre a l'accent du runtime quand la ligne n'a qu'un runtime au travail ; `ToolAggregate.dominant`, `ToolAggregate.active_tool_name` et `state_rank` n'existent plus.

#### US-001: Le résumé de l'arbre dérive de AgentRank
**Description:** As a développeur multi-agents, I want que l'onglet et la section Agents montrent le même état pour un même agent so that je n'aie jamais deux réponses à « que fait cet agent ? ».

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given les sessions d'une ligne d'onglet ou de workspace, when le résumé est calculé, then chaque session reçoit son rang par `AgentRank::of(session, unread)` avec le non-lu de sa surface, et le résumé prend le rang le plus urgent parmi Blocked, Errored, Done et Working, avec le nombre de sessions de ce rang.
- [x] Given une session de rang Idle ou NoStatus, when le résumé est calculé, then elle ne produit aucun statut, comme aujourd'hui.
- [x] Given une surface dont le résultat n'a pas été vu et dont l'agent est reparti au travail, when l'arbre et la section sont rendus, then les deux montrent Working (aujourd'hui l'arbre montre Done).
- [x] Given un résultat non lu sans session vivante sur la ligne, ou non attribué à une surface (`src-app/src/workspace/mod.rs:39-86`), when le résumé est calculé, then il compte comme Done, comme le garantit aujourd'hui le test `sidebar_agent_summary_surfaces_unread_completion_without_live_session` (`meta.rs:211`).
- [x] Given un workspace déplié, when son résumé est calculé, then il ne compte que les sessions sans surface et les non-lus non attribués (`meta.rs:87-98`, `src-app/src/app/sidebar/workspace_row.rs:77-81`), comme aujourd'hui.
- [x] Given aucune session et aucun non-lu sur la ligne, when le résumé est calculé, then il vaut `None` et la PR peut prendre la place.
- [x] Un test croise, pour chaque combinaison d'état (`Thinking`, `WaitingForInput`, `Finished`, `Errored`), de non-lu et de capacité OSC 7501, l'état de la ligne de l'arbre et celui de la ligne de la section, et les trouve égaux là où les deux en affichent un.

#### US-002: La matrice de l'arbre prend l'accent du runtime
**Description:** As a développeur multi-agents, I want que la matrice d'un onglet au travail ait la couleur de son runtime so that l'arbre et la section Agents peignent le même agent de la même couleur.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given une ligne dont toutes les sessions Working ont le même runtime, when la matrice est rendue, then elle prend `TerminalAgent::accent()` de ce runtime (`src-app/src/agent_launcher.rs:91-93`).
- [x] Given un runtime qui ne déclare pas d'accent, when sa matrice est rendue, then elle reste `muted`.
- [x] Given une ligne avec des sessions Working de runtimes différents, when la matrice est rendue, then elle reste `muted`.
- [x] Given `reduce_motion` activé, when la matrice est rendue, then elle reste immobile, comme aujourd'hui (`lane.rs:327-389`).
- [x] Les tests de `meta.rs` et `lane.rs` qui construisent `tint: None` (`meta.rs:165-303`, `lane.rs:430`, `lane.rs:453`) sont mis à jour pour ce comportement.

#### US-003: Suppression du code mort de l'agrégat par outil
**Description:** As a mainteneur, I want retirer les champs de l'agrégat par outil que rien n'affiche so that il ne reste qu'un ordre de priorité dans le code.

**Priority:** P2
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `ToolAggregate.dominant`, `ToolAggregate.active_tool_name` et `state_rank` (`ai_types.rs:69-175`) sont supprimés, ainsi que les assertions sur `dominant` (`ai_types.rs:218`, `ai_types.rs:228`, `ai_types.rs:239`).
- [x] L'infobulle des lignes de l'arbre garde le nom des outils et le suffixe « +N » (`meta.rs:113-150`), vérifié par les tests existants.
- [x] Given deux sessions du même outil dans des états différents, when l'infobulle est construite, then elle liste l'outil une fois avec « +1 », comme aujourd'hui.
- [x] Given une ligne sans session hookée mais avec un agent détecté, when l'infobulle est construite, then elle liste l'agent détecté avec « running », comme aujourd'hui (`meta.rs:132-137`).

---

### EP-002: La section Agents groupée par statut

La section présente ses agents en groupes par statut, sur des lignes d'une ligne, et ses lignes affichent toujours leur statut.

**Definition of Done:** la section rend un en-tête par groupe non vide et une ligne de 32 px par agent ; le curseur clavier et le défilement suivent les lignes ; les en-têtes sont nommés pour les technologies d'assistance ; `sidebar_show.agent_status` n'existe plus ; le budget de 16 ms tient en release.

#### US-004: Le modèle de groupes et la hauteur de la section
**Description:** As a développeur multi-agents, I want que la section calcule ses groupes et sa hauteur à partir des lignes triées so that le rendu, le clavier et le dimensionnement partagent une seule structure.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given les lignes triées par `order_agents`, when les groupes sont construits, then chaque groupe réunit les lignes d'un même statut affiché, dans l'ordre Need you, Error, Done, Working, Idle, No status, sans changer l'ordre relatif des lignes.
- [x] Given une session sans capacité OSC 7501 dans son délai de grâce de 3 s (`agents_section.rs:280-288`), when les groupes sont construits, then elle est dans Idle, et passe dans No status quand le délai expire, au réveil déjà programmé (`agents_section.rs:1132-1151`).
- [x] Given un statut qu'aucun agent n'a, when les groupes sont construits, then aucun groupe ni en-tête n'est produit pour lui.
- [x] Given aucun agent, when la section est construite, then aucun groupe n'existe et la ligne « No agents running » reste seule (`agents_section.rs:834-847`).
- [x] La hauteur du contenu vaut, pour chaque groupe, 24 px d'en-tête puis 32 px par ligne avec 2 px entre deux lignes, plus 8 px avant chaque en-tête sauf le premier ; `AGENT_ROW_HEIGHT`, `AGENT_RANK_SPACING` et le champ `starts_rank` (`agents_section.rs:31-34`, `agents_section.rs:240-251`) disparaissent.
- [x] Given 6 agents répartis en 2 groupes, when la hauteur automatique est calculée, then la section les montre tous sans défilement et reste sous 330 px.
- [x] Le plafond de 330 px, la poignée et la hauteur persistée restent ; le plancher de redimensionnement (`AGENTS_RESIZE_FLOOR`, `agents_section.rs:22-23`) devient l'en-tête de section, un en-tête de groupe et une ligne.
- [x] Les tests de hauteur existants (`the_section_takes_its_content_height_plus_the_header_and_a_bottom_margin`, `hiding_the_workspace_line_folds_each_row_to_one_line`, `the_section_height_is_capped_and_a_collapsed_section_keeps_only_its_header`, `a_chosen_height_is_bounded_by_one_row_620_px_and_the_content`, `a_long_list_caps_at_330_px`) sont réécrits sur ces valeurs.

#### US-005: En-têtes de groupe et lignes d'une ligne
**Description:** As a développeur multi-agents, I want voir un en-tête coloré par statut et une ligne courte par agent so that je sache d'un regard combien d'agents attendent, échouent, ont fini ou travaillent.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [x] Given un groupe, when il est rendu, then son en-tête de 24 px affiche le libellé (« Need you », « Error », « Done », « Working », « Idle » ou « No status ») en 12 px Medium, puis, 6 px après, le nombre d'agents en `muted` à chiffres tabulaires, le texte commençant à 15 px du bord de la sidebar comme celui des lignes (marge 8, padding 7).
- [x] L'en-tête prend la couleur needs-input pour Need you, `agent_error` pour Error et finished pour Done (`src-app/src/app/constants.rs:233-249`), et `muted` pour Working, Idle et No status, dans tous les presets et dans les deux variantes de thème.
- [x] Given un agent, when sa ligne est rendue, then elle fait 32 px sur le shell des lignes d'onglet (marge 8, padding 7 par 6, rayon 9), avec le logo du runtime à 15 px dans l'emplacement de 20 px, le titre en 14 px Medium suivi de « · » et du nom du workspace en 14 px `muted`, puis le statut en fin de ligne.
- [x] Le statut d'une ligne est le badge « Need you », « Error » ou « Done » du rendu actuel, la matrice dans la teinte du runtime avec le shimmer du titre pour Working, le badge `muted` « No status » avec son infobulle actuelle, et rien pour Idle.
- [x] Given un titre et un nom de workspace plus longs que la ligne, when elle est rendue, then le texte s'ellipse en fin sans passer à la ligne, le suffixe se coupe en premier, et l'infobulle « title · workspace » existante apparaît (`agents_section.rs:640-643`).
- [x] Given `sidebar_show.agent_logo` ou `sidebar_show.agent_workspace` éteint, when une ligne est rendue, then le logo ou le suffixe disparaît et la ligne garde 32 px.
- [x] Given `reduce_motion` activé, when une ligne Working est rendue, then la matrice et le shimmer restent immobiles.
- [x] La ligne méta (`agents_section.rs:613-639`) et les espaceurs de rang (`agents_section.rs:848-857`) n'existent plus ; la ligne de l'onglet actif garde la teinte active et les autres la teinte de survol.
- [x] Aucun champ déclaré par OSC 7501 (`title`, `message`, `app`) n'apparaît dans un en-tête ou une ligne.

#### US-006: Clavier, défilement et accessibilité des groupes
**Description:** As a développeur qui navigue au clavier ou avec un lecteur d'écran, I want parcourir les agents groupés comme avant so that les en-têtes structurent la liste sans gêner la navigation.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Given le curseur de la sidebar dans la section, when l'utilisateur appuie sur haut, bas, Home ou End, then il passe d'agent en agent et ne s'arrête jamais sur un en-tête (`agents_nav_targets`, `agents_section.rs:408-413`).
- [x] Given un curseur posé sur un agent hors de la zone visible, when il y arrive, then la liste défile jusqu'à sa ligne en comptant les en-têtes ; `agent_row_child_index` (`agents_section.rs:404-406`), qui suppose aujourd'hui un espaceur avant chaque ligne, est remplacé.
- [x] Given un agent qui change de groupe pendant qu'il porte le curseur, when la section est rendue à nouveau, then le curseur reste sur ce même agent.
- [x] Given un lecteur d'écran, when il parcourt la section, then chaque en-tête est annoncé avec son libellé et son nombre (par exemple « Need you, 2 agents »), avec le rôle `Heading`, ou `Group` si la révision de GPUI épinglée n'expose pas `Heading` ; le choix et sa raison figurent dans le rapport de la story.
- [x] Chaque ligne reste un `ListItem` nommé par `agent_row_label` (`agents_section.rs:394-402`), et la liste reste un `List` nommé « Agents » (`agents_section.rs:824-825`).
- [x] Given Entrée sur un agent, when elle est pressée, then le pane de l'agent prend le focus comme aujourd'hui (`src-app/src/app/sidebar/keyboard.rs:278-285`).
- [x] Les tests `the_cursor_row_wears_the_sidebar_cursor_ring_and_is_scrolled_into_view` et `a_row_is_a_list_item_named_by_agent_title_workspace_and_state` couvrent une liste d'au moins deux groupes.

#### US-007: Retrait du réglage Status des lignes Agents
**Description:** As a utilisateur de la section Agents, I want que les lignes affichent toujours leur statut so that une section groupée par statut ne puisse pas perdre ce qui l'organise.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Le champ `agent_status`, son accesseur (`crates/paneflow-config/src/schema/config.rs:123`, `config.rs:237-239`), son entrée de schéma JSON (`schemas/paneflow.schema.json:152-156`) et ses références de test (`crates/paneflow-config/src/schema.rs:77`, `schema.rs:644`, `schema.rs:653`, `schema.rs:657`) sont retirés.
- [x] Customize agent rows ne propose plus que Logo et Workspace ; l'item Status (`agents_section.rs:1256-1263`) et ses branches dans `src-app/src/app/sidebar/customize_menu.rs` (`:53-55`, `:76`, `:90`, `:98`, `:331`, tests `:405` et `:446`) disparaissent, et `AgentRowDetails` perd son champ `status` (`agents_section.rs:226`).
- [x] Given une configuration `{"sidebar_show": {"agent_status": false}}`, when elle est chargée, then le résultat est identique à la même configuration sans la clé, vérifié en étendant `test_retired_keys_load_as_if_absent` (`crates/paneflow-config/src/loader_tests/core_config.rs:203-231`).
- [x] Given cette ancienne valeur `false`, when la section est rendue, then les lignes affichent quand même leur statut.
- [x] Given un clic sur Logo ou Workspace dans Customize agent rows, when le menu enregistre le réglage, then `paneflow.json` ne contient pas `agent_status`.

---

### EP-003: Statuts de l'arbre masquables

L'utilisateur choisit si les lignes de workspace et d'onglet affichent le statut de leurs agents.

**Definition of Done:** `sidebar_show.status` existe, vaut vrai par défaut, se règle depuis Customize sidebar › Show et la palette, et retire badge, matrice, shimmer et infobulle de statut des lignes de workspace et d'onglet quand il est éteint, sans toucher aux autres signaux d'attention.

#### US-008: La clé sidebar_show.status
**Description:** As a utilisateur, I want une clé de configuration pour les statuts de l'arbre so that mon choix persiste comme les autres détails de la sidebar.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `SidebarShow` gagne `status: Option<bool>` avec `deserialize_with = "lenient_value_or_default"`, et `status_enabled()` vaut vrai par défaut, sur le modèle de `agents_enabled` (`config.rs:225-227`), déclarés avant tout `mod tests`.
- [x] Le schéma JSON déclare `status` en `["boolean","null"]`, défaut `true`, avec une description qui nomme ce que la clé masque dans l'arbre.
- [x] Given une valeur invalide (`"status": "yes"`), when la configuration est chargée, then la clé prend son défaut sans erreur, comme les autres clés de `sidebar_show` (`schema.rs:634-669`).
- [x] La fixture complète du schéma (`schema.rs:69-79`) et les tests d'aller-retour (`crates/paneflow-config/src/loader_tests/core_roundtrip.rs`) couvrent la nouvelle clé.

#### US-009: L'arbre masque ses statuts quand Status est éteint
**Description:** As a développeur multi-agents, I want éteindre les statuts de l'arbre so that le statut d'un agent ne s'affiche qu'une fois, dans la section Agents.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001, US-008

**Acceptance Criteria:**
- [x] Given `sidebar_show.status` éteint, when une ligne d'onglet ou de workspace est rendue, then elle n'affiche ni badge, ni matrice, ni shimmer de titre, ni infobulle de statut (le résumé passé à `infer_lane` vaut `None`, `src-app/src/app/sidebar/tab_row.rs:61-66`, `workspace_row.rs:75-88`).
- [x] Given `status` éteint et une PR ouverte sur l'onglet, when la ligne est rendue, then la PR prend la place selon `infer_lane` (`lane.rs:20-30`) ; un workspace n'affiche sa PR que replié, comme aujourd'hui (`workspace_row.rs:83-88`).
- [x] Given `status` éteint, when un agent attend, then le bouton d'attention des espaces masqués, le marqueur du header de pane, les notifications, la section Agents et ⌘⇧J se comportent comme avec `status` allumé.
- [x] Given `status` éteint et la section Agents masquée (`sidebar_show.agents` à faux), when un agent attend, then la sidebar n'affiche aucun statut d'agent et aucun avertissement n'apparaît.
- [x] Given `status` allumé, when l'arbre est rendu, then il est identique au rendu d'avant ce PRD, aux changements de US-001 et US-002 près.

#### US-010: Item Status dans Customize sidebar › Show et dans la palette
**Description:** As a utilisateur, I want basculer les statuts de l'arbre depuis le menu de la sidebar et la palette so that je règle ce détail là où je règle les autres.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Customize sidebar › Show liste Branch, Diffstat, PR, Status, Indent guide, puis la règle, Agents et Usage (`customize_menu.rs:243-300`) ; Status utilise l'icône `icons/palette/notification-01.svg` et sa coche suit `status_enabled()`.
- [x] Given un clic sur Status, when le menu l'enregistre, then `sidebar_show.status` est écrit par `persist_setting` (`customize_menu.rs:61`) avec les autres clés de `sidebar_show` inchangées, et l'arbre se met à jour sans redémarrage.
- [x] La palette gagne une bascule « Show agent status in the sidebar » dans la section Sidebar (`src-app/src/app/command_palette/catalog.rs:1336-1390`), qui lit `status_enabled()` et écrit par `sidebar_show_toggle` (`catalog.rs:712-724`).
- [x] Given une écriture de configuration qui échoue, when l'utilisateur bascule Status, then le chemin d'erreur existant de `persist_setting` s'applique et la coche reflète la configuration effectivement chargée.
- [x] Les tests de `customize_menu.rs` (`:393-467`) couvrent la nouvelle bascule.

---

### EP-004: Contrat visuel et documentation

DESIGN.md, le changelog et la documentation utilisateur décrivent ce que l'app fait après ce PRD.

**Definition of Done:** DESIGN.md, CHANGELOG.md et la documentation utilisateur décrivent les groupes, la ligne d'une ligne, l'item Status de l'arbre et le menu Customize agent rows réduit, sans aucune mention de `agent_status`.

#### US-011: DESIGN.md, CHANGELOG et documentation utilisateur
**Description:** As a contributeur ou utilisateur, I want une documentation qui décrit la section groupée et le réglage Status so that le contrat visuel et le comportement restent vérifiables.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005, US-007, US-009, US-010

**Acceptance Criteria:**
- [ ] DESIGN.md décrit les en-têtes de groupe (24 px, 12 px Medium, couleurs, compteur, 8 px avant chaque en-tête sauf le premier), la ligne de 32 px avec le suffixe workspace, la disparition de la ligne méta et de l'écart de rang, le nouveau plancher de redimensionnement, l'item Status de Customize sidebar et Customize agent rows réduit à Logo et Workspace ; la phrase « There is no counter and no sort control » (DESIGN.md:564) est remplacée.
- [ ] CHANGELOG.md, section Unreleased : l'entrée « Choose what each Agents row shows » (CHANGELOG.md:15) ne cite plus le statut ni `sidebar_show.agent_status` ; une entrée Changed décrit les groupes et l'alignement de l'arbre sur la section (Working au lieu de Done pour un agent reparti, matrice à l'accent du runtime) ; une entrée Added décrit `sidebar_show.status`.
- [ ] La documentation utilisateur est modifiée dans le dépôt du site paneflow-web (page features, description des lignes Agents et de Customize sidebar › Show, aujourd'hui `docs/user/features.md:93` et `docs/user/features.md:108-109` ; page configuration/schema, `sidebar_show.status`), puis `docs/user/` est resynchronisé depuis le site.
- [ ] Given une recherche de `agent_status` dans DESIGN.md, CHANGELOG.md, `schemas/` et `docs/user/`, when elle est lancée, then elle ne trouve rien.

---

### EP-005: Exploration, un agent qui attend ses sous-agents n'est pas au repos

Un agent qui a lancé des sous-agents et rendu la main apparaît aujourd'hui dans Idle (ou Done), alors que du travail continue pour lui. Cet epic explore comment le montrer en restant sur OSC 7501. Il ne livre aucun code produit : il livre des observations, un inventaire et une décision.

**Contexte vérifié le 2026-11-11.** La section classe une session avec `AgentRank::of` (`src-app/src/app/sidebar/agents.rs:19-27`) : une session qui a déjà déclaré un état et dont le dernier état est `idle`, ou `done` déjà vu, tombe dans Idle. No status ne concerne que les sessions qui n'ont jamais parlé OSC 7501. La spec r0.4 (2026-10-10) ne définit rien sur les sous-agents, mais elle donne aux enregistrements une hiérarchie par `id` (segments séparés par `/`, 8 niveaux) et précise qu'un programme peut être `idle` à son prompt pendant qu'un de ses workers est dans un autre état, la présentation restant au terminal. L'hôte garde déjà ces enregistrements enfants (`ProgramStatusRecords`, `crates/paneflow-host/src/program_status.rs:62-66`, jusqu'à 256), mais ne publie que l'enregistrement racine, ou à défaut le plus récent (`current`, `program_status.rs:119-125`). Ce que les runtimes émettent réellement pendant que des sous-agents tournent n'a pas été observé.

**Definition of Done:** le flux OSC 7501 d'une session avec sous-agents est capturé pour chaque runtime supporté qui en propose ; l'écart entre ce que l'hôte reçoit et ce que la sidebar affiche est décrit ; une option est retenue par Arthur, avec ce qu'elle change aux contraintes de ce PRD, et consignée dans ce document ; la suite est tracée (amendement de ce PRD, nouveau PRD, ou demande aux runtimes).

#### US-012: Observer ce que les runtimes déclarent pendant que des sous-agents tournent
**Description:** As a mainteneur, I want le flux OSC 7501 réel d'un agent qui délègue so that la solution parte de ce que les runtimes émettent et non d'une supposition.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given Claude Code, Codex, OpenCode et Pi dans leur version courante, when un agent lance au moins un sous-agent, then la séquence des rapports OSC 7501 est capturée de la délégation jusqu'au retour du dernier sous-agent, avec pour chaque rapport son `state`, son `id` et l'instant relatif ; un runtime sans sous-agents est noté comme tel.
- [ ] La capture couvre les deux formes de délégation quand le runtime les propose : sous-agent au premier plan (l'agent attend dans son tour) et sous-agent en arrière-plan (l'agent rend la main).
- [ ] Pour chaque runtime, le relevé dit quel état porte l'enregistrement racine pendant que les sous-agents tournent (`working`, `idle` ou `done`), et si des enregistrements enfants existent pour eux.
- [ ] Le relevé note la version de chaque runtime, la date, et si le runtime passe par un démon partagé qui changerait ce que le pane reçoit.
- [ ] La capture lit les octets reçus par le pane ou les enregistrements de l'hôte ; elle n'utilise ni hook ni lecture d'écran.

#### US-013: Inventaire de ce que Paneflow reçoit, garde et affiche
**Description:** As a mainteneur, I want savoir où l'information se perd entre l'hôte et la sidebar so that le coût de chaque option soit connu avant de choisir.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] L'inventaire suit un rapport enfant (`id` non vide) de la réception dans l'hôte jusqu'à la sidebar, et nomme l'endroit où il cesse d'être transmis (publication de l'hôte, réduction du worker dans `crates/paneflow-serve/src/state.rs`, trame IPC, ou projection de l'app).
- [ ] Il dit ce que la spec r0.4 permet et interdit pour ce cas : hiérarchie par `id`, durée de vie de `working` (jusqu'au remplacement, à l'effacement, à la sortie du processus ou au prompt suivant), absence de heartbeat, clés inconnues ignorées, liste d'états fermée.
- [ ] Il liste les surfaces qui lisent l'état d'une session et qu'un nouvel affichage toucherait : lignes de l'arbre, section Agents, ⌘⇧J, bouton d'attention des espaces masqués, marqueur du header de pane, notifications de fin de tour, acquittement de `done`.
- [ ] Il nomme les contraintes de ce PRD et d'ARCHITECTURE.md qu'une solution devrait lever ou garder (« aucune donnée nouvelle ne traverse l'hôte, le worker ou l'IPC », « OSC 7501 est la seule source d'état de tour »).

#### US-014: Choisir comment montrer un agent qui attend ses sous-agents
**Description:** As a développeur multi-agents, I want que la sidebar distingue un agent au repos d'un agent qui attend ses sous-agents so that je ne prenne pas pour disponible un agent dont le travail continue.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012, US-013

**Acceptance Criteria:**
- [ ] Les options sont comparées sur les faits de US-012 et US-013, au minimum : (a) dériver Working pour une session dont la racine est `idle` ou `done` tant qu'un enregistrement enfant est `working` ; (b) un statut distinct pour l'attente de sous-agents, avec son groupe ou son badge ; (c) demander aux runtimes de garder la racine à `working`, ou d'émettre des enregistrements enfants, s'ils n'émettent rien d'exploitable ; (d) ne rien changer.
- [ ] Chaque option dit ce qu'elle affiche dans l'arbre et dans la section, dans quel groupe tombe la session, ce que deviennent la notification de fin de tour et le badge Done quand la racine dit `done` pendant que des enfants travaillent, et ce qui se passe quand un enfant reste `working` après la mort de son sous-agent.
- [ ] Aucune option retenue ne lit l'écran, un hook, le clavier ou l'arbre des processus : OSC 7501 reste la seule source d'état de tour.
- [ ] Given un runtime qui n'émet aucun rapport exploitable pour ses sous-agents, when l'option est choisie, then le document dit ce que Paneflow affiche pour lui (comportement actuel) et quelle demande est adressée au runtime, avec son lien si elle est déposée ; aucune issue n'est ouverte sans l'accord d'Arthur.
- [ ] La décision d'Arthur, sa date et ses raisons sont consignées dans ce PRD, ainsi que la suite : stories ajoutées ici, nouveau PRD, ou abandon.

## Functional Requirements

- FR-01: La section Agents doit présenter ses agents en groupes par statut affiché, dans l'ordre Need you, Error, Done, Working, Idle, No status, sans groupe vide.
- FR-02: Chaque groupe doit commencer par un en-tête qui affiche son libellé et le nombre de ses agents.
- FR-03: Chaque agent doit tenir sur une ligne de 32 px avec logo, titre, suffixe workspace et statut.
- FR-04: Les lignes de la section Agents doivent toujours afficher leur statut ; aucun réglage ne doit le masquer.
- FR-05: L'ordre des agents à l'intérieur d'un groupe doit rester celui de `order_agents`, et ⌘⇧J doit garder son ordre actuel.
- FR-06: Une session sans capacité OSC 7501 doit rester dans Idle pendant son délai de grâce de 3 s, puis passer dans No status.
- FR-07: Le curseur clavier ne doit jamais s'arrêter sur un en-tête de groupe.
- FR-08: Quand `sidebar_show.status` est éteint, les lignes de workspace et d'onglet ne doivent afficher ni badge, ni matrice, ni shimmer, ni infobulle de statut d'agent.
- FR-09: Le système ne doit PAS modifier le bouton d'attention des espaces masqués, le marqueur du header de pane, les notifications ni ⌘⇧J selon `sidebar_show.status`.
- FR-10: L'arbre et la section doivent calculer l'état d'une session avec `AgentRank::of`.
- FR-11: Une configuration qui contient `sidebar_show.agent_status` doit se charger comme si la clé était absente.
- FR-12: Le système ne doit PAS afficher de durée, ni de champ déclaré par OSC 7501, dans la section Agents.

## Non-Functional Requirements

- **Performance:** la pire construction et la pire frame complète de `render_sidebar` restent sous 16 ms avec 50 agents sur 30 workspaces, en build release (`agents_section.rs:2414-2491`). Aucun nouveau minuteur : les seuls réveils restent l'horloge d'animation partagée, à 12 réveils par seconde au plus (`lane.rs:573-589`), et le réveil unique du délai de grâce de 3 s.
- **Accessibility:** 100 % des états lisibles sans la couleur (mot de l'en-tête, mot du badge ou forme de la matrice) ; 100 % des agents atteignables au clavier ; chaque en-tête annoncé avec son libellé et son nombre ; sous `reduce_motion`, 0 élément animé hors statut vivant, et matrice et shimmer immobiles.
- **Security:** 0 champ déclaré par OSC 7501 (`title`, `message`, `app`) rendu dans la section ; l'identité affichée vient du runtime détecté par le shim et le pane.
- **Reliability:** une configuration contenant `sidebar_show.agent_status` se charge à l'identique de la même configuration sans la clé dans 100 % des cas testés ; une valeur invalide de `sidebar_show.status` prend son défaut sans erreur.
- **Compatibility:** comportement identique sur Linux (Wayland et X11), macOS et Windows, sans code derrière un `cfg` de plateforme.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Aucun agent | Espace actif sans session d'agent | Aucun en-tête de groupe, ligne vide existante | « No agents running » |
| 2 | Un seul groupe | Tous les agents au même statut | Un seul en-tête, sans espace de 8 px au-dessus | Aucun |
| 3 | Agent qui démarre sans OSC 7501 | Session sans capacité, moins de 3 s après son lancement | Ligne dans Idle sans statut, puis dans No status après 3 s | Infobulle « No status » existante après 3 s |
| 4 | Agent qui change de groupe sous le curseur | Le statut change pendant que le curseur clavier est sur la ligne | La ligne change de groupe, le curseur la suit | Aucun |
| 5 | Résultat vu | L'onglet d'un agent Done devient visible | L'agent passe de Done à Idle, le compteur de Done baisse | Aucun |
| 6 | Titre et workspace longs | Texte plus large que les 270 px utiles | Ellipse en fin, suffixe coupé en premier, pas de retour à la ligne | Infobulle « title · workspace » |
| 7 | Plus d'agents que le plafond | Contenu au-delà de 330 px | Liste défilante, en-têtes défilant avec leurs lignes | Aucun |
| 8 | Statuts de l'arbre et section masqués | `sidebar_show.status` et `sidebar_show.agents` à faux | Aucun statut dans la sidebar ; bouton des espaces masqués, marqueur de pane et notifications inchangés | Aucun |
| 9 | Ancienne configuration | `sidebar_show.agent_status` présent dans `paneflow.json` | Chargement identique à sans la clé ; un éditeur signale la clé via le schéma (`paneflow.schema.json:115`) | Aucun dans l'app |
| 10 | Valeur invalide | `"sidebar_show": {"status": "yes"}` | La clé prend son défaut (vrai) | Aucun |
| 11 | Runtimes mélangés au travail | Deux runtimes Working sous un même onglet ou workspace replié | Matrice de l'arbre en `muted` | Aucun |
| 12 | Non-lu sans session | Résultat non lu dont la session a disparu ou non attribué | L'arbre montre Done ; la section ne liste pas d'agent pour lui | Infobulle « Agent finished » existante |
| 13 | Réduction des mouvements | `reduce_motion` activé | Matrice et shimmer immobiles dans l'arbre et la section | Aucun |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Le défilement au clavier vise la mauvaise ligne une fois les en-têtes ajoutés (`agent_row_child_index` suppose un espaceur par ligne) | High | Med | Critère dédié dans US-006 et test du curseur sur une liste à deux groupes |
| 2 | L'alignement de l'arbre sur `AgentRank` surprend (Working au lieu de Done pour un agent reparti) | Med | Low | Test croisé de US-001, entrée Changed dans le changelog |
| 3 | Le rendu s'écarte de la maquette validée (densité, alignement des en-têtes) | Med | Med | Revue `/better-interface` et passe visuelle d'Arthur sur macOS et Windows |
| 4 | Les utilisateurs gênés par le doublon ne trouvent pas l'item Status, laissé activé par défaut | Med | Med | Entrée Added dans le changelog, documentation, suivi des retours au mois 1 |
| 5 | Les en-têtes dépassent le budget de 16 ms avec 50 agents | Low | Med | Test de budget en release pour US-004, US-005 et US-006 |
| 6 | Le rôle `Heading` n'est pas exposé par la révision de GPUI épinglée | Med | Low | Repli sur `Role::Group`, déjà utilisé dans l'app |
| 7 | La documentation du site et `docs/user/` divergent | Med | Low | US-011 modifie le site puis resynchronise, sans édition directe du miroir |

## Non-Goals

What this version explicitly does NOT include:

- Pas de durée ni d'horodatage de changement d'état : décision du 2026-11-02, calcul coûteux pour un bruit visuel.
- Pas de point à gauche ni de glyphes statiques à la Herdr : la DA de Paneflow, sa matrice en comète et son shimmer restent.
- Pas de réglage pour masquer le statut des lignes Agents : masquer le statut d'une section groupée par statut revient à masquer la section, ce que `sidebar_show.agents` fait déjà.
- Pas de repli par groupe ni de masquage du groupe Idle ; à revoir au bilan du mois 1 si des utilisateurs gardent plus de 10 agents au repos.
- Pas de gel du réordonnancement sous le pointeur ni d'anti-rebond : aucune plainte observée, et OSC 7501 change d'état aux frontières de tour.
- Pas de changement du marqueur du header de pane : il ne lit que la surface active (`src-app/src/app/ipc_handler/agent_frames.rs:354-373`), qui porte une seule session, donc l'erreur et l'attente ne s'y rencontrent pas en pratique.
- Pas de correction des valeurs par défaut lues par la palette pour Branch, Diffstat, PR et Indent guide (`catalog.rs:1343-1376`) : bug distinct, voir Open Questions.

## Files NOT to Modify

- `crates/paneflow-host/**`, `crates/paneflow-serve/**`, `crates/paneflow-ipc-client/src/agent.rs` : aucune donnée nouvelle, l'état de tour reste à quatre valeurs.
- `src-app/src/app/sidebar/agents.rs` : `AgentRank`, `order_agents` et l'ordre de ⌘⇧J sont réutilisés tels quels.
- `src-app/src/app/workspace_ops/focus.rs` : ⌘⇧J inchangé.
- `src-app/src/pane.rs`, `src-app/src/app/ipc_handler/agent_frames.rs` : le marqueur du header de pane ne change pas.
- `src-app/src/app/sidebar/space_switcher.rs`, `src-app/src/app/attention.rs` : le bouton des espaces masqués garde sa propre agrégation.
- `native/libghostty/**`, `src-app/build.rs` : hors sujet, moteur et build.
- `docs/user/**` à la main : miroir généré du site paneflow-web.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** groupes calculés par une fonction pure sur les `AgentRow` triées, partagée par le rendu, la hauteur et le clavier ; recommandé plutôt qu'un regroupement dans le rendu. Engineering to confirm feasibility.
- **Data Model:** l'arbre garde `SidebarAgentSummary` (état, nombre, teinte) comme type de sortie et ne change que son calcul ; alternative : remplacer `SidebarAgentState` par `AgentRank` dans `Lane`. Trade-off : diff plus petit contre une seule énumération dans tout le rendu.
- **API Design:** aucune méthode IPC ni événement nouveau. Le rôle d'accessibilité des en-têtes : `Heading` recommandé, `Group` en repli ; à confirmer sur la révision de GPUI épinglée.
- **Dependencies:** aucune bibliothèque nouvelle. Icône de l'item Status : réutiliser `icons/palette/notification-01.svg` (recommandé, DESIGN.md:1260-1268 demande deux usages avant une nouvelle primitive) ou dessiner une icône de matrice.
- **Migration:** aucune migration ; serde ignore les clés inconnues de `sidebar_show`. Backward compatibility requirement: yes, une configuration avec `agent_status` se charge. Rollback plan : revenir au commit précédent ; `sidebar_show.status` est ignoré par une version qui ne le connaît pas.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Statuts d'agent répétés dans la sidebar, scénario de 6 agents, `status` éteint | 5 | 0 | Month-1 | Test de rendu et passe visuelle d'Arthur |
| Agents visibles sans défilement sous 330 px, 2 groupes | 4 | Au moins 6 | Month-1 | Test de hauteur de US-004 |
| Hauteur d'une ligne d'agent | 55 px | 32 px | Month-1 | Test de hauteur de US-004 |
| Combinaisons d'état où l'arbre et la section divergent | 2 cas connus (No status, Done contre Working) | 0 | Month-1 | Test croisé de US-001 |
| Pire frame de `render_sidebar`, 50 agents sur 30 workspaces, release | Sous 16 ms | Sous 16 ms | Month-1 | Test de budget en release |
| Retours utilisateurs signalant le doublon | Plusieurs, qualitatifs, avant le 2026-11-02 | 0 nouveau | Month-1 à Month-6 | Issues GitHub et retours directs, relevés par Arthur |

## Open Questions

- Faut-il corriger, dans un commit séparé, les valeurs par défaut lues par la palette pour Branch, Diffstat, PR et Indent guide (`unwrap_or(true)` à `catalog.rs:1343-1376`, contre des accesseurs à `false` dans `config.rs:209-223`) ? Arthur, avant EP-003, qui touche le même bloc.
- Le groupe Idle doit-il devenir repliable si des utilisateurs gardent plus de 10 agents au repos ? Arthur, au bilan du mois 1.
- Faut-il, à terme, éteindre les statuts de l'arbre par défaut quand la section Agents est visible ? Arthur, selon les retours du mois 1.
[/PRD]
