[PRD]
# PRD: Section Agents dans la sidebar

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-30 | Arthur Jean | PRD initial : section « Agents » repliable et redimensionnable sous l'arbre des workspaces, triée par urgence, alimentée uniquement par OSC 7501, avec détection de la capacité OSC 7501 par session, ⌘⇧J aligné sur le même ordre et préférence `sidebar_show.agents`. 4 epics, 16 stories. |

## Problem Statement

**Convention de référence.** Les chemins désignent Paneflow au commit `58bf6819e`. La maquette validée est l'artboard `Main.dc.html` du canvas https://claude.ai/artifact/22uUkskjaT3c49S9ixHDBj (version 12, privé). Le protocole visé est OSC 7501 révision 0.4 du 2026-10-10 ([spec](https://superlogical.com/rex/docs/build/program-status)).

1. **Savoir quel agent t'attend demande de parcourir tout l'arbre.** Les statuts existent ligne par ligne (`src-app/src/app/sidebar/lane.rs:74-120`), mais ils sont dispersés dans l'ordre des workspaces. Avec plusieurs workspaces ouverts, un agent bloqué peut être hors de l'écran. La file d'attention en dialogue a été retirée le 2026-10-30 (`58bf6819e`) parce que son rendu en overlay ne convenait pas, pas parce que le besoin avait disparu.
2. **⌘⇧J ne connaît qu'un seul état.** `handle_jump_next_waiting` ne parcourt que `AgentState::WaitingForInput`, dans l'ordre des workspaces puis des panes (`src-app/src/app/workspace_ops/focus.rs:142-190`). Une erreur ou un résultat non lu n'est jamais atteint au clavier.
3. **Un agent muet est indiscernable d'un agent idle.** Depuis 0.17.7, OSC 7501 est la seule source d'état (`CHANGELOG.md:108`). Un Claude Code antérieur à 2.1.295, un agent qui n'émet pas encore le protocole, ou un agent lancé dans un tmux interne n'affichent rien. Rien n'indique pourquoi, car l'hôte ne garde aucune trace de la requête `OSC 7501 ; ?` : elle est répondue dans le moteur et ne ressort qu'en `WritePty` (`crates/paneflow-host/src/runtime.rs:1328`).

**Why now:** Claude Code émet OSC 7501 depuis la v2.1.295 du 2026-10-08 ([classmethod](https://dev.classmethod.jp/en/articles/20261009-cc-updates-v2-1-295/)), et l'hôte Paneflow répond déjà à la requête de la révision 0.4 (`CHANGELOG.md:19`). La source d'état est désormais déclarée par l'agent lui-même, sans aucune lecture d'écran. La file d'attention vient d'être retirée et Arthur a validé l'emplacement (le bas de la sidebar) et le rendu dans la maquette.

## Overview

Une section « Agents » s'ajoute en bas de la sidebar, entre l'arbre des workspaces et le footer. Elle liste les sessions d'agents reconnues de l'espace actif, triées par urgence : bloqué, en erreur, terminé non vu, en cours, idle, puis sans statut. Chaque ligne reprend les composants des lignes d'onglet : logo de l'agent dans l'emplacement de 20 px, titre en 14 px Medium, statut à droite (cloche, croix, point, matrice), et une ligne méta avec le nom du workspace. Un clic donne le focus au pane de l'agent. ⌘⇧J parcourt les agents qui attendent l'utilisateur dans le même ordre.

L'état vient uniquement d'OSC 7501, comme aujourd'hui. Il n'y a ni lecture d'écran ni hook. Le « vu » réutilise l'acquittement existant (`AgentCompletionNotification`, `acknowledge_visible_completions`) : ouvrir l'onglet d'un agent `done` acquitte son résultat. L'agent passe alors au rang idle et perd sa pastille, comme dans l'arbre. Le record OSC 7501 garde la durée de vie de la spec, gérée par l'hôte (`crates/paneflow-host/src/program_status.rs:106`). Une seule donnée nouvelle traverse la frontière hôte, worker, app : la capacité OSC 7501 de la session. Elle est vraie dès que l'hôte a répondu à `OSC 7501 ; ?` ou appliqué un premier record. Sans elle, passé un délai de grâce de 3 s, la ligne affiche « No status » avec une infobulle qui explique pourquoi.

La section se replie depuis son en-tête « Agents » et son chevron. Sa hauteur suit son contenu jusqu'à 330 px, se règle avec une poignée, et persiste dans `window-state.json` avec l'état replié. Elle s'affiche ou se masque par Customize sidebar › Show › Agents, persisté sous `sidebar_show.agents` (vrai par défaut). Le nom « section Agents de la sidebar » évite la collision avec le « dock Agents » du PRD navigateur (`tasks/prd-agents-browser.md:2`), qui désigne le dock de droite.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Atteindre l'agent le plus urgent de l'espace actif | 1 clic ou 1 ⌘⇧J, quel que soit le défilement de l'arbre | Inchangé |
| Rendre explicite l'absence de statut | 100 % des sessions reconnues sans capacité OSC 7501 affichent « No status » après 3 s | Inchangé |
| Coût de rendu de la sidebar | `render_sidebar` sous 16 ms avec 30 workspaces et 50 sessions d'agents (build release) | Inchangé |
| Zéro lecture d'écran pour l'état | 0 appel de classification d'écran ajouté, vérifié par revue et par le test de garde existant | Inchangé |

## Target Users

### Développeur qui fait tourner plusieurs agents en parallèle
- **Role:** Arthur et les utilisateurs de Paneflow qui lancent 3 à 10 agents (Claude Code, Codex, OpenCode, Pi) dans plusieurs workspaces.
- **Behaviors:** répartit les tâches par workspace, revient vers un agent quand il a fini ou demande une permission, surveille la sidebar.
- **Pain points:** un agent bloqué sur une permission passe inaperçu s'il est hors de l'écran. Un résultat terminé n'est revu qu'en scrollant l'arbre. Un agent muet ressemble à un agent idle.
- **Current workaround:** parcourir l'arbre à l'œil, ⌘⇧J pour le seul état « waiting », notifications système.
- **Success looks like:** un coup d'œil en bas de la sidebar dit qui attend, dans quel ordre, et pourquoi un agent n'a pas d'état.

### Utilisateur venant de Herdr
- **Role:** développeur habitué au panneau d'agents de Herdr.
- **Behaviors:** trie ses agents par priorité et saute de l'un à l'autre au clavier.
- **Pain points:** perdre ce panneau en changeant d'outil.
- **Current workaround:** aucun dans Paneflow depuis le retrait de la file d'attention.
- **Success looks like:** retrouver la même logique de tri et le saut au clavier.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Herdr** : panneau d'agents trié par urgence (blocked, done non lu, working, idle). Le `done` devient idle une fois l'agent vu. L'état vient de la lecture d'écran et de règles par agent mises à jour à distance (`C:\dev\herdr\docs\preview\website\src\content\docs\agents.mdx:64-98`). Paneflow garde le tri et le « vu », sans la lecture d'écran.
- **cmux** : anneau bleu sur le pane en attente, onglet non lu, saut au dernier événement, alimentés par des hooks ([comparatif](https://flaviocopes.com/herdr-vs-cmux.md)). Paneflow garde le saut clavier, sans hooks pour l'état.
- **Ghostty** : libghostty parse OSC 7501 depuis la PR 14560 (2026-10-06), sans l'afficher ([swareco](https://www.swareco.com/blog/osc-7501-program-status-protocol)).
- **Market gap:** aucun multiplexeur ne fournit de panneau d'agents piloté uniquement par un protocole déclaratif, sans coût de lecture d'écran ni règles à maintenir.

### Best Practices Applied
- `done` signifie « résultat pas encore vu ». C'est au terminal de décider quand le masquer (spec r0.4, section lifetime). Le focus de l'onglet acquitte.
- Tous les champs déclarés sont non fiables. L'identité affichée vient du shim et du pane, jamais du champ `app` (spec r0.4, section security).
- Saut clavier vers le prochain élément à traiter (cmux, Herdr).

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- La réponse du moteur à `OSC 7501 ; ?` arrive dans `BackendEvent::WritePty` avec le préfixe `ESC ] 7501 ; ?` et peut être reconnue sans ambiguïté. Basé sur `native/libghostty/prebuilt/x86_64-pc-windows-msvc/include/ghostty/vt/terminal.h:2752-2773` et `crates/paneflow-host/src/runtime.rs:1328`. Validé par US-003.
- Claude Code ≥ 2.1.295 envoie la requête au démarrage et n'active le protocole que si le terminal répond ([kucoin](https://www.kucoin.com/news/flash/claude-code-integrates-osc-7501-terminal-protocol-within-48-hours-of-proposal), source secondaire).
- `last_event_at_ms` et `state_seq` de `AgentSession` (`src-app/src/ai_types.rs:7-22`) avancent à chaque changement d'état et suffisent pour départager deux agents du même rang. Non vérifié pour `last_event_at_ms`, vérifié par US-001.
- `acknowledge_visible_completions` (`src-app/src/app/workspace_ops/tab.rs:199-223`) acquitte les résultats non lus dès que l'onglet devient visible, quelle que soit l'origine du focus (clic dans l'arbre, dans la section ou ⌘⇧J).
- Codex, Grok, Copilot, OpenCode, Pi et Gemini émettront OSC 7501. Ce n'est confirmé que pour Claude Code ; c'est un pari assumé par Arthur.

### Hard Constraints
- OSC 7501 est la seule source d'état de tour (`ARCHITECTURE.md:363-366`) : pas de lecture d'écran, pas de hook, pas d'heuristique clavier.
- Aucune I/O synchrone ni sous-processus sur le thread de rendu (`AGENTS.md`, section Boundaries).
- Linux, macOS et Windows sont des cibles. Le code `cfg(windows)` n'est linté que par la lane Windows de la CI.
- Aucun commentaire dans le code source. Tout nouvel item se déclare avant un éventuel `mod tests` (piège `items_after_test_module`).
- libghostty n'est pas modifié. La détection de la capacité se fait côté hôte.
- La sidebar est une vue en cache (`src-app/src/app/sidebar/view.rs:28`) : l'état de la section vit sur `PaneFlowApp`.
- Le rendu suit DESIGN.md et la maquette validée, avec les primitives existantes (`squircle_skin`, `ROW_RADIUS` = 9, `sidebar_row_shell`).

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration

Gates additionnels :
- Stories qui modifient le catalogue des runtimes (US-005) : `cargo test -p paneflow-agent-config --locked`.
- Stories qui touchent du code `#[cfg(windows)]` : relecture de l'ordre des items par inspection, la lane `Windows x86_64 libghostty check` faisant foi après le push.
- Stories d'interface (EP-002, EP-003, US-015) : passe visuelle manuelle par Arthur sur macOS et Windows, comparée à la maquette validée. L'agent d'implémentation ne lance pas l'application et ne fait pas de capture.

## Epics & User Stories

### EP-001: Modèle, ordre et capacité OSC 7501

Donne à l'app la liste triée des agents et la capacité OSC 7501 de chaque session, sans aucune interface. C'est le socle commun de la section et de ⌘⇧J.

**Definition of Done:** une fonction pure produit les lignes triées à partir des sessions et de l'état non lu ; ⌘⇧J l'utilise ; chaque `AgentSession` porte sa capacité OSC 7501 publiée par l'hôte ; le catalogue connaît la version minimale de Claude Code ; tous les tests ajoutés passent.

#### US-001: Fonction de tri des agents par urgence
**Description:** As a développeur multi-agents, I want une liste d'agents ordonnée blocked, error, done non vu, working, idle, sans statut so that l'agent le plus urgent soit toujours en tête.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given des sessions dans plusieurs workspaces, when la fonction de tri s'exécute, then elle renvoie un rang par session : 0 `WaitingForInput`, 1 `Errored`, 2 `Finished` avec résultat non lu, 3 `Thinking`, 4 `Finished` acquitté, 5 session sans capacité OSC 7501.
- [ ] Given deux sessions du même rang, when elles sont triées, then la plus récente selon `last_event_at_ms`, puis `state_seq`, passe en premier, et l'ordre des workspaces puis des onglets départage les égalités restantes.
- [ ] Given une session dont `last_event_at_ms` est `None`, when elle est triée, then elle se place après les sessions datées de son rang, sans panique.
- [ ] Given un filtre d'espace, when la fonction s'exécute pour la section, then seules les sessions des workspaces de l'espace actif (`crate::app::spaces::in_space`) sont renvoyées.
- [ ] La fonction ne lit ni écran, ni hook, ni fichier, et ses tests unitaires sont placés avant tout `mod tests` existant du module hôte.

#### US-002: ⌘⇧J suit l'ordre de la section
**Description:** As a développeur multi-agents, I want que ⌘⇧J parcoure les agents bloqués, en erreur, puis terminés non vus, dans l'ordre de la section so that un seul raccourci m'amène à tout ce qui m'attend.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given des agents aux rangs 0, 1 et 2, when l'utilisateur appuie plusieurs fois sur ⌘⇧J (Ctrl+Shift+J sous Linux et Windows), then le focus passe de l'un à l'autre dans l'ordre de la fonction de tri, puis revient au premier.
- [ ] Given que le parcours a lieu sur tous les workspaces comme aujourd'hui (`focus.rs:161-180`), when un agent urgent est dans un espace masqué, then il est atteint après ceux de l'espace actif.
- [ ] Given un agent `done` non vu atteint par ⌘⇧J, when son onglet devient visible, then son résultat est acquitté et il sort du cycle au prochain appui.
- [ ] Given aucun agent aux rangs 0 à 2, when l'utilisateur appuie sur ⌘⇧J, then rien ne change et aucune erreur n'est journalisée.
- [ ] Given la section masquée ou repliée, when l'utilisateur appuie sur ⌘⇧J, then le comportement est identique.

#### US-003: Validate assumption: la réponse à `OSC 7501 ; ?` est observable dans le flux WritePty de l'hôte
**Description:** As a mainteneur, I want prouver que l'hôte peut reconnaître la réponse du moteur à la requête de support so that la capacité OSC 7501 se détecte sans modifier libghostty.

**Priority:** P0
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given une session hôte de test, when le programme écrit `ESC ] 7501 ; ? ST`, then un test observe exactement une réponse `WritePty` qui commence par `ESC ] 7501 ; ?:states=`.
- [ ] Given une sortie qui contient le texte `7501` hors séquence OSC (par exemple `echo 7501`), when elle est traitée, then aucune réponse de ce type n'est observée.
- [ ] Given qu'aucun callback OSC 7501 n'est installé côté desktop (`desktop_terminals_neither_answer_nor_report_program_status`), when le test tourne, then une seule réponse atteint le PTY.
- [ ] Si l'hypothèse est infirmée, la story consigne le résultat et US-004 passe en BLOCKED avec la décision à prendre (patch libghostty ou capacité limitée au premier record).

#### US-004: Publier la capacité OSC 7501 de chaque session
**Description:** As a développeur multi-agents, I want que l'app sache si un agent parle OSC 7501 so that un agent muet soit distingué d'un agent idle.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Given une session dont l'hôte a répondu à `OSC 7501 ; ?` ou appliqué un record valide, when le manifeste de session est publié, then il porte un champ de capacité à vrai.
- [ ] Given que la capacité est devenue vraie, when les records sont vidés (RIS, reset manuel, `clear`) sans changement de programme, then elle reste vraie.
- [ ] Given un nouveau processus d'agent dans le même pane (nouvelle génération de runtime), when il démarre, then la capacité repart à faux jusqu'à sa propre requête ou son premier record.
- [ ] Given un worker et un hôte de versions différentes, when la version du protocole ne correspond pas, then la négociation existante (`core_protocol`) refuse la connexion avec son erreur de version existante, sans champ partiellement lu.
- [ ] Given le worker, when il sérialise une session, then le champ de capacité figure dans `to_value` (`crates/paneflow-serve/src/state.rs:255-285`), et `AgentSession` l'expose côté app.
- [ ] Un test d'intégration hôte, worker, app reproduit la séquence `?` puis `state=working` et vérifie la capacité et l'état de bout en bout.

#### US-005: Version minimale de statut dans le catalogue des runtimes
**Description:** As a développeur multi-agents, I want que l'infobulle « No status » cite la version qui apporte OSC 7501 quand elle est connue so that je sache quoi mettre à jour.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given le schéma strict des descripteurs, when un champ optionnel de version minimale de statut est ajouté, then `paneflow-agent-config/build.rs` le valide et le génère dans le registre constant, sans parseur TOML au runtime.
- [ ] Given `runtimes/claude-code/runtime.toml`, when il déclare `2.1.295`, then le registre expose cette valeur pour Claude Code.
- [ ] Given un descripteur sans ce champ, when le catalogue est généré, then la valeur est absente et aucune version n'est inventée.
- [ ] Given une valeur qui n'est pas une version `X.Y.Z`, when le build s'exécute, then il échoue en nommant le descripteur.
- [ ] `runtimes/README.md` documente le champ.

---

### EP-002: Rendu de la section Agents

Affiche la section validée en bas de la sidebar et la rend utilisable à la souris.

**Definition of Done:** la section apparaît sous l'arbre avec ses lignes, son état vide, « No status », le clic qui donne le focus et le repli ; la passe visuelle d'Arthur la juge conforme à la maquette ; `render_sidebar` reste sous 16 ms avec 30 workspaces et 50 sessions.

#### US-006: Conteneur, en-tête et hauteur automatique
**Description:** As a développeur multi-agents, I want une section « Agents » en bas de la sidebar qui prend la hauteur de son contenu so that elle ne prenne jamais plus de place que nécessaire.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given la sidebar, when elle se rend, then la section se place entre la liste `workspace-list` et le footer, sans trait séparateur.
- [ ] Given l'en-tête, when il se rend, then il fait 36 px, padding horizontal 8, avec un bouton de 22 px de haut (padding 7 et 6, rayon 7) qui contient « Agents » en 13 px `muted` puis `icons/chevron-down.svg` en 12 px `muted`, à 8 px d'écart. Il n'y a ni compteur ni sélecteur de tri.
- [ ] Given N lignes, when la section est dépliée, then sa hauteur est en-tête + contenu + 4 px de marge basse, plafonnée à 330 px, et la liste défile au-delà.
- [ ] Given une fenêtre trop basse, when la section prendrait trop de place, then sa hauteur est réduite pour que l'arbre garde au moins 120 px, et l'en-tête reste toujours visible.
- [ ] Given aucune session d'agent dans l'espace actif, when la section se rend, then elle affiche une ligne de 32 px « No agents running » en 13 px `muted` à 15 px du bord.
- [ ] Le rendu n'effectue aucune I/O et l'état de la section vit sur `PaneFlowApp`, pas sur `SidebarView`.

#### US-007: Lignes d'agent avec logo, titre, statut et méta
**Description:** As a développeur multi-agents, I want des lignes d'agent identiques aux lignes d'onglet so that la section se lise comme le reste de la sidebar.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given une ligne, when elle se rend, then elle utilise `sidebar_row_shell` (marge 8, padding 7×6, rayon 9), avec un emplacement de 20 px qui contient le logo du runtime en 15 px, rendu par `render_logo` avec la teinte du runtime (`#D97757` pour Claude Code, `text` sinon).
- [ ] Given le titre, when il se rend, then il prend le titre de l'onglet en 14 px Medium sur 20 px, tronqué avec infobulle au-delà de `SIDEBAR_TITLE_TOOLTIP_MIN_CHARS`.
- [ ] Given l'état, when la ligne se rend, then le statut à droite réutilise `render_lane` : cloche ambre « Input », `x_circle` rouge « Error », point bleu de 7 px « Done », matrice teintée par le runtime pour « Working » avec le titre en shimmer, et rien pour idle.
- [ ] Given la ligne méta, when elle se rend, then elle montre `icons/folder.svg` en 12 px et le nom du workspace en 14 px `muted`, à 3 px d'écart, aligné sur le titre (28 px), avec 3 px d'écart vertical sous la ligne de titre.
- [ ] Given deux rangs consécutifs différents, when ils se suivent, then 8 px supplémentaires les séparent (2 + 8 = 10 px, comme l'espacement de groupe de l'arbre).
- [ ] Given `reduce_motion` actif, when une ligne est « Working », then la matrice reste figée et le titre ne scintille pas.
- [ ] Given un onglet dont le titre est vide, when sa ligne se rend, then elle affiche le nom d'affichage du runtime à la place.
- [ ] Given 50 sessions et 30 workspaces, when `render_sidebar` s'exécute en build release, then le canari de 16 ms (`src-app/src/app/sidebar/mod.rs:223`) ne se déclenche pas.

#### US-008: Clic sur une ligne pour donner le focus au pane
**Description:** As a développeur multi-agents, I want cliquer sur un agent pour arriver dans son pane so that je réponde sans chercher l'onglet.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given une ligne, when l'utilisateur clique, then Paneflow active le workspace, sélectionne l'onglet qui contient la surface de la session et donne le focus à son pane.
- [ ] Given un agent `done` non vu, when sa ligne est cliquée, then `acknowledge_visible_completions` acquitte son résultat, et la ligne passe au rang idle et perd sa pastille Done.
- [ ] Given l'onglet actif du workspace actif, when sa session apparaît dans la section, then sa ligne porte le fond actif (`text` à 0.16), et les autres le fond de survol (`text` à 0.10).
- [ ] Given une session dont la surface n'existe plus au moment du clic, when l'utilisateur clique, then rien ne se passe, aucune panique, et la ligne disparaît au prochain rendu.
- [ ] Given un clic sur une ligne, when il est traité, then aucun octet n'est écrit au PTY de l'agent.

#### US-009: Ligne « No status » pour un agent sans capacité OSC 7501
**Description:** As a développeur multi-agents, I want voir qu'un agent ne déclare pas son état et pourquoi so that je ne le confonde pas avec un agent idle.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-004, US-005, US-007

**Acceptance Criteria:**
- [ ] Given une session sans capacité, when 3 s se sont écoulées depuis son démarrage, then sa ligne affiche « No status » en 10 px Medium `muted` à la place du statut.
- [ ] Given une session sans capacité de moins de 3 s, when elle se rend, then sa ligne n'affiche aucun statut.
- [ ] Given un runtime avec version minimale connue, when l'utilisateur survole « No status », then l'infobulle dit « No OSC 7501 status received. Claude Code reports it from version 2.1.295. » avec le nom et la version du runtime.
- [ ] Given un runtime sans version connue, when l'utilisateur survole, then l'infobulle dit « No OSC 7501 status received from {agent}. »
- [ ] Given une session qui devient capable, when son premier record arrive, then la ligne quitte le rang 5 et affiche son état au rendu suivant.

#### US-010: Repli de la section
**Description:** As a développeur multi-agents, I want replier la section sur son en-tête so that l'arbre reprenne toute la hauteur quand je n'en ai pas besoin.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given la section dépliée, when l'utilisateur clique sur l'en-tête, then seule la ligne d'en-tête de 36 px reste et le chevron pivote de -90° en 180 ms (sans animation sous `reduce_motion`).
- [ ] Given la section repliée, when l'utilisateur clique à nouveau, then elle reprend sa hauteur précédente.
- [ ] Given le bouton d'en-tête, when il est lu par l'accessibilité, then il expose le rôle bouton, le nom « Agents » et son état déplié ou replié.
- [ ] Given la section repliée, when un agent passe à `blocked`, then rien ne se déplie automatiquement.

---

### EP-003: Redimensionnement, persistance et préférence

Rend la hauteur réglable et garde les choix de l'utilisateur entre deux lancements.

**Definition of Done:** la poignée fonctionne à la souris et au clavier, la hauteur et le repli survivent à un redémarrage, et la section s'active ou se désactive depuis Customize sidebar, la palette de commandes et `paneflow.json`.

#### US-011: Poignée de redimensionnement
**Description:** As a développeur multi-agents, I want régler la hauteur de la section so that je choisisse l'équilibre entre l'arbre et les agents.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given le bord haut de la section dépliée, when la souris le survole, then une zone de 9 px affiche le curseur de redimensionnement vertical et un trait de 1 px en `text` à 0.16, invisible au repos.
- [ ] Given un glisser, when l'utilisateur tire, then la hauteur suit le pointeur, bornée entre en-tête + une ligne (95 px) et le plus petit de : 620 px, la hauteur du contenu, la hauteur disponible moins 120 px pour l'arbre.
- [ ] Given un double-clic sur la poignée, when il est traité, then la section revient à la hauteur de son contenu, plafonnée à 330 px.
- [ ] Given le focus clavier sur la poignée, when l'utilisateur appuie sur ↑ ou ↓, then la hauteur change de 16 px. Home et End vont aux bornes, Entrée revient à la hauteur du contenu.
- [ ] Given la section repliée ou vide, when elle se rend, then la poignée est absente.

#### US-012: Persistance de la hauteur et du repli
**Description:** As a développeur multi-agents, I want retrouver la hauteur et l'état replié au relancement so that je ne règle la section qu'une fois.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-010, US-011

**Acceptance Criteria:**
- [ ] Given une hauteur réglée ou un repli, when Paneflow écrit `window-state.json`, then il ajoute des champs optionnels de hauteur (absente = hauteur du contenu) et de repli (absent = déplié), par l'écriture atomique existante (`src-app/src/window_state.rs:85-89`).
- [ ] Given un `window-state.json` écrit par une version antérieure, when il est lu, then le chargement réussit avec les valeurs par défaut.
- [ ] Given une hauteur persistée plus grande que l'espace disponible, when la fenêtre s'ouvre plus petite, then les bornes d'US-011 s'appliquent sans écraser la valeur persistée.
- [ ] Given un fichier corrompu ou au-delà de 4096 octets, when il est lu, then le comportement existant s'applique et la section utilise ses valeurs par défaut.
- [ ] Aucune écriture n'a lieu sur le thread de rendu pendant un glisser : la hauteur s'écrit à la fin du geste.

#### US-013: Préférence `sidebar_show.agents`
**Description:** As a développeur multi-agents, I want masquer la section depuis Customize sidebar so that la sidebar reste celle d'avant si je le préfère.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given `SidebarShow` (`crates/paneflow-config/src/schema/config.rs:197-211`), when un champ optionnel `agents` est ajouté avec son helper `agents_enabled()`, then il vaut vrai par défaut.
- [ ] Given le sous-menu Customize sidebar › Show, when il s'ouvre, then une ligne « Agents » avec `icons/sparkles.svg` en 13 px et une coche apparaît sous Indent guide, séparée par un trait de menu.
- [ ] Given un clic sur cette ligne, when il est traité, then `toggle_sidebar_show` réécrit tout l'objet `sidebar_show` avec la clé `agents`, et la section apparaît ou disparaît sans redémarrage.
- [ ] Given une valeur non booléenne dans `paneflow.json`, when la config est lue, then `lenient_value_or_default` applique vrai, sans erreur bloquante.
- [ ] Given la palette de commandes, when l'utilisateur tape « agents », then une commande d'affichage ou de masquage de la section existe à côté des autres bascules `sidebar_show` (`src-app/src/app/command_palette/catalog.rs:1356-1387`).
- [ ] `schemas/paneflow.schema.json` déclare la clé.

---

### EP-004: Accessibilité, clavier et documentation

Finit la section pour le clavier et les lecteurs d'écran, puis met à jour les contrats écrits.

**Definition of Done:** chaque ligne et chaque contrôle a un nom accessible, la section est atteignable au clavier, et DESIGN.md, ARCHITECTURE.md, la doc des raccourcis et de la configuration ainsi que le CHANGELOG décrivent le comportement livré.

#### US-014: Noms accessibles et navigation clavier
**Description:** As a utilisateur au clavier ou avec un lecteur d'écran, I want parcourir la section et entendre l'agent et son état so that la file soit utilisable sans souris.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008, US-009

**Acceptance Criteria:**
- [ ] Given une ligne, when l'accessibilité la lit, then son nom combine le nom de l'agent, le titre, le workspace et l'état (« Claude Code, Attention sidebar, paneflow, waiting for input »).
- [ ] Given la section, when elle est exposée à l'accessibilité, then elle porte le nom « Agents » et ses lignes un rôle d'élément de liste.
- [ ] Given le curseur clavier de la sidebar (`src-app/src/app/sidebar/keyboard.rs`), when il dépasse la dernière ligne de l'arbre, then il entre dans la section, et Entrée donne le focus au pane comme un clic.
- [ ] Given la section repliée ou masquée, when le curseur clavier se déplace, then il saute la section.
- [ ] Given le curseur sur une ligne, when elle se rend, then elle porte l'anneau de curseur existant (`sidebar_cursor_ring`, `text` à 0.35).

#### US-015: Contrats de design et d'architecture
**Description:** As a mainteneur, I want que DESIGN.md et ARCHITECTURE.md décrivent la section so that les prochaines modifications respectent le contrat validé.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-012, US-013

**Acceptance Criteria:**
- [ ] Given DESIGN.md, when la section sidebar est mise à jour, then elle décrit l'en-tête, les lignes, les rangs, la poignée, les bornes de hauteur, l'état vide et « No status », avec les valeurs de cette PRD.
- [ ] Given ARCHITECTURE.md, when la section sur le cycle de vie des agents est mise à jour, then elle décrit la capacité OSC 7501 (origine, remise à zéro, transport) et rappelle qu'elle ne change aucun état.
- [ ] Given un écart entre le code livré et ces documents, when la story est revue, then le document suit le code.

#### US-016: Documentation utilisateur et CHANGELOG
**Description:** As a utilisateur, I want une documentation à jour du raccourci, de la préférence et de la section so that je comprenne ce qui a changé.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002, US-013

**Acceptance Criteria:**
- [ ] Given `CHANGELOG.md`, when la version qui livre la section est préparée, then une entrée décrit la section, le nouvel ordre de ⌘⇧J, « No status » et `sidebar_show.agents`.
- [ ] Given que `docs/user/` est un miroir du site, when la doc utilisateur change, then les modifications de `features.md`, `keybindings.md` et `configuration/schema.md` sont faites dans paneflow-web puis resynchronisées, sans édition manuelle du miroir.
- [ ] Given la doc des raccourcis, when elle décrit ⌘⇧J, then elle nomme les trois rangs parcourus et leur ordre.

---

## Functional Requirements

- FR-01: La section doit lister chaque session d'agent reconnue (identité issue du shim et du registre de sessions) des workspaces de l'espace actif, une ligne par session. (US-001, US-006)
- FR-02: La section doit ordonner les lignes en six rangs : blocked, error, done non vu, working, idle, sans capacité OSC 7501 ; à rang égal, la plus récente d'abord. (US-001)
- FR-03: L'état d'une ligne doit venir uniquement du statut déclaré OSC 7501 déjà réduit par le worker. Le système ne doit jamais lire l'écran, un hook ou une frappe pour en déduire un état. (US-001, US-004)
- FR-04: Un clic sur une ligne doit activer le workspace, l'onglet et le pane de la session, et acquitter un résultat non lu par le mécanisme existant. (US-008)
- FR-05: ⌘⇧J doit parcourir les rangs blocked, error et done non vu dans l'ordre de la section, sur tous les workspaces. (US-002)
- FR-06: Une session doit être marquée capable OSC 7501 dès que l'hôte répond à sa requête de support ou applique son premier record ; la capacité repart à faux à chaque nouveau processus d'agent. (US-003, US-004)
- FR-07: Une session non capable depuis au moins 3 s doit afficher « No status », avec une infobulle qui nomme la version minimale quand le catalogue la connaît. (US-005, US-009)
- FR-08: La hauteur de la section doit suivre son contenu jusqu'à 330 px, se régler entre 95 et 620 px en laissant au moins 120 px à l'arbre, et persister avec l'état replié. (US-006, US-011, US-012)
- FR-09: La préférence `sidebar_show.agents`, vraie par défaut, doit afficher ou masquer la section sans redémarrage. (US-013)
- FR-10: Le système ne doit pas afficher les champs `msg`, `title` ni `app` d'un record OSC 7501 dans la section. (Non-Goals)

## Non-Functional Requirements

- **Performance:** `render_sidebar` reste sous 16 ms (canari existant) avec 30 workspaces et 50 sessions d'agents en build release. Le tri est en O(n log n) pour n ≤ 256 sessions. Le spinner et le shimmer de la section passent par `SpinnerClock`, sans nouvelle horloge, et une section sans agent « Working » ne programme aucun réveil d'animation.
- **Security:** zéro octet écrit au PTY par la section. L'identité affichée vient du runtime reconnu, jamais du champ `app` déclaré. Aucun champ texte d'un record n'est rendu dans la section, donc aucune nouvelle surface d'injection.
- **Accessibility:** tous les contrôles de la section (en-tête, lignes, poignée) ont un nom accessible. La section est atteignable au clavier par le curseur de la sidebar, et ⌘⇧J fonctionne quelle que soit sa visibilité. Les animations s'arrêtent sous `reduce_motion`. L'état est toujours porté par une forme et un mot, jamais par la couleur seule.
- **Scalability:** jusqu'à 256 sessions listées, avec défilement au-delà de la hauteur de la section ; 1 à 30 workspaces.
- **Reliability:** passer de « No status » à un état ne prend qu'un rendu après réception du premier record. Le délai de grâce est de 3 s. Une `window-state.json` illisible ne bloque jamais le démarrage. Un worker déconnecté laisse les lignes à leur dernier état connu, comme l'arbre.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Aucun agent | Espace actif sans session d'agent | Une ligne d'état vide de 32 px, pas de poignée | "No agents running" |
| 2 | Démarrage d'un agent | Session de moins de 3 s sans capacité | Aucun statut, pas de « No status » | None |
| 3 | Agent muet | Claude Code < 2.1.295, agent sans OSC 7501, tmux dans le pane | « No status » après 3 s, au rang 5 | "No OSC 7501 status received. Claude Code reports it from version 2.1.295." |
| 4 | Agent qui plante en `blocked` | Le processus meurt | L'hôte libère le record à la sortie ou sur OSC 133 A, la session disparaît ou passe à idle | None |
| 5 | Agent figé sans shell integration | Le processus vit encore mais ne répond plus | Le dernier état déclaré reste affiché, limite assumée de la spec | None |
| 6 | Plusieurs agents dans un même onglet | Panes séparés dans un onglet | Une ligne par session ; le titre de l'onglet se répète et le logo les distingue | None |
| 7 | Fenêtre très basse | Hauteur disponible < en-tête + ligne + 120 px | La section se réduit, l'arbre garde 120 px, l'en-tête reste visible | None |
| 8 | Surface disparue pendant le clic | L'onglet se ferme entre rendu et clic | Clic ignoré, ligne retirée au rendu suivant | None |
| 9 | Programme non agent qui déclare un état | Un script émet `state=blocked:app=deploy` | Absent de la section ; chip d'en-tête de pane et notification existants | None |
| 10 | Usurpation d'identité | Un programme émet `app=claude-code` | L'identité vient du runtime reconnu ; un pane sans agent reconnu n'apparaît pas | None |
| 11 | Agent urgent dans un espace masqué | Session bloquée hors espace actif | Absent de la section, signalé par la marque d'attention d'espace existante, atteint par ⌘⇧J | None |
| 12 | Fichier d'état ancien ou corrompu | `window-state.json` sans les nouveaux champs ou illisible | Valeurs par défaut : hauteur du contenu, dépliée | None |
| 13 | Rafale de records | Plus de 256 KiB d'événements en attente | Comportement hôte existant : records vidés, avertissement journalisé, capacité conservée | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Peu d'agents émettent OSC 7501 au lancement ; seul Claude Code ≥ 2.1.295 est confirmé | High | Med | « No status » rend le manque visible ; Arthur assume le pari d'adoption |
| 2 | La spec r0.4 change encore (4 révisions en 2 semaines) | Med | Med | Épinglage sur r0.4 ; la section ne lit que l'état réduit par le worker, pas le format brut |
| 3 | La réponse à la requête n'est pas reconnaissable de façon fiable | Low | High | Spike US-003 avant tout code ; repli documenté sur la capacité au premier record seulement |
| 4 | Le champ de capacité exige une montée de version du protocole hôte | Med | Low | Négociation `core_protocol` existante ; l'hôte s'arrête avant une mise à jour faite depuis l'app |
| 5 | Le rendu de 50 lignes dépasse le budget de 16 ms | Low | Med | Critère de perf dans US-007, mesure par le canari et `work_counters::SIDEBAR_RENDERS` |
| 6 | Confusion avec le « dock Agents » du PRD navigateur | Low | Low | Nom « section Agents de la sidebar » dans le code et la doc ; libellé d'interface « Agents » validé |
| 7 | Le titre d'onglet ne suffit pas quand un onglet porte plusieurs agents | Med | Low | Logo et ligne méta ; question ouverte pour un titre par surface |

## Non-Goals

What this version explicitly does NOT include:

- Aucune lecture d'écran, aucun hook et aucune heuristique pour l'état, même en dépannage. OSC 7501 reste la seule source (`ARCHITECTURE.md:363`).
- Pas d'affichage de `msg`, `kind`, `progress` ni `title` des records dans les lignes. La maquette validée ne les montre pas ; l'en-tête de pane et les notifications les portent déjà.
- Pas de compteurs, de légende ni de mode de tri dans l'en-tête. Arthur les a retirés de la maquette.
- Pas de liste des agents des autres espaces dans la section ; la marque d'attention d'espace couvre ce cas.
- Pas de sondage de version d'agent par sous-processus (`claude --version`) pour expliquer « No status ».
- Pas de prise en charge d'OSC 7501 derrière un tmux interne ; limite assumée.
- Pas de PR d'adoption OSC 7501 vers des agents tiers ; décision d'Arthur.

## Files NOT to Modify

- `native/libghostty/**` et les bindings de `crates/paneflow-terminal-ghostty/src/callback_ffi.rs` : ABI épinglée, la détection se fait côté hôte.
- `crates/paneflow-host/src/program_status.rs`, pour ses limites et sa durée de vie : conformes à la spec r0.4. La capacité s'ajoute à côté, sans changer `acknowledge_seen` ni `release_finished_program`.
- `crates/paneflow-shim/**`, `crates/paneflow-ai-hook/**`, `crates/paneflow-mcp/**` : budgets de taille, et aucun rôle dans l'état.
- `src-app/Cargo.toml`, pour les quatre `rev` GPUI : épinglage exact.
- `src-app/src/app/attention.rs`, pour la sémantique de la marque d'attention d'espace : elle reste la vue des espaces masqués.
- `docs/user/**` à la main : miroir synchronisé depuis paneflow-web.
- `tasks/prd-agents-browser.md` et son statut : autre PRD, autre « dock ».

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** où placer la fonction de tri ? Recommandé : un module de la sidebar (par exemple `sidebar/agents.rs`) qui réutilise `sidebar_agent_summary` pour les rangs, et que `jump_next_session_where` appelle aussi. Engineering to confirm qu'aucun cycle de modules n'apparaît.
- **Data Model:** la capacité doit-elle vivre dans `ProgramStatus` ou dans `runtime.rs` à côté de `drain_engine_events` ? Recommandé : un drapeau de runtime levé quand un `WritePty` commence par `ESC ] 7501 ; ?` ou quand `apply` accepte un record, remis à zéro à chaque génération de runtime. Trade-off : reconnaître le préfixe sur les octets écrits plutôt que patcher libghostty.
- **API Design:** nom du champ sur le fil hôte, worker et app (`program_status_capable` ?), et montée de version de `core_protocol`. Recommandé : un booléen optionnel, absent = faux, pour garder la lecture tolérante.
- **Catalogue:** nom du champ de version minimale dans `runtime.toml` (`display.status_since` ou une section `[status]`). Recommandé : un champ optionnel sous `display`, généré dans `RuntimeDisplay`.
- **Interaction:** existe-t-il déjà une poignée de redimensionnement réutilisable, par exemple dans le diff dock ? Sinon, une poignée GPUI avec `on_mouse_down`, `on_mouse_move` et capture, et un rôle d'accessibilité de séparateur si l'API AccessKit de GPUI l'expose.
- **Focus:** quel chemin d'activation pour une surface qui n'est pas le terminal actif de son pane ? `jump_next_session_where` ne voit que `active_terminal_opt()` (`focus.rs:172-178`). Recommandé : la sélection d'onglet existante (`select_workspace_tab`) puis le focus du pane.
- **Migration:** `window-state.json` gagne des champs optionnels, lecture rétrocompatible, pas de migration. Rollback : une version antérieure ignore les champs inconnus. Engineering to confirm que serde ne refuse pas les champs inconnus dans `PersistedWindowSize`.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Gestes pour atteindre l'agent le plus urgent de l'espace actif | Défilement de l'arbre + clic, ou ⌘⇧J limité à `WaitingForInput` | 1 clic ou 1 ⌘⇧J pour blocked, error et done non vu | Month-1 | Tests d'US-002 et passe manuelle d'Arthur |
| Sessions muettes signalées comme telles | 0 % (rien n'est affiché) | 100 % après 3 s | Month-1 | Test d'intégration d'US-004 et US-009 |
| Temps de `render_sidebar` avec 30 workspaces et 50 sessions | N/A (nouveau) | < 16 ms, aucun déclenchement du canari | Month-1 | Trace `paneflow_app::sidebar` en build release |
| Lectures d'écran pour l'état | 0 | 0 | Month-6 | Revue de code et test de garde existant |
| Agents de la section avec un état déclaré | Claude Code ≥ 2.1.295 seulement | Au moins 3 runtimes du catalogue | Month-6 | Revue du catalogue et de leurs changelogs |

## Open Questions

- Un agent `done` vu doit-il garder le libellé « Done » au rang idle, ou le perdre comme dans l'arbre ? Ce PRD le fait disparaître, conformément à la maquette validée et à l'arbre. Arthur, avant US-008.
- La section doit-elle couvrir tous les espaces plutôt que l'espace actif ? Ce PRD retient l'espace actif, par cohérence avec l'arbre. Arthur, avant US-001.
- Quand un onglet porte plusieurs agents, faut-il un titre par surface plutôt que le titre d'onglet ? Arthur, avant US-007.
- Quelles versions de Codex, Grok et des autres runtimes ajouter au catalogue une fois publiées ? À suivre dans leurs changelogs ; US-005 ne fournit que Claude Code.
[/PRD]
