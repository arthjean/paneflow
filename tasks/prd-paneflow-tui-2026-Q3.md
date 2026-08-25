[PRD]
# PRD: paneflow-tui - frontend terminal de Paneflow - 2026-Q3

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-08-24 | Arthur Jean | Rédaction initiale, 5 epics, 19 stories |

## Problem Statement

1. **Paneflow est inaccessible partout où il n'y a pas de GPU exploitable.** Le binaire lie GPUI, qui exige Vulkan sur Linux. Une session SSH, un conteneur, un serveur de build, une VM sans accélération, une machine distante de collègue: aucun de ces contextes ne peut lancer Paneflow aujourd'hui. Or lancer des agents de codage en parallèle sur une machine distante est un usage central, pas un cas marginal.

2. **Le marché des cockpits d'agents s'est structuré en 2025-2026 autour d'outils terminal, et Paneflow n'y est pas.** Herdr (Rust, client-serveur, sidebar d'état d'agent) a atteint la première place du GitHub Trending le 2026-06-30 avec un ordre de grandeur de quinze mille étoiles en environ cent jours, en se positionnant explicitement comme "tmux pour agents de codage". Zellij et tmux ne connaissent pas l'état des agents. cmux et Conductor sont mac-only ou couplés au process d'une application de bureau. Paneflow a la seule proposition cross-platform réelle mais ne l'exprime que dans une GUI.

3. **Les utilisateurs qui vivent dans le terminal ne convertissent pas vers une GUI.** Le public cible de Paneflow est précisément celui qui a déjà tmux ou Zellij dans son flux de travail. Leur demander de quitter le terminal pour adopter Paneflow est un coût de changement que la plupart refusent, quel que soit le mérite de la GUI.

**Why now:** trois facteurs convergent. Le backend VT de Paneflow est déjà neutre et sans GPUI depuis la migration libghostty, donc le coût technique de la TUI a chuté. La fenêtre concurrentielle se referme: Herdr occupe l'espace mental "multiplexeur d'agents" depuis moins de quatre mois et n'a pas encore de rival cross-platform crédible. Et la GUI est stabilisée en 0.8.2 sur les trois plateformes, ce qui libère la capacité d'ouvrir un second frontend sans mettre le premier en risque.

## Overview

`paneflow-tui` est un second frontend de Paneflow, écrit en Rust avec ratatui et crossterm, distribué comme un binaire distinct de la GUI et dépourvu de toute dépendance graphique. Il tourne dans n'importe quel terminal hôte, y compris via SSH, et rend les mêmes concepts que la GUI: workspaces, onglets, panes, agents de codage avec leur état, statut git, ports détectés.

L'architecture retenue est celle du moindre couplage. Plutôt que d'extraire d'abord un coeur headless d'un `PaneFlowApp` monolithique de plus de soixante champs, `paneflow-tui` consomme les crates du workspace qui sont déjà exemptes de GPUI: `paneflow-config` fournit `LayoutNode` et `SurfaceDefinition`, la représentation d'arbre de panes déjà sérialisée dans session.json; `paneflow-terminal-ghostty` fournit le moteur VT et un modèle de cellule neutre presque isomorphe à celui de ratatui; `paneflow-ipc-client`, `paneflow-process` et `paneflow-agent-config` fournissent le plan de contrôle, la gestion de processus et la détection d'agents. Une seule extraction est faite dans src-app, celle de la palette de thèmes vers une représentation RGB neutre, parce qu'elle correspond à une duplication réelle et immédiate entre les deux frontends. Toute autre mutualisation sera décidée après coup, sur constat de duplication, jamais par anticipation.

La qualité visuelle est un objectif explicite, pas un sous-produit. Les mécanismes retenus sont ceux qui distinguent une TUI soignée d'un assemblage de widgets: fusion des bordures par carte de connectivité pour obtenir des jonctions correctes et une ligne partagée entre panes voisins, palette sémantique dont le fond de sidebar reste `Color::Reset` pour préserver la transparence du terminal hôte, discipline d'une cellule par glyphe garantie par test, mesure de largeur systématiquement en colonnes terminal et jamais en octets, et un état géométrique unique lu à la fois par le rendu et par le test de collision de la souris pour qu'un clic ne puisse jamais être décalé du rendu.

La persistance de session, le mode démon et le detach/reattach sont hors périmètre de cette version. C'est une contrainte produit assumée, et le risque associé est documenté explicitement dans la section Risques.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Paneflow utilisable sans GPU | `paneflow-tui` lance une session complète sur un hôte sans Vulkan et via SSH | Zéro régression sur les trois plateformes |
| Aucune dépendance graphique | `cargo tree -p paneflow-tui` ne contient ni gpui, ni wgpu, ni ash, ni blade | Gate CI actif sur les quatre legs de la matrice |
| Parité fonctionnelle du socle | Splits, onglets, sidebar avec état d'agent, clavier, thème | Copy-mode, souris, recherche, OSC 52 |
| Latence de saisie | P95 keystroke vers présentation inférieure à 25 ms en local | Inférieure à 25 ms maintenue sous 4 panes actifs |
| Adoption | Asset TUI publié et documenté | Au moins 10 pour cent des téléchargements de release |

## Target Users

### Développeur en session SSH
- **Role:** développe ou supervise des agents de codage sur une machine distante, un serveur de build, une VM ou un conteneur.
- **Behaviors:** vit dans tmux ou screen, se connecte en SSH plusieurs fois par jour, garde plusieurs agents en parallèle sur des branches différentes.
- **Pain points:** Paneflow ne démarre pas sans GPU, donc la machine distante est un angle mort complet. tmux ne sait pas si un agent attend une réponse, ce qui oblige à parcourir les panes un par un.
- **Current workaround:** tmux avec des fenêtres nommées à la main, plus un aller-retour visuel constant pour repérer l'agent bloqué.
- **Success looks like:** la même sidebar d'état d'agent que la GUI, dans le terminal, sur la machine distante, sans installer de serveur graphique.

### Utilisateur de multiplexeur qui refuse la GUI
- **Role:** développeur dont l'environnement complet est au clavier dans un terminal, souvent avec Neovim.
- **Behaviors:** configure ses raccourcis, refuse les outils qui imposent la souris, évalue un outil sur sa vitesse de démarrage et son empreinte.
- **Pain points:** les cockpits d'agents disponibles sont soit des GUI qui tuent les agents à la fermeture de l'app, soit mac-only, soit non agent-aware.
- **Current workaround:** scripts tmux maison, ou Herdr, qui est cross-platform mais dont le modèle client-serveur impose un démon.
- **Success looks like:** un binaire unique, un préfixe familier, un démarrage immédiat, et l'état des agents visible en permanence.

### Utilisateur existant de la GUI Paneflow
- **Role:** déjà converti, utilise Paneflow en poste de travail.
- **Behaviors:** alterne entre poste local et machines distantes selon le projet.
- **Pain points:** doit changer d'outil et de modèle mental dès qu'il sort de son poste local.
- **Current workaround:** tmux à distance, Paneflow en local, deux configurations de raccourcis distinctes.
- **Success looks like:** les mêmes concepts, les mêmes thèmes et des raccourcis reconnaissables des deux côtés.

## Research Findings

### Competitive Context
- **Herdr:** Rust, client-serveur, le démon possède les PTY et survit aux coupures SSH, sidebar d'état d'agent pour plus de quinze CLIs sans hooks, API socket, attach distant. Nous différons par l'absence de démon en v1, assumée, et par l'existence d'une GUI native du même auteur sur les trois plateformes.
- **Zellij:** apprécié pour la découvrabilité et les layouts, critiqué pour son immaturité relative face à tmux, sa configuration KDL et des panics rapportés laissant des processus orphelins. Il n'est pas agent-aware. Nous reprenons sa découvrabilité, pas son modèle de configuration.
- **tmux:** conserve les utilisateurs distants par sa stabilité et son ubiquité serveur. Son interception d'OSC 52 et son absence de support kitty complet sont des reproches récurrents. Nous devons faire mieux sur ces deux points précis, sans quoi la TUI sera perçue comme un tmux inférieur.
- **cmux, Conductor, Crystal devenu Nimbalyst:** GUI, mac-only ou couplées au process de l'application, avec le défaut structurel que les agents meurent à la fermeture de l'app.
- **Market gap:** aucun cockpit d'agents cross-platform ne propose aujourd'hui à la fois une GUI native et une TUI issues du même codebase.

### Best Practices Applied
- Le passthrough VT moderne est le socle attendu en 2026: protocole clavier kitty, OSC 52, DECSET 2026 en sortie synchronisée, true color. Les nouveaux multiplexeurs Rust les embarquent dès le premier jour.
- ratatui impose un mode immédiat à double tampon diffé: l'application possède tout l'état, y compris l'offset de défilement, le curseur et la sélection. Le rendu ne doit jamais bloquer.
- La distribution Rust en TUI achoppe classiquement sur la dérive glibc: une cible statique est nécessaire pour que le binaire soit utilisable sur les serveurs anciens.

### Notable Risk
Sans protocole clavier désambiguïsé, Shift+Enter est indistinguable d'Enter. Les CLIs d'agents modernes s'en servent pour les retours à la ligne dans un prompt. Une TUI qui casse ce chord est inutilisable pour son cas d'usage principal.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- Le protocole clavier kitty est activable et fiable via crossterm sur les terminaux cibles Linux et macOS, et win32-input-mode couvre le cas Windows. Validé par US-003 avant tout travail d'entrée.
- Le modèle de cellule de `paneflow-terminal-ghostty` se projette sans perte sur `ratatui::Cell` pour les attributs, les couleurs, les caractères larges et les graphèmes combinés. Fondé sur la lecture de `crates/paneflow-terminal-ghostty/src/model.rs:177`.
- `LayoutNode` de `paneflow-config` suffit comme arbre de panes du TUI sans généraliser `layout/` de src-app. Fondé sur son usage existant dans session.json.
- L'allocation d'un `Arc<[Cell]>` par snapshot reste sous le budget de frame à 200 colonnes par 60 lignes. À mesurer en US-008, pas supposé.
- Les utilisateurs acceptent l'absence de detach en v1 parce que la TUI ouvre un usage qui était impossible auparavant. C'est l'hypothèse produit la plus fragile du document.

### Hard Constraints
- `cargo tree -p paneflow-tui` ne doit contenir aucune dépendance graphique. C'est le critère mécanique qui définit la réussite du découplage.
- Linux, macOS et Windows sont des cibles de première classe, conformément à la règle cross-platform du projet. Aucun chemin Linux-only.
- La GUI ne doit subir aucune régression. Une seule refactorisation touche src-app en v1, celle de la palette.
- Le worktree contient trente-six fichiers modifiés non commités, dont `layout/serde.rs`, `workspace/mod.rs` et `pane.rs`. Toute story touchant src-app doit être séquencée après stabilisation de ce travail en cours.
- Licence GPL-3.0-or-later, cohérente avec le reste du workspace.
- Aucun démon, aucune persistance de session, aucun detach dans cette version.

## Quality Gates

Ces commandes doivent passer pour chaque user story:
- `cargo fmt --check` - formatage canonique, gate du pipeline de release sur les quatre legs de la matrice
- `cargo clippy --workspace -- -D warnings` - lints workspace
- `cargo test --workspace` - suite complète
- `cargo build -p paneflow-tui` - le TUI compile indépendamment de la GUI
- `cargo tree -p paneflow-tui | grep -E 'gpui|wgpu|ash|blade'` - doit ne rien retourner

Pour toute story marquée rendu (US-005, US-006, US-009 à US-016), une vérification visuelle manuelle est requise en plus des gates: lancer `paneflow-tui` dans au moins trois terminaux de la matrice de compatibilité (un terminal supportant kitty, un terminal VTE, et Windows Terminal), exercer le geste décrit dans les critères et confirmer le rendu. Les gates automatisés couvrent le contenu du `Buffer` ratatui, pas la présentation réelle par le terminal hôte.

## Epics & User Stories

### EP-001: Socle et frontière anti-GPUI

Créer la crate, la boucle d'événements et la frontière mécanique qui garantit que le TUI reste utilisable sans GPU, et lever l'incertitude clavier avant d'investir dans l'entrée.

**Definition of Done:** `paneflow-tui` démarre, occupe le terminal, se restaure proprement dans tous les chemins de sortie, et un test d'architecture échoue si une dépendance graphique entre dans son graphe.

#### US-001: Créer la crate paneflow-tui avec sa frontière de dépendances
**Description:** As a mainteneur, I want une crate `paneflow-tui` isolée dont le graphe de dépendances est vérifié par un test so that une contamination graphique accidentelle échoue en CI au lieu d'être découverte par un utilisateur SSH.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given le workspace, when on ajoute `crates/paneflow-tui` aux membres, then il n'apparaît pas dans `default-members` et `cargo run` reste non ambigu
- [ ] Given la crate, when on exécute `cargo tree -p paneflow-tui`, then aucune ligne ne contient gpui, wgpu, ash ou blade
- [ ] Given un ajout de dépendance introduisant transitivement une crate graphique, when la CI exécute le test d'architecture, then le test échoue avec un message nommant la crate fautive et son chemin d'introduction
- [ ] Given le binaire produit, when on l'exécute sur un hôte sans bibliothèque Vulkan installée, then il démarre sans erreur de chargement dynamique
- [ ] Given `cargo build -p paneflow-tui --target x86_64-unknown-linux-musl`, when la cible statique est demandée, then le binaire produit ne dépend d'aucune version de glibc

#### US-002: Boucle d'événements et cycle de vie du terminal
**Description:** As a utilisateur, I want que le TUI prenne et rende le terminal proprement dans tous les cas de sortie so that une fermeture, un plantage ou un signal ne me laisse jamais un terminal inutilisable.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un démarrage, when le TUI s'initialise, then il entre en écran alternatif et en mode raw, et l'écran précédent est restauré à la sortie normale
- [ ] Given un panic dans le code de rendu, when le processus se termine, then le hook de panic restaure le mode raw et l'écran principal avant d'imprimer le message
- [ ] Given SIGINT, SIGTERM ou SIGHUP sur Unix et l'équivalent console sur Windows, when le signal arrive, then le terminal est restauré et les PTY enfants reçoivent l'ordre d'arrêt
- [ ] Given une session SSH coupée brutalement, when le terminal hôte disparaît, then le processus se termine sans laisser de PTY orphelin
- [ ] Given une boucle au repos sans aucune activité PTY, when on mesure sur dix secondes, then aucune frame n'est présentée et la consommation CPU reste sous 1 pour cent

#### US-003: Valider l'hypothèse de désambiguïsation clavier
**Description:** As a mainteneur, I want savoir avant de concevoir l'entrée quelles combinaisons sont réellement capturables sur chaque terminal cible so that la conception du clavier ne repose pas sur une hypothèse fausse.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un binaire de sonde, when on l'exécute dans un terminal donné, then il rapporte si le protocole kitty est négocié, quels drapeaux sont acceptés, et si win32-input-mode est actif
- [ ] Given la matrice de terminaux cible, when la sonde est exécutée sur chacun, then un tableau documenté indique pour Shift+Enter, Ctrl+Enter, Ctrl+Shift+lettre et les touches de fonction si le chord est distinguable
- [ ] Given un terminal sans aucun protocole d'amélioration, when la sonde s'exécute, then elle le signale explicitement au lieu d'échouer silencieusement
- [ ] Given les résultats, when ils contredisent l'hypothèse de départ, then le PRD est amendé avant le démarrage de US-014

#### US-004: Palette neutre partagée entre les deux frontends
**Description:** As a utilisateur des deux frontends, I want que les thèmes soient définis une seule fois so that un thème corrigé ou ajouté apparaisse identique en GUI et en TUI sans double saisie.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given les thèmes bundlés, when ils sont déplacés vers une représentation RGB neutre, then la GUI dérive ses `Hsla` de cette source et son rendu est inchangé pixel pour pixel sur au moins un thème vérifié
- [ ] Given le TUI, when il résout un thème par son nom, then il obtient la même définition que la GUI pour ce nom
- [ ] Given un thème absent ou un nom inconnu, when il est demandé, then le thème par défaut est utilisé et un avertissement est journalisé sans interrompre le démarrage
- [ ] Given un slot de couleur destiné à rester transparent, when il est résolu côté TUI, then il produit `Color::Reset` et non une couleur opaque
- [ ] Given un test de non-régression, when la GUI et le TUI résolvent tous les thèmes bundlés, then aucun slot ne diverge entre les deux

---

### EP-002: Rendu du terminal

Projeter la grille du moteur VT dans le tampon ratatui avec une fidélité d'attributs complète et un coût de frame mesuré.

**Definition of Done:** un agent de codage tourne dans un pane du TUI et son affichage est indistinguable de celui de la GUI en couleurs, attributs, caractères larges et position de curseur, sous un budget de frame documenté.

#### US-005: Projeter la grille VT dans le tampon ratatui
**Description:** As a utilisateur, I want voir la sortie de mon agent avec ses couleurs et ses attributs exacts so that le rendu du terminal ne trahisse pas ce que l'agent a réellement émis.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-004

**Acceptance Criteria:**
- [ ] Given une grille contenant gras, italique, souligné, barré, faible et clignotant, when elle est projetée, then chaque attribut apparaît dans le style de la cellule ratatui correspondante
- [ ] Given une cellule en vidéo inverse dont le fond est transparent, when elle est projetée, then le fond est résolu vers la couleur de fond effective avant l'échange, et le texte reste lisible
- [ ] Given un caractère large sur deux colonnes, when il est projeté, then la cellule de continuation est vide et la colonne suivante n'est pas écrasée
- [ ] Given un graphème composé de plusieurs points de code, when il est projeté, then il occupe une seule cellule et conserve ses marques combinantes
- [ ] Given une grille plus petite que la zone allouée, when elle est projetée, then les colonnes et lignes restantes sont explicitement remises à l'état par défaut au lieu de conserver le contenu de la frame précédente
- [ ] Given une coordonnée hors des limites du tampon, when le blit la rencontre, then elle est ignorée sans panic

#### US-006: Positionner le curseur réel du terminal hôte
**Description:** As a utilisateur, I want que le curseur clignotant du terminal soit au bon endroit dans le pane actif so that la saisie soit visuellement correcte et que les lecteurs d'écran suivent.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un pane focalisé dont le VT expose une position de curseur, when la frame est présentée, then le curseur du terminal hôte est positionné à cette cellule
- [ ] Given un pane non focalisé, when la frame est présentée, then aucun curseur hôte ne lui est attribué
- [ ] Given un VT ayant masqué son curseur, when la frame est présentée, then le curseur hôte est masqué
- [ ] Given une position de curseur hors de la zone visible après défilement, when la frame est présentée, then le curseur hôte est masqué plutôt que placé à une position arbitraire

#### US-007: Redimensionnement et propagation aux PTY
**Description:** As a utilisateur, I want redimensionner ma fenêtre de terminal sans corrompre l'affichage so that le travail continue pendant et après le redimensionnement.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un redimensionnement, when les nouvelles dimensions sont connues, then chaque PTY visible reçoit sa nouvelle taille en lignes et colonnes
- [ ] Given une rafale de redimensionnements pendant un glissement de bordure de fenêtre, when les événements arrivent, then ils sont coalescés et au plus une propagation par intervalle de rendu atteint les PTY
- [ ] Given un terminal réduit sous 20 colonnes ou 5 lignes, when la frame est rendue, then un message explicite remplace l'interface au lieu d'un affichage corrompu
- [ ] Given un agrandissement après une réduction extrême, when la taille redevient viable, then l'interface est restaurée sans redémarrage

#### US-008: Cadence de rendu et budget de frame
**Description:** As a utilisateur, I want que la TUI reste réactive sous un agent verbeux so that une sortie massive ne rende pas l'interface inutilisable.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un flux PTY continu, when le rendu est cadencé, then au plus une frame est présentée par tranche de 16 ms
- [ ] Given des mises à jour provenant d'un pane masqué, when elles arrivent, then elles ne déclenchent pas de présentation
- [ ] Given une grille de 200 colonnes par 60 lignes, when elle est projetée entièrement, then le temps de projection reste sous 4 ms au P95, mesuré par une sonde activable par variable d'environnement
- [ ] Given un agent produisant 10 Mo par seconde, when le flux est absorbé, then l'interface reste interactive et le clavier répond en moins de 100 ms
- [ ] Given un terminal annonçant la sortie synchronisée, when une frame est présentée, then elle est encadrée par DECSET 2026, et l'encadrement est omis sur un terminal qui ne l'annonce pas

---

### EP-003: Structure spatiale

Reproduire les concepts spatiaux de Paneflow dans le terminal: panes, onglets, workspaces, avec la qualité de chrome qui distingue une TUI soignée.

**Definition of Done:** un utilisateur navigue entre plusieurs workspaces, onglets et panes au clavier, voit l'état de chaque agent dans la sidebar, et les bordures forment des jonctions correctes.

#### US-009: Panes divisés et navigation du focus
**Description:** As a utilisateur, I want diviser un pane et me déplacer entre les panes so that je puisse suivre plusieurs agents dans le même écran.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un pane focalisé, when l'utilisateur demande une division verticale ou horizontale, then l'arbre `LayoutNode` est mis à jour et les deux panes sont rendus selon leurs ratios
- [ ] Given plusieurs panes, when l'utilisateur navigue dans une direction, then le focus va au pane voisin dans cette direction, et reste inchangé s'il n'y en a pas
- [ ] Given un pane fermé, when il disparaît, then son espace est redistribué et le focus va à un pane existant, jamais à un noeud vide
- [ ] Given le dernier pane d'un onglet, when il est fermé, then l'onglet se ferme aussi
- [ ] Given une division demandée alors que le pane fait moins de 4 colonnes ou 3 lignes, when elle est tentée, then elle est refusée avec un message et l'arbre reste inchangé
- [ ] Given le plafond de panes du projet, when il est atteint, then une nouvelle division est refusée explicitement

#### US-010: Onglets
**Description:** As a utilisateur, I want grouper mes panes en onglets nommés so that je sépare plusieurs tâches dans un même workspace.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given un workspace, when l'utilisateur crée un onglet, then il devient actif et porte son propre arbre de panes
- [ ] Given plus d'onglets que la largeur disponible, when la barre est rendue, then les onglets sont défilés et un marqueur de dépassement apparaît de chaque côté concerné
- [ ] Given un nom d'onglet plus large que sa cellule, when il est rendu, then il est tronqué en colonnes terminal et non en octets, et reste correct pour un nom CJK
- [ ] Given un onglet nommé automatiquement, when il est rendu, then il est visuellement moins appuyé qu'un onglet nommé par l'utilisateur
- [ ] Given le dernier onglet fermé, when la fermeture aboutit, then le workspace se ferme ou revient à un état vide explicite, jamais à un écran vide sans repère

#### US-011: Sidebar des workspaces avec état d'agent
**Description:** As a utilisateur, I want voir en permanence quels agents travaillent, ont fini ou attendent une réponse so that je cesse de parcourir mes panes un par un.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given plusieurs workspaces, when la sidebar est rendue, then chacun affiche son nom, sa branche git et un indicateur d'état
- [ ] Given un agent dont l'état change, when le changement est détecté, then l'indicateur du workspace correspondant change dans la frame suivante
- [ ] Given tous les indicateurs d'état, when ils sont mesurés, then chacun occupe exactement une colonne terminal, garanti par un test
- [ ] Given un état "terminé mais non consulté", when il est rendu, then il est visuellement distinct d'un état "terminé et acquitté"
- [ ] Given un groupe de workspaces replié contenant un agent en attente, when le groupe est rendu, then son indicateur agrégé remonte l'état demandant attention
- [ ] Given une sidebar plus étroite que le contenu, when elle est rendue, then les noms sont élidés en colonnes terminal et l'indicateur d'état reste visible
- [ ] Given aucun workspace au premier lancement, when la sidebar est rendue, then un message d'amorçage indique l'action à effectuer

#### US-012: Chrome de panes et fusion des bordures
**Description:** As a utilisateur, I want des bordures nettes avec des jonctions correctes so que la structure de mon écran soit lisible d'un coup d'oeil.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given deux panes voisins, when les bordures sont rendues, then ils partagent une seule ligne et non deux lignes adjacentes
- [ ] Given une intersection de trois ou quatre segments, when elle est rendue, then le glyphe de jonction correspondant est utilisé, vérifié par un test au niveau du `Buffer`
- [ ] Given un pane focalisé, when les bordures sont rendues, then seules les cellules touchant ce pane prennent la couleur d'accentuation
- [ ] Given un pane assez large, when sa bordure supérieure est rendue, then son étiquette y est inscrite, et elle est omise sous une largeur minimale au lieu d'être tronquée illisiblement
- [ ] Given un seul pane sans voisin, when il est rendu, then aucune bordure interne n'est peinte

---

### EP-004: Entrée clavier et souris

Rendre la TUI pilotable au clavier sans casser les chords dont les agents de codage dépendent, et ajouter la souris là où elle apporte réellement.

**Definition of Done:** l'utilisateur pilote toute la TUI au clavier, découvre les raccourcis sans documentation externe, et Shift+Enter atteint l'agent intact partout où le terminal hôte le permet.

#### US-013: Registre d'actions, préfixe configurable et aide
**Description:** As a utilisateur, I want un préfixe familier et un moyen de découvrir les raccourcis dans l'outil so that je n'aie pas à lire la documentation pour être opérationnel.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-010

**Acceptance Criteria:**
- [ ] Given la configuration par défaut, when l'utilisateur presse le préfixe, then un bandeau indique qu'il est actif et rappelle les actions les plus fréquentes
- [ ] Given le mode préfixe actif, when l'utilisateur presse à nouveau le préfixe, then le chord littéral est transmis au pane actif
- [ ] Given le mode préfixe actif, when l'utilisateur presse une touche sans binding, then le mode est quitté sans effet de bord
- [ ] Given un binding utilisateur qui réutilise le préfixe comme touche de second niveau, when la configuration est chargée, then le binding est désactivé avec un diagnostic nommant le champ fautif
- [ ] Given deux bindings en conflit, when la configuration est chargée, then le conflit est signalé et la configuration reste chargeable
- [ ] Given la commande d'aide, when elle est invoquée, then un panneau liste les actions groupées avec leurs raccourcis effectifs, y compris ceux redéfinis par l'utilisateur

#### US-014: Transmission fidèle des touches à l'agent
**Description:** As a utilisateur d'un agent de codage, I want que Shift+Enter et les autres chords atteignent l'agent intacts so that je puisse composer un prompt multiligne sans le soumettre par accident.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given un terminal hôte supportant le protocole kitty, when le TUI démarre, then il négocie les drapeaux nécessaires et les restaure à la sortie
- [ ] Given le protocole kitty négocié, when l'utilisateur presse Shift+Enter dans un pane, then l'agent reçoit le chord physique désambiguïsé et non un simple retour à la ligne
- [ ] Given un terminal hôte sans protocole d'amélioration, when l'utilisateur presse Shift+Enter, then le comportement de repli documenté s'applique et l'utilisateur en est informé une fois par session
- [ ] Given Windows sans support kitty, when le TUI démarre, then win32-input-mode est utilisé et les chords documentés comme supportés fonctionnent
- [ ] Given un pane dont le VT a activé le mode kitty côté agent, when une touche est transmise, then l'encodage émis correspond au mode annoncé par ce VT et non au mode du terminal hôte
- [ ] Given une séquence collée depuis le presse-papiers, when elle atteint un agent en mode bracketed paste, then elle est encadrée correctement et n'est pas soumise ligne par ligne

#### US-015: Souris
**Description:** As a utilisateur, I want cliquer pour focaliser et faire défiler à la molette so that les gestes évidents fonctionnent sans quitter le clavier pour autant.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] Given la capture souris active, when l'utilisateur clique dans un pane, then ce pane prend le focus et la zone cliquée correspond exactement à la zone rendue
- [ ] Given la molette sur un pane, when elle est actionnée, then le pane défile dans son scrollback sans affecter les autres panes
- [ ] Given un pane dont l'application interne demande le rapport souris, when l'utilisateur clique dedans, then l'événement est transmis à l'application au lieu d'être consommé par le TUI
- [ ] Given la capture souris désactivée par configuration, when l'interface est rendue, then aucun élément cliquable n'est présenté comme tel
- [ ] Given un glissement sur une bordure, when il est effectué, then le ratio du split est ajusté et borné aux limites minimales de pane

#### US-016: Copy-mode, scrollback, recherche et presse-papiers
**Description:** As a utilisateur, I want relire et copier ce qu'un agent a produit so that je puisse récupérer une erreur ou un extrait sans le perdre au défilement.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-015

**Acceptance Criteria:**
- [ ] Given un pane, when l'utilisateur entre en copy-mode, then il navigue dans le scrollback au clavier et un bandeau indique le mode actif
- [ ] Given le copy-mode, when l'utilisateur recherche un motif, then les occurrences sont surlignées et la navigation entre elles est possible dans les deux sens
- [ ] Given une sélection, when l'utilisateur la copie, then le contenu est placé dans le presse-papiers du poste local via OSC 52, y compris à travers une session SSH
- [ ] Given un terminal refusant OSC 52, when une copie est demandée, then l'échec est signalé explicitement au lieu d'être silencieux
- [ ] Given une séquence OSC 52 émise par l'agent lui-même, when elle traverse le TUI, then elle est transmise sans réécriture
- [ ] Given une sélection couvrant des caractères larges, when elle est copiée, then le texte extrait ne contient pas de cellule de continuation parasite
- [ ] Given la lecture du presse-papiers par OSC 52, when un agent la demande, then elle est refusée par défaut

---

### EP-005: Intégration au plan de contrôle et distribution

Faire du TUI un citoyen de plein droit de l'écosystème Paneflow: mêmes commandes, même socket, même canal de distribution.

**Definition of Done:** `paneflow send`, `paneflow ls` et le pont MCP fonctionnent contre une instance TUI comme contre la GUI, et un binaire TUI est publié pour les trois plateformes sans perturber l'auto-mise à jour de la GUI.

#### US-017: Exposer le plan de contrôle depuis le TUI
**Description:** As a utilisateur d'agents, I want que mes commandes Paneflow et le pont MCP fonctionnent quand je tourne en TUI so that mes automatisations ne dépendent pas du frontend choisi.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given une instance TUI, when un client exécute une commande de listage, then il obtient les workspaces, onglets et surfaces réels de cette instance
- [ ] Given une instance TUI, when un client envoie du texte à une surface, then le texte atteint le PTY correspondant
- [ ] Given une instance GUI déjà en cours détenant le socket, when une instance TUI démarre, then elle le détecte, ne clobbe pas le socket, et informe l'utilisateur de la situation
- [ ] Given un socket résiduel d'un processus mort, when le TUI démarre, then il le récupère sans intervention manuelle
- [ ] Given le pont MCP configuré, when un agent lit une autre surface depuis le TUI, then il obtient le scrollback de cette surface
- [ ] Given un client demandant une méthode non supportée par le TUI, when elle est appelée, then une erreur structurée le dit explicitement au lieu d'échouer silencieusement

#### US-018: Publier le binaire TUI et le rendre invocable par son nom
**Description:** As a utilisateur, I want taper `paneflow-tui` dans n'importe quel terminal après une installation standard so that le second frontend soit atteignable sans chemin absolu, sans qu'un client GUI déjà installé n'installe le TUI à la place de la GUI.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given la convention de nommage retenue, when un asset TUI est publié, then son nom ne satisfait aucun suffixe attendu par le sélecteur d'assets des clients GUI déjà installés
- [ ] Given un client GUI de version antérieure interrogeant une release contenant des assets TUI, when il choisit son asset, then il sélectionne l'asset GUI de sa plateforme, vérifié par un test sur une liste d'assets mixte
- [ ] Given le sélecteur d'assets corrigé, when il traite les noms historiques avec et sans préfixe de version, then il continue de les sélectionner correctement
- [ ] Given une release, when les artefacts sont produits, then un binaire TUI existe pour Linux x86_64 et aarch64, macOS aarch64 et Windows x86_64
- [ ] Given une installation par paquet système ou par installeur sur chaque plateforme, when l'utilisateur tape `paneflow-tui` dans un terminal neuf, then le binaire se lance sans chemin absolu
- [ ] Given le cask Homebrew macOS, qui n'expose aujourd'hui aucune stanza `binary` et laisse donc `paneflow` hors du PATH, when l'installation aboutit, then `paneflow` et `paneflow-tui` sont tous deux invocables depuis un terminal
- [ ] Given le format AppImage, qui est un fichier unique embarquant le runtime graphique, when la distribution TUI est décidée, then il en est exclu au profit d'une archive dédiée et l'exclusion est documentée dans les notes d'installation
- [ ] Given un binaire TUI Linux, when il est exécuté sur une distribution ancienne, then il fonctionne sans dépendance à une version récente de glibc

#### US-019: Parité cross-platform vérifiée
**Description:** As a utilisateur macOS ou Windows, I want que le TUI se comporte comme sur Linux so that le choix de ma plateforme ne dégrade pas l'outil.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-014, US-017

**Acceptance Criteria:**
- [ ] Given les trois plateformes, when la suite de tests est exécutée sur chaque leg de la matrice, then elle passe intégralement
- [ ] Given Windows, when un PTY est ouvert via ConPTY, then le redimensionnement, l'arrêt et la lecture se comportent comme sur Unix
- [ ] Given macOS, when le TUI démarre dans le terminal système et dans un terminal supportant kitty, then les deux fonctionnent avec le comportement documenté pour chacun
- [ ] Given une fonctionnalité indisponible sur une plateforme, when elle est invoquée, then elle est refusée avec un message nommant la limitation, jamais par un échec silencieux
- [ ] Given le chemin de résolution des chemins de configuration et de runtime, when il est exercé sur les trois plateformes, then aucun chemin POSIX n'est codé en dur

## Functional Requirements

- FR-01: Le système doit fournir un binaire `paneflow-tui` exécutable sans bibliothèque graphique installée.
- FR-02: Le système doit rendre le contenu d'un émulateur VT dans le terminal hôte en préservant couleurs, attributs, caractères larges et graphèmes combinés.
- FR-03: Le système doit permettre de diviser, fermer et naviguer entre panes, et de créer, nommer, fermer et naviguer entre onglets et workspaces.
- FR-04: Le système doit afficher en permanence l'état de chaque agent détecté, avec une distinction visuelle entre travail en cours, attente d'entrée, terminé non consulté et terminé acquitté.
- FR-05: Le système doit transmettre les touches au pane actif selon le protocole clavier annoncé par ce pane, indépendamment du protocole du terminal hôte.
- FR-06: Le système doit exposer le même socket de plan de contrôle que la GUI et répondre aux mêmes méthodes de listage et d'envoi.
- FR-07: Le système doit restaurer l'état du terminal hôte dans tous les chemins de sortie, y compris panic et signal.
- FR-08: Le système ne doit pas réécrire les séquences OSC 52 émises par les applications hébergées.
- FR-09: Le système ne doit pas persister de session, ni exposer de mode détaché, ni lancer de processus démon.
- FR-10: Le système doit refuser explicitement toute opération indisponible sur la plateforme courante plutôt que d'échouer silencieusement.

## Non-Functional Requirements

- **Performance:** projection d'une grille de 200 colonnes par 60 lignes en moins de 4 ms au P95. Latence de la frappe à la présentation inférieure à 25 ms au P95 en local avec 4 panes actifs. Au plus une frame présentée par tranche de 16 ms. Zéro frame présentée sur dix secondes au repos, avec une consommation CPU sous 1 pour cent.
- **Empreinte:** binaire release strippé sous 15 Mo par plateforme. RSS sous 60 Mo avec 4 panes actifs et 10000 lignes de scrollback par pane. Démarrage jusqu'à l'écran interactif sous 150 ms sur un disque local.
- **Robustesse du découplage:** `cargo tree -p paneflow-tui` retourne zéro occurrence de gpui, wgpu, ash et blade, vérifié par un test d'architecture exécuté en CI.
- **Débit:** absorption d'un flux PTY de 10 Mo par seconde sans dépasser 100 ms de latence de réponse au clavier.
- **Compatibilité:** compilation et suite de tests vertes sur les quatre legs de la matrice de release. Binaire Linux fonctionnel sans dépendance à une version de glibc postérieure à celle des distributions supportées.
- **Correction Unicode:** toute mesure et troncature de texte s'effectue en colonnes terminal. Tout glyphe d'indicateur d'état occupe exactement une colonne, vérifié par test.
- **Sécurité:** lecture du presse-papiers par OSC 52 refusée par défaut. Socket de plan de contrôle restreint au même utilisateur, conformément au modèle existant du projet.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Terminal trop petit | Fenêtre réduite sous 20 colonnes ou 5 lignes | Interface remplacée par un message, aucun rendu partiel corrompu | "Terminal trop petit: 20x5 minimum requis" |
| 2 | Absence de protocole clavier | Terminal sans kitty ni win32-input-mode | Repli documenté, avertissement unique par session | "Chords limités: ce terminal ne désambiguïse pas les touches" |
| 3 | Palette réduite | Terminal en 8 ou 16 couleurs | Dégradation vers la palette disponible sans texte illisible | Aucun |
| 4 | Rafale de redimensionnements | Glissement de bordure de fenêtre | Coalescing, au plus une propagation PTY par intervalle de rendu | Aucun |
| 5 | Perte du terminal | Coupure SSH, SIGHUP | Arrêt propre des PTY, aucun processus orphelin | Aucun |
| 6 | Panic interne | Bug de rendu | Terminal restauré avant impression du message de panic | Message de panic sur terminal restauré |
| 7 | Flux massif | Agent produisant 10 Mo par seconde | Interface interactive maintenue, rendu cadencé | Aucun |
| 8 | Texte CJK ou emoji | Nom d'onglet ou de workspace large | Troncature en colonnes, aucun chevauchement de cellules | Aucun |
| 9 | Premier lancement | Aucun workspace existant | Écran d'amorçage avec l'action à effectuer | "Aucun workspace: créez-en un avec le préfixe puis c" |
| 10 | Socket occupé | Instance GUI déjà en cours | Démarrage sans clobber, plan de contrôle non exposé, information à l'utilisateur | "Une instance Paneflow détient déjà le socket: commandes externes indisponibles" |
| 11 | Socket résiduel | Processus précédent tué | Récupération automatique sans intervention | Aucun |
| 12 | OSC 52 refusé | Terminal hôte bloquant la copie | Échec explicite au lieu d'un silence | "Copie refusée par le terminal hôte" |
| 13 | Division impossible | Pane sous la taille minimale ou plafond atteint | Refus explicite, arbre inchangé | "Division impossible: espace insuffisant" |
| 14 | Thème inconnu | Nom absent dans la configuration | Repli sur le thème par défaut, avertissement journalisé | Aucun à l'écran |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Le protocole clavier imbriqué casse Shift+Enter et rend la TUI inutilisable pour les agents | High | High | US-003 valide l'hypothèse avant toute conception d'entrée; US-014 traite les trois régimes kitty, legacy et win32; matrice de terminaux testés documentée |
| 2 | L'absence de detach fait fuir les utilisateurs SSH, qui perdent tout à la déconnexion | Medium | High | Non-goal assumé et documenté dans le produit et le README; message explicite à la connexion SSH; la persistance est le sujet suivant une fois les deux frontends stabilisés |
| 3 | Dérive fonctionnelle et visuelle entre GUI et TUI | High | Medium | Palette partagée par US-004; mêmes concepts et vocabulaire d'actions; les fonctionnalités absentes de la TUI sont listées explicitement plutôt que réinventées |
| 4 | La double émulation VT ajoute une latence perceptible | Medium | Medium | NFR chiffrée à 25 ms P95; sonde de latence activable; budget de frame mesuré en US-008 avant d'ajouter du chrome |
| 5 | Le worktree contient 36 fichiers non commités touchant layout, workspace et pane | High | Medium | Une seule story touche src-app en v1 (US-004); elle est séquencée après stabilisation du travail en cours; aucune généralisation de layout |
| 6 | Contamination graphique par une dépendance transitive | Medium | High | Test d'architecture en CI sur le modèle du guard existant confinant alacritty; gate exécuté sur chaque leg |
| 7 | Un client GUI installé télécharge le binaire TUI à la place de la GUI | Medium | High | Convention de nommage non colisionnante en protection immédiate des clients existants; correctif du sélecteur en défense en profondeur; test sur liste d'assets mixte |
| 8 | Le coût d'allocation par snapshot devient le goulot à grande taille de grille | Medium | Medium | Mesuré en US-008 avant toute optimisation; un chemin d'itération sans matérialisation reste possible si la mesure l'exige |
| 9 | Le périmètre de 19 stories dérive en cours de route | Medium | Medium | EP-004 et EP-005 contiennent les stories P1 sacrifiables; le socle P0 est livrable seul |

## Non-Goals

- **Persistance de session, démon et detach/reattach.** Décision produit explicite: la TUI et la GUI sont d'abord stabilisées, la persistance vient ensuite. C'est le principal écart avec Herdr et le risque produit numéro 2.
- **Vue diff et revue de code.** Environ treize mille lignes de code GPUI sans équivalent terminal raisonnable à ce stade.
- **Visionneuse markdown.** Dépend du widget Markdown de la fork Zed, entièrement GPUI.
- **Interface de réglages dans la TUI.** La configuration se fait par le fichier `paneflow.json` partagé avec la GUI.
- **Mode responsive mobile.** Un terminal étroit obtient un message de taille minimale, pas une disposition alternative.
- **Langage de composition configurable pour les lignes de sidebar.** Sur-ingénierie tant qu'aucun besoin utilisateur ne l'exige.
- **Images en ligne et protocoles graphiques.** Le passthrough kitty graphics est reporté après la stabilisation du socle.
- **Extraction d'un coeur headless partagé entre les deux frontends.** Reporté jusqu'à constat de duplication réelle, une fois le second consommateur existant.

## Files NOT to Modify

- `src-app/src/layout/render.rs` - émission de flex GPUI, sans équivalent terminal, hors périmètre.
- `src-app/src/terminal/view.rs` et `src-app/src/terminal/element/` - couche de rendu GPUI de la GUI, toute modification met la GUI en risque sans bénéfice pour la TUI.
- `src-app/src/settings/`, `src-app/src/window_chrome/`, `src-app/src/diff/`, `src-app/src/markdown/` - fonctionnalités hors périmètre déclarées Non-Goals.
- `src-app/src/main.rs` en dehors d'un éventuel ajout de crate - le point d'entrée de la GUI initialise GPUI et n'est pas le chemin du TUI.
- Les 36 fichiers actuellement modifiés et non commités du worktree - coordination requise avant toute story touchant src-app, afin de ne pas écraser du travail en cours.

## Technical Considerations

- **Architecture:** consommer les crates déjà exemptes de GPUI plutôt qu'extraire un coeur headless au préalable. Recommandé parce que `PaneFlowApp` compte plus de soixante champs saturés de handles GPUI et qu'extraire avant d'avoir un second consommateur serait une abstraction spéculative. L'ingénierie confirme-t-elle que la duplication d'état d'UI entre les deux frontends reste sous contrôle sans coeur partagé, ou faut-il prévoir un point de convergence dès EP-003 ?
- **Modèle de layout:** réutiliser `LayoutNode` et `SurfaceDefinition` de `paneflow-config`, déjà neutres et persistés par session.json, plutôt que rendre `layout/` générique sur le type de feuille. L'alternative coûterait six fichiers de src-app dont trois sont actuellement modifiés. La représentation `LayoutNode` couvre-t-elle tous les besoins de navigation directionnelle du TUI, ou manque-t-il des requêtes que seul l'arbre de src-app expose ?
- **Blit VT:** le modèle de cellule de `paneflow-terminal-ghostty` expose déjà caractère, graphèmes combinés, couleurs, attributs, largeur et sélection sous une forme quasi isomorphe à celle de ratatui. `Content` matérialise cependant un `Arc<[Cell]>` par instantané. Faut-il mesurer avant d'envisager un chemin d'itération sans matérialisation, ou l'ordre de grandeur est-il déjà connu côté GUI ?
- **Dépendances:** ratatui 0.30 avec le découpage `ratatui-core` et `ratatui-crossterm` désormais effectif, et crossterm pour les événements et la négociation du protocole clavier. `Buffer::cell_mut` retournant un `Option`, le blit doit borner explicitement. Faut-il ne dépendre que de `ratatui-core` dans les couches basses pour limiter la surface ?
- **Distribution:** binaire séparé plutôt que sous-commande, puisque le binaire GUI lie GPUI et exigerait un GPU pour démarrer. Cible statique pour Linux afin d'échapper à la dérive glibc. Faut-il publier des paquets système pour le TUI, ou l'archive suffit-elle en v1 ?
- **Migration:** aucune migration de données. La configuration `paneflow.json` est partagée, la section de raccourcis du TUI est distincte de celle de la GUI puisque les combinaisons capturables diffèrent. Faut-il un espace de noms explicite dans le fichier de configuration pour éviter toute ambiguïté ?

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Paneflow utilisable sans GPU | Impossible | Session complète fonctionnelle via SSH | Month-1 | Vérification manuelle sur hôte sans Vulkan, documentée |
| Occurrences graphiques dans le graphe TUI | N/A (crate inexistante) | 0 | Month-1 | `cargo tree -p paneflow-tui` en CI |
| Latence frappe vers présentation P95 | N/A | Inférieure à 25 ms | Month-1 | Sonde interne activable par variable d'environnement |
| Part des téléchargements de l'asset TUI | N/A | Au moins 10 pour cent des téléchargements de release | Month-3 | Compteurs de la GitHub Release API |
| Chords critiques fonctionnels | N/A | Shift+Enter fonctionnel sur au moins 5 terminaux de la matrice | Month-1 | Matrice de compatibilité documentée par US-003 et vérifiée par US-014 |
| Taille du binaire TUI | N/A | Inférieure à 15 Mo strippé par plateforme | Month-1 | Artefacts de release |

## Open Questions

- Faut-il un message explicite à la connexion prévenant que la session ne survivra pas à la déconnexion SSH, ou est-ce trop décourageant pour la première impression ? À trancher par Arthur avant US-002.
- La convention de nommage exacte des assets TUI doit être figée avant la première publication, puisqu'elle est irréversible pour les clients qui l'auront vue. À trancher par Arthur avant US-018.
- Le TUI doit-il pouvoir se connecter au socket d'une instance GUI en cours pour la piloter, plutôt que de refuser de démarrer son propre plan de contrôle ? Cela ressemblerait à un client léger et anticiperait le modèle client-serveur. À évaluer par l'ingénierie pendant EP-005, sans élargir le périmètre de la v1.
- Quelle est la liste exacte des terminaux constituant la matrice de compatibilité officielle ? À figer avec US-003, puisqu'elle conditionne les critères de US-014 et US-019.
[/PRD]
