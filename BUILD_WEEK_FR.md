# OpenAI Build Week : Paneflow

## Vue d'ensemble du projet

**Nom du projet :** Paneflow

**Pitch :** Application GPUI multiplateforme pour agents de développement en parallèle.

## Histoire du projet

**Développé pendant l'OpenAI Build Week :** J'ai utilisé Codex avec GPT-5.6 Sol pour faire migrer les terminaux Linux de Paneflow d'Alacritty vers `libghostty-vt`. J'ai fait de Ghostty le backend Linux par défaut, ajouté des tests différentiels et du fuzzing, et rendu les builds natifs reproductibles. Tous les panneaux de terminal visibles dans les captures d'écran de la soumission utilisent ce nouveau backend Ghostty. Alacritty reste disponible comme solution de repli.

### Inspiration

J'ai généralement plusieurs agents de développement qui travaillent en même temps. Les démarrer est facile. Me souvenir de celui qui attend, de la branche que chacun a modifiée, de ce qui a échoué et de la possibilité que deux agents s'apprêtent à travailler sur la même chose devient vite compliqué.

Je supervisais des grilles tmux et une multitude de fenêtres de terminal, principalement de mémoire. Je voulais un espace de travail dans lequel chaque agent resterait visible et où je pourrais intervenir à tout moment.

C'est ainsi qu'est né Paneflow.

### Fonctionnement

Paneflow est une application GPUI multiplateforme conçue pour exécuter des agents de développement en parallèle. Elle fait fonctionner Codex, Claude Code, Gemini, OpenCode et d'autres agents en ligne de commande dans de véritables panneaux de terminal.

La barre latérale indique quels agents réfléchissent, s'exécutent, attendent, ont terminé ou sont bloqués. Chaque espace de travail garde son répertoire et sa branche Git visibles. Paneflow propose également des dispositions persistantes, une file d'attention, des notifications de bureau et une vue Review permettant de comparer côte à côte les diffs des worktrees.

Paneflow Conductor constitue le plan de contrôle local derrière l'interface. Son CLI et son socket JSON-RPC permettent à un humain ou à un agent principal de lister les agents en cours d'exécution, d'inspecter leur état, de lire la sortie de leur terminal, d'envoyer un prompt et d'attendre des événements de cycle de vie.

Les agents peuvent également inspecter un autre panneau grâce à un bridge MCP intégré en lecture seule qui expose trois outils : `list_panes`, `read_pane` et `search_pane`. La sortie de terminal renvoyée par MCP est encapsulée comme donnée non fiable. L'envoi de prompts reste contrôlé par l'humain par défaut, avec la possibilité d'activer explicitement l'envoi automatique.

Paneflow fonctionne en local et propose des versions natives pour Linux, macOS sur Apple Silicon et Windows x64.

### Conception technique

Paneflow est écrit en Rust avec GPUI, le framework d'interface native accélérée par GPU sur lequel repose Zed.

Chaque panneau s'appuie sur un véritable PTY. Les hooks du cycle de vie des agents alimentent un bus d'événements local, tandis que l'application suit les espaces de travail, les branches, les modifications des dépôts, les notifications et l'état des sessions. Conductor expose ce même état au moyen d'un protocole JSON-RPC local et d'un CLI public. Le serveur MCP fournit une surface plus restreinte, en lecture seule, pour l'inspection entre agents.

L'application est distribuée sous forme de paquets natifs et ne nécessite ni Electron, ni WSL, ni environnement d'exécution hébergé pour les agents.

### Ce que j'ai développé pendant l'OpenAI Build Week

Paneflow existait avant la Build Week. J'utilise donc le commit `e82b3da` du 10 juillet comme référence antérieure à l'événement. Le travail décrit ci-dessous a commencé après l'ouverture de la période de soumission et peut être distingué grâce à des commits datés et des sessions Codex horodatées.

Pendant la Build Week, j'ai utilisé Codex avec GPT-5.6 Sol pour migrer la stack de terminal Linux de Paneflow vers `libghostty-vt`.

La migration comprend :

- Une couche de session de terminal indépendante du backend et contrôlée par Paneflow.
- Un wrapper Rust sûr autour de l'API C évolutive de Ghostty.
- Un cycle de vie PTY complet sous Linux couvrant le lancement, les entrées, le redimensionnement, l'historique, la recherche, la sélection, le presse-papiers, les événements OSC, l'arrêt et la restauration des sessions.
- Ghostty comme backend Linux par défaut, avec Alacritty conservé comme solution de repli explicite.
- Des tests différentiels déterministes qui transmettent les mêmes flux de terminal aux deux backends et comparent leurs sorties normalisées.
- Des cibles de fuzzing pour le rendu, les entrées, le reflow, les séquences malformées et les limites entre fragments.
- Des builds statiques reproductibles pour Linux x86_64 et ARM64.
- Des vérifications CI portant sur les bindings générés, les sources des dépendances, le contenu des paquets, les licences natives et l'isolation multiplateforme.
- Une passe d'amélioration de l'interface couvrant la barre latérale, la barre d'onglets, les surfaces de review, le chrome des fenêtres et le feedback des interactions.

Codex m'a aidé à retracer les comportements à travers Paneflow, Ghostty, Ghostling et Zed. Je l'ai utilisé pour implémenter les lots de la PRD de migration dans l'ordre de leurs dépendances, générer des tests ciblés, diagnostiquer les bugs de redimensionnement et de reflow, examiner les frontières FFI et corriger les échecs du pipeline CI natif.

J'ai pris les décisions d'architecture en amont : GPUI reste responsable du rendu, Ghostty est épinglé et lié statiquement, `cargo build` ne télécharge jamais de code natif, les accès unsafe restent confinés dans de petits wrappers, Linux reçoit le nouveau backend en premier et Alacritty demeure disponible comme solution de repli. Codex a travaillé dans le cadre de ces contraintes.

### Difficultés rencontrées

`libghostty-vt` expose des données de rendu empruntées au travers d'une API C. Une mutation du terminal peut invalider ces données. Aucun pointeur ni slice emprunté ne peut donc franchir la limite d'un verrou ou d'une frame. Le wrapper Rust copie les données dont Paneflow a besoin pendant que le terminal est verrouillé et n'expose au moteur de rendu GPUI que des instantanés possédés.

Ghostty fournit le moteur de terminal, tandis que Paneflow reste responsable du PTY, du cycle de vie des processus, du moteur de rendu, de la persistance, des événements produit et de l'intégration aux différentes plateformes. Les bugs les plus visibles apparaissaient pendant le redimensionnement, le reflow et le déplacement de la barre de défilement : le terminal pouvait sembler correct au repos, puis sauter lorsque ses dimensions changeaient.

Le packaging a représenté une autre partie importante du travail. Un build Paneflow standard doit fonctionner sans Zig ni checkout local de Ghostty. Le pipeline de release produit donc à l'avance des archives statiques épinglées et vérifie leurs headers, bindings, sommes de contrôle, symboles et licences.

### Ce dont je suis fier

Le backend Linux constitue désormais un véritable parcours produit et non plus une expérimentation isolée. Il est utilisé par défaut dans les builds Linux standards, dispose d'une solution de repli immédiate et est distribué sans ajouter de dépendance d'exécution native pour les utilisateurs.

J'ai également utilisé Paneflow pour construire Paneflow. Plusieurs sessions Codex s'exécutaient côte à côte pendant que j'inspectais leur état, examinais les modifications, diagnostiquais les échecs et reprenais le contrôle de certains panneaux lorsque c'était nécessaire.

### Ce que j'ai appris

Codex s'est montré particulièrement utile lorsque la tâche disposait d'une frontière claire, d'invariants explicites et d'une méthode ciblée pour vérifier le résultat. La boucle productive était concrète : reproduire un bug, capturer les logs, confier à Codex une investigation circonscrite, examiner le diff et exécuter la vérification pertinente la plus restreinte.

Cette migration a également confirmé la raison d'être de Paneflow. Dès que plusieurs agents travaillent sur l'architecture, l'implémentation, les tests et la review, observer leur travail devient une partie du problème d'ingénierie.

### Prochaines étapes

Je compte continuer à approfondir Conductor, l'observabilité des agents et la review des worktrees. Mon prochain objectif est de porter le backend Ghostty sur Windows, puis sur macOS, afin de remplacer entièrement Alacritty dans Paneflow.

Dépôt : https://github.com/arthjean/paneflow

Site web : https://paneflow.dev

## Technologies utilisées

- Rust
- GPUI
- Codex
- GPT-5.6 Sol
- libghostty-vt
- Tokio
- portable-pty
- Model Context Protocol (MCP)
- JSON-RPC 2.0
- Serde
- Tree-sitter
- Zig
- GitHub Actions
- Wayland
- X11
