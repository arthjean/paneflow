[PRD]
# PRD: Expérience fichiers du dock (arbre, onglets, affichage, liens agents)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-18 | Arthur Jean | PRD initial, issu de la recherche du 2026-11-18 sur t3code (`52f0f8af`) et Zed (`5b282ac8bb`) : panneau Files du dock, icônes et mesure de l'arbre, éditeur (retour à la ligne, recherche, révélation de ligne), aperçus, liens fichiers-agents par le terminal. 6 epics, 37 stories, 3 phases. |
| 1.1 | 2026-11-18 | Arthur Jean | Chaque story reçoit une ligne « Référence t3code » vers le code source de t3code (`52f0f8af`) et des paquets `@pierre/*` installés, chemins vérifiés un par un. US-032 cite aussi la technique de bloc de Zed. |
| 1.2 | 2026-11-18 | Arthur Jean | Mesure US-016 : ligne de base `bench/baselines/linux-x86_64/tree.json` enregistrée par `scripts/bench-tree.sh --set-baseline` sur `f69413df`, arbre propre, Ryzen 7 7800X3D, part CPU 0,99 ([résultat](../bench/results/tree-20261118T182030Z-f69413dfc269.json)). 50 000 entrées : lignes après dépliage 19,57 ms (27,65 Mio alloués), 1 000 ajouts et suppressions 37,12 ms (38,17 Mio), recherche 2,41 ms. 300 000 entrées : 164,81 ms (173,04 Mio), 267,52 ms (232,48 Mio), 14,12 ms. Ces calculs tournent hors du fil de rendu, mais la construction des lignes dépasse 16 ms sur 50 000 entrées : **décision, porter** l'index de lignes visibles par nœud et l'application par différence de t3code. Story de suivi US-038 ajoutée au PRD, dans le nouvel EP-007. |
| 1.3 | 2026-11-18 | Arthur Jean | Revue d'EP-002. Deux défauts corrigés. (1) `sync_files_sidebar_session`, appelé à chaque rendu du dock, relançait la révélation d'US-012 dès que la sélection quittait le fichier actif : la recherche était vidée à la frappe suivante et un dossier parent replié se redépliait. La révélation ne se déclenche plus qu'au changement de fichier actif (`FilesSidebar::reveal_request`). (2) La recherche fusionnait deux dossiers qui ne diffèrent que par la casse (`Docs/`, `docs/`) et donnait un chemin faux aux fichiers du second. (3) `tree.json` manquait à la liste des lignes de base connues de `src-app/src/bench_harness.rs`, ce qui faisait échouer `every_committed_baseline_is_clean_current_and_on_its_platform` ; le test valide maintenant son schéma, son arbre propre et sa plateforme. US-015 et US-016 certifiées. Passe visuelle d'US-009 à US-014 sur un build debug du `PANEFLOW_HOME` `~/.paneflow-dev-files`, dans un Xvfb privé avec openbox et lavapipe, piloté par agent-desktop et xdotool, sur un dépôt de test (`.gitignore`, fichiers à point, dossier illisible, `Docs/` et `docs/`, chemin de 14 niveaux) : barre dans l'ordre, rotation de « Refresh files » jusqu'à la fin de la relecture, « Expand all » sans les dossiers ignorés puis « Collapse all », indicateur de chargement tournant puis statique sous `reduce_motion`, « Unable to load folder. » sur un dossier illisible ; ignorés atténués et ouvrables, fichiers à point normaux, `.git` masqué ; recherche dans un dossier jamais déplié avec parents dépliés et surlignage, « No matching files. », « More matches available. Refine your search. », Échap, `.gitignore` respecté ; révélation au changement d'onglet sans vol de focus, fichier ignoré jamais révélé, et le correctif (1) confirmé à l'écran (recherche et repli conservés) ; menu de l'arbre, « Reveal in File Manager » sélectionnant le fichier par `FileManager1`, toast d'échec du repli `xdg-open` ; icônes claires et sombres en Paneflow Dark, Paneflow Light et Claude Light, fenêtre de 800 par 500. Question ouverte d'US-014 tranchée : les icônes de Pierre remplacent celles de Paneflow quand un équivalent existe. EP-002 certifié. Constat hors epic : sur 14 niveaux, le retrait de 18 px dépasse la largeur par défaut de l'arbre et masque le nom du fichier. |

## Problem Statement

**Convention de référence.** Les chemins désignent Paneflow au commit `8650e3c3`. La référence produit est t3code au commit `52f0f8af`. Chaque story porte une ligne « Référence t3code » : chemins relatifs à `/home/arthur/dev/t3code`, numéros de ligne au commit `52f0f8af` ; les paquets `@pierre/*` sont les copies installées sous `node_modules/.pnpm/`. Les constats viennent de `tasks/research-t3code-files-experience.md`, une note locale non suivie par git : chaque constat repris ici cite le fichier du dépôt qui le prouve.

1. **Le dock ne permet pas de parcourir un projet sans ouvrir un fichier.** L'arbre n'apparaît que lorsqu'un onglet de fichier est actif (`src-app/src/app/diff_dock/mod.rs:558`) et se ferme avec le dernier (`src-app/src/app/diff_dock/tabs.rs:271-279`). Sa largeur est fixe, 250 px (`src-app/src/app/files_sidebar/mod.rs:20`). Le filtre ne cherche que dans les dossiers déjà dépliés (`src-app/src/app/files_sidebar/filter.rs:25`). Le fil d'Ariane est statique (`src-app/src/app/diff_dock/file_chrome.rs:55-110`).
2. **Les onglets de fichier sont plafonnés et volatils.** 8 onglets au plus, le plus ancien fermé d'office (`src-app/src/app/diff_dock/model.rs:22`, `tabs.rs:308-319`). Aucun clic droit ni clic milieu sur un onglet (`src-app/src/app/diff_dock/render.rs:303-338`). Rien n'est restauré au redémarrage : `TabSession` ne porte aucun état du dock (`crates/paneflow-config/src/schema/session.rs:180-195`).
3. **La plupart des types de fichier affichent l'icône générique.** 19 icônes, 3 noms exacts et la dernière extension seulement (`src-app/src/file_icons.rs:1-50`). `.js`, `.yaml`, `.html`, `.c`, `.java` et `.sh` retombent sur `file.svg`.
4. **L'éditeur manque des fonctions de lecture de base.** Ni retour à la ligne, ni recherche dans le fichier, ni saut à une ligne : aucune action dans `src-app/src/app/diff_dock/code/view/keymap.rs:5-45`. Un fichier de plus de 10 Mio est refusé (`src-app/src/app/diff_dock/code/load.rs:12`).
5. **Soixante extensions sont refusées au lieu d'être affichées.** Images, archives, polices, bases de données (`src-app/src/app/files_tree.rs:51-57`). Le Markdown rendu n'existe que hors du dock, ouvert depuis un terminal.
6. **Les fichiers et les agents ne se parlent pas.** Un chemin de code cliqué dans un terminal part vers l'éditeur externe (`src-app/src/app/event_handlers/mod.rs:626-635`). Aucun glisser depuis l'arbre (aucun `on_drag` dans `src-app/src/app/files_sidebar/`). Aucun moyen de commenter une ligne pour un agent.

**Why now:** t3code livre cette expérience et Arthur la juge excellente sur le rendu, le défilement et les liens avec les agents. Paneflow se positionne comme un cockpit pour agents : relire ce qu'un agent vient d'écrire, puis lui répondre sur une ligne précise, est le geste central, et il oblige aujourd'hui à sortir vers un éditeur externe. L'éditeur du dock, sa synchronisation disque et sa détection de conflit existent déjà : la base est prête.

## Overview

Le dock gagne une vraie surface Files. Un onglet « Files » montre l'arbre sur toute la largeur du dock ; dès qu'un fichier est ouvert, l'arbre reste à droite du contenu, redimensionnable de 160 à 560 px. Les onglets de fichier n'ont plus de plafond, se ferment au clic milieu, ont un menu contextuel, se rouvrent et reviennent au redémarrage. Le fil d'Ariane ouvre le contenu de chaque dossier. L'arbre garde son interface (lignes de 28 px, lettres git, chevrons de dossier) et reçoit une barre d'outils, une recherche sur tout le projet, la révélation du fichier actif, environ 75 icônes de type avec variantes claire et sombre, et un banc de mesure.

L'éditeur reçoit une table de correspondance entre lignes et positions verticales, qui remplace l'hypothèse « une ligne = une rangée de 18 px ». Elle rend possibles le retour à la ligne et le widget de commentaire sous une ligne. S'y ajoutent la recherche et le remplacement, la révélation d'une plage de lignes et la mémoire de défilement. Un aiguillage par type remplace la liste d'extensions refusées : images, tableaux CSV, Markdown rendu, archives, très gros fichiers texte, et un écran « pas d'aperçu » avec des actions.

Les liens avec les agents passent par le terminal, décision d'Arthur du 2026-11-18. Un chemin ou un commentaire de ligne est collé pré-rempli dans le terminal de l'agent et n'est jamais validé à la place de l'utilisateur. ACP est reporté. Pour le moteur de coloration, ce PRD ne livre qu'un prototype mesuré du moteur lexical de Pierre (`@pierre/highlights`, Apache-2.0) derrière le banc de l'éditeur ; le choix entre réécriture native en Rust et binaire embarqué se fera sur ces mesures.

Le travail est découpé en trois phases. Phase 1 : EP-001 et EP-002 (dock et arbre). Phase 2 : EP-003 et EP-004 (éditeur et aperçus). Phase 3 : EP-005 (liens agents) et EP-007 (projection incrémentale de l'arbre), puis EP-006, la validation sur matériel réel. Chaque epic se construit et se vérifie sur Fedora ; les contrôles Windows sont regroupés dans EP-006.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Parcourir un projet sans ouvrir de fichier | L'onglet Files montre l'arbre sur 100 % de la largeur du dock, et la recherche trouve un fichier d'un dossier jamais déplié | Inchangé |
| Lever le plafond d'onglets | 0 fermeture d'office ; 50 onglets ouverts restaurés au redémarrage | Inchangé |
| Reconnaître les types de fichier | Au moins 75 icônes ; `.js`, `.yaml`, `.html`, `.c`, `.java`, `.sh` ont chacune une icône propre | Inchangé |
| Afficher au lieu de refuser | Les 60 extensions refusées tombent à 0 : chacune a une visionneuse ou l'écran « pas d'aperçu » avec actions | Inchangé |
| Répondre à un agent sur une ligne | Commentaire de plage collé dans le terminal de l'agent en 3 gestes au plus (plage, texte, envoi) | Inchangé |
| Décider du moteur de coloration sur mesure | Rapport de mesure du prototype sur la machine de référence, même corpus que `bench/baselines/linux-x86_64/editor.json` | Moteur retenu intégré (PRD suivant) |

## Target Users

### Développeur qui supervise des agents
- **Role:** Arthur et les utilisateurs de Paneflow qui font tourner Claude Code, Codex ou OpenCode dans des panneaux et relisent ce que les agents écrivent.
- **Behaviors:** ouvre le dock pour lire un fichier modifié, suit les chemins `fichier:ligne` affichés par l'agent, répond à l'agent dans son terminal.
- **Pain points:** le chemin cliqué part dans un autre éditeur ; pour pointer une ligne à l'agent, il recopie le chemin et le numéro à la main ; l'arbre disparaît quand le dernier fichier est fermé.
- **Current workaround:** un éditeur externe ouvert à côté, et des chemins tapés ou glissés depuis le gestionnaire de fichiers du système.
- **Success looks like:** cliquer le chemin, lire la ligne dans le dock, commenter la plage, retrouver le commentaire pré-rempli dans le terminal de l'agent.

### Développeur qui explore un dépôt inconnu
- **Role:** utilisateur qui découvre un projet avant de lancer un agent dessus.
- **Behaviors:** déplie l'arbre, cherche un fichier par son nom, ouvre dix à trente fichiers, y compris des images et des CSV.
- **Pain points:** recherche limitée aux dossiers dépliés, 8 onglets au plus, icône générique partout, images refusées.
- **Current workaround:** `find` ou `rg --files` dans un terminal, éditeur externe pour le reste.
- **Success looks like:** chercher dans tout le projet depuis l'arbre, garder ses onglets au redémarrage, reconnaître un type de fichier à son icône.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **t3code** : arbre `@pierre/trees` à droite du contenu, un onglet par fichier sans plafond, recherche par index serveur avec anti-rebond de 120 ms et 200 résultats, fil d'Ariane avec menu de dossier. Son arbre n'a pas d'icône de dossier et ne surveille pas le disque ; son éditeur enregistre sans contrôle de conflit. Paneflow garde sa surveillance du disque et sa bannière de conflit.
- **Zed** : révèle le fichier actif dans le panneau de projet et n'auto-révèle jamais une entrée ignorée par git ([docs](https://zed.dev/docs/project-panel)). Les commentaires de ligne existent derrière un drapeau désactivé, et l'action `SendReviewToAgent` n'a aucun gestionnaire (`crates/editor/src/actions.rs:942`) : aucun format à reprendre. Sa technique de bloc sous une ligne (`crates/editor/src/display_map/block_map.rs:159-336`) est le modèle du widget de commentaire.
- **CLI d'agents** : `@chemin/relatif` est compris par Claude Code, Codex CLI et Gemini CLI ; le suffixe `#L5-10` n'est documenté que pour Claude Code ([docs VS Code de Claude Code](https://code.claude.com/docs/en/vs-code), [Gemini CLI](https://geminicli.com/docs/cli/tutorials/file-management/)).
- **Market gap:** aucun de ces produits ne permet de commenter une plage de lignes et de l'envoyer à un agent CLI qui tourne dans un terminal, quel que soit le CLI.

### Best Practices Applied
- Ne pas surveiller récursivement les dossiers lourds : l'arbre garde une surveillance par dossier chargé, et « tout déplier » est plafonné ([wiki File Watcher de VS Code](https://www.github.com/microsoft/vscode/wiki/File-Watcher-Issues)).
- Décoder hors du fil d'interface avec des plafonds explicites : `image::Limits` borne largeur, hauteur et allocation, mais certains décodeurs ignorent `max_alloc`, donc les dimensions sont contrôlées avant décodage (documentation du crate `image`).
- Retirer la séquence de fin de collage encadré `ESC[201~` de tout texte inséré dans un terminal, pour qu'un contenu de fichier ne puisse pas sortir du collage et s'exécuter.
- Les mesures du moteur de Pierre (320 Mio/s contre 9 Mio/s pour `tree-sitter-highlight`, frappe en 0,75 µs) viennent de son propre banc sur Apple M4 Pro ([benchmark README](https://github.com/pierrecomputer/pierre/blob/main/packages/highlights/benchmark/README.md)) : elles ne valent pas décision sans mesure sur la machine de référence de Paneflow.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- Le parcours de projet du sélecteur de chemins (`src-app/src/terminal/path_picker/index.rs:325-366`, 100 000 entrées) est réutilisable hors du terminal après un changement de visibilité. Déduit de la lecture du module, non essayé.
- Une table lignes-positions peut remplacer les sites qui supposent une rangée de 18 px sans régression mesurable de la frappe. Validé par US-017 sur le banc.
- Le binaire WebAssembly de Pierre s'exécute dans un moteur Rust avec ses deux fonctions d'accueil. Validé par US-022. wasmi n'a pas pu être documenté ; le prototype part sur wasmtime.
- `@chemin` est compris par Claude Code, Codex CLI et Gemini CLI. Non vérifié pour OpenCode, Pi et Hermes : ils reçoivent le chemin simple.
- Un dépôt GPUI accepte une charge utile de glisser propre à l'application sur une vue terminal, comme le font déjà `TabDrag` et `SurfaceDrag` (`src-app/src/pane.rs:2210-2231`).

### Hard Constraints
- Le fil de rendu ne bloque jamais : lecture de dossier, parcours de projet, décodage d'image, index de lignes, liste d'archive et processus externes passent par `smol::unblock` ou l'exécuteur d'arrière-plan (AGENTS.md).
- Linux, macOS et Windows : toute fonction a un chemin sur les trois, au minimum un repli documenté.
- Aucun commentaire dans le code source. Dans un fichier qui contient un `mod tests`, tout nouvel élément est déclaré avant ce module.
- Aucune assertion de durée dans un test unitaire ; les budgets de latence vivent dans les bancs et les perf gates.
- Texte envoyé à un agent : pré-rempli, jamais validé automatiquement.
- Licence GPL-3.0-or-later : les icônes et tables reprises (Apache-2.0, ISC, CC0) sont attribuées.
- Les binaires d'aide plafonnés (`paneflow-shim`, `paneflow-ai-hook`, `paneflow-mcp`) ne reçoivent aucune dépendance nouvelle.
- Le texte d'interface est en anglais ; les libellés entre guillemets ci-dessous sont à reprendre tels quels.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - format, obligatoire avant chaque commit
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lint, cibles de test incluses
- `cargo nextest run --workspace --locked --profile ci` - suite de tests de la CI
- `bash scripts/check-test-wallclock.sh` - aucune nouvelle assertion de durée dans les tests
- `cargo deny check advisories licenses sources` - seulement quand une dépendance change
- `bash scripts/check-helper-sizes.sh` - après `cargo nextest run`, seulement quand une dépendance change
- `scripts/bench-editor.sh` - seulement pour les stories qui touchent `src-app/src/app/diff_dock/code/`

Pour les stories d'interface, en plus :
- Passe visuelle sur un build debug lancé par `scripts/dev.sh --tag files`, selon la Delivery Gate de `DESIGN.md` (section 9) : Paneflow Dark et Paneflow Light plus un thème tiers, `reduce_motion` activé, fenêtre de 800 par 500, chemins longs, et les états survol, actif, sélectionné, désactivé, chargement, vide et erreur de chaque contrôle touché.
- `DESIGN.md` mis à jour dans le même commit quand une géométrie ou un contrat de composant change.

## Epics & User Stories

### EP-001: Dock Files : surface, onglets et fil d'Ariane

Phase 1. Le dock devient un endroit où l'on parcourt un projet : un onglet Files autonome, un arbre redimensionnable, des onglets sans plafond qui survivent au redémarrage, un fil d'Ariane navigable.

**Definition of Done:** l'onglet Files montre l'arbre sans fichier ouvert ; 50 onglets de fichier s'ouvrent, se ferment en lot, se rouvrent et sont restaurés ; le fil d'Ariane ouvre un fichier voisin en deux clics ; `DESIGN.md` décrit les nouvelles géométries.

#### US-001: L'onglet Files montre l'arbre sur toute la largeur
**Description:** As a développeur qui explore un dépôt, I want un onglet « Files » qui affiche l'arbre sans fichier ouvert so that je parcours le projet avant de choisir quoi lire.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/components/files/FilePreviewPanel.tsx` (corps et explorateur, 1589-1821), `apps/web/src/components/files/filePreviewMode.ts:40-49` (quand l'explorateur s'affiche), `apps/web/src/components/files/FileExplorerPane.tsx`, `apps/web/src/rightPanelStore.ts:916-976` (`openFile`, `navigateFolder`, surface `files` unique), `apps/web/src/components/chat/RightPanelSurfaceContent.tsx:242-277`

**Acceptance Criteria:**
- [x] Given un dock sans onglet de fichier, when je choisis « File » dans le sélecteur de surface ou que j'exécute `diff_new_file_tab`, then un onglet intitulé « Files » s'ouvre et l'arbre occupe toute la largeur utile du dock
- [x] Given l'onglet Files actif, when j'ouvre un fichier depuis l'arbre, then l'onglet Files est remplacé par l'onglet de ce fichier et l'arbre passe à droite du contenu
- [x] Given le dernier onglet de fichier fermé, when le dock reste ouvert, then l'onglet actif suivant s'affiche et l'arbre n'occupe plus d'espace
- [x] Given un espace de travail sans dossier racine lisible, when j'ouvre l'onglet Files, then l'arbre affiche « Unable to load folder. » avec un bouton « Retry » et rien d'autre ne change dans le dock
- [x] Un seul onglet Files existe par dock : rouvrir la commande active l'onglet existant

#### US-002: Largeur de l'arbre réglable et mémorisée
**Description:** As a développeur qui supervise des agents, I want régler la largeur de l'arbre so that les noms longs restent lisibles sans voler la place du code.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001
**Référence t3code :** `apps/web/src/components/files/FileExplorerPane.tsx:9-115` (160, 560, 256, réserve de 256 px pour le contenu, clavier 88-110), `apps/web/src/components/preview/RightPanelResizeHandle.tsx:14-33`, `apps/web/src/hooks/useResizableWidth.ts:38-67`

**Acceptance Criteria:**
- [x] Given un fichier ouvert, when je fais glisser le bord gauche de l'arbre, then sa largeur suit le pointeur entre 160 et 560 px, sans jamais laisser moins de 256 px au contenu ; si le dock ne peut pas offrir les deux, l'arbre garde 160 px
- [x] La largeur par défaut est 256 px
- [x] Given une largeur réglée, when je redémarre Paneflow, then l'arbre retrouve cette largeur, lue dans `window-state.json`
- [x] Given la poignée au focus clavier, when j'appuie sur Flèche gauche ou droite, then la largeur change de 16 px ; Début et Fin vont au minimum et au maximum
- [x] Given une valeur hors bornes ou non numérique dans `window-state.json`, when Paneflow démarre, then la largeur par défaut s'applique et le fichier reste lisible
- [x] La zone de saisie de la poignée fait 7 px et le trait visible 1 px, comme la poignée du dock (`src-app/src/app/diff_dock/render.rs:48-76`)

#### US-003: Aucun plafond d'onglets de fichier
**Description:** As a développeur qui explore un dépôt, I want ouvrir autant de fichiers que je veux so that Paneflow ne ferme jamais un onglet à ma place.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/rightPanelStore.ts:916-950` (un onglet par fichier, réutilisation de l'onglet existant, aucun plafond)

**Acceptance Criteria:**
- [x] Given 8 onglets de fichier ouverts, when j'en ouvre un neuvième, then les 9 restent ouverts
- [x] `MAX_DIFF_FILE_TABS` et la fermeture du plus ancien onglet n'existent plus dans le code
- [x] Given 50 onglets ouverts, when j'active le dernier, then la bande d'onglets défile pour le montrer en entier
- [x] Given un fichier déjà ouvert, when je le rouvre, then son onglet existant est activé et aucun doublon n'apparaît
- [x] La ligne « 8 file tabs maximum » est retirée de `DESIGN.md`

#### US-004: Menu contextuel d'onglet et clic milieu
**Description:** As a développeur qui explore un dépôt, I want fermer des onglets en lot so that je range le dock après une exploration.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003
**Référence t3code :** `apps/web/src/components/RightPanelTabs.tsx:1252-1343` (menu d'onglet, clic milieu), `apps/web/src/components/chat/useRightPanelActions.ts:258-283` (copie du chemin)

**Acceptance Criteria:**
- [x] Given un clic droit sur un onglet de fichier, when le menu s'ouvre, then il liste « Copy Path », « Copy Relative Path », « Close », « Close Others », « Close to the Right », « Close All »
- [x] Given un clic milieu sur un onglet de fichier sans modification, when je relâche, then l'onglet se ferme
- [x] Given une fermeture en lot qui vise des onglets modifiés non enregistrés, when elle s'exécute, then les onglets enregistrés se ferment, les onglets modifiés restent et un toast indique « N modified tabs kept »
- [x] Given un clic milieu sur un onglet modifié, when je relâche, then la règle existante de double confirmation s'applique (`tabs.rs:328-354`)
- [x] Les onglets Changes et Terminal ne sont jamais fermés par « Close Others », « Close to the Right » ou « Close All »

#### US-005: Rouvrir un onglet fermé
**Description:** As a développeur qui explore un dépôt, I want rouvrir le dernier onglet fermé so that une fermeture par erreur ne coûte rien.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003
**Référence t3code :** `apps/web/src/rightPanelStore.ts:1322-1346`, `apps/web/src/closedViewStore.ts`, `apps/web/src/reopenClosedView.ts`, `apps/web/src/components/ReopenClosedViewShortcut.tsx`

**Acceptance Criteria:**
- [x] Given un onglet de fichier fermé, when je choisis « Reopen Closed Tab » dans le menu contextuel de la bande d'onglets ou dans le menu de nouvel onglet, then le fichier se rouvre à la même position de défilement
- [x] L'historique garde les 20 dernières fermetures par dock, dans l'ordre inverse de fermeture
- [x] Given un historique vide, when le menu s'ouvre, then l'entrée est désactivée
- [x] Given un fichier supprimé depuis sa fermeture, when je le rouvre, then l'entrée est retirée de l'historique et un toast indique « File no longer exists »

#### US-006: Onglets de fichier restaurés au redémarrage
**Description:** As a développeur qui supervise des agents, I want retrouver mes onglets de fichier après un redémarrage so that une mise à jour ne me fait pas perdre mon contexte de lecture.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003
**Référence t3code :** `apps/web/src/rightPanelStore.ts:101-102` (clé de stockage), `:551-718` (migration validée), `:1291-1318` (persistance)

**Acceptance Criteria:**
- [x] Given des onglets de fichier ouverts, when Paneflow redémarre, then chaque dock retrouve ses onglets dans le même ordre, avec le même onglet actif
- [x] Given 50 onglets restaurés, when Paneflow démarre, then seul le fichier de l'onglet actif est lu ; les autres sont lus à leur première activation
- [x] Given un fichier restauré qui n'existe plus, when la session se charge, then son onglet est omis sans message et les autres sont restaurés
- [x] Given un `session.json` écrit par une version antérieure, when il est chargé, then la session s'ouvre sans erreur et sans onglet de fichier
- [x] Le contenu modifié non enregistré n'est pas restauré : l'onglet rouvre le contenu du disque
- [x] `scripts/bench-startup.sh` ne montre aucune régression au-delà de 5 % avec 50 onglets à restaurer

#### US-007: Fil d'Ariane navigable
**Description:** As a développeur qui explore un dépôt, I want ouvrir le contenu d'un dossier depuis le fil d'Ariane so that j'atteins un fichier voisin sans chercher dans l'arbre.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001
**Référence t3code :** `apps/web/src/components/files/FileBreadcrumbs.tsx` (menus de dossier 231-279, retour 103-133, états 135-226), `apps/web/src/components/files/FilePreviewPanel.tsx:1417-1425` (défilement du dernier segment), `apps/web/src/rightPanelStore.ts:951-976` (`folderTrail`)

**Acceptance Criteria:**
- [x] Given un fichier ouvert, when je clique un segment de dossier, then ce dossier est déplié, sélectionné et centré dans l'arbre
- [x] Given un clic sur le chevron entre deux segments, when le menu s'ouvre, then il liste le contenu du dossier de gauche, dossiers d'abord, le fichier courant marqué
- [x] Given une entrée dossier dans ce menu, when je la choisis, then le menu affiche son contenu sur place avec une entrée « Back to <nom> » ; une entrée fichier ouvre le fichier et ferme le menu
- [x] Given un dossier en cours de lecture, vide ou supprimé, when le menu s'ouvre, then il affiche « Loading folder… », « This folder is empty. » ou « This folder is no longer available. »
- [x] Given un chemin plus large que l'en-tête, when le fichier s'ouvre, then le fil d'Ariane défile horizontalement et le dernier segment est visible
- [x] La lecture du dossier se fait hors du fil de rendu, et une réponse arrivée après la fermeture du menu est ignorée

#### US-008: Sélecteur de fichiers du projet
**Description:** As a développeur qui explore un dépôt, I want ouvrir un fichier par son nom depuis n'importe où so that je n'ai pas à déplier l'arbre.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003
**Référence t3code :** `apps/web/src/components/files/ProjectFilePicker.tsx:49-122`, `apps/web/src/components/files/ProjectFilePicker.logic.ts:4,36-63` (200 résultats, surlignage), `apps/web/src/components/files/projectFilesQueryState.ts:216-236` (classement par fréquence et récence)

**Acceptance Criteria:**
- [x] Given le focus hors d'un terminal et hors d'un champ de saisie, when j'appuie sur `secondary-p`, then un sélecteur s'ouvre sur les fichiers de l'espace de travail actif
- [x] Given une requête, when je tape, then les 200 meilleurs fichiers s'affichent, classés par le score du sélecteur de chemins (`src-app/src/terminal/path_picker/fuzzy.rs`), les caractères trouvés surlignés
- [x] Given une requête vide, when le sélecteur s'ouvre, then il liste les fichiers ouverts depuis le dock, classés par un score qui combine nombre d'ouvertures et ancienneté de la dernière
- [x] Given Entrée sur un résultat, when je valide, then le fichier s'ouvre dans le dock, qui s'ouvre s'il était fermé
- [x] Given un projet de plus de 100 000 entrées, when le parcours atteint ce plafond, then le sélecteur affiche « Showing the first 100,000 files. »
- [x] Given aucun résultat, when je tape, then « No matching files. » s'affiche

---

### EP-002: Arbre : barre d'outils, recherche, icônes et mesure

Phase 1. L'arbre garde ses lignes, son retrait, ses lettres git et ses chevrons. Il gagne les comportements du panneau Files de t3code et ses icônes de fichier.

**Definition of Done:** un fichier d'un dossier jamais déplié se trouve par la recherche ; le fichier actif est révélé ; au moins 75 icônes sont résolues par nom et extension composée, en clair et en sombre ; un banc mesure l'arbre et sa ligne de base est enregistrée.

#### US-009: Barre d'outils de l'arbre
**Description:** As a développeur qui explore un dépôt, I want rafraîchir, tout déplier et tout replier so that je contrôle l'arbre sans cliquer dossier par dossier.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/components/files/FileBrowserPanel.tsx:549-631` (barre de 40 px, rafraîchir, recherche, tout déplier), `:148-150,329-337` (tout déplier sans les dossiers ignorés), `:307-322,375-380` (indicateur de chargement), `apps/web/src/pierre-tree-theme.ts` (animation et `prefers-reduced-motion`), `apps/web/src/components/files/useDirectoryEntries.ts:32-161` (4 lectures simultanées, relance)

**Acceptance Criteria:**
- [x] La barre de l'arbre contient, dans cet ordre : « Refresh files », le champ « Search files », « Expand all » ou « Collapse all » selon l'état
- [x] Given « Refresh files », when je clique, then tous les dossiers chargés sont relus et l'icône tourne jusqu'à la fin de la lecture
- [x] Given « Expand all », when je clique, then les dossiers non ignorés par git se déplient ; au-delà de 2 000 dossiers l'opération s'arrête et un toast indique « Expanded the first 2,000 folders »
- [x] Given un dossier en cours de lecture, when il est déplié, then sa ligne montre un indicateur de chargement à la place du chevron
- [x] Given `reduce_motion` activé, when un dossier charge, then l'indicateur est statique
- [x] Given une lecture de dossier qui échoue, when elle se termine, then la ligne du dossier affiche « Unable to load folder. » et un clic relance la lecture

#### US-010: Entrées ignorées par git affichées en atténué
**Description:** As a développeur qui supervise des agents, I want voir `target/` ou `node_modules/` en atténué so that je peux ouvrir un fichier généré sans quitter Paneflow.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/components/files/FileBrowserPanel.tsx:396-406` (statut « ignoré » atténué), `apps/server/src/workspace/WorkspaceEntries.ts:494-535` (`git check-ignore` par lots de 1000, raccourci au-delà de 1000 enfants)

**Acceptance Criteria:**
- [x] Given un dossier qui contient une entrée ignorée par `.gitignore`, when il est déplié, then l'entrée apparaît à l'opacité `DIMMED_OPACITY` et reste ouvrable
- [x] Given un fichier ou dossier dont le nom commence par un point, when son parent est déplié, then il apparaît à l'opacité normale, sauf `.git` qui reste masqué
- [x] Given un dossier ignoré jamais déplié, when l'arbre se charge, then son contenu n'est ni lu ni surveillé
- [x] Given un dossier ignoré replié après avoir été déplié, when il est replié, then sa surveillance est arrêtée
- [x] Given une entrée ignorée, when le fichier actif change, then elle n'est jamais révélée automatiquement

#### US-011: Recherche sur tout le projet
**Description:** As a développeur qui explore un dépôt, I want chercher un fichier dans tout le projet depuis l'arbre so that je le trouve même dans un dossier jamais déplié.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009
**Référence t3code :** `apps/web/src/components/files/FileBrowserPanel.tsx:114-152,184-189` (fusion des résultats avec leurs parents, 256 caractères, 200 résultats, Échap), `:602-624` (messages), `apps/web/src/state/queries.ts:50,307-343` (anti-rebond de 120 ms), `apps/server/src/workspace/WorkspaceSearchIndex.ts:53-57` (index de 25 000 entrées)

**Acceptance Criteria:**
- [x] Given une requête dans « Search files », when je m'arrête de taper 120 ms, then l'arbre n'affiche plus que les fichiers trouvés dans tout le projet et leurs dossiers parents, dépliés
- [x] La requête est limitée à 256 caractères et les résultats à 200
- [x] Given plus de 200 fichiers trouvés, when les résultats s'affichent, then « More matches available. Refine your search. » apparaît sous la barre
- [x] Given aucun fichier trouvé, when la recherche se termine, then « No matching files. » s'affiche
- [x] Given Échap dans le champ, when j'appuie, then la requête est vidée et l'arbre retrouve ses dossiers dépliés d'avant la recherche
- [x] Given une nouvelle frappe pendant une recherche, when le résultat précédent arrive, then il est ignoré
- [x] Le parcours du projet se fait hors du fil de rendu et respecte `.gitignore`

#### US-012: Révéler le fichier actif dans l'arbre
**Description:** As a développeur qui supervise des agents, I want voir où se trouve le fichier que je lis so that je repère ses voisins.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/components/files/FileBrowserPanel.tsx:467-528` (vider la recherche, déplier les ancêtres, centrer, ne pas rejouer après un rafraîchissement), `apps/web/src/components/files/loadFileAncestors.ts:1-15`

**Acceptance Criteria:**
- [x] Given un fichier ouvert depuis un terminal, le fil d'Ariane ou un changement d'onglet, when il devient actif, then ses dossiers parents se déplient, sa ligne est sélectionnée et centrée dans l'arbre
- [x] Given une recherche active, when un fichier est révélé, then la recherche est vidée d'abord
- [x] La révélation ne déplace jamais le focus clavier
- [x] Given un fichier hors de la racine de l'espace de travail ou ignoré par git, when il devient actif, then l'arbre ne change pas
- [x] Given un rafraîchissement de l'arbre, when il se termine, then la dernière révélation n'est pas rejouée

#### US-013: Menu contextuel de l'arbre
**Description:** As a développeur qui explore un dépôt, I want ouvrir une entrée avec une autre application so that je traite ce que Paneflow n'affiche pas.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/fileContextMenu.ts:62-150` (ouvrir, révéler avec libellé par OS, ouvrir avec), `apps/web/src/components/files/FileBrowserPanel.tsx:198-272`

**Acceptance Criteria:**
- [x] Given un clic droit sur une ligne, when le menu s'ouvre, then il liste « Open with Default App », le libellé de révélation, « Open With » suivi des éditeurs installés, « Copy Path », « Copy Relative Path »
- [x] Le libellé de révélation est « Reveal in Finder » sur macOS, « Reveal in File Explorer » sur Windows, « Reveal in File Manager » sur Linux
- [x] Given la révélation d'un fichier, when elle s'exécute, then le gestionnaire s'ouvre sur le dossier parent avec le fichier sélectionné sur macOS et Windows ; sur Linux il sélectionne le fichier si le service `org.freedesktop.FileManager1` répond, sinon il ouvre le dossier parent
- [x] Given aucun éditeur détecté, when le menu s'ouvre, then « Open With » est désactivé
- [x] Given une commande externe qui échoue, when elle se termine, then un toast donne le nom de l'application et l'erreur
- [x] Chaque lancement de processus se fait hors du fil de rendu

#### US-014: Jeu d'icônes de fichier étendu
**Description:** As a développeur qui explore un dépôt, I want une icône par type de fichier so that je reconnais un fichier sans lire son extension.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None
**Référence t3code :** `@pierre/trees` (`node_modules/.pnpm/@pierre+trees@1.0.0-beta.6*/node_modules/@pierre/trees/dist/builtInIcons.js`, symboles 218-287), `apps/web/src/pierre-icons.ts:26-133` (22 ajouts et leurs licences), `@pierre/trees` (`node_modules/.pnpm/@pierre+trees@1.0.0-beta.6*/node_modules/@pierre/trees/dist/style.js`, palette claire et sombre), `apps/web/src/components/chat/PierreEntryIcon.tsx:7-79`

**Acceptance Criteria:**
- [x] `src-app/assets/icons/files/light/` et `src-app/assets/icons/files/dark/` contiennent chacun au moins 75 SVG de 16 par 16, un par jeton, issus du jeu de `@pierre/trees` 1.0.0-beta.6 et des ajouts de t3code
- [x] Les icônes propres à Paneflow sans équivalent (angular, react-native, makefile, log) sont conservées dans les deux variantes
- [x] Given un thème clair puis un thème sombre, when l'arbre s'affiche, then chaque icône utilise la variante du thème
- [x] Un script suivi par git régénère les deux dossiers à partir d'un chemin source passé en argument
- [x] Un fichier d'attribution à la racine du dépôt cite les sources et leurs licences (Apache-2.0, ISC, CC0), et les métadonnées de paquet y renvoient
- [x] Les dossiers gardent leurs chevrons : aucune icône de dossier n'est ajoutée
- [x] Given un SVG absent pour un jeton, when la ligne s'affiche, then l'icône générique est utilisée et un test échoue sur ce jeton

#### US-015: Résolution du chemin vers l'icône
**Description:** As a développeur qui explore un dépôt, I want que `vite.config.ts`, `Dockerfile` et `a.test.ts` aient leur icône so that les fichiers de configuration se distinguent du code.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-014
**Référence t3code :** `@pierre/trees` (`node_modules/.pnpm/@pierre+trees@1.0.0-beta.6*/node_modules/@pierre/trees/dist/render/iconResolver.js:8-60`), `@pierre/trees` (`node_modules/.pnpm/@pierre+trees@1.0.0-beta.6*/node_modules/@pierre/trees/dist/builtInIcons.js:288-575`, tables de noms et d'extensions), `apps/web/src/pierre-icons.ts:135-192` (5 noms et 41 extensions ajoutés), `apps/web/src/pierre-icons.test.ts`

**Acceptance Criteria:**
- [x] La résolution suit cet ordre : nom de fichier exact en minuscules, extensions ajoutées par l'application de la plus longue à la plus courte, table de noms intégrée, table d'extensions intégrée de la plus longue à la plus courte, icône par défaut
- [x] Les tables reprennent celles de `@pierre/trees` (noms et extensions) et de t3code (`apps/web/src/pierre-icons.ts:135-192`)
- [x] Given `a.test.ts`, when il est résolu, then `test.ts` est essayé avant `ts`
- [x] Given `.js`, `.yaml`, `.html`, `.c`, `.java`, `.sh`, when ils sont résolus, then aucun ne retombe sur l'icône par défaut
- [x] Given un nom sans extension et inconnu, un nom vide ou un nom fait uniquement de points, when il est résolu, then l'icône par défaut est renvoyée sans panique
- [x] L'arbre, les onglets du dock, l'arbre Changes et le sélecteur de chemins utilisent la même fonction

#### US-016: Banc de mesure de l'arbre et décision d'optimisation
**Description:** As a mainteneur de Paneflow, I want mesurer l'arbre sur de gros dossiers so that les optimisations de t3code ne sont portées que si elles gagnent quelque chose.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010
**Référence t3code :** `apps/web/src/components/files/fileTreePathReconciliation.ts:3-76` (seuil de 300 mesuré sur 50 000 et 332 000 entrées), `@pierre/trees` (`node_modules/.pnpm/@pierre+trees@1.0.0-beta.6*/node_modules/@pierre/trees/dist/path-store/src/child-index.js:2-118` et `path-store/src/store.js:15-93`, compteurs de lignes visibles), `@pierre/trees` (`node_modules/.pnpm/@pierre+trees@1.0.0-beta.6*/node_modules/@pierre/trees/dist/model/virtualization.js:4-45`, fenêtre conservée)

**Acceptance Criteria:**
- [x] `scripts/bench-tree.sh` et `scripts/bench-tree.ps1` lancent une suite qui partage `src-app/src/bench_harness.rs` et compare à `bench/baselines/<os>-<arch>/tree.json`
- [x] La suite mesure, sur un dossier plat de 50 000 puis de 300 000 entrées : la construction des lignes après un dépliage, l'application de 1 000 ajouts et suppressions, et le filtrage
- [x] La ligne de base Linux x86_64 est enregistrée par `--set-baseline` sur un arbre git propre
- [x] Le PRD reçoit une entrée de changelog avec les valeurs mesurées et la décision : porter ou non l'index de lignes visibles par nœud et l'application par différence de t3code
- [x] Given une construction des lignes au-dessus de 16 ms sur 50 000 entrées, when la décision est écrite, then une story de suivi est ajoutée à ce PRD ; en dessous, la décision consigne qu'aucun portage n'est fait
- [x] `bench/README.md` décrit la suite

---

### EP-003: Éditeur : géométrie, lecture et moteur de coloration

Phase 2. L'éditeur cesse de supposer une rangée par ligne, puis reçoit le retour à la ligne, la recherche et la révélation de plage. Le moteur lexical de Pierre est mesuré, sans être livré.

**Definition of Done:** le retour à la ligne, la recherche, le remplacement et la révélation de plage fonctionnent ; le banc de l'éditeur ne régresse pas de plus de 5 % ; le rapport de mesure du prototype est écrit et la décision de moteur consignée.

#### US-017: Table de correspondance entre lignes et positions
**Description:** As a mainteneur de Paneflow, I want une seule fonction qui donne la position verticale d'une ligne so that une ligne peut occuper plusieurs rangées et un bloc peut s'insérer entre deux lignes.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None
**Référence t3code :** `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/components/VirtualizedFile.js:11,68-71,124-158,512-534`, hauteur fixe par défaut, mesures seulement pour les lignes repliées, totaux tous les 5 000 lignes), `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/constants.js:87-92`, 20 px, blocs de 50 lignes), `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/utils/createWindowFromScrollPosition.js:3-21`)

**Acceptance Criteria:**
- [ ] Une structure unique répond à : position verticale d'une ligne, ligne à une position verticale, hauteur totale du document, rangées visibles
- [ ] Les sites qui multiplient ou divisent par `CODE_ROW_HEIGHT` pour situer une ligne passent par cette structure : `code/element.rs`, `code/navigation/layout.rs`, `code/view/change_markers.rs`, `code/view/input_handler.rs`, `code/view/mod.rs`, `code/view/motion.rs`, `code/view/pointer.rs`, `code/view/disk_sync.rs`, `code/view/render.rs`
- [ ] Given aucune ligne repliée et aucun bloc, when un fichier s'affiche, then le rendu est identique au pixel près à celui d'avant, vérifié par les tests de géométrie existants
- [ ] Given un document vide ou d'une seule ligne, when la structure est interrogée hors bornes, then elle renvoie la première ou la dernière ligne sans panique
- [ ] `scripts/bench-editor.sh` ne montre aucune régression au-delà de 5 % sur `keystroke_to_runs` et sur la requête d'un écran

#### US-018: Retour à la ligne
**Description:** As a développeur qui supervise des agents, I want que les longues lignes se replient so that je lis du Markdown ou du JSON sans défilement horizontal.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017
**Référence t3code :** `apps/web/src/components/files/FilePreviewPanel.tsx:1193-1195,1543-1551` (bascule par panneau sur un réglage global), `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/components/VirtualizedFile.js:124-158`), `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/components/Virtualizer.js:286-364`, ancrage de la première ligne visible)

**Acceptance Criteria:**
- [ ] Given un fichier ouvert, when j'active « Word Wrap » dans le menu Editor Controls, then les lignes se replient à la largeur du contenu et le défilement horizontal disparaît
- [ ] Le réglage `editor.word_wrap` (faux par défaut) fixe l'état initial ; la bascule du menu ne vaut que pour l'onglet
- [ ] Given une ligne repliée, when je clique, sélectionne ou déplace le curseur avec Haut et Bas, then le curseur suit les rangées visuelles
- [ ] Given le retour à la ligne actif, when la largeur du dock change, then la ligne en haut de l'écran reste en haut
- [ ] Les marqueurs git, les numéros de ligne et la minimap restent alignés sur la première rangée de chaque ligne
- [ ] Given un fichier en lecture seule pour ligne de plus de 10 000 caractères, when le retour à la ligne est actif, then cette ligne se replie et aucune image ne dépasse 16 ms sur le banc

#### US-019: Recherche et remplacement dans le fichier
**Description:** As a développeur qui supervise des agents, I want chercher dans le fichier ouvert so that je retrouve un nom sans le parcourir.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017
**Référence t3code :** `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/editor/command.js:5-54,123-132`, rechercher, remplacer, occurrence suivante), `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/constants.js:55-85`, libellés de la barre de recherche)

**Acceptance Criteria:**
- [ ] Given le focus dans l'éditeur, when j'appuie sur `secondary-f`, then une barre de recherche s'ouvre, pré-remplie avec la sélection si elle tient sur une ligne
- [ ] Given une requête, when je tape, then toutes les occurrences sont surlignées et un compteur affiche « 3 of 17 »
- [ ] Entrée va à l'occurrence suivante, Maj+Entrée à la précédente, Échap ferme la barre et rend le focus à l'éditeur
- [ ] La barre offre deux bascules : « Match Case » et « Regular Expression »
- [ ] Given la rangée de remplacement dépliée, when je choisis « Replace » ou « Replace All », then le texte est remplacé et « Replace All » s'annule d'un seul Ctrl+Z
- [ ] Given une expression régulière invalide, when je tape, then le champ passe à l'état d'erreur avec le message de l'analyseur et aucune occurrence n'est surlignée
- [ ] Given un fichier en lecture seule, when la barre est ouverte, then la rangée de remplacement est absente
- [ ] Given la requête `:120`, when je valide, then l'éditeur va à la ligne 120 ; un numéro hors bornes va à la dernière ligne
- [ ] La recherche sur un fichier de 10 Mio s'exécute hors du fil de rendu et une requête plus récente annule la précédente

#### US-020: Ouverture à une ligne avec plage surlignée
**Description:** As a développeur qui supervise des agents, I want qu'un fichier s'ouvre sur la ligne demandée so that je lis tout de suite ce que l'agent a désigné.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-017
**Référence t3code :** `apps/web/src/components/files/fileLineReveal.ts:16-24` (centrage), `apps/web/src/components/files/FilePreviewPanel.tsx:472-687` (borne de ligne, garde de 20 images, abandon au premier geste), `apps/web/src/components/files/fileSurfaceChrome.tsx:27-59` (fond de la ligne révélée), `apps/web/src/rightPanelStore.ts:916-937` (`revealLine`, `revealRequestId`). t3code ne surligne qu'une ligne ; ce PRD surligne la plage

**Acceptance Criteria:**
- [ ] L'ouverture d'un onglet de fichier accepte une ligne, une colonne et une ligne de fin, toutes facultatives
- [ ] Given une ligne demandée, when le fichier s'affiche, then la plage est centrée verticalement et surlignée d'un fond qui laisse la sélection lisible
- [ ] Given une plage de plusieurs lignes, when elle est révélée, then toutes ses lignes sont surlignées
- [ ] Given une ligne au-delà de la fin du fichier, when il s'ouvre, then la dernière ligne est révélée
- [ ] Given un fichier déjà ouvert, when il est rouvert avec une autre ligne, then son onglet est activé et la nouvelle plage révélée
- [ ] Given une frappe, un clic ou un défilement de l'utilisateur, when il survient, then le surlignage disparaît

#### US-021: Mémoire de défilement par fichier
**Description:** As a développeur qui explore un dépôt, I want retrouver ma position dans un fichier rouvert so that je reprends ma lecture.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/components/files/FilePreviewPanel.tsx:501-599` (mémoire et restauration), `apps/web/src/components/diffs/useCodeViewScrollRestore.ts`

**Acceptance Criteria:**
- [ ] Given un onglet fermé, when je rouvre le fichier dans la même session, then le défilement et le curseur reviennent à leur position
- [ ] La mémoire garde les 200 derniers fichiers par fenêtre et oublie les plus anciens
- [ ] Given un fichier raccourci entre-temps, when il est rouvert, then la position est ramenée à la dernière ligne
- [ ] Given une ouverture avec une ligne demandée, when le fichier s'affiche, then la ligne demandée l'emporte sur la position mémorisée

#### US-022: Prototype du moteur lexical WebAssembly derrière le banc
**Description:** As a mainteneur de Paneflow, I want faire tourner le moteur de Pierre sur le corpus du banc so that sa vitesse est mesurée sur la machine de référence.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None
**Référence t3code :** `@pierre/highlights` (`node_modules/.pnpm/@pierre+highlights@0.5.1/node_modules/@pierre/highlights/dist/highlighter.js:9-10,44-46,80,157-242`, mémoire, fonctions d'accueil Unicode, choix du langage), `@pierre/highlights` (`node_modules/.pnpm/@pierre+highlights@0.5.1/node_modules/@pierre/highlights/dist/live.js:101,441-550`, reprise incrémentale et budget de 1 ms), `apps/web/src/lib/syntaxHighlighting.ts:17-22`, `apps/web/src/components/DiffWorkerPoolProvider.tsx:21-26,64-71`, `@pierre/diffs` (`node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4*/node_modules/@pierre/diffs/dist/highlighter/highlights-editor.js:26-163`)

**Acceptance Criteria:**
- [ ] Le binaire `highlights.wasm` de `@pierre/highlights` 0.5.1 est exécuté depuis Rust, avec ses deux fonctions d'accueil de classes Unicode, derrière une fonctionnalité Cargo désactivée par défaut
- [ ] Le code du prototype n'est compilé dans aucun binaire de release ni dans les trois binaires d'aide plafonnés
- [ ] La suite du banc de l'éditeur gagne des cas qui mesurent le prototype sur le corpus de `code/bench_corpus.rs` : coloration complète du fichier Rust de 300 Ko, une frappe au milieu, la requête d'un écran de 60 lignes
- [ ] Les mêmes cas existent pour le moteur tree-sitter actuel, pour une comparaison côte à côte dans la même exécution
- [ ] Given un langage inconnu du moteur, when il est demandé, then le prototype renvoie du texte sans couleur et aucun cas n'échoue
- [ ] Le rapport de mesure est ajouté au PRD : débit, médiane et 95e centile de la frappe, mémoire retenue, taille ajoutée au binaire, coût d'un appel

#### US-023: Validate assumption: la justesse du moteur lexical suffit pour Paneflow
**Description:** As a mainteneur de Paneflow, I want comparer les couleurs des deux moteurs so that le choix du moteur repose sur la vitesse et sur la justesse.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-022
**Référence t3code :** `@pierre/highlights` (`node_modules/.pnpm/@pierre+highlights@0.5.1/node_modules/@pierre/highlights/dist/token-types.js:2-76`, 73 catégories), `@pierre/highlights` (`node_modules/.pnpm/@pierre+highlights@0.5.1/node_modules/@pierre/highlights/dist/languages.js:3-350`, 152 langages), `apps/web/src/lib/syntaxHighlighting.ts:13-15` (alias de dialectes)

**Acceptance Criteria:**
- [ ] Pour Rust, TypeScript, Python, Markdown avec blocs de code et HTML avec script, un fichier du corpus est coloré par les deux moteurs et les écarts sont comptés par catégorie de jeton
- [ ] Le rapport liste les constructions où le moteur lexical se trompe ou perd une distinction : macros et durées de vie en Rust, génériques en TypeScript, chaînes multi-lignes
- [ ] Le rapport compare la couverture : langages colorés par chaque moteur, et langages imbriqués
- [ ] Given un écart jugé bloquant sur Rust, when le rapport est écrit, then il recommande de garder tree-sitter pour ce langage et le dit
- [ ] Le rapport se termine par une recommandation entre trois voies (réécriture native en Rust, binaire embarqué, statu quo tree-sitter étendu) et la décision d'Arthur est consignée dans le changelog du PRD
- [ ] Aucun code de production ne change dans cette story

---

### EP-004: Aperçus et très gros fichiers

Phase 2. Un aiguillage par type remplace la liste d'extensions refusées. Chaque fichier a une visionneuse ou un écran qui propose quoi en faire.

**Definition of Done:** `BINARY_EXTENSIONS` ne sert plus à refuser un fichier ; une image, un CSV, un Markdown, une archive et un fichier texte de 200 Mio s'ouvrent dans le dock ; tout autre binaire montre l'écran « pas d'aperçu » avec ses trois actions.

#### US-024: Aiguillage par type et écran « pas d'aperçu »
**Description:** As a développeur qui explore un dépôt, I want que chaque fichier s'ouvre sur quelque chose d'utile so that un binaire ne se termine pas sur un message d'erreur.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/components/files/FilePreviewPanel.tsx:1210-1220,1607-1783` (ordre d'aiguillage), `packages/shared/src/filePreview.ts:38-96` (`filePreviewKind`), `apps/web/src/components/files/AttachmentFilePreview.tsx:394-407` (écran « No preview for this file »), `apps/server/src/workspace/fileHeaderMimeType.ts:1-20`

**Acceptance Criteria:**
- [ ] L'ouverture d'un fichier choisit sa visionneuse dans cet ordre : image par extension, tableau pour `.csv` et `.tsv`, Markdown, archive par extension, sinon chargeur de texte
- [ ] Given un fichier que le chargeur de texte reconnaît comme binaire ou non UTF-8, when il s'ouvre, then l'écran « No preview for this file » s'affiche avec l'icône du type, la taille, et les boutons « Open with Default App », le libellé de révélation et « Open With »
- [ ] Given un fichier d'une extension de l'ancienne liste `BINARY_EXTENSIONS`, when il apparaît dans l'arbre, then sa ligne n'est plus atténuée pour cette raison
- [ ] Given un fichier supprimé pendant l'ouverture, when le chargement échoue, then l'onglet affiche « File no longer exists » avec « Retry »
- [ ] Given un fichier spécial (tube, périphérique), when il est ouvert, then l'écran « pas d'aperçu » s'affiche sans lecture du fichier
- [ ] Le choix repose sur l'extension, puis sur les 8 premiers Kio pour le texte ; aucune lecture ne se fait sur le fil de rendu

#### US-025: Visionneuse de très gros fichiers texte
**Description:** As a développeur qui supervise des agents, I want ouvrir un journal de 200 Mio so that je lis la fin d'une exécution sans quitter Paneflow.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-024
**Référence t3code :** `apps/web/src/components/files/LargeTextViewer.tsx:12-253` (16 Kio immédiats, en-tête, colonnes), `apps/web/src/components/files/largeText.worker.ts:12-201` (index des débuts de ligne, pages), `apps/web/src/components/files/largeTextProtocol.ts:2-5` (256 Mio, 5 000 000 de lignes, pages de 32), `apps/web/src/components/files/VirtualTablePreview.tsx:261-347`, `apps/web/src/components/files/FilePreviewPanel.tsx:1894-1933` (barre « Full file »)

**Acceptance Criteria:**
- [ ] Given un fichier texte entre 10 Mio et 256 Mio, when il s'ouvre, then une visionneuse en lecture seule, sans coloration, l'affiche avec la bannière « Large file: read-only, no syntax colors »
- [ ] Given l'ouverture, when les 16 premiers Kio sont lus, then le premier écran s'affiche avant la fin de l'indexation et l'en-tête indique « Indexing… » puis « N lines · taille »
- [ ] L'index des débuts de ligne est construit hors du fil de rendu et les lignes sont lues par pages à la demande ; la mémoire retenue ne dépasse pas 8 octets par ligne plus 12 pages
- [ ] Given un fichier de plus de 256 Mio, de plus de 5 000 000 de lignes, avec une ligne de plus de 1 Mio ou un octet nul, when il s'ouvre, then l'écran « pas d'aperçu » s'affiche avec la raison
- [ ] La sélection, la copie et le défilement jusqu'à la dernière ligne fonctionnent ; une copie de plus de 16 Mio est refusée avec un toast
- [ ] Given le fichier modifié sur le disque, when la modification est détectée, then l'index est reconstruit et la position conservée si la ligne existe encore
- [ ] Given l'onglet fermé pendant l'indexation, when il se ferme, then l'indexation s'arrête

#### US-026: Visionneuse d'images
**Description:** As a développeur qui explore un dépôt, I want voir une image du dépôt so that je vérifie une ressource qu'un agent vient de produire.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-024
**Référence t3code :** `apps/web/src/components/files/PreviewImageContent.tsx:30-133` (barre, zoom ×1,25 entre 0,05 et 8, ajuster, taille réelle), `apps/web/src/components/files/decodeImagePreview.ts`, `apps/web/src/components/files/imageDecode.worker.ts:3-213` (plafonds 50 Mo, 16 384 px, 32 Mpx), `packages/shared/src/mediaPreview.ts:22-36` (extensions d'image)

**Acceptance Criteria:**
- [ ] Les fichiers PNG, JPEG, GIF, WebP, BMP, ICO, TIFF et SVG s'ouvrent dans une visionneuse ; un GIF ou un WebP animé affiche sa première image
- [ ] La barre affiche la taille « L × H », « Zoom out », le pourcentage, « Zoom in », « Fit » et « Actual size »
- [ ] Le zoom avance par facteur 1,25 entre 5 % et 800 % ; « Fit » ajuste l'image à la zone sans jamais dépasser 100 %, avec 16 px de marge
- [ ] Given une image plus grande que la zone, when je fais défiler ou glisser, then l'image se déplace
- [ ] Le décodage se fait hors du fil de rendu, et la barre affiche « Loading image… » jusqu'à sa fin
- [ ] Given un fichier de plus de 50 Mo, un côté de plus de 16 384 px ou plus de 32 mégapixels, when il s'ouvre, then l'écran « pas d'aperçu » s'affiche avec la raison, sans décodage ; les dimensions sont lues dans l'en-tête avant toute allocation
- [ ] Given un fichier corrompu, AVIF ou HEIC, when il s'ouvre, then « Unable to load image. » ou l'écran « pas d'aperçu » s'affiche
- [ ] Un SVG est rendu sans exécuter de script et sans charger de ressource externe
- [ ] Given l'image modifiée sur le disque, when la modification est détectée, then elle est rechargée en gardant le zoom

#### US-027: Markdown : bascule entre rendu et source dans le dock
**Description:** As a développeur qui supervise des agents, I want lire un Markdown rendu dans le dock so that je lis le plan d'un agent sans ouvrir un autre panneau.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-024
**Référence t3code :** `apps/web/src/components/files/FileMarkdownPreview.tsx`, `apps/web/src/components/files/filePreviewMode.ts:13-14` (extensions Markdown), `apps/web/src/components/files/FilePreviewPanel.tsx:1092-1155,1261-1296` (bascule, préférence, source forcée quand une ligne est demandée)

**Acceptance Criteria:**
- [ ] Given un fichier `.md`, `.mdx` ou `.markdown`, when il est ouvert, then l'en-tête montre une bascule « Show rendered markdown » / « Show markdown source »
- [ ] La source est l'affichage par défaut ; le dernier choix est mémorisé dans la configuration et vaut pour tous les fichiers Markdown
- [ ] La vue rendue réutilise `MarkdownView` (`src-app/src/markdown/view.rs`) et se recharge quand le fichier change
- [ ] Given une ouverture avec une ligne demandée, when le fichier s'affiche, then la source est affichée quelle que soit la préférence
- [ ] Given un fichier modifié non enregistré, when je passe au rendu, then le rendu montre le contenu en cours d'édition
- [ ] Given un Markdown au-dessus de la limite du rendu, when je bascule, then la source reste affichée et un toast indique « File too large to render »
- [ ] Le clic sur un chemin `.md` dans un terminal garde son comportement actuel

#### US-028: Tableau pour CSV et TSV
**Description:** As a développeur qui explore un dépôt, I want lire un CSV en colonnes so that je vérifie des données sans tableur.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-024
**Référence t3code :** `apps/web/src/components/files/DelimitedTablePreview.tsx`, `packages/shared/src/delimitedPreview.ts:3-108` (guillemets, marque d'ordre, détection du `;`), `apps/web/src/components/files/VirtualTablePreview.tsx`, `apps/web/src/components/files/largeTextProtocol.ts:4` (1 024 colonnes)

**Acceptance Criteria:**
- [ ] Given un fichier `.csv` ou `.tsv` de 10 Mio au plus, when il s'ouvre, then un tableau s'affiche, première ligne en en-tête fixe, numéros de ligne à gauche, avec une bascule « Show source » / « Show table »
- [ ] Le tableau est le mode par défaut ; le dernier choix est mémorisé dans la configuration
- [ ] L'analyse gère les guillemets et les sauts de ligne dans une cellule, ignore la marque d'ordre des octets, et retient `;` comme séparateur seulement s'il est cohérent sur les deux premières lignes
- [ ] Les lignes sont virtualisées ; seules les lignes visibles sont mises en page
- [ ] La largeur de chaque colonne est calculée sur les 100 premières lignes, entre 60 et 400 px ; une cellule plus longue est tronquée
- [ ] Given plus de 1 024 colonnes, when le fichier s'ouvre, then les colonnes suivantes sont masquées et une bannière l'indique
- [ ] Given un fichier dont une ligne n'a pas le même nombre de colonnes, when il s'affiche, then les cellules manquantes sont vides et aucune ligne n'est perdue
- [ ] Given un CSV de plus de 10 Mio, when il s'ouvre, then la visionneuse de très gros fichiers l'affiche en texte
- [ ] L'analyse se fait hors du fil de rendu

#### US-029: Liste du contenu d'une archive
**Description:** As a développeur qui explore un dépôt, I want voir ce que contient une archive so that je vérifie un paquet produit par un build.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-024
**Référence t3code :** `apps/web/src/components/files/BinaryViewer.tsx`, `apps/web/src/components/files/binaryPreview.worker.ts:45-156` (lecture des en-têtes tar) et `:228-243` (signatures zip et gzip), `packages/client-runtime/src/zip/boundedZip.ts:5-8` (50 Mo, 10 Mo par entrée, 2 048 entrées, ratio de 200)

**Acceptance Criteria:**
- [ ] Given un fichier `.zip`, `.jar`, `.tar`, `.tar.gz` ou `.tgz` de 50 Mo au plus, when il s'ouvre, then un tableau liste Name, Size et Modified, dossiers d'abord
- [ ] Un fil d'Ariane interne et une entrée « Back » permettent d'entrer dans un dossier de l'archive et d'en sortir
- [ ] Aucune entrée n'est extraite sur le disque ni ouverte : la story ne fait que lister
- [ ] Given plus de 2 048 entrées, when la liste se construit, then elle s'arrête à 2 048 et une bannière l'indique
- [ ] Given une archive corrompue, chiffrée ou de plus de 50 Mo, when elle s'ouvre, then l'écran « pas d'aperçu » s'affiche avec la raison
- [ ] Un nom d'entrée absolu ou contenant `..` est affiché tel quel et ne sert à aucun accès au disque
- [ ] La lecture se fait hors du fil de rendu avec un délai de 30 s

---

### EP-005: Liens entre fichiers et agents par le terminal

Phase 3. Un fichier, une plage de lignes ou un commentaire rejoint le terminal d'un agent en texte pré-rempli. Un chemin affiché par un agent ouvre le dock à la bonne ligne.

**Definition of Done:** depuis l'arbre et depuis l'éditeur, un chemin ou un commentaire de plage arrive dans le terminal d'un agent sans être validé ; un chemin `fichier:ligne` cliqué dans un terminal ouvre le dock sur la plage ; aucun contenu de fichier ne peut sortir du collage encadré.

#### US-030: Envoyer un fichier à un agent
**Description:** As a développeur qui supervise des agents, I want envoyer un chemin de fichier à un agent depuis l'arbre ou un onglet so that je n'ai plus à le taper.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None
**Référence t3code :** `apps/web/src/components/files/FileBrowserPanel.tsx:209-259` (« Add to chat », toast sans conversation ouverte), `packages/shared/src/composerTrigger.ts:31-34` (forme de la mention). t3code insère dans son compositeur ; ce PRD insère dans le terminal de l'agent

**Acceptance Criteria:**
- [ ] Les menus contextuels de l'arbre et des onglets de fichier contiennent « Send to Agent »
- [ ] Given un seul agent dans l'espace de travail actif, when je choisis l'entrée, then le chemin est inséré dans son terminal et le focus y passe
- [ ] Given plusieurs agents, when je survole l'entrée, then un sous-menu les liste dans l'ordre de `order_agents` (`src-app/src/app/sidebar/agents.rs:69`), avec le logo du runtime et le titre de l'onglet
- [ ] Given aucun agent, when le menu s'ouvre, then l'entrée est désactivée avec le libellé « No agent in this workspace »
- [ ] Le texte inséré est `@` suivi du chemin relatif à la racine de l'espace de travail et d'une espace pour Claude Code, Codex et Gemini ; pour les autres runtimes, le chemin relatif suivi d'une espace ; un fichier hors de la racine donne son chemin absolu
- [ ] Given un chemin qui contient une espace, when il est inséré, then il est mis entre guillemets selon la règle du runtime
- [ ] Le texte n'est jamais suivi d'un retour chariot : rien n'est soumis à l'agent

#### US-031: Glisser des fichiers de l'arbre vers un terminal
**Description:** As a développeur qui supervise des agents, I want glisser un fichier de l'arbre sur un terminal so that le geste marche aussi pour un shell.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-030
**Référence t3code :** `apps/web/src/components/files/fileTreeDragMention.ts:35-85` (sélection entière ou ligne seule, pas d'ouverture, désélection), `apps/web/src/components/chat/composerMentionDrag.ts:5-103` (zone de dépôt)

**Acceptance Criteria:**
- [ ] Given une ligne de l'arbre, when je la fais glisser, then un aperçu avec l'icône et le nom suit le pointeur et le fichier ne s'ouvre pas
- [ ] Given un dépôt sur le terminal d'un agent, when je relâche, then le texte de US-030 est inséré
- [ ] Given un dépôt sur un terminal sans agent, when je relâche, then le chemin est inséré avec l'échappement du shell de ce terminal, comme pour un dépôt de fichier du système (`src-app/src/terminal/input.rs:1167-1185`)
- [ ] Given un terminal en lecture seule ou terminé, when je survole, then aucune zone de dépôt n'apparaît et le dépôt ne fait rien
- [ ] Le dépôt dans l'arbre lui-même reste sans effet
- [ ] Rien n'est soumis : aucun retour chariot n'est envoyé

#### US-032: Commenter une plage de lignes
**Description:** As a développeur qui supervise des agents, I want attacher un commentaire à une plage de lignes so that je dis à l'agent quoi changer, à l'endroit exact.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017
**Référence t3code :** `apps/web/src/components/files/fileCommentAnnotations.ts:5-70` (brouillon, étiquette `L12 to L15`, suivi des lignes), `apps/web/src/components/files/FilePreviewPanel.tsx:820-1040` (création depuis la gouttière, un seul brouillon, annulation). Technique du bloc sous une ligne : Zed, `/home/arthur/dev/zed/crates/editor/src/display_map/block_map.rs:159-336` et `crates/editor/src/git.rs:557-692,2440-2490`

**Acceptance Criteria:**
- [ ] Given le pointeur au-dessus de la gouttière pendant 200 ms, when il s'arrête sur une ligne, then un bouton « + » apparaît avec l'infobulle « Comment on this line (drag for a range) »
- [ ] Given un clic sur « + », when je relâche sans glisser, then un brouillon s'ouvre sous cette ligne ; un glisser sélectionne une plage et le brouillon s'ouvre sous sa dernière ligne
- [ ] Given une sélection dans l'éditeur, when je choisis « Comment Selection » dans son menu contextuel, then le brouillon s'ouvre pour les lignes de la sélection
- [ ] Le brouillon affiche l'étiquette « L12 » ou « L12-L15 », un champ de saisie multi-ligne au focus, et les boutons « Send to Agent » et « Cancel »
- [ ] Un seul brouillon existe par onglet : en ouvrir un second remplace le premier après confirmation si son champ n'est pas vide
- [ ] Given une modification du fichier au-dessus de la plage, when le texte bouge, then le brouillon suit ses lignes ; si toutes ses lignes sont supprimées, il est fermé
- [ ] Échap ou « Cancel » ferme le brouillon et rend le focus à l'éditeur
- [ ] Given la visionneuse de très gros fichiers ou un aperçu non textuel, when le pointeur survole la gouttière, then aucun bouton n'apparaît
- [ ] Les lignes situées sous le brouillon sont décalées de sa hauteur et le défilement reste continu

#### US-033: Envoyer un commentaire de plage à un agent
**Description:** As a développeur qui supervise des agents, I want que mon commentaire arrive pré-rempli dans le terminal de l'agent so that je le relis et l'envoie moi-même.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-030, US-032
**Référence t3code :** `apps/web/src/reviewCommentContext.ts:61-99` (contenu du commentaire), `packages/contracts/src/composerContext.ts:45-46` (16 000 et 32 000 caractères), `packages/shared/src/composerContextReferences.ts:226-427` (mise en forme et échappement des balises), `apps/web/src/lib/composerContextRecords.ts:401-554` (troncature)

**Acceptance Criteria:**
- [ ] Given un brouillon non vide, when je choisis « Send to Agent » ou `secondary-enter`, then le terminal de l'agent reçoit, en un seul collage encadré : le chemin relatif suivi de `:12` ou `:12-15`, une ligne vide, le commentaire, une ligne vide, puis les lignes citées dans un bloc de code dont la langue est l'extension du fichier
- [ ] Le choix de l'agent suit les règles de US-030 ; avec plusieurs agents, le bouton ouvre la liste et retient le dernier agent choisi pour cet espace de travail
- [ ] Le commentaire est tronqué à 16 000 caractères et les lignes citées à 32 000, avec la marque « … truncated … »
- [ ] Les caractères de contrôle autres que le saut de ligne et la tabulation sont retirés du texte, dont toute séquence `ESC[201~`
- [ ] Given un terminal qui n'a pas activé le collage encadré, when j'envoie, then la confirmation de collage multi-ligne existante s'affiche (`src-app/src/terminal/input.rs:1187-1209`)
- [ ] Le collage ne se termine pas par un retour chariot : rien n'est soumis
- [ ] Given l'envoi réussi, when le texte est inséré, then le brouillon se ferme et le focus passe au terminal ; en cas d'échec, le brouillon reste ouvert avec son texte et un toast donne la raison
- [ ] Given un champ vide ou fait d'espaces, when le brouillon est affiché, then « Send to Agent » est désactivé

#### US-034: Ouvrir dans le dock un chemin cliqué dans un terminal
**Description:** As a développeur qui supervise des agents, I want qu'un chemin `fichier:ligne` cliqué ouvre le dock so that je lis ce que l'agent désigne sans changer d'application.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-020
**Référence t3code :** `apps/web/src/terminal-links.ts:45-170` (détection des chemins et de `:ligne:colonne`), `apps/web/src/markdown-links.ts:74-106`, `apps/web/src/components/chat/MarkdownFileLink.tsx:347-359` (clic dans le panneau, modificateur pour l'éditeur externe)

**Acceptance Criteria:**
- [ ] Given un chemin de code dans la sortie d'un terminal, when je clique avec le modificateur de lien existant, then le fichier s'ouvre dans le dock de l'onglet, à la ligne et à la colonne indiquées, la ligne surlignée
- [ ] Given le même geste avec Maj en plus, when je clique, then le fichier s'ouvre dans l'éditeur externe, comme aujourd'hui
- [ ] Le réglage `terminal.open_links_in` (`dock` par défaut, ou `external_editor`) inverse ces deux gestes
- [ ] Given le dock fermé, when le lien est suivi, then le dock s'ouvre
- [ ] Given un chemin vers un fichier que le dock n'affiche qu'en « pas d'aperçu », when je clique, then cet écran s'affiche avec ses actions
- [ ] Given un fichier qui n'existe plus, when je clique, then un toast indique « File not found » et rien ne s'ouvre
- [ ] Les chemins `.md` gardent leur comportement actuel

#### US-035: Repli par nom de fichier pour un chemin relatif
**Description:** As a développeur qui supervise des agents, I want qu'un chemin relatif à un sous-dossier s'ouvre quand même so that les chemins tronqués par un agent restent cliquables.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-034
**Référence t3code :** `apps/web/src/components/ChatMarkdown.tsx:805-897` (`findWorkspaceFileMatch`) (dossier courant, autres racines, puis nom de fichier ; seule la dernière recherche compte)

**Acceptance Criteria:**
- [ ] Given un chemin relatif qui n'existe pas depuis le dossier courant du terminal, when le lien est évalué, then il est essayé depuis la racine de l'espace de travail
- [ ] Given aucun des deux, when un seul fichier du projet se termine par ce chemin, then ce fichier est ouvert
- [ ] Given plusieurs fichiers candidats, when je clique, then le sélecteur de fichiers s'ouvre pré-rempli avec le chemin
- [ ] Given aucun candidat, when le pointeur survole le texte, then il n'est pas présenté comme un lien
- [ ] La recherche de candidats se fait hors du fil de rendu, avec un délai de 200 ms au-delà duquel le texte n'est pas un lien

---

### EP-007: Arbre : projection incrémentale

Phase 3, avant EP-006. La mesure d'US-016 place la construction des lignes au-dessus de 16 ms sur 50 000 entrées ; cet epic porte l'index de lignes visibles par nœud et l'application par différence de t3code.

**Definition of Done:** `rows_after_expand_50k` et `apply_1000_changes_50k` passent sous 16 ms sur la machine de référence, et la ligne de base de l'arbre est réenregistrée.

#### US-038: Projection incrémentale de l'arbre
**Description:** As a développeur qui explore un gros dépôt, I want que l'arbre se mette à jour sans reconstruire toutes ses lignes so that un dépliage ou une rafale de changements sur disque s'affiche dans l'image suivante.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-016
**Référence t3code :** `@pierre/trees` (`node_modules/.pnpm/@pierre+trees@1.0.0-beta.6*/node_modules/@pierre/trees/dist/path-store/src/child-index.js:2-118` et `path-store/src/store.js:15-93`, compteurs de lignes visibles par nœud), `apps/web/src/components/files/fileTreePathReconciliation.ts:3-76` (application par différence au-delà de 300 changements)

Story de suivi issue de la décision d'US-016 (changelog 1.2), sortie d'EP-002 pour que l'epic de mesure ne porte pas l'optimisation qu'elle déclenche : `rows_after_expand_50k` mesure 19,57 ms, au-dessus du budget de 16 ms.

**Acceptance Criteria:**
- [ ] Chaque dossier chargé porte le nombre de ses lignes visibles, tenu à jour au dépliage, au repli et à chaque changement de son contenu
- [ ] Une ligne de l'arbre est construite seulement quand elle entre dans la fenêtre affichée ; rien n'est alloué par ligne hors de cette fenêtre
- [ ] Given 1 000 ajouts et suppressions dans un dossier, when ils arrivent du surveillant, then seules les entrées changées sont appliquées, sans copier l'arbre entier
- [ ] Sur la machine de référence, `rows_after_expand_50k` et `apply_1000_changes_50k` passent sous 16 ms, mesurés par `scripts/bench-tree.sh` en release
- [ ] La ligne de base `bench/baselines/linux-x86_64/tree.json` est réenregistrée par `--set-baseline` sur un arbre propre

---

### EP-006: Validation sur matériel réel

Clôture. Les branches `cfg(windows)` ne se compilent pas sous Linux et ne sont relues que par inspection pendant les epics précédents. Cet epic les exerce sur Windows 11 et consigne l'état de macOS.

**Definition of Done:** chaque story d'EP-001 à EP-005 et d'EP-007 a été exercée sur Windows 11, les défauts trouvés sont corrigés ou consignés, et l'état de macOS (essayé ou inspecté) est écrit pour chaque epic.

#### US-036: Passe Windows 11 du dock et de l'arbre
**Description:** As a utilisateur de Paneflow sur Windows, I want que le dock Files et l'arbre fonctionnent comme sur Linux so that la version Windows n'est pas en retrait.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-002, US-003, US-004, US-005, US-006, US-007, US-009, US-010, US-011, US-012, US-013, US-014, US-015
**Référence t3code :** aucune : story de validation sur matériel

**Acceptance Criteria:**
- [ ] Sur Windows 11 x64, chaque critère d'EP-001 et d'EP-002 qui ne dépend pas de la machine de mesure est rejoué et son résultat consigné dans le PRD
- [ ] « Reveal in File Explorer » ouvre l'Explorateur avec le fichier sélectionné, y compris pour un chemin avec espaces et un chemin de plus de 260 caractères
- [ ] La recherche et les icônes fonctionnent sur un projet situé sur un lecteur autre que `C:` et sur un partage WSL
- [ ] `Windows x86_64 libghostty check` est vert sur le commit validé
- [ ] Given un défaut trouvé, when il est corrigé, then la correction est rejouée sur Fedora ; un défaut non corrigé est consigné avec son ticket
- [ ] L'état de macOS est écrit pour EP-001 et EP-002 : essayé sur matériel ou relu par inspection

#### US-037: Passe Windows 11 de l'éditeur, des aperçus et des liens agents
**Description:** As a utilisateur de Paneflow sur Windows, I want que l'éditeur, les aperçus et l'envoi aux agents fonctionnent comme sur Linux so that je supervise mes agents de la même façon.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-018, US-019, US-020, US-024, US-025, US-026, US-027, US-028, US-030, US-031, US-032, US-033, US-034
**Référence t3code :** aucune : story de validation sur matériel

**Acceptance Criteria:**
- [ ] Sur Windows 11 x64, chaque critère d'EP-003, EP-004 et EP-005 qui ne dépend pas de la machine de mesure est rejoué et son résultat consigné dans le PRD
- [ ] Un fichier aux fins de ligne CRLF s'ouvre, se replie, se cherche et se commente ; les lignes citées dans un commentaire n'ont pas de retour chariot en double
- [ ] L'envoi d'un chemin et d'un commentaire fonctionne vers un agent lancé dans PowerShell et dans cmd, avec l'échappement du shell concerné
- [ ] Un chemin Windows avec lettre de lecteur et barres obliques inverses, suivi de `:ligne`, cliqué dans un terminal, ouvre le dock à la bonne ligne
- [ ] Given un défaut trouvé, when il est corrigé, then la correction est rejouée sur Fedora ; un défaut non corrigé est consigné avec son ticket
- [ ] L'état de macOS est écrit pour EP-003, EP-004 et EP-005 : essayé sur matériel ou relu par inspection

## Functional Requirements

- FR-01: Le dock doit offrir un onglet Files qui affiche l'arbre sans fichier ouvert.
- FR-02: Le système ne doit jamais fermer un onglet de fichier sans action de l'utilisateur.
- FR-03: Une fermeture en lot ne doit jamais fermer un onglet dont le contenu modifié n'est pas enregistré.
- FR-04: La recherche de l'arbre doit couvrir tous les fichiers non ignorés du projet, dépliés ou non.
- FR-05: La résolution d'icône doit être une fonction unique, utilisée par l'arbre, les onglets, l'arbre Changes et le sélecteur de chemins.
- FR-06: Les dossiers de l'arbre doivent garder leurs chevrons ; aucune icône de dossier n'est ajoutée.
- FR-07: L'ouverture d'un fichier doit choisir une visionneuse par type ; aucune extension ne doit produire un simple message d'erreur.
- FR-08: Tout texte envoyé à un terminal par ces fonctions doit être inséré sans retour chariot final et ne doit jamais être soumis.
- FR-09: Tout texte issu d'un fichier et inséré dans un terminal doit être débarrassé des caractères de contrôle autres que saut de ligne et tabulation.
- FR-10: Quand un fichier ouvert change sur le disque, la détection de conflit existante doit continuer de s'appliquer ; le système ne doit PAS enregistrer automatiquement.
- FR-11: Toute lecture de disque, tout décodage et tout lancement de processus ajoutés par ce PRD doivent s'exécuter hors du fil de rendu.
- FR-12: Le prototype de moteur lexical ne doit PAS être compilé dans un binaire de release.

## Non-Functional Requirements

- **Performance:** aucune image au-dessus de 16 ms pendant le défilement de l'arbre sur 50 000 entrées et de l'éditeur avec retour à la ligne, mesuré par les bancs en build release sur Linux x86_64. Résultats de recherche de l'arbre affichés en 150 ms au 95e centile après l'anti-rebond, sur 100 000 entrées. `keystroke_to_runs` à 5 % au plus au-dessus de la ligne de base (485 µs de médiane). Premier écran d'un fichier de 256 Mio en 200 ms.
- **Security:** plafonds de décodage : 50 Mo, 16 384 px par côté, 32 mégapixels par image ; 50 Mo et 2 048 entrées par archive ; 0 extraction sur disque. 0 caractère de contrôle autre que saut de ligne et tabulation dans un texte collé. La recherche ne suit aucun lien symbolique hors de la racine.
- **Accessibility:** 100 % des contrôles ajoutés atteignables au clavier, avec infobulle ; `reduce_motion` arrête l'indicateur de chargement ; contraste du texte atténué à 0,55 d'opacité vérifié sur les thèmes clair et sombre.
- **Scalability:** 50 onglets de fichier ouverts sans fermeture d'office ; projets jusqu'à 100 000 fichiers indexés ; fichiers texte jusqu'à 256 Mio et 5 000 000 de lignes ; tableaux jusqu'à 1 024 colonnes.
- **Reliability:** 0 résultat périmé affiché : chaque chargement, recherche et décodage porte un compteur de génération. 0 perte de contenu modifié lors d'une fermeture en lot. Un `session.json` de la version précédente se charge dans 100 % des cas.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Projet vide | Onglet Files sur un dossier sans fichier | Arbre vide, barre d'outils active | "This folder is empty." |
| 2 | Chargement d'un dossier | Dépliage d'un dossier lent (réseau, WSL) | Indicateur sur la ligne, arbre utilisable | Aucun |
| 3 | Dossier illisible | Droits retirés pendant la session | Ligne en erreur, clic pour relancer | "Unable to load folder." |
| 4 | Tout déplier sur un gros dépôt | Plus de 2 000 dossiers | Arrêt à 2 000 | "Expanded the first 2,000 folders" |
| 5 | Recherche tronquée | Plus de 200 fichiers trouvés | 200 premiers affichés | "More matches available. Refine your search." |
| 6 | Fermeture en lot avec modifications | « Close All » avec 3 onglets modifiés | Onglets modifiés conservés | "3 modified tabs kept" |
| 7 | Fichier disparu | Onglet restauré, rouvert ou lien cliqué vers un fichier supprimé | Onglet omis, entrée retirée ou rien d'ouvert | "File no longer exists" / "File not found" |
| 8 | Écriture concurrente | Un agent écrit un fichier modifié dans l'éditeur | Bannière de conflit existante, aucun écrasement | Bannière existante |
| 9 | Image hors plafonds | 20 000 px de côté | Aucun décodage | "No preview for this file" avec la raison |
| 10 | Archive piégée | Entrées `../..` ou 100 000 entrées | Liste arrêtée à 2 048, aucun accès disque | Bannière de troncature |
| 11 | Très gros fichier | 300 Mio ou ligne de 2 Mio | Aucune indexation | "No preview for this file" avec la raison |
| 12 | Contenu hostile collé | Fichier contenant `ESC[201~` puis une commande | Séquence retirée, texte inerte | Aucun |
| 13 | Aucun agent | « Send to Agent » sans agent dans l'espace | Entrée désactivée | "No agent in this workspace" |
| 14 | Terminal sans collage encadré | Commentaire multi-ligne vers un shell simple | Confirmation existante | Dialogue existant |
| 15 | Onglet fermé pendant un travail | Fermeture pendant indexation, décodage ou recherche | Travail annulé, résultat ignoré | Aucun |
| 16 | Expression régulière invalide | Recherche dans le fichier | Champ en erreur, aucune occurrence | Message de l'analyseur |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | La table lignes-positions touche une trentaine de sites et casse le rendu ou ralentit la frappe | Med | High | US-017 isolée, sans changement de comportement, avec les tests de géométrie existants et le banc à 5 % |
| 2 | Des onglets sans plafond font monter la mémoire (5,41 Mo retenus pour un onglet de 2 Mo, `bench/baselines/linux-x86_64/editor.json`) | Med | Med | Chargement paresseux à la restauration (US-006) ; mesure à 50 onglets ; libération de l'arbre de syntaxe des onglets inactifs si la mesure l'impose |
| 3 | Afficher les dossiers ignorés épuise les surveillances du système quand l'utilisateur déplie `node_modules` | Med | Med | Surveillance par dossier chargé seulement, arrêtée au repli ; « Expand all » ignore ces dossiers et plafonne à 2 000 |
| 4 | Un contenu de fichier collé dans un terminal s'exécute | Low | High | Collage encadré, retrait des caractères de contrôle, jamais de retour chariot (US-033), test dédié |
| 5 | Le prototype WebAssembly ajoute une dépendance lourde ou gonfle les binaires d'aide | Med | Med | Fonctionnalité Cargo désactivée par défaut, hors release, `check-helper-sizes.sh` |
| 6 | Le moteur lexical est moins juste que tree-sitter sur Rust | Med | Med | US-023 compte les écarts avant toute intégration ; la voie hybride reste ouverte |
| 7 | Les logos de marque du jeu d'icônes (Simple Icons, CC0) posent une question de marque | Low | Low | Attribution, usage limité à désigner un type de fichier, retrait possible jeton par jeton |
| 8 | `@chemin` n'est pas compris par un CLI | Med | Low | Limité aux trois CLI confirmés, chemin simple ailleurs, table par runtime en un seul endroit |
| 9 | Les défauts Windows ne sont vus qu'en EP-006 | High | Med | Inspection des branches `cfg(windows)` à chaque story, déclarations avant `mod tests`, EP-006 dédié |
| 10 | 37 stories dépassent une livraison | High | Med | Trois phases indépendantes ; la phase 1 se livre seule |

## Non-Goals

What this version explicitly does NOT include:

- **ACP et toute surface de conversation intégrée.** Reporté par Arthur le 2026-11-18. L'enregistrement d'un commentaire reste structuré (chemin, plage, texte, lignes) pour pouvoir partir en bloc ACP plus tard.
- **Suivi Garder/Rejeter des changements d'un agent** à la manière du journal d'actions de Zed : il suppose que l'agent écrive via ACP.
- **Aperçus PDF, Office (docx, pptx, xlsx, odt, ods, odp, epub, rtf) et notebooks.** Reportés par Arthur : pas prioritaires.
- **Aperçus audio, vidéo, SQLite, police, Mermaid et HTML rendu.** Aucun lecteur ni moteur de page dans GPUI ; charger une police l'inscrit dans le registre global de texte. L'écran « pas d'aperçu » et « Open with Default App » couvrent ces fichiers.
- **Intégration du moteur lexical dans l'éditeur.** Ce PRD s'arrête à la mesure et à la décision (US-022, US-023). L'intégration fera l'objet d'un PRD suivant.
- **Icônes de dossier.** Décision d'Arthur : les chevrons restent.
- **Fusion des chaînes de dossiers à enfant unique.** Elle change la présentation des lignes de l'arbre, qu'Arthur veut garder.
- **Enregistrement automatique.** t3code enregistre 500 ms après la frappe sans contrôle de conflit ; Paneflow garde l'enregistrement explicite et sa bannière de conflit.
- **Créer, renommer, déplacer ou supprimer des fichiers depuis l'arbre.** Absent de t3code aussi.
- **Recherche de contenu dans tout le projet.** La recherche de l'arbre porte sur les noms de fichier.
- **Persistance de la largeur du dock lui-même.** Seule la largeur de l'arbre est mémorisée.

## Files NOT to Modify

- `src-app/src/terminal/types.rs` et `src-app/src/terminal/pty_session/session_backend.rs` : frontière du moteur de terminal, gardée par un test.
- `src-app/src/git_command.rs` : git passe par la CLI ; aucun lecteur de formats git.
- `vendor/**` et les valeurs `rev` de GPUI dans `src-app/Cargo.toml` : GPUI est épinglé à une révision exacte.
- `crates/paneflow-shim`, `crates/paneflow-ai-hook`, `crates/paneflow-mcp` : binaires d'aide à taille plafonnée, aucune dépendance nouvelle.
- `crates/paneflow-home` : la disposition du dossier utilisateur ne change pas ; un nouveau chemin s'ajoute par ce crate.
- `bench/baselines/**` : écrit seulement par `--set-baseline` sur un arbre propre.
- `docs/user/**` : miroir du site, synchronisé depuis le dépôt du site.
- `native/libghostty/**` : liaisons générées.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture, table lignes-positions :** une structure à sommes cumulées (hauteur par ligne, points de contrôle tous les N lignes) ou une liste de blocs ancrés à la manière de Zed (`BlockProperties`, hauteur en lignes) ? Recommandé : une seule structure qui sert le retour à la ligne et les blocs. L'ingénierie confirme le coût sur le banc.
- **Architecture, onglets :** les onglets sont adressés par index (`src-app/src/app/diff_dock/tabs.rs:249`). Les fermetures en lot parcourent-elles les index du plus grand au plus petit, ou les onglets reçoivent-ils un identifiant stable ? Recommandé : identifiant stable, qui sert aussi la restauration et l'historique.
- **Data Model, session :** nouveaux champs de `TabSession` (`crates/paneflow-config/src/schema/session.rs:180-195`) avec `#[serde(default, skip_serializing_if)]`. Chemins absolus ou relatifs à la racine de l'espace ? Recommandé : relatifs quand le fichier est sous la racine, pour survivre à un déplacement du dépôt.
- **Data Model, largeur :** `files_tree_width` dans `window-state.json` (`src-app/src/window_state.rs:28-56`), à côté de `sidebar_width`.
- **Recherche de l'arbre :** réutiliser le parcours du sélecteur de chemins en relevant sa visibilité, ou le déplacer dans un module commun ? L'index est-il mis en cache par racine et invalidé par les événements du surveillant de l'arbre ?
- **Icônes :** des SVG figés par thème (deux dossiers) ou un rendu en deux couches teintées à l'exécution ? Recommandé : SVG figés, car `img()` ne teinte pas. L'ingénierie confirme le poids ajouté au binaire.
- **Dependencies :** `image` 0.25.10 avec `default-features = false` et les seuls formats listés ; un analyseur CSV (crate `csv` ou analyse maison) ; un lecteur zip (aucun dans l'arbre de dépendances aujourd'hui) ; wasmtime pour le prototype, avec module précompilé ou compilé au lancement du banc ? Chaque ajout passe `cargo deny`.
- **Très gros fichiers :** réutiliser `CodeElement` avec une source de document paginée, ou une vue dédiée ? Le premier garde la sélection et la copie existantes.
- **Révélation sous Linux :** `org.freedesktop.FileManager1.ShowItems` par D-Bus, avec repli sur l'ouverture du dossier. Quelle dépendance D-Bus est déjà présente ?
- **Migration :** aucune migration de données. Retour arrière : les nouveaux champs de session sont ignorés par une version antérieure.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Onglets de fichier ouverts sans fermeture d'office | 8 | 50 vérifiés, aucun plafond | Month-1 | Test et passe manuelle |
| Icônes de type de fichier | 19 | 75 ou plus | Month-1 | Comptage des SVG, test de résolution |
| Extensions refusées à l'ouverture | 60 | 0 | Month-1 | Test de l'aiguillage sur l'ancienne liste |
| Fichier trouvé dans un dossier jamais déplié | Impossible | 100 % des fichiers non ignorés, jusqu'à 100 000 | Month-1 | Test de recherche |
| Taille maximale d'un fichier texte affiché | 10 Mio | 256 Mio | Month-1 | Test de la visionneuse |
| Gestes pour envoyer un commentaire de plage à un agent | N/A (new) | 3 au plus | Month-1 | Passe manuelle |
| `keystroke_to_runs`, médiane | 485 µs | 510 µs au plus | Month-1 | `scripts/bench-editor.sh` |
| Construction des lignes de l'arbre, 50 000 entrées | Non mesurée | Mesurée, 16 ms au plus | Month-1 | `scripts/bench-tree.sh` |
| Décision de moteur de coloration | Aucune mesure locale | Rapport et décision consignés | Month-1 | Changelog du PRD |
| Chemins de code ouverts dans le dock plutôt que dans l'éditeur externe | 0 % | 100 % par défaut | Month-1 | Test de l'événement de lien |

## Open Questions

- **Raccourci de réouverture d'onglet.** `secondary-shift-t` appartient à `undo_close_pane` (`src-app/src/keybindings/defaults.rs:144`). US-005 passe par les menus. Arthur décide, avant EP-001, si la touche doit rouvrir l'onglet du dock quand le dock a le focus.
- **Fichiers à point.** US-010 les affiche à l'opacité normale et ne masque que `.git`. Aujourd'hui ils sont masqués. Arthur confirme avant US-010.
- **Format de mention par runtime.** `@chemin` pour Claude Code, Codex et Gemini repose sur leur documentation ; OpenCode, Pi et Hermes reçoivent le chemin simple. À confirmer par un essai réel par runtime pendant US-030.
- **Suffixe de lignes.** US-033 écrit `chemin:12-15`. Claude Code documente aussi `@chemin#L12-15`. Faut-il ce format pour Claude Code ? Arthur décide avant US-033.
- **Icônes existantes.** US-014 remplace les icônes de Paneflow par celles de Pierre quand un équivalent existe (rust, typescript, python, go, css, json, markdown, docker, ruby, swift, toml, react). Arthur confirme à la passe visuelle de US-014.
- **Largeur du dock.** Elle n'est pas mémorisée aujourd'hui. Hors périmètre ici ; à décider si la largeur de l'arbre mémorisée rend ce manque gênant.
[/PRD]
