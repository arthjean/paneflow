[PRD]
# PRD: Migration du backend terminal Linux vers libghostty - 2026-Q3

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-07-14 | Arthur Jean | PRD initial pour intégrer libghostty-vt, conserver GPUI et promouvoir Ghostty comme backend Linux par défaut |
| 1.1 | 2026-07-16 | Arthur Jean | Scope fonctionnel clôturé; qualification, packaging, dogfood et promotion extraits vers un PRD dédié |
| 1.2 | 2026-07-16 | Arthur Jean | Migration Linux actée: Ghostty devient le défaut des builds standards, sans processus d'approbation supplémentaire |

## Problem Statement

1. Le terminal Paneflow repose sur alacritty_terminal 0.26 pour le moteur VT, le PTY, l'event loop, les modes, la recherche et une partie des types de rendu. Malgré une première couche de types neutres, SharedTerm reste un Arc/FairMutex/Term Alacritty et huit fichiers disposent encore d'une dérogation explicite. Cette adhérence empêche de remplacer proprement le moteur sur une seule plateforme.
2. Ghostling prouve que libghostty-vt peut alimenter un renderer hôte, mais son exécutable Raylib est une démo POSIX mono-thread. Il ne couvre pas les exigences Paneflow: PTY de production, promotion asynchrone, sessions multi-panes, recherche, persistance, clipboard, événements produit, packaging et rollback.
3. libghostty-vt apporte un moteur moderne avec reflow, graphemes Unicode, render state incrémental, modes, sélection, liens et sémantiques OSC 133. Son API C 0.1.0 reste néanmoins explicitement instable, ses pointeurs empruntés sont invalidés par les mutations et il ne fournit ni PTY, ni event loop, ni cycle de vie enfant.
4. Une bascule directe exposerait les utilisateurs Linux à des régressions difficiles à détecter: signaux Ctrl-C/Ctrl-Z, alt-screen, IME, bracketed paste, mouse reporting, OSC 7/52/133, final-output drain, scrollback, restauration et processus zombies. macOS et Windows ne doivent subir aucun changement fonctionnel pendant cette phase.

**Why now:** Paneflow dispose déjà d'un snapshot de rendu partiellement neutre et Ghostling épingle un commit libghostty-vt concret et compilable. C'est le bon moment pour créer une seam durable, mesurer la parité contre Alacritty et dogfooder la migration depuis Fedora avant d'étendre le travail à macOS puis Windows.

## Overview

La solution introduit une façade Paneflow TerminalSessionBackend, implémentée par un adaptateur Alacritty et, sous Linux uniquement, un adaptateur Ghostty. Le renderer GPUI, les surfaces produit, le format de session, les thèmes et les politiques de sécurité restent inchangés. L'adaptateur Ghostty compose un wrapper Rust sûr autour de libghostty-vt et un transport PTY Linux séparé. Les consommateurs ne voient que des commandes, événements et snapshots appartenant à Paneflow.

La chaîne native est hermétique et statique: source Ghostty au SHA ae52f97dcac558735cfa916ea3965f247e5c6e9e, Zig 0.15.2, header et bindings épinglés, aucun téléchargement pendant cargo build, aucune dépendance libghostty.so au runtime. Les pointeurs C ne franchissent jamais le verrou ni la frame. Les callbacks synchrones poussent uniquement des événements bornés et ne réentrent jamais dans ghostty_terminal_vt_write.

Le scope livré couvre l'intégration fonctionnelle du backend Ghostty sous Linux derrière la feature `libghostty-linux`. Depuis la décision de migration du 2026-07-16, cette feature est activée par défaut: `auto` résout Ghostty dans les builds Linux standards, dont `cargo run`, tandis qu'Alacritty reste un rollback explicite. Aucune session active ne change de moteur en cours d'exécution. macOS et Windows restent Alacritty-only.

## Scope Closure

Ce PRD est DONE depuis le 2026-07-16 pour EP-001 à EP-004: chaîne native, seam backend, wrapper FFI, runtime PTY, intégration de session et parité fonctionnelle. EP-005 est clôturé dans `tasks/prd-linux-libghostty-promotion-2026-Q3.md`, désormais réécrit comme décision de migration Linux DONE. Les anciennes conditions manuelles et temporelles de promotion ont été supprimées.

## Goals

Les objectifs fonctionnels et de rollout Linux sont livrés. Les tests de performance, stress et compatibilité continuent comme contrôles de régression ordinaires.

| Goal | Delivered Target |
|------|------------------|
| Rendre Ghostty standard sous Linux | 100 % des nouvelles sessions Linux `auto` utilisent Ghostty dans les builds standards x86_64 et ARM64 |
| Atteindre la parité Paneflow | 0 divergence non documentée sur le corpus et les contrats P0/P1 de rendu, input, PTY, recherche, sessions, OSC et surfaces agents |
| Préserver le rollback | `terminal.backend = alacritty` sélectionne Alacritty pour les nouvelles sessions Linux |
| Isoler les autres plateformes | macOS et Windows restent Alacritty-only, sans dépendance native Ghostty |

## Target Users

### Développeur Linux multi-agents

- **Role:** utilisateur principal de Paneflow sur Fedora, Ubuntu/Debian, Arch ou openSUSE, sous Wayland ou X11/XWayland.
- **Behaviors:** ouvre plusieurs panes Claude Code, Codex, OpenCode ou shells, utilise recherche, scrollback, copier-coller, IME, TUI plein écran et restauration de workspace.
- **Pain points:** le moteur terminal actuel est profondément couplé à Alacritty et limite l'adoption d'un moteur plus moderne sans risque de régression transversale.
- **Current workaround:** rester sur Alacritty ou tester Ghostling séparément, sans intégration aux workflows Paneflow.
- **Success looks like:** le même workspace et les mêmes CLIs fonctionnent sans différence visible, avec Ghostty sélectionné par défaut et un rollback Alacritty immédiat.

### Mainteneur et release engineer Paneflow

- **Role:** personne qui met à jour le moteur terminal, valide les releases et diagnostique les régressions natives.
- **Behaviors:** travaille avec Cargo, Zig, CI multi-architecture, paquets deb/rpm/AppImage/tar et matrices macOS/Windows.
- **Pain points:** une ABI C mouvante, un PTY incomplet ou une bibliothèque partagée manquante peuvent casser la release hors de la machine de développement.
- **Current workaround:** conserver une dépendance Rust unique et tester manuellement les chemins terminaux les plus risqués.
- **Success looks like:** un pin reproductible, des bindings vérifiés, une matrice différentielle, des diagnostics sans PII et des régressions détectées par la CI normale.

## Research Findings

Key findings that informed this PRD:

### Competitive Context

- **alacritty_terminal 0.26:** moteur Rust stable déjà intégré à Paneflow, avec PTY Unix/ConPTY et event loop. Il reste la référence comportementale et le fallback.
- **libghostty-vt au pin Ghostling:** API C riche pour terminal, rendu, input, sélection, scrollback, OSC et Unicode. L'API annonce encore des breaking changes et ne fournit pas le host PTY.
- **Ghostling f9034e43:** preuve minimale de feed, render state, key/mouse encoders et forkpty sous Linux/macOS. Raylib, le mapping clavier US, l'absence de clipboard complet et le host POSIX l'excluent comme composant embarqué.
- **Market gap:** aucun composant étudié ne remplace à lui seul le moteur, le PTY, le renderer GPUI et les contrats produit Paneflow. La valeur vient de la seam hôte et de la validation différentielle.

### Best Practices Applied

- Isoler le build natif dans un crate -sys avec links = "ghostty-vt", puis placer toute politique d'ownership et de threading dans un wrapper Rust sûr.
- Lier statiquement, épingler source, compilateur, header et bindings, et interdire FetchContent ou un clone réseau pendant cargo build.
- Copier les données de rendu empruntées dans un snapshot Paneflow pendant un verrou exclusif unique. Ne jamais conserver un pointeur C entre deux mutations.
- Alimenter Alacritty et Ghostty avec les mêmes streams, selon plusieurs découpages de chunks, puis comparer un snapshot normalisé à chaque évolution du backend.
- Mesurer séparément débit parser, input-to-frame, CPU, RSS, durée de verrou et cycle de vie PTY. vtebench seul ne mesure ni latence interactive ni framerate.
- Traiter le flux PTY comme non fiable: caps explicites, callbacks non réentrants, paste safety, OSC 52 borné et protocoles Kitty à risque désactivés tant qu'ils ne sont pas nécessaires.

### Existing Codebase Findings

- src-app/Cargo.toml:64-70 déclare alacritty_terminal et polling.
- src-app/src/terminal/types.rs:897-961 contient un guard de confinement couvrant huit fichiers. Le guard doit être durci, jamais supprimé.
- src-app/src/terminal/pty_session.rs concentre spawn, signal mask, PTY, event loop, OSC 7/52, output generation, final drain, scrollback, restauration et teardown.
- src-app/src/terminal/view.rs conserve le flux placeholder -> spawn background -> promotion et batch les événements par tranches de 4 ms et 100 événements.
- src-app/src/terminal/types.rs:504-645 et src-app/src/terminal/element/mod.rs:497-836 forment déjà la cible de snapshot neutre consommée par GPUI.
- src-app/src/search.rs, src-app/src/terminal/search.rs et src-app/src/terminal/element/hyperlink.rs lisent encore directement la grille Alacritty.
- tasks/prd-memory-optimization-2026-Q3-status.json est DONE. Ses profils scrollback, caps et règles de cache sont des contrats à préserver.
- tasks/prd-cli-cockpit-ergonomics-2026-Q3-status.json garde EP-003 OSC 133 en IN_REVIEW. Cette migration ne doit ni dupliquer ni écraser ce travail.
- Le commit Paneflow de référence est 6eba52c9525c35fdaea9bb11dcb0b41561482242. Le commit Ghostling de référence est f9034e43a50a2f3a8101e35497f486090c1ddd6e.

### Primary Sources

- [Ghostling README](https://github.com/ghostty-org/ghostling/blob/f9034e43a50a2f3a8101e35497f486090c1ddd6e/README.md)
- [Ghostling CMake pin](https://github.com/ghostty-org/ghostling/blob/f9034e43a50a2f3a8101e35497f486090c1ddd6e/CMakeLists.txt)
- [libghostty-vt pinned header](https://raw.githubusercontent.com/ghostty-org/ghostty/ae52f97dcac558735cfa916ea3965f247e5c6e9e/include/ghostty/vt.h)
- [libghostty-vt terminal contract](https://raw.githubusercontent.com/ghostty-org/ghostty/ae52f97dcac558735cfa916ea3965f247e5c6e9e/include/ghostty/vt/terminal.h)
- [libghostty-vt render contract](https://raw.githubusercontent.com/ghostty-org/ghostty/ae52f97dcac558735cfa916ea3965f247e5c6e9e/include/ghostty/vt/render.h)
- [libghostty-vt allocator contract](https://raw.githubusercontent.com/ghostty-org/ghostty/ae52f97dcac558735cfa916ea3965f247e5c6e9e/include/ghostty/vt/allocator.h)
- [Cargo native links guidance](https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key)
- [Rustonomicon FFI guidance](https://doc.rust-lang.org/nomicon/ffi.html)
- [Ghostty 1.3 stability and security notes](https://ghostty.org/docs/install/release-notes/1-3-0)
- [Alacritty vtebench scope](https://github.com/alacritty/vtebench)

## Assumptions & Constraints

### Assumptions (to validate)

- Le pin libghostty-vt de Ghostling produit une archive statique Linux x86_64 et ARM64 avec Zig 0.15.2. US-001 valide cette hypothèse avant l'adaptateur.
- portable-pty couvre spawn, cwd/env, read/write, resize, PID, exit et teardown nécessaires sous Linux. US-001 doit le prouver ou sélectionner un host Unix privé derrière la même interface.
- Le snapshot Content actuel peut représenter les données Ghostty nécessaires sans modifier le pipeline paint GPUI.
- Une seule acquisition de verrou par frame suffit pour copier le render state sans contention visible avec huit panes actives.
- Le format texte du scrollback permet de restaurer une session créée avec Alacritty dans Ghostty, et inversement, sans migration de schéma.
- EP-003 OSC 133 du PRD CLI Cockpit sera DONE avant US-014. Si ce n'est pas le cas, US-014 reste bloquée sans réimplémenter ce tracker.

### Hard Constraints

- libghostty-vt est intégré uniquement sous target_os = "linux" dans ce PRD.
- macOS Intel/Apple Silicon et Windows x64/ARM64 continuent d'utiliser Alacritty et ne requièrent ni Zig ni une bibliothèque Ghostty.
- GPUI reste le windowing, l'input host et le renderer. Raylib et main.c de Ghostling ne sont jamais embarqués.
- Le chemin Ghostty ne doit pas utiliser le parser, la grille, l'event loop ou le PTY Alacritty.
- La bibliothèque Ghostty est liée statiquement. Aucun paquet ne dépend de libghostty.so au runtime.
- cargo build ne doit effectuer aucun clone, FetchContent ou téléchargement. Il utilise l'archive statique versionnée et épinglée par cible, ou un répertoire explicitement fourni par `PANEFLOW_LIBGHOSTTY_DIR`.
- Tout unsafe lié à libghostty reste confiné au crate -sys et au wrapper audité. Aucun pointeur ou slice emprunté C ne sort de la durée documentée.
- Le renderer, l'input, la recherche, la persistance et les surfaces agents consomment exclusivement des types Paneflow après EP-001.
- Les caps existants restent au minimum: input pending 64 KiB, OSC 52 100 KiB, 8 opérations clipboard, 4 000 lignes et 400 000 caractères persistés, scrollback par défaut 10 000 lignes.
- L'identité backend est immuable pour une session active. Le fallback automatique n'est permis qu'avant le spawn du child.
- Aucun contenu terminal, commande, cwd, clipboard ou texte de session n'entre dans les diagnostics ou la télémétrie.
- Le dépôt C:/dev/ghostling est une référence read-only. L'implémentation vit uniquement dans un worktree Paneflow isolé.

## Validation technique

Ces commandes restent les contrôles techniques normaux du backend. Elles ne constituent pas une autorisation produit distincte pour activer Ghostty sous Linux:

- `cargo fmt --check` - format Rust obligatoire avant chaque commit et push.
- `cargo clippy --workspace --locked -- -D warnings` - lints workspace sans warning.
- `cargo test --workspace --locked` - tests unitaires, intégration et contrats existants.
- `cargo build -p paneflow-app --release --locked` - build release du chemin Ghostty Linux par défaut.
- `cargo deny check` - licences, advisories, bans et sources Cargo.

For UI and interaction stories:

- Vérification manuelle sur Fedora Wayland natif puis X11 ou XWayland, avec une pane shell, une pane agent et une TUI alt-screen.
- Comparaison des goldens existants sans réécrire les sorties attendues pour masquer une divergence.

## Epics & User Stories

### EP-001: Contrat backend et chaîne native maîtrisée

Créer les fondations mesurables de la migration avant d'introduire le moteur Ghostty dans une session réelle.

**Definition of Done:** le build statique et le transport PTY ont une décision prouvée, l'adaptateur Alacritty passe tous les tests existants derrière une API Paneflow, et un corpus d'au moins 100 streams fixe la baseline.

#### US-001: Valider la chaîne libghostty et le transport PTY Linux

**Description:** As a mainteneur Paneflow, I want prouver le build natif et les primitives PTY avant l'intégration so that les deux risques structurants sont résolus avec des preuves reproductibles.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**

- [x] Given le SHA Ghostty épinglé par Ghostling et Zig 0.15.2, when le spike build Linux x86_64 et ARM64, then une archive statique libghostty-vt et son build info sont produits avec les symboles attendus.
- [x] Given la feature Cargo libghostty-linux, when paneflow-app est construit sous Linux, then elle active la chaîne native épinglée; when elle est absente ou que la cible n'est pas Linux, then aucune dépendance ou commande Ghostty n'est résolue.
- [x] Given deux builds depuis un cache de dépendances propre, when leurs archives, header et bindings sont comparés, then le pin source, le checksum du header et la configuration Zig sont identiques et tracés dans un manifest unique.
- [x] Given portable-pty, when un test Linux couvre spawn avec cwd/env, echo read/write, resize, PID, exit code, signal Ctrl-C, fermeture du groupe et absence de zombie, then la bibliothèque est retenue et sa version est verrouillée dans Cargo.lock.
- [x] Given portable-pty ne couvre pas un invariant P0, when le spike conclut, then un host Unix privé derrière la même interface est documenté comme alternative et la story d'intégration reste bloquée plutôt que de livrer un chemin partiel.
- [x] Given un checkout Linux standard sans Zig ni source Ghostty, when le build démarre, then il utilise l'archive statique versionnée; given une régénération explicite avec un outil, une source ou un checksum invalide, then le script de maintenance échoue avec une action corrective et sans téléchargement depuis `build.rs`.
- [x] Given les sources natives et leurs dépendances SIMD, when l'inventaire est généré, then chaque licence et notice requise est enregistrée pour les paquets Paneflow.

#### US-002: Introduire TerminalSessionBackend et l'adaptateur Alacritty

**Description:** As a mainteneur Paneflow, I want une façade backend appartenant à Paneflow so that le renderer et les workflows ne dépendent plus directement des types Alacritty.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**

- [x] Given les usages actuels, when TerminalSessionBackend est introduit, then ses commandes, événements, snapshots, modes, points, sélection, recherche et source de scrollback utilisent uniquement des types Paneflow.
- [x] Given une session Alacritty, when elle est appelée par la nouvelle façade, then spawn, placeholder, promotion, input, resize, recherche, extraction, restauration et shutdown conservent le comportement actuel.
- [x] Given le renderer GPUI, when il construit un layout, then il reçoit un snapshot neutre et un handle backend, sans Arc/FairMutex/Term Alacritty dans sa signature.
- [x] Given le guard de confinement, when une importation alacritty_terminal apparaît hors de l'adaptateur et de ses tests, then le test échoue avec le chemin fautif.
- [x] Given un backend display-only ou déjà fermé, when write, resize ou shutdown est demandé, then l'opération est un no-op ou une erreur typée selon le contrat, sans panic ni canal bloqué.
- [x] Given les builds macOS et Windows, when la nouvelle enum backend est compilée sans la feature Linux, then seule la variante Alacritty existe et aucun symbole Ghostty n'est référencé.

#### US-003: Créer le corpus Alacritty et les baselines de migration

**Description:** As a release engineer, I want une référence déterministe du comportement actuel so that chaque différence Ghostty est détectée avant de toucher au défaut Linux.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**

- [x] Given au moins 100 streams déterministes sans données utilisateur, when ils sont joués dans l'adaptateur Alacritty, then les snapshots normalisés couvrent ASCII, Unicode, graphemes, wide cells, SGR, couleurs, cursor, wrap, reflow, alt-screen, scrollback, modes, titres et réponses PTY.
- [x] Given chaque stream, when il est découpé par chunks de 1, 7, 64, 4 096 octets et par un découpage pseudo-aléatoire à seed fixe, then le snapshot final et les événements ordonnés restent identiques.
- [x] Given les goldens terminaux existants, when le corpus est exécuté, then leurs attentes restent inchangées et servent de baseline de rendu.
- [x] Given un benchmark parser et un scénario end-to-end à huit panes, when la baseline est capturée, then débit, p50/p95 input-to-frame, CPU, RSS et durée du verrou sont enregistrés avec matériel, build et seed.
- [x] Given des séquences VT tronquées, malformées ou dépassant les caps, when elles sont jouées, then aucun panic, blocage ou allocation non bornée ne survient et le résultat est déterministe.
- [x] Given une fixture issue d'une session réelle, when elle est ajoutée, then commandes, cwd, clipboard, tokens et contenu privé sont remplacés par des données synthétiques avant commit.

---

### EP-002: ABI isolée et moteur Ghostty display-only sûr

Encapsuler l'ABI instable et produire toutes les capacités moteur sans PTY réel, afin que les erreurs FFI soient testées hors du cycle de vie processus.

**Definition of Done:** un terminal Ghostty display-only consomme des bytes, produit le snapshot neutre, encode les inputs et fournit recherche/sélection/scrollback avec zéro raw handle exposé et zéro pointeur emprunté conservé.

#### US-004: Encapsuler l'ABI libghostty dans deux crates privées

**Description:** As a mainteneur Rust, I want séparer les symboles C du wrapper sûr so that les invariants d'ownership, d'allocateur et de threading soient auditables dans un périmètre minimal.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**

- [x] Given paneflow-libghostty-sys, when Cargo résout le crate, then links = "ghostty-vt", les bindings pré-générés et le build statique n'existent que sous Linux.
- [x] Given paneflow-terminal-ghostty, when un terminal, render state, encoder ou iterator est créé, then un type RAII opaque possède le handle et appelle exactement une fonction free dans Drop.
- [x] Given une erreur partielle pendant l'initialisation, when le wrapper retourne, then tous les handles déjà créés sont libérés dans l'ordre inverse sans double-free ni fuite.
- [x] Given une string, un grid ref, une row, une cell ou un buffer emprunté, when le verrou ou l'appel FFI prend fin, then les données nécessaires ont été copiées dans des valeurs Rust et aucun pointeur n'est conservé.
- [x] Given une allocation produite par libghostty, when elle est libérée, then ghostty_free reçoit le même allocateur; Rust et libc ne la libèrent jamais directement.
- [x] Given un callback C qui panic côté Rust, when il est invoqué, then la panic est contenue avant la frontière FFI, un événement d'erreur borné est produit et aucun unwind ne traverse C.
- [x] Given un build info, une version API ou un layout incompatible avec le manifest, when Ghostty s'initialise, then l'initialisation échoue de façon typée avant tout spawn PTY.

#### US-005: Produire le snapshot Ghostty neutre pour GPUI

**Description:** As a renderer Paneflow, I want un Content appartenant à Rust produit depuis le render state Ghostty so that GPUI peint le nouveau moteur sans connaître l'ABI C.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-004

**Acceptance Criteria:**

- [x] Given des bytes VT, when le moteur les reçoit, then ghostty_terminal_vt_write met à jour l'état et snapshot copie cellules, graphemes, styles, couleurs, cursor, modes, sélection et viewport dans les types Paneflow.
- [x] Given une cellule wide, spacer head/tail, combining mark, inverse, dim, bold, italic ou underline, when elle est convertie, then les flags neutres correspondent au contrat des goldens actuels.
- [x] Given un render update, when le snapshot est produit, then l'accès terminal exclusif est acquis une seule fois, les dirty flags globaux et par ligne sont remis à zéro après copie, puis le verrou est relâché avant paint.
- [x] Given les coordonnées Ghostty et les lignes négatives/display_offset attendues par Paneflow, when le viewport contient du scrollback, then la translation est bijective pour hit-testing, sélection, cursor et scrollbar.
- [x] Given un grapheme plus long que le buffer stack initial, when il est lu, then le wrapper redimensionne sous un cap documenté et copie tous les codepoints sans troncature silencieuse.
- [x] Given 0 colonne, 0 ligne ou une dimension supérieure à u16, when création ou resize est demandé, then la valeur est rejetée ou clampée selon TerminalWindowSize sans panic ni appel FFI invalide.

#### US-006: Adapter modes, encodeurs et effets synchrones

**Description:** As a utilisateur de TUI, I want que Ghostty encode les interactions et réponses terminal selon ses modes actifs so that clavier, souris, focus et queries restent compatibles.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**

- [x] Given un KeyInput, MouseInput, focus event ou paste Paneflow, when le backend l'encode, then les bytes tiennent compte de bracketed paste, application cursor, alt-scroll, mouse tracking et Kitty keyboard actifs.
- [x] Given write_pty, bell, title, enquiry, xtversion, size, color scheme ou device attributes, when libghostty déclenche l'effet, then un BackendEvent neutre ou une réponse PTY ordonnée est produit.
- [x] Given une mise à jour title ou pwd, when le moteur la publie, then la string est copiée avant la mutation suivante et passe les mêmes filtres Paneflow qu'Alacritty.
- [x] Given un callback synchrone, when il s'exécute, then il ne rappelle jamais ghostty_terminal_vt_write, ne touche jamais GPUI et termine en moins de 1 ms au p99 dans le benchmark dédié.
- [x] Given une réponse couleur ou taille, when plusieurs queries arrivent dans le même chunk, then les réponses restent dans l'ordre du flux avant tout input utilisateur ultérieur.
- [x] Given une touche, un bouton souris ou un mode inconnu, when l'encodeur ne peut pas le représenter, then il retourne une erreur typée ou zéro byte sans inventer une séquence ni panic.

#### US-007: Adapter grille, recherche, sélection, liens et scrollback

**Description:** As a power user, I want les outils de navigation Paneflow sur la grille Ghostty so that changer de moteur ne supprime aucune capacité de travail.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**

- [x] Given le snapshot neutre, when une recherche texte ou regex est exécutée, then les mêmes matches, limites, coordonnées et règles de casse que le backend Alacritty sont retournés.
- [x] Given une sélection linéaire, bloc ou ligne, when elle traverse wrap, wide cells ou scrollback, then le texte copié et les rectangles peints sont identiques à la baseline.
- [x] Given une cellule OSC 8, when son lien est demandé, then l'URI est copiée sous le verrou, validée par les protections existantes et exposée sans grid ref C persistant.
- [x] Given extract_scrollback, when la grille contient plus de 4 000 lignes ou 400 000 caractères, then seules les données les plus récentes aux frontières UTF-8 et lignes complètes sont retournées.
- [x] Given restore_scrollback depuis une session, when le texte contient ESC, CSI, OSC, DCS ou C1 hostile, then la sanitation existante les neutralise avant feed et aucun titre ou lien actif n'est créé.
- [x] Given une référence de grille invalidée par resize, trim ou alt-screen, when recherche ou sélection la consulte, then elle est abandonnée proprement sans use-after-free, panic ni point hors grille.

---

### EP-003: Runtime PTY Linux et intégration de session

Connecter le moteur sûr à un vrai shell Linux tout en conservant la promotion asynchrone, les signaux et la gestion des processus de Paneflow.

**Definition of Done:** une session Ghostty sur Fedora spawn un shell hors thread, rejoue l'input pending, rend l'output, resize le PTY, publie un exit unique, draine l'output final et ferme le groupe sans zombie.

#### US-008: Implémenter le host PTY Linux portable

**Description:** As a utilisateur Linux, I want un PTY indépendant d'Alacritty so that le chemin Ghostty remplace réellement le backend terminal actuel.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001, US-002

**Acceptance Criteria:**

- [x] Given la décision US-001, when le host ouvre un shell, then il utilise portable-pty validé ou l'alternative Unix isolée, jamais alacritty_terminal::tty ni le forkpty copié de Ghostling.
- [x] Given SpawnParams, when le child démarre, then shell, args, cwd, env, TERM=xterm-256color, COLORTERM, TERM_PROGRAM et variables protégées suivent le contrat actuel.
- [x] Given un spawn depuis le background executor, when le child hérite des signaux, then le masque foreground est appliqué autour du spawn et restauré ensuite, afin que Ctrl-C et Ctrl-Z fonctionnent.
- [x] Given une taille TerminalWindowSize, when le host resize, then rows, cols et pixels atteignent le PTY et le child reçoit un changement de taille sans événement dupliqué.
- [x] Given le PID du shell, when Paneflow surveille ou ferme la pane, then parent guard, foreground command, groupe de processus et stratégie de teardown existants restent disponibles.
- [x] Given cwd invalide, shell absent, permission refusée ou limite PTY atteinte, when le spawn échoue, then aucun second child n'est créé, les handles sont fermés et l'erreur existante reste visible dans la pane.

#### US-009: Construire la pompe I/O bornée et le cycle de vie

**Description:** As a mainteneur runtime, I want une boucle I/O qui sérialise PTY, scanner, moteur et événements so that le flux reste ordonné et borné sous charge ou fermeture.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004, US-006, US-008

**Acceptance Criteria:**

- [x] Given des bytes lus du PTY, when la pompe les traite, then scanner borné, ghostty_terminal_vt_write, effets synchrones et Wakeup suivent l'ordre exact du flux.
- [x] Given input utilisateur et réponses générées par le terminal, when les deux attendent un write, then les réponses protocolaires conservent leur ordre relatif et les partial writes sont rejoués sans perte.
- [x] Given une rafale d'output, when les événements atteignent la vue, then Wakeup est coalescé, le drain respecte 4 ms ou 100 événements par tick et les événements Exit/Title/Cwd ne sont jamais supprimés.
- [x] Given EAGAIN, EINTR, short read, short write ou broken pipe, when l'I/O continue ou se termine, then aucune boucle active, perte silencieuse non tracée ou allocation non bornée ne survient.
- [x] Given un child qui sort après une dernière rafale, when EOF et exit arrivent, then l'output final est drainé pendant au plus 2 secondes et un seul événement ChildExited est publié avec code ou signal.
- [x] Given shutdown, drop ou fermeture forcée, when le child résiste, then le ladder de terminaison se termine en moins de 2 secondes, ferme tous les descripteurs et laisse 0 zombie.
- [x] Given un OSC 7/52/133 tronqué ou dépassant son cap, when le scanner reçoit plusieurs chunks, then il se réinitialise proprement sans bloquer le feed VT ni conserver un payload non borné.

#### US-010: Composer le backend Ghostty avec placeholder et promotion

**Description:** As a utilisateur Paneflow, I want que la nouvelle session apparaisse immédiatement puis soit promue sans remplacement visuel so that le spawn natif ne bloque jamais GPUI.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-005, US-007, US-009

**Acceptance Criteria:**

- [x] Given une nouvelle pane Ghostty, when elle est créée, then un moteur display-only et la même entité TerminalView s'affichent avant le spawn background.
- [x] Given des inputs avant promotion, when leur total reste sous 64 KiB, then ils sont bufferisés puis rejoués exactement une fois et dans l'ordre après attachement du PTY.
- [x] Given plus de 64 KiB avant promotion, when le cap est atteint, then l'excédent est rejeté selon le comportement documenté sans croissance mémoire ni blocage de la vue.
- [x] Given une initialisation libghostty impossible, when aucun child n'a encore été spawn, then la session choisit Alacritty, crée exactement un child et enregistre une raison de fallback structurée.
- [x] Given le PTY échoue après initialisation Ghostty, when la promotion ne peut pas aboutir, then la pane conserve l'overlay d'erreur existant et ne lance pas un second shell Alacritty.
- [x] Given une session promue, when rendu, recherche, resize, input ou shutdown est demandé, then tous les appels vont vers la même identité Ghostty jusqu'à sa fermeture.

#### US-011: Ajouter sélection backend, configuration et diagnostics

**Description:** As a utilisateur et release engineer, I want sélectionner et diagnostiquer le backend sans ambiguïté so that le défaut Linux, le rollback et les builds non-Linux restent sûrs.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002, US-010

**Acceptance Criteria:**

- [x] Given terminal.backend = auto, ghostty ou alacritty, when la config est chargée, then une enum validée et documentée décide du backend de chaque nouvelle session.
- [x] Given un build Linux standard, when `auto` est résolu, then Ghostty est sélectionné; given un build Linux avec `--no-default-features`, then Alacritty reste disponible pour le développement et le diagnostic.
- [x] Given macOS ou Windows avec terminal.backend = ghostty dans une config partagée, when une session démarre, then Alacritty est utilisé avec un warning unique et l'application continue sans Zig ni symbole Ghostty.
- [x] Given une valeur inconnue ou une feature absente, when la config est résolue, then le diagnostic existant signale la valeur, Alacritty reste utilisable et aucun crash de démarrage ne survient.
- [x] Given les diagnostics terminal, when ils sont consultés, then backend demandé/résolu, fallback reason, SHA, version API, Zig, optimisation et SIMD sont visibles sans texte terminal, cwd, commande ni clipboard.
- [x] Given une modification de config pendant une session active, when la config est rechargée, then la session conserve son identité; seules les nouvelles sessions utilisent le nouveau choix.

---

### EP-004: Parité Paneflow sur Linux

Fermer les écarts visibles et produit avant la bascule Linux par défaut, sans utiliser les nouvelles capacités Ghostty comme prétexte à modifier l'expérience.

**Definition of Done:** Ghostty produit zéro divergence non documentée sur le corpus, les goldens et les contrats P0/P1 de rendu, input, protocoles, recherche, agents et persistance.

#### US-012: Atteindre la parité rendu, Unicode et resize

**Description:** As a utilisateur Linux, I want un affichage identique ou explicitement corrigé so that mes shells et TUIs ne changent pas visuellement avec le backend.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-010

**Acceptance Criteria:**

- [x] Given le corpus de référence, when Alacritty et Ghostty produisent leurs snapshots, then cellules, graphemes, styles, couleurs, cursor, modes, wrap, alt-screen et scrollback ont 0 divergence non documentée.
- [x] Given les goldens plain, ANSI16, indexed256, truecolor, inverse, dim, wide/CJK, sélection et formes de cursor, when Ghostty les alimente, then le pipeline paint existant produit les mêmes sorties attendues.
- [x] Given graphemes combinés, emoji, ZWJ, double-width et caractères invalides, when ils sont rendus et sélectionnés, then aucune cellule ne se décale et le remplacement invalide est déterministe.
- [x] Given un resize sur l'écran primaire, when les colonnes changent, then les lignes wrappées reflowent et les coordonnées scrollback restent cohérentes; l'écran alternatif ne reflowe pas.
- [x] Given une tempête de 200 resizes ou une surface 0x0 transitoire, when les événements sont coalescés, then aucun panic, deadlock, SIGWINCH incohérent ou cursor hors grille ne survient.
- [x] Given huit panes avec output simultané, when GPUI peint 60 frames, then chaque pane effectue au plus une acquisition de verrou terminal par frame.

#### US-013: Atteindre la parité input, clipboard et protocoles

**Description:** As a utilisateur de CLI et TUI, I want que toutes mes interactions produisent les mêmes bytes et effets so that aucun outil ne détecte un terminal dégradé.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-006, US-009, US-010

**Acceptance Criteria:**

- [x] Given les keybindings, IME, marked text, dead keys et paste actuels, when ils traversent Ghostty, then les bytes PTY et les événements GPUI correspondent à la baseline Alacritty.
- [x] Given bracketed paste actif ou inactif, when du texte multi-ligne ou des caractères de contrôle sont collés, then la politique Paneflow et la validation Ghostty empêchent une soumission implicite et préservent les newlines attendues.
- [x] Given mouse tracking, SGR mouse, focus reports ou alt-scroll, when l'utilisateur clique, drag, scroll ou change le focus, then les coordonnées et séquences restent conformes aux modes actifs.
- [x] Given OSC 52 avec le mode par défaut CopyOnly, when un programme écrit jusqu'à 100 KiB, then au plus 8 opérations sont mises en attente; les reads sont refusés et tout payload supérieur est ignoré.
- [x] Given OSC 4/10/11/12, DA, DSR ou size queries dans un même flux, when les réponses sont générées, then elles arrivent dans l'ordre et avant l'input utilisateur suivant.
- [x] Given Kitty graphics file, temp file ou shared memory, when un payload arrive en v1, then le stockage reste à 0, les mediums restent désactivés et le parser respecte un cap APC explicite.
- [x] Given une séquence input, OSC ou APC malformée, when elle traverse des limites de chunks arbitraires, then elle ne provoque ni panic, allocation non bornée, clipboard action ni byte PTY inventé.

#### US-014: Préserver les workflows Paneflow, OSC 133 et les sessions

**Description:** As a utilisateur multi-agents, I want retrouver les contrats produit au-dessus du terminal so that le changement de moteur reste invisible dans mes workflows Paneflow.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-007, US-010, US-013; external prerequisite: tasks/prd-cli-cockpit-ergonomics-2026-Q3 EP-003 must be DONE

**Acceptance Criteria:**

- [x] Given EP-003 OSC 133 du PRD CLI Cockpit, when US-014 démarre, then son status est DONE et ses CommandMark/exit codes sont consommés ou adaptés, jamais réimplémentés dans un second store.
- [x] Given OSC 7, title et process fallback, when le shell change de cwd, then la sidebar, le split cwd et la restauration reçoivent la même valeur validée que sous Alacritty.
- [x] Given OSC 133 absent, désactivé ou malformé, when une commande s'exécute, then le terminal fonctionne sans marks, sans erreur visible et sans allocation par chunk dédiée au cas absent.
- [x] Given output terminal, when Wakeup est traité, then output_generation, activity burst, service detection, ports, waiting state et child exit suivent les mêmes transitions.
- [x] Given recherche locale, fleet search, copy-mode et hyperlinks, when ils parcourent Ghostty, then résultats, caps, navigation et priorités OSC8/URL/path restent identiques.
- [x] Given save/restore, when le scrollback vient de l'autre backend, then 4 000 lignes et 400 000 caractères maximum sont restaurés après sanitation, sans changement du format session.
- [x] Given les profils Normal, CachedAgent et Review, when un terminal est créé, caché ou libéré, then les budgets de scrollback et cache du PRD mémoire DONE restent appliqués.
- [x] Given un child qui sort sans input ou après input utilisateur, when should_close_on_exit est évalué, then l'overlay et la fermeture de pane gardent la logique actuelle, y compris code non nul et signal.

---

### Follow-up: migration Linux

EP-005 est DONE dans `tasks/prd-linux-libghostty-promotion-2026-Q3.md`. Il active Ghostty par défaut sur tous les chemins Linux standards, conserve le rollback Alacritty et laisse macOS/Windows inchangés. Les anciens rapports et conditions de promotion ne font plus partie du rollout.

## Functional Requirements

- FR-01: Le système DOIT exposer terminal.backend = auto, ghostty ou alacritty pour les nouvelles sessions.
- FR-02: auto DOIT sélectionner Ghostty dans les builds Linux standards; Alacritty reste sélectionné sur macOS, Windows et les builds Linux explicites avec `--no-default-features`.
- FR-03: Les consommateurs UI/produit DOIVENT utiliser exclusivement commandes, événements, points, modes, snapshots et handles Paneflow.
- FR-04: Le chemin Ghostty DOIT épingler source, Zig, header, bindings, build info et licences.
- FR-05: libghostty-vt DOIT être liée statiquement sans dépendance .so distribuée.
- FR-06: Les handles, allocateurs, callbacks et pointeurs empruntés DOIVENT être confinés aux crates natives.
- FR-07: Le backend Ghostty DOIT utiliser un PTY Linux indépendant d'Alacritty.
- FR-08: Spawn background, placeholder, promotion, pending input, resize, final drain, exit status et teardown DOIVENT conserver le contrat actuel.
- FR-09: GPUI DOIT continuer à peindre le snapshot Content existant sans renderer Raylib ou GTK.
- FR-10: Clavier, IME, paste, souris, focus, alt-scroll et modes DOIVENT conserver leurs bytes et règles actuels.
- FR-11: OSC 7, OSC 8, OSC 52, OSC 133, color/size queries et protocol replies DOIVENT préserver ordre, caps et sécurité.
- FR-12: Recherche, copy-mode, sélection, liens, fleet search, services, ports et surfaces agents DOIVENT fonctionner sur les deux backends.
- FR-13: Le format de session et les caps de scrollback persisté NE DOIVENT PAS changer.
- FR-14: Un échec Ghostty avant spawn DOIT fallback vers Alacritty avec un child unique et une raison structurée.
- FR-15: Une session active NE DOIT PAS changer de backend ni tenter une migration de grille en mémoire.
- FR-16: Les diagnostics DOIVENT inclure identité, version et fallback sans contenu utilisateur.
- FR-17: Le travail DOIT réutiliser les contrats des PRDs mémoire, CLI Cockpit et control-plane existants sans modifier leurs statuts.
- FR-18: Le dépôt Ghostling, Raylib et l'ancien worktree Hera NE DOIVENT PAS devenir des dépendances runtime ou des sources copiées aveuglément.

## Non-Functional Requirements

- **Verrou terminal:** au plus 1 acquisition par pane et par frame.
- **Backpressure:** input pré-promotion <= 64 KiB, clipboard queue <= 8 opérations, OSC 52 <= 100 KiB, événement batch <= 100 par tick et <= 4 ms.
- **Persistance:** extraction <= 4 000 lignes et <= 400 000 caractères, sans coupe UTF-8 invalide.
- **Reliability runtime:** shutdown et final drain <= 2 s chacun.
- **Security:** 0 panic traversant FFI, 0 pointeur emprunté conservé après mutation, Kitty image storage = 0 et file/temp/shared-memory désactivés.
- **Privacy:** 0 byte de contenu terminal, commande, cwd ou clipboard dans logs, métriques, fixtures commitées ou télémétrie.
- **Scalability:** aucune queue non bornée avec jusqu'à 32 panes, et les budgets restent tenus dans le benchmark de 8 panes actives.
- **Input accessibility:** 100 % des fixtures clavier, IME, dead keys, mouse et paste existantes passent sur Ghostty.

Les tests de performance, stress, compatibilité CI, distribution et reproductibilité restent exécutés comme contrôles de régression du backend Linux.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Entrée native absente | Archive versionnée, archive générée et override explicite absents | Build arrêté avant link, aucune récupération réseau | Message indiquant les trois emplacements acceptés |
| 2 | Pin ou ABI incohérent | Header, build info, symboles ou layouts diffèrent | Initialisation refusée, CI rouge | "libghostty ABI mismatch; regenerate and review bindings" |
| 3 | Ghostty ne s'initialise pas | Allocation ou validation échoue avant spawn | Fallback Alacritty, un seul child | Warning diagnostic, pane utilisable |
| 4 | PTY ne spawn pas | Shell/cwd/permission/limite PTY | Aucun retry Alacritty, handles fermés, overlay actuel | Erreur de spawn existante avec cause |
| 5 | Callback Rust panic | Effet synchrone imprévu | Panic contenue, event error borné, pas d'unwind C | Diagnostic backend, session non basculée |
| 6 | Flux VT hostile | OSC/APC tronqué, payload géant, chunks adverses | Cap, reset scanner, pas de panic/allocation non bornée | Aucun message utilisateur |
| 7 | Clipboard hostile | OSC 52 > 100 KiB ou read en CopyOnly | Ignoré, queue <= 8, aucun clipboard read | Aucun message utilisateur |
| 8 | Resize extrême | 0x0, > u16 ou 200 resizes rapides | Clamp/rejet, coalescing, cursor et PTY cohérents | Aucun message utilisateur |
| 9 | Sortie pendant fermeture | Derniers bytes puis EOF/exit | Drain <= 2 s, output visible, exit unique | Overlay exit actuel |
| 10 | Backpressure input | > 64 KiB avant promotion | Excédent rejeté, mémoire bornée | Diagnostic debug, pas de toast répétitif |
| 11 | Référence de grille périmée | Reflow, trim ou alt-screen | Référence abandonnée, recalcul ou no-op | Aucun message utilisateur |
| 12 | Config partagée non-Linux | ghostty demandé sur macOS/Windows | Alacritty, warning unique, pas de Zig | "Ghostty backend is Linux-only; using Alacritty" |
| 13 | Restore cross-backend hostile | Session texte avec escapes ou troncature | Sanitation, feed texte seulement, caps appliqués | Aucune exécution de séquence |
| 14 | Runtime Ghostty défaillant après spawn | Erreur non récupérable de moteur | Pas de bascule en mémoire; pane fautive arrêtée proprement et rollback pour nouvelles sessions | "Ghostty backend failed; reopen with terminal.backend=alacritty" |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | ABI libghostty-vt instable | High | High | Pin exact, bindings commités, checksum header, layout tests, build info et upgrade drill |
| 2 | UB, use-after-free ou mauvais allocateur FFI | Med | High | Crate -sys minimal, RAII, copies sous verrou, ghostty_free, callbacks catch_unwind, audit ciblé |
| 3 | Régression PTY/signaux/process group | Med | High | Spike portable-pty, tests Ctrl-C/Ctrl-Z, final drain, stress 200 cycles et contrôle zombies |
| 4 | Écart caché input/OSC/alt-screen | High | High | Corpus >= 100 streams, chunk matrix, goldens, fuzz différentiel et stories de parité dédiées |
| 5 | Dual-backend devient permanent et coûteux | High | Med | Interface unique, Alacritty confiné; suppression Linux traitée dans une décision séparée |
| 6 | Build natif ou packaging non reproductible | Med | High | Source/toolchain/cache épinglés, static link, aucun réseau build.rs, smokes de chaque format |
| 7 | Régression performance à plusieurs panes | Med | High | Baseline US-003, seuils relatifs, verrou unique et benchmark 8 panes en CI |
| 8 | Collision avec OSC 133 en IN_REVIEW | Med | Med | Gate externe DONE avant US-014, réutilisation du store existant, aucun changement de son tracker |
| 9 | Crash natif impossible à fallback en cours de session | Low | High | Pas de promesse de hot fallback, fuzz, stress PTY et rollback config pour nouvelles sessions |
| 10 | Source Ghostty ou dépendances augmentent fortement le binaire | Med | Med | Budget suivi, inventaire des licences et mesure du delta par artefact |

## Non-Goals

Explicit boundaries for this PRD:

- Intégrer Ghostty sur macOS ou Windows. Ces ports feront l'objet de PRDs distincts après stabilisation Linux.
- Embarquer l'exécutable Ghostling, Raylib, sa fenêtre, son renderer, son mapping clavier ou son forkpty.
- Remplacer GPUI, les modules paint, les thèmes, le système de polices ou les keybindings Paneflow.
- Activer ou rendre les images Kitty. Les mediums file/temp/shared-memory et le stockage image restent désactivés.
- Ajouter de nouvelles fonctions visibles comme ligatures, shaders, tabs Ghostty, split natif Ghostty ou configuration Ghostty.
- Supprimer alacritty_terminal du workspace. Il reste le backend macOS/Windows et le rollback Linux.
- Basculer automatiquement une session active de Ghostty vers Alacritty après le spawn.
- Modifier le format session, persister un dump natif Ghostty ou migrer une grille en mémoire.
- Refaire EP-003 OSC 133, les budgets mémoire ou les stories control-plane déjà suivies ailleurs.
- Contribuer des changements upstream à Ghostty, Ghostling, Alacritty ou portable-pty dans ce périmètre.

## Files NOT to Modify

- C:/dev/ghostling/** - dépôt de référence read-only; aucun changement ni vendoring de main.c/Raylib.
- C:/dev/paneflow-hera-m6/** - ancien worktree expérimental supprimé de main; référence éventuelle uniquement.
- src-app/src/terminal/element/paint/** - renderer GPUI déjà neutre; corriger l'adaptateur/snapshot plutôt que masquer une divergence dans paint.
- src-app/src/terminal/element/font.rs, color.rs et geometry.rs - aucune refonte font/color/layout dans une migration backend.
- src-app/src/terminal/element/golden/*.txt existants - ne pas modifier les attentes pour faire passer Ghostty; ajouter des fixtures séparées si nécessaire.
- src-app/src/keybindings/** - aucune nouvelle combinaison ou remap dans ce PRD.
- src-app/src/update/** et les étapes de signature release - la migration ne change ni updater, ni publisher, ni trust model.
- tasks/prd-memory-optimization-2026-Q3* et tasks/prd-cli-cockpit-ergonomics-2026-Q3* - lire leurs contrats, ne pas modifier leurs PRD/status.
- Toute source upstream Alacritty ou Ghostty hors du pin géré - aucun fork opportuniste dans cette migration.

## Technical Considerations

- **Architecture:** faut-il un trait object ou une enum fermée pour TerminalSessionBackend? Recommandé: enum fermée feature-gated, méthodes communes Paneflow, afin de garder un hot path exhaustif sans exposer l'ABI.
- **Module depth:** faut-il séparer TerminalHost et TerminalEngine publiquement? Recommandé: une façade session publique; host PTY et engine restent des sous-modules privés de l'adaptateur Ghostty.
- **Native source:** comment construire sans réseau pendant Cargo? Solution livrée: archives statiques vérifiées et versionnées pour x86_64/ARM64, avec génération depuis le SHA épinglé dans les workflows de maintenance et release.
- **FFI:** faut-il générer les bindings sur chaque machine? Recommandé: bindings générés et commités, régénération CI dédiée seulement, afin de ne pas imposer libclang aux utilisateurs.
- **PTY:** portable-pty couvre-t-il tous les invariants Linux actuels? Recommandé: validation US-001; si un invariant P0 manque, implémentation Unix privée derrière TerminalHost sans changer la façade.
- **Concurrency:** faut-il rendre Ghostty Sync? Recommandé: non. Le terminal est sérialisé, le render state est copié sous une acquisition exclusive et seul le snapshot Rust sort du verrou.
- **Events:** comment gérer les callbacks synchrones? Recommandé: callback minimal, catch_unwind, copie bornée, queue BackendEvent et interdiction de reentrancy/GPUI.
- **OSC 52/133:** l'API C expose-t-elle assez d'information au pin? Recommandé: utiliser les sémantiques natives disponibles et conserver un scanner streaming borné uniquement pour les données absentes, notamment clipboard et exit code.
- **Data model:** faut-il persister le backend par pane? Recommandé: non en v1. La session persiste le scrollback texte et résout le backend courant au restore; l'identité n'est qu'un diagnostic runtime.
- **Config:** comment préserver une config partagée entre OS? Recommandé: enum globale auto/ghostty/alacritty, résolution ghostty limitée à Linux, warning unique et fallback Alacritty ailleurs.
- **Migration et upgrade:** `auto` utilise Ghostty dans les builds Linux standards. Le rollback reste explicite; tout nouveau SHA repasse les contrôles ABI, corpus, packages et liaison statique.

## Success Metrics

| Metric | Target de clôture | How Measured |
|--------|-------------------|--------------|
| Backend Linux par défaut | `terminal.backend = auto` crée une session Ghostty complète dans les builds standards | Tests backend et exécution locale |
| Parité fonctionnelle | Rendu, input, recherche, sélection, OSC, sessions et workflows Paneflow utilisables | Corpus, tests ciblés et usage réel |
| Isolation plateforme | macOS et Windows restent sur Alacritty; aucun changement de backend actif en cours de session | Graphe Cargo target-specific et résolution de configuration |
| Sécurité runtime | FFI confinée, données C copiées, caps et sanitation conservés | Tests de contrats et inspection du wrapper |

La migration Linux par défaut est suivie comme DONE dans `tasks/prd-linux-libghostty-promotion-2026-Q3.md`.

## Open Questions

Aucune question ouverte dans le scope fonctionnel clôturé. La stabilité future de l'API C, l'extension à macOS/Windows et le retrait éventuel d'Alacritty relèvent de décisions séparées.
[/PRD]
