[PRD]
# PRD: Migration du backend terminal Linux vers libghostty - 2026-Q3

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-07-14 | Arthur Jean | PRD initial pour intégrer libghostty-vt, conserver GPUI et promouvoir Ghostty comme backend Linux par défaut |

## Problem Statement

1. Le terminal Paneflow repose sur alacritty_terminal 0.26 pour le moteur VT, le PTY, l'event loop, les modes, la recherche et une partie des types de rendu. Malgré une première couche de types neutres, SharedTerm reste un Arc/FairMutex/Term Alacritty et huit fichiers disposent encore d'une dérogation explicite. Cette adhérence empêche de remplacer proprement le moteur sur une seule plateforme.
2. Ghostling prouve que libghostty-vt peut alimenter un renderer hôte, mais son exécutable Raylib est une démo POSIX mono-thread. Il ne couvre pas les exigences Paneflow: PTY de production, promotion asynchrone, sessions multi-panes, recherche, persistance, clipboard, événements produit, packaging et rollback.
3. libghostty-vt apporte un moteur moderne avec reflow, graphemes Unicode, render state incrémental, modes, sélection, liens et sémantiques OSC 133. Son API C 0.1.0 reste néanmoins explicitement instable, ses pointeurs empruntés sont invalidés par les mutations et il ne fournit ni PTY, ni event loop, ni cycle de vie enfant.
4. Une bascule directe exposerait les utilisateurs Linux à des régressions difficiles à détecter: signaux Ctrl-C/Ctrl-Z, alt-screen, IME, bracketed paste, mouse reporting, OSC 7/52/133, final-output drain, scrollback, restauration et processus zombies. macOS et Windows ne doivent subir aucun changement fonctionnel pendant cette phase.

**Why now:** Paneflow dispose déjà d'un snapshot de rendu partiellement neutre et Ghostling épingle un commit libghostty-vt concret et compilable. C'est le bon moment pour créer une seam durable, mesurer la parité contre Alacritty et dogfooder la migration depuis Fedora avant d'étendre le travail à macOS puis Windows.

## Overview

La solution introduit une façade Paneflow TerminalSessionBackend, implémentée par un adaptateur Alacritty et, sous Linux uniquement, un adaptateur Ghostty. Le renderer GPUI, les surfaces produit, le format de session, les thèmes et les politiques de sécurité restent inchangés. L'adaptateur Ghostty compose un wrapper Rust sûr autour de libghostty-vt et un transport PTY Linux séparé. Les consommateurs ne voient que des commandes, événements et snapshots appartenant à Paneflow.

La chaîne native est hermétique et statique: source Ghostty au SHA ae52f97dcac558735cfa916ea3965f247e5c6e9e, Zig 0.15.2, header et bindings épinglés, aucun téléchargement pendant cargo build, aucune dépendance libghostty.so au runtime. Les pointeurs C ne franchissent jamais le verrou ni la frame. Les callbacks synchrones poussent uniquement des événements bornés et ne réentrent jamais dans ghostty_terminal_vt_write.

Le rollout comporte trois états. D'abord, auto continue de résoudre Alacritty et ghostty est un opt-in Linux derrière la feature libghostty-linux. Ensuite, CI, packaging, corpus différentiel, fuzz, benchmarks et dogfood valident le backend. Enfin, auto résout Ghostty pour toute nouvelle session Linux. Alacritty reste sélectionnable comme rollback Linux et demeure l'unique backend macOS/Windows. Aucune session active ne change de moteur en cours d'exécution.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Rendre Ghostty utilisable sous Linux | Opt-in fonctionnel sur Fedora x86_64 avec fallback Alacritty avant spawn | 100 % des nouvelles sessions Linux en mode auto utilisent Ghostty dans les artefacts x86_64 et ARM64 |
| Atteindre la parité Paneflow | 0 divergence non documentée sur au moins 100 streams déterministes et tous les goldens terminaux existants | 0 régression P0/P1 sur rendu, input, PTY, recherche, sessions, OSC et surfaces agents |
| Tenir les budgets de performance | Débit parser >= 95 % d'Alacritty, régression p95 input-to-frame <= 5 %, CPU et RSS <= 10 % | Les mêmes seuils sont tenus avec 8 panes actives et le scrollback configuré à 10 000 lignes |
| Prouver la fiabilité du rollout | 200 cycles spawn/resize/close sans crash, deadlock, fuite de child ni zombie | 30 jours de dogfood, au moins 200 sessions démarrées, 0 crash natif, 0 deadlock et 0 fallback d'initialisation inexpliqué |

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
- **Success looks like:** un pin reproductible, des bindings vérifiés, une matrice différentielle, des diagnostics sans PII et une promotion bloquée automatiquement si une gate échoue.

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
- Alimenter Alacritty et Ghostty avec les mêmes streams, selon plusieurs découpages de chunks, puis comparer un snapshot normalisé avant toute promotion.
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
- cargo build ne doit effectuer aucun clone, FetchContent ou téléchargement. Les dépendances natives sont préparées et vérifiées avant le build.
- Tout unsafe lié à libghostty reste confiné au crate -sys et au wrapper audité. Aucun pointeur ou slice emprunté C ne sort de la durée documentée.
- Le renderer, l'input, la recherche, la persistance et les surfaces agents consomment exclusivement des types Paneflow après EP-001.
- Les caps existants restent au minimum: input pending 64 KiB, OSC 52 100 KiB, 8 opérations clipboard, 4 000 lignes et 400 000 caractères persistés, scrollback par défaut 10 000 lignes.
- L'identité backend est immuable pour une session active. Le fallback automatique n'est permis qu'avant le spawn du child.
- Aucun contenu terminal, commande, cwd, clipboard ou texte de session n'entre dans les diagnostics ou la télémétrie.
- Le dépôt C:/dev/ghostling est une référence read-only. L'implémentation vit uniquement dans un worktree Paneflow isolé.

## Quality Gates

These commands must pass for every user story:

- `cargo fmt --check` - format Rust obligatoire avant chaque commit et push.
- `cargo clippy --workspace --locked -- -D warnings` - lints workspace sans warning.
- `cargo test --workspace --locked` - tests unitaires, intégration et contrats existants.
- `cargo build -p paneflow-app --release --locked --features libghostty-linux` - build release du chemin Ghostty Linux.
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

- [ ] Given le SHA Ghostty épinglé par Ghostling et Zig 0.15.2, when le spike build Linux x86_64 et ARM64, then une archive statique libghostty-vt et son build info sont produits avec les symboles attendus.
- [ ] Given la feature Cargo libghostty-linux, when paneflow-app est construit sous Linux, then elle active la chaîne native épinglée; when elle est absente ou que la cible n'est pas Linux, then aucune dépendance ou commande Ghostty n'est résolue.
- [ ] Given deux builds depuis un cache de dépendances propre, when leurs archives, header et bindings sont comparés, then le pin source, le checksum du header et la configuration Zig sont identiques et tracés dans un manifest unique.
- [ ] Given portable-pty, when un test Linux couvre spawn avec cwd/env, echo read/write, resize, PID, exit code, signal Ctrl-C, fermeture du groupe et absence de zombie, then la bibliothèque est retenue et sa version est verrouillée dans Cargo.lock.
- [ ] Given portable-pty ne couvre pas un invariant P0, when le spike conclut, then un host Unix privé derrière la même interface est documenté comme alternative et la story d'intégration reste bloquée plutôt que de livrer un chemin partiel.
- [ ] Given Zig absent, d'une version différente, la source Ghostty manquante ou un checksum invalide, when le build démarre, then il échoue avant le link avec un message indiquant la version et l'action corrective, sans téléchargement automatique.
- [ ] Given les sources natives et leurs dépendances SIMD, when l'inventaire est généré, then chaque licence et notice requise est enregistrée pour les paquets Paneflow.

#### US-002: Introduire TerminalSessionBackend et l'adaptateur Alacritty

**Description:** As a mainteneur Paneflow, I want une façade backend appartenant à Paneflow so that le renderer et les workflows ne dépendent plus directement des types Alacritty.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**

- [ ] Given les usages actuels, when TerminalSessionBackend est introduit, then ses commandes, événements, snapshots, modes, points, sélection, recherche et source de scrollback utilisent uniquement des types Paneflow.
- [ ] Given une session Alacritty, when elle est appelée par la nouvelle façade, then spawn, placeholder, promotion, input, resize, recherche, extraction, restauration et shutdown conservent le comportement actuel.
- [ ] Given le renderer GPUI, when il construit un layout, then il reçoit un snapshot neutre et un handle backend, sans Arc/FairMutex/Term Alacritty dans sa signature.
- [ ] Given le guard de confinement, when une importation alacritty_terminal apparaît hors de l'adaptateur et de ses tests, then le test échoue avec le chemin fautif.
- [ ] Given un backend display-only ou déjà fermé, when write, resize ou shutdown est demandé, then l'opération est un no-op ou une erreur typée selon le contrat, sans panic ni canal bloqué.
- [ ] Given les builds macOS et Windows, when la nouvelle enum backend est compilée sans la feature Linux, then seule la variante Alacritty existe et aucun symbole Ghostty n'est référencé.

#### US-003: Créer le corpus Alacritty et les baselines de migration

**Description:** As a release engineer, I want une référence déterministe du comportement actuel so that chaque différence Ghostty est détectée avant de toucher au défaut Linux.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**

- [ ] Given au moins 100 streams déterministes sans données utilisateur, when ils sont joués dans l'adaptateur Alacritty, then les snapshots normalisés couvrent ASCII, Unicode, graphemes, wide cells, SGR, couleurs, cursor, wrap, reflow, alt-screen, scrollback, modes, titres et réponses PTY.
- [ ] Given chaque stream, when il est découpé par chunks de 1, 7, 64, 4 096 octets et par un découpage pseudo-aléatoire à seed fixe, then le snapshot final et les événements ordonnés restent identiques.
- [ ] Given les goldens terminaux existants, when le corpus est exécuté, then leurs attentes restent inchangées et servent de baseline de rendu.
- [ ] Given un benchmark parser et un scénario end-to-end à huit panes, when la baseline est capturée, then débit, p50/p95 input-to-frame, CPU, RSS et durée du verrou sont enregistrés avec matériel, build et seed.
- [ ] Given des séquences VT tronquées, malformées ou dépassant les caps, when elles sont jouées, then aucun panic, blocage ou allocation non bornée ne survient et le résultat est déterministe.
- [ ] Given une fixture issue d'une session réelle, when elle est ajoutée, then commandes, cwd, clipboard, tokens et contenu privé sont remplacés par des données synthétiques avant commit.

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

- [ ] Given paneflow-libghostty-sys, when Cargo résout le crate, then links = "ghostty-vt", les bindings pré-générés et le build statique n'existent que sous Linux.
- [ ] Given paneflow-terminal-ghostty, when un terminal, render state, encoder ou iterator est créé, then un type RAII opaque possède le handle et appelle exactement une fonction free dans Drop.
- [ ] Given une erreur partielle pendant l'initialisation, when le wrapper retourne, then tous les handles déjà créés sont libérés dans l'ordre inverse sans double-free ni fuite.
- [ ] Given une string, un grid ref, une row, une cell ou un buffer emprunté, when le verrou ou l'appel FFI prend fin, then les données nécessaires ont été copiées dans des valeurs Rust et aucun pointeur n'est conservé.
- [ ] Given une allocation produite par libghostty, when elle est libérée, then ghostty_free reçoit le même allocateur; Rust et libc ne la libèrent jamais directement.
- [ ] Given un callback C qui panic côté Rust, when il est invoqué, then la panic est contenue avant la frontière FFI, un événement d'erreur borné est produit et aucun unwind ne traverse C.
- [ ] Given un build info, une version API ou un layout incompatible avec le manifest, when Ghostty s'initialise, then l'initialisation échoue de façon typée avant tout spawn PTY.

#### US-005: Produire le snapshot Ghostty neutre pour GPUI

**Description:** As a renderer Paneflow, I want un Content appartenant à Rust produit depuis le render state Ghostty so that GPUI peint le nouveau moteur sans connaître l'ABI C.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-004

**Acceptance Criteria:**

- [ ] Given des bytes VT, when le moteur les reçoit, then ghostty_terminal_vt_write met à jour l'état et snapshot copie cellules, graphemes, styles, couleurs, cursor, modes, sélection et viewport dans les types Paneflow.
- [ ] Given une cellule wide, spacer head/tail, combining mark, inverse, dim, bold, italic ou underline, when elle est convertie, then les flags neutres correspondent au contrat des goldens actuels.
- [ ] Given un render update, when le snapshot est produit, then l'accès terminal exclusif est acquis une seule fois, les dirty flags globaux et par ligne sont remis à zéro après copie, puis le verrou est relâché avant paint.
- [ ] Given les coordonnées Ghostty et les lignes négatives/display_offset attendues par Paneflow, when le viewport contient du scrollback, then la translation est bijective pour hit-testing, sélection, cursor et scrollbar.
- [ ] Given un grapheme plus long que le buffer stack initial, when il est lu, then le wrapper redimensionne sous un cap documenté et copie tous les codepoints sans troncature silencieuse.
- [ ] Given 0 colonne, 0 ligne ou une dimension supérieure à u16, when création ou resize est demandé, then la valeur est rejetée ou clampée selon TerminalWindowSize sans panic ni appel FFI invalide.

#### US-006: Adapter modes, encodeurs et effets synchrones

**Description:** As a utilisateur de TUI, I want que Ghostty encode les interactions et réponses terminal selon ses modes actifs so that clavier, souris, focus et queries restent compatibles.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**

- [ ] Given un KeyInput, MouseInput, focus event ou paste Paneflow, when le backend l'encode, then les bytes tiennent compte de bracketed paste, application cursor, alt-scroll, mouse tracking et Kitty keyboard actifs.
- [ ] Given write_pty, bell, title, enquiry, xtversion, size, color scheme ou device attributes, when libghostty déclenche l'effet, then un BackendEvent neutre ou une réponse PTY ordonnée est produit.
- [ ] Given une mise à jour title ou pwd, when le moteur la publie, then la string est copiée avant la mutation suivante et passe les mêmes filtres Paneflow qu'Alacritty.
- [ ] Given un callback synchrone, when il s'exécute, then il ne rappelle jamais ghostty_terminal_vt_write, ne touche jamais GPUI et termine en moins de 1 ms au p99 dans le benchmark dédié.
- [ ] Given une réponse couleur ou taille, when plusieurs queries arrivent dans le même chunk, then les réponses restent dans l'ordre du flux avant tout input utilisateur ultérieur.
- [ ] Given une touche, un bouton souris ou un mode inconnu, when l'encodeur ne peut pas le représenter, then il retourne une erreur typée ou zéro byte sans inventer une séquence ni panic.

#### US-007: Adapter grille, recherche, sélection, liens et scrollback

**Description:** As a power user, I want les outils de navigation Paneflow sur la grille Ghostty so that changer de moteur ne supprime aucune capacité de travail.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**

- [ ] Given le snapshot neutre, when une recherche texte ou regex est exécutée, then les mêmes matches, limites, coordonnées et règles de casse que le backend Alacritty sont retournés.
- [ ] Given une sélection linéaire, bloc ou ligne, when elle traverse wrap, wide cells ou scrollback, then le texte copié et les rectangles peints sont identiques à la baseline.
- [ ] Given une cellule OSC 8, when son lien est demandé, then l'URI est copiée sous le verrou, validée par les protections existantes et exposée sans grid ref C persistant.
- [ ] Given extract_scrollback, when la grille contient plus de 4 000 lignes ou 400 000 caractères, then seules les données les plus récentes aux frontières UTF-8 et lignes complètes sont retournées.
- [ ] Given restore_scrollback depuis une session, when le texte contient ESC, CSI, OSC, DCS ou C1 hostile, then la sanitation existante les neutralise avant feed et aucun titre ou lien actif n'est créé.
- [ ] Given une référence de grille invalidée par resize, trim ou alt-screen, when recherche ou sélection la consulte, then elle est abandonnée proprement sans use-after-free, panic ni point hors grille.

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

- [ ] Given la décision US-001, when le host ouvre un shell, then il utilise portable-pty validé ou l'alternative Unix isolée, jamais alacritty_terminal::tty ni le forkpty copié de Ghostling.
- [ ] Given SpawnParams, when le child démarre, then shell, args, cwd, env, TERM=xterm-256color, COLORTERM, TERM_PROGRAM et variables protégées suivent le contrat actuel.
- [ ] Given un spawn depuis le background executor, when le child hérite des signaux, then le masque foreground est appliqué autour du spawn et restauré ensuite, afin que Ctrl-C et Ctrl-Z fonctionnent.
- [ ] Given une taille TerminalWindowSize, when le host resize, then rows, cols et pixels atteignent le PTY et le child reçoit un changement de taille sans événement dupliqué.
- [ ] Given le PID du shell, when Paneflow surveille ou ferme la pane, then parent guard, foreground command, groupe de processus et stratégie de teardown existants restent disponibles.
- [ ] Given cwd invalide, shell absent, permission refusée ou limite PTY atteinte, when le spawn échoue, then aucun second child n'est créé, les handles sont fermés et l'erreur existante reste visible dans la pane.

#### US-009: Construire la pompe I/O bornée et le cycle de vie

**Description:** As a mainteneur runtime, I want une boucle I/O qui sérialise PTY, scanner, moteur et événements so that le flux reste ordonné et borné sous charge ou fermeture.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004, US-006, US-008

**Acceptance Criteria:**

- [ ] Given des bytes lus du PTY, when la pompe les traite, then scanner borné, ghostty_terminal_vt_write, effets synchrones et Wakeup suivent l'ordre exact du flux.
- [ ] Given input utilisateur et réponses générées par le terminal, when les deux attendent un write, then les réponses protocolaires conservent leur ordre relatif et les partial writes sont rejoués sans perte.
- [ ] Given une rafale d'output, when les événements atteignent la vue, then Wakeup est coalescé, le drain respecte 4 ms ou 100 événements par tick et les événements Exit/Title/Cwd ne sont jamais supprimés.
- [ ] Given EAGAIN, EINTR, short read, short write ou broken pipe, when l'I/O continue ou se termine, then aucune boucle active, perte silencieuse non tracée ou allocation non bornée ne survient.
- [ ] Given un child qui sort après une dernière rafale, when EOF et exit arrivent, then l'output final est drainé pendant au plus 2 secondes et un seul événement ChildExited est publié avec code ou signal.
- [ ] Given shutdown, drop ou fermeture forcée, when le child résiste, then le ladder de terminaison se termine en moins de 2 secondes, ferme tous les descripteurs et laisse 0 zombie.
- [ ] Given un OSC 7/52/133 tronqué ou dépassant son cap, when le scanner reçoit plusieurs chunks, then il se réinitialise proprement sans bloquer le feed VT ni conserver un payload non borné.

#### US-010: Composer le backend Ghostty avec placeholder et promotion

**Description:** As a utilisateur Paneflow, I want que la nouvelle session apparaisse immédiatement puis soit promue sans remplacement visuel so that le spawn natif ne bloque jamais GPUI.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-005, US-007, US-009

**Acceptance Criteria:**

- [ ] Given une nouvelle pane Ghostty, when elle est créée, then un moteur display-only et la même entité TerminalView s'affichent avant le spawn background.
- [ ] Given des inputs avant promotion, when leur total reste sous 64 KiB, then ils sont bufferisés puis rejoués exactement une fois et dans l'ordre après attachement du PTY.
- [ ] Given plus de 64 KiB avant promotion, when le cap est atteint, then l'excédent est rejeté selon le comportement documenté sans croissance mémoire ni blocage de la vue.
- [ ] Given une initialisation libghostty impossible, when aucun child n'a encore été spawn, then la session choisit Alacritty, crée exactement un child et enregistre une raison de fallback structurée.
- [ ] Given le PTY échoue après initialisation Ghostty, when la promotion ne peut pas aboutir, then la pane conserve l'overlay d'erreur existant et ne lance pas un second shell Alacritty.
- [ ] Given une session promue, when rendu, recherche, resize, input ou shutdown est demandé, then tous les appels vont vers la même identité Ghostty jusqu'à sa fermeture.

#### US-011: Ajouter sélection backend, configuration et diagnostics

**Description:** As a utilisateur et release engineer, I want sélectionner et diagnostiquer le backend sans ambiguïté so that l'opt-in, le rollback et les builds non-Linux restent sûrs.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002, US-010

**Acceptance Criteria:**

- [ ] Given terminal.backend = auto, ghostty ou alacritty, when la config est chargée, then une enum validée et documentée décide du backend de chaque nouvelle session.
- [ ] Given la phase opt-in avant US-018, when auto est résolu sous Linux, then Alacritty reste sélectionné; ghostty exige Linux et la feature libghostty-linux.
- [ ] Given macOS ou Windows avec terminal.backend = ghostty dans une config partagée, when une session démarre, then Alacritty est utilisé avec un warning unique et l'application continue sans Zig ni symbole Ghostty.
- [ ] Given une valeur inconnue ou une feature absente, when la config est résolue, then le diagnostic existant signale la valeur, Alacritty reste utilisable et aucun crash de démarrage ne survient.
- [ ] Given les diagnostics terminal, when ils sont consultés, then backend demandé/résolu, fallback reason, SHA, version API, Zig, optimisation et SIMD sont visibles sans texte terminal, cwd, commande ni clipboard.
- [ ] Given une modification de config pendant une session active, when la config est rechargée, then la session conserve son identité; seules les nouvelles sessions utilisent le nouveau choix.

---

### EP-004: Parité Paneflow sur Linux

Fermer les écarts visibles et produit avant toute promotion, sans utiliser les nouvelles capacités Ghostty comme prétexte à modifier l'expérience.

**Definition of Done:** Ghostty produit zéro divergence non documentée sur le corpus, les goldens et les contrats P0/P1 de rendu, input, protocoles, recherche, agents et persistance.

#### US-012: Atteindre la parité rendu, Unicode et resize

**Description:** As a utilisateur Linux, I want un affichage identique ou explicitement corrigé so that mes shells et TUIs ne changent pas visuellement avec le backend.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-010

**Acceptance Criteria:**

- [ ] Given le corpus de référence, when Alacritty et Ghostty produisent leurs snapshots, then cellules, graphemes, styles, couleurs, cursor, modes, wrap, alt-screen et scrollback ont 0 divergence non documentée.
- [ ] Given les goldens plain, ANSI16, indexed256, truecolor, inverse, dim, wide/CJK, sélection et formes de cursor, when Ghostty les alimente, then le pipeline paint existant produit les mêmes sorties attendues.
- [ ] Given graphemes combinés, emoji, ZWJ, double-width et caractères invalides, when ils sont rendus et sélectionnés, then aucune cellule ne se décale et le remplacement invalide est déterministe.
- [ ] Given un resize sur l'écran primaire, when les colonnes changent, then les lignes wrappées reflowent et les coordonnées scrollback restent cohérentes; l'écran alternatif ne reflowe pas.
- [ ] Given une tempête de 200 resizes ou une surface 0x0 transitoire, when les événements sont coalescés, then aucun panic, deadlock, SIGWINCH incohérent ou cursor hors grille ne survient.
- [ ] Given huit panes avec output simultané, when GPUI peint 60 frames, then chaque pane effectue au plus une acquisition de verrou terminal par frame.

#### US-013: Atteindre la parité input, clipboard et protocoles

**Description:** As a utilisateur de CLI et TUI, I want que toutes mes interactions produisent les mêmes bytes et effets so that aucun outil ne détecte un terminal dégradé.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-006, US-009, US-010

**Acceptance Criteria:**

- [ ] Given les keybindings, IME, marked text, dead keys et paste actuels, when ils traversent Ghostty, then les bytes PTY et les événements GPUI correspondent à la baseline Alacritty.
- [ ] Given bracketed paste actif ou inactif, when du texte multi-ligne ou des caractères de contrôle sont collés, then la politique Paneflow et la validation Ghostty empêchent une soumission implicite et préservent les newlines attendues.
- [ ] Given mouse tracking, SGR mouse, focus reports ou alt-scroll, when l'utilisateur clique, drag, scroll ou change le focus, then les coordonnées et séquences restent conformes aux modes actifs.
- [ ] Given OSC 52 avec le mode par défaut CopyOnly, when un programme écrit jusqu'à 100 KiB, then au plus 8 opérations sont mises en attente; les reads sont refusés et tout payload supérieur est ignoré.
- [ ] Given OSC 4/10/11/12, DA, DSR ou size queries dans un même flux, when les réponses sont générées, then elles arrivent dans l'ordre et avant l'input utilisateur suivant.
- [ ] Given Kitty graphics file, temp file ou shared memory, when un payload arrive en v1, then le stockage reste à 0, les mediums restent désactivés et le parser respecte un cap APC explicite.
- [ ] Given une séquence input, OSC ou APC malformée, when elle traverse des limites de chunks arbitraires, then elle ne provoque ni panic, allocation non bornée, clipboard action ni byte PTY inventé.

#### US-014: Préserver les workflows Paneflow, OSC 133 et les sessions

**Description:** As a utilisateur multi-agents, I want retrouver les contrats produit au-dessus du terminal so that le changement de moteur reste invisible dans mes workflows Paneflow.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-007, US-010, US-013; external gate: tasks/prd-cli-cockpit-ergonomics-2026-Q3 EP-003 must be DONE

**Acceptance Criteria:**

- [ ] Given EP-003 OSC 133 du PRD CLI Cockpit, when US-014 démarre, then son status est DONE et ses CommandMark/exit codes sont consommés ou adaptés, jamais réimplémentés dans un second store.
- [ ] Given OSC 7, title et process fallback, when le shell change de cwd, then la sidebar, le split cwd et la restauration reçoivent la même valeur validée que sous Alacritty.
- [ ] Given OSC 133 absent, désactivé ou malformé, when une commande s'exécute, then le terminal fonctionne sans marks, sans erreur visible et sans allocation par chunk dédiée au cas absent.
- [ ] Given output terminal, when Wakeup est traité, then output_generation, activity burst, service detection, ports, waiting state et child exit suivent les mêmes transitions.
- [ ] Given recherche locale, fleet search, copy-mode et hyperlinks, when ils parcourent Ghostty, then résultats, caps, navigation et priorités OSC8/URL/path restent identiques.
- [ ] Given save/restore, when le scrollback vient de l'autre backend, then 4 000 lignes et 400 000 caractères maximum sont restaurés après sanitation, sans changement du format session.
- [ ] Given les profils Normal, CachedAgent et Review, when un terminal est créé, caché ou libéré, then les budgets de scrollback et cache du PRD mémoire DONE restent appliqués.
- [ ] Given un child qui sort sans input ou après input utilisateur, when should_close_on_exit est évalué, then l'overlay et la fermeture de pane gardent la logique actuelle, y compris code non nul et signal.

---

### EP-005: CI, packaging, dogfood et bascule Linux par défaut

Transformer un backend fonctionnel en composant distribuable, mesuré et réversible avant de modifier le défaut Linux.

**Definition of Done:** les deux architectures Linux, cinq familles de distribution, Wayland/X11, fuzz, benchmarks, upgrade drill et 30 jours de dogfood passent; auto sélectionne Ghostty sous Linux et reste Alacritty ailleurs.

#### US-015: Étendre la CI Linux, l'ABI et la supply chain

**Description:** As a release engineer, I want que chaque changement natif repasse les vérifications critiques so that une mise à jour Ghostty ne casse pas silencieusement l'ABI ou les artefacts.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**

- [ ] Given une PR touchant le backend, le pin ou les bindings, when CI s'exécute, then Linux x86_64 et aarch64 construisent et testent libghostty-linux en debug et release.
- [ ] Given macOS x86_64/aarch64 et Windows x86_64/ARM64, when leurs jobs existants s'exécutent, then ils compilent/testent Alacritty sans installer Zig ni initialiser la source Ghostty.
- [ ] Given le header épinglé, when les bindings sont régénérés dans le job dédié, then tout diff, changement de layout, version API ou symbole non approuvé fait échouer la CI.
- [ ] Given le fuzz différentiel, when une PR s'exécute, then au moins 60 secondes par cible parser/snapshot passent; le nightly exécute au moins 30 minutes par cible.
- [ ] Given Cargo et les sources natives, when l'audit de dépendances s'exécute, then cargo-deny passe et l'inventaire de licences Ghostty, Highway, simdutf et Zig packages est attaché à l'artefact.
- [ ] Given le cache Zig ou la source épinglée indisponible, when le job build démarre, then il échoue explicitement au bootstrap et ne télécharge rien depuis build.rs.
- [ ] Given un changement de SHA Ghostty, when la PR est ouverte, then bindings, ABI, corpus différentiel, fuzz, benchmarks, licence et build info sont tous revalidés avant merge.

#### US-016: Valider packaging et matrice distro/display

**Description:** As a utilisateur Linux, I want que le backend livré fonctionne dans chaque format supporté so that l'installation réelle ne dépend pas de la machine de build.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-015

**Acceptance Criteria:**

- [ ] Given les artefacts tar, AppImage, deb et rpm x86_64/ARM64, when leurs dépendances dynamiques sont inspectées, then aucune entrée libghostty.so n'existe et les notices natives sont incluses.
- [ ] Given Ubuntu/Debian, Fedora, Arch et openSUSE, when le paquet est installé, then un smoke headless crée Ghostty, spawn un PTY, écrit un marqueur, resize, lit le marqueur et ferme le child sans zombie.
- [ ] Given Fedora Wayland natif puis X11/XWayland, when Paneflow ouvre shell, agent et TUI alt-screen, then rendu, input, clipboard, resize et fermeture passent le runbook manuel.
- [ ] Given une machine sans CMake, Zig, clang ou source Ghostty après installation, when Paneflow démarre, then le backend fonctionne car l'artefact contient l'archive liée statiquement.
- [ ] Given le binaire release stripé, when sa taille est comparée à la baseline Alacritty, then l'augmentation reste <= 15 MiB; au-delà, la promotion est bloquée et le delta est expliqué.
- [ ] Given une architecture Linux autre que x86_64 ou ARM64, when un artefact est demandé, then la release ne publie pas un paquet non testé et affiche clairement les architectures supportées.

#### US-017: Passer les gates performance, résilience et dogfood

**Description:** As a mainteneur Paneflow, I want une preuve longue et chiffrée so that le changement de défaut repose sur des données plutôt que sur un smoke local.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-012, US-013, US-014, US-016

**Acceptance Criteria:**

- [ ] Given les mêmes payloads, build release et matériel, when parser Alacritty et Ghostty sont comparés, then le débit Ghostty est >= 95 % de la baseline.
- [ ] Given huit panes actives, when le scénario end-to-end s'exécute, then la régression p95 input-to-frame est <= 5 %, CPU et RSS <= 10 %, et la durée de verrou p95 < 2 ms.
- [ ] Given 200 cycles spawn, 200 resizes par cycle et fermeture, when le stress test termine, then 0 crash, 0 deadlock, 0 child orphelin, 0 zombie et 0 croissance RSS résiduelle > 5 % sont observés.
- [ ] Given 30 jours de dogfood Fedora et au moins 200 sessions démarrées, when les diagnostics locaux sont synthétisés, then 0 crash natif, 0 deadlock et 0 fallback d'initialisation inexpliqué sont enregistrés.
- [ ] Given une mise à jour contrôlée vers un second SHA Ghostty, when le runbook d'upgrade est exécuté sur une branche jetable, then les gates détectent les diffs ABI/comportement ou produisent un passage complet reproductible.
- [ ] Given une gate sous le seuil, when le rapport de promotion est généré, then US-018 reste bloquée avec la mesure fautive; aucune exception manuelle non documentée n'est autorisée.
- [ ] Given les mesures de dogfood, when elles sont stockées, then elles ne contiennent aucun byte terminal, commande, cwd, texte clipboard, identifiant agent ou donnée personnelle.

#### US-018: Promouvoir Ghostty par défaut sur Linux et documenter le rollback

**Description:** As a utilisateur Linux, I want que auto choisisse le backend validé avec un rollback par configuration so that la migration est effective sans verrouiller les utilisateurs.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**

- [ ] Given le rapport US-017 complet, when une nouvelle session Linux utilise terminal.backend = auto, then Ghostty est résolu et son identité apparaît dans les diagnostics.
- [ ] Given terminal.backend = alacritty sous Linux, when une nouvelle session démarre, then le backend historique est utilisé sans migration de données ni redémarrage global.
- [ ] Given macOS ou Windows avec auto, when une nouvelle session démarre, then Alacritty reste le seul backend et le comportement est inchangé.
- [ ] Given un échec Ghostty avant spawn, when auto résout la session, then Alacritty démarre exactement un child et une fallback reason actionnable est journalisée.
- [ ] Given une session Ghostty active puis un changement de config, when l'utilisateur force Alacritty, then la session active reste Ghostty et les nouvelles sessions utilisent Alacritty.
- [ ] Given les notes de release et la documentation config, when la version est publiée, then elles expliquent portée Linux, feature, diagnostics, rollback et limites sans annoncer macOS/Windows.
- [ ] Given une gate absente, expirée ou rouge, when le changement de défaut est tenté, then le test de promotion échoue et auto reste Alacritty.
- [ ] Given la première release stable avec Ghostty par défaut, when 30 jours supplémentaires se sont écoulés, then Alacritty reste encore disponible au moins jusqu'à la release stable suivante; sa suppression Linux est un PRD séparé.

## Functional Requirements

- FR-01: Le système DOIT exposer terminal.backend = auto, ghostty ou alacritty pour les nouvelles sessions.
- FR-02: auto DOIT sélectionner Ghostty uniquement sous Linux après US-018 et Alacritty sur macOS/Windows.
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
- FR-17: CI DOIT construire/tester Ghostty sur Linux x86_64/ARM64 et Alacritty inchangé sur les autres cibles.
- FR-18: Les paquets Linux DOIVENT prouver leur linkage statique et passer un smoke PTY/runtime.
- FR-19: Le travail DOIT réutiliser les contrats des PRDs mémoire, CLI Cockpit et control-plane existants sans modifier leurs statuts.
- FR-20: Le dépôt Ghostling, Raylib et l'ancien worktree Hera NE DOIVENT PAS devenir des dépendances runtime ou des sources copiées aveuglément.

## Non-Functional Requirements

- **Performance parser:** débit release Ghostty >= 95 % du débit Alacritty sur les mêmes payloads et le même matériel.
- **Latence interactive:** régression p95 input-to-frame <= 5 % et p99 <= 33 ms avec 8 panes actives.
- **Verrou terminal:** au plus 1 acquisition par pane et par frame, durée p95 < 2 ms.
- **Mémoire et CPU:** RSS et CPU du scénario 8 panes <= 110 % de la baseline; croissance RSS résiduelle après stress <= 5 %.
- **Backpressure:** input pré-promotion <= 64 KiB, clipboard queue <= 8 opérations, OSC 52 <= 100 KiB, événement batch <= 100 par tick et <= 4 ms.
- **Persistance:** extraction <= 4 000 lignes et <= 400 000 caractères, sans coupe UTF-8 invalide.
- **Reliability:** 200 cycles spawn/resize/close avec 0 crash, 0 deadlock, 0 orphelin et 0 zombie; shutdown et final drain <= 2 s chacun.
- **Dogfood:** 30 jours et >= 200 sessions avec 0 crash natif, 0 deadlock et 0 fallback inexpliqué avant promotion.
- **Security:** 0 panic traversant FFI, 0 pointeur emprunté conservé après mutation, Kitty image storage = 0 et file/temp/shared-memory désactivés.
- **Privacy:** 0 byte de contenu terminal, commande, cwd ou clipboard dans logs, métriques, fixtures commitées ou télémétrie.
- **Compatibility:** 100 % des jobs Linux x86_64/ARM64, macOS x86_64/aarch64 et Windows x86_64/ARM64 existants restent verts.
- **Distribution:** smokes réussis sur 5 familles Linux ciblées et sur Wayland natif plus X11/XWayland; augmentation du binaire stripé <= 15 MiB.
- **Scalability:** aucune queue non bornée avec jusqu'à 32 panes, et les budgets restent tenus dans le benchmark de 8 panes actives.
- **Input accessibility:** 100 % des fixtures clavier, IME, dead keys, mouse et paste existantes passent sur Ghostty.
- **Reproducibility:** 100 % des changements de SHA déclenchent bindings, ABI, corpus, fuzz, benchmark et licence; 0 téléchargement depuis cargo build.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Toolchain native absente | Zig absent ou != 0.15.2 | Build arrêté avant link, aucune récupération réseau | "libghostty requires Zig 0.15.2; install or select the pinned toolchain" |
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
| 15 | Distro/display non validé | Smoke package ou Wayland/X11 rouge | Promotion bloquée, artefact non publié comme défaut | Rapport de promotion indique la gate |
| 16 | Upgrade Ghostty casse le contrat | Changement de SHA | CI détecte bindings, ABI ou diff comportemental | PR bloquée avec diff ciblé |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | ABI libghostty-vt instable | High | High | Pin exact, bindings commités, checksum header, layout tests, build info et upgrade drill |
| 2 | UB, use-after-free ou mauvais allocateur FFI | Med | High | Crate -sys minimal, RAII, copies sous verrou, ghostty_free, callbacks catch_unwind, audit ciblé |
| 3 | Régression PTY/signaux/process group | Med | High | Spike portable-pty, tests Ctrl-C/Ctrl-Z, final drain, stress 200 cycles et contrôle zombies |
| 4 | Écart caché input/OSC/alt-screen | High | High | Corpus >= 100 streams, chunk matrix, goldens, fuzz différentiel et stories de parité dédiées |
| 5 | Dual-backend devient permanent et coûteux | High | Med | Interface unique, Alacritty confiné, promotion datée; suppression Linux traitée après une release stable séparée |
| 6 | Build natif ou packaging non reproductible | Med | High | Source/toolchain/cache épinglés, static link, aucun réseau build.rs, smokes de chaque format |
| 7 | Régression performance à plusieurs panes | Med | High | Baseline US-003, seuils relatifs, verrou unique, benchmark 8 panes et gate bloquante |
| 8 | Collision avec OSC 133 en IN_REVIEW | Med | Med | Gate externe DONE avant US-014, réutilisation du store existant, aucun changement de son tracker |
| 9 | Crash natif impossible à fallback en cours de session | Low après gates | High | Pas de promesse de hot fallback, fuzz, 30 jours dogfood, rollback config pour nouvelles sessions |
| 10 | Source Ghostty ou dépendances augmentent fortement le binaire | Med | Med | Budget +15 MiB, licence/native inventory, mesure par artefact et promotion bloquée au-delà |

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
- **Native source:** comment fournir le source sans réseau pendant Cargo? Recommandé: source Ghostty repository-owned et épinglée sous third_party/ghostty, avec bootstrap de son cache Zig avant Cargo et vérification des hashes.
- **FFI:** faut-il générer les bindings sur chaque machine? Recommandé: bindings générés et commités, régénération CI dédiée seulement, afin de ne pas imposer libclang aux utilisateurs.
- **PTY:** portable-pty couvre-t-il tous les invariants Linux actuels? Recommandé: validation US-001; si un invariant P0 manque, implémentation Unix privée derrière TerminalHost sans changer la façade.
- **Concurrency:** faut-il rendre Ghostty Sync? Recommandé: non. Le terminal est sérialisé, le render state est copié sous une acquisition exclusive et seul le snapshot Rust sort du verrou.
- **Events:** comment gérer les callbacks synchrones? Recommandé: callback minimal, catch_unwind, copie bornée, queue BackendEvent et interdiction de reentrancy/GPUI.
- **OSC 52/133:** l'API C expose-t-elle assez d'information au pin? Recommandé: utiliser les sémantiques natives disponibles et conserver un scanner streaming borné uniquement pour les données absentes, notamment clipboard et exit code.
- **Data model:** faut-il persister le backend par pane? Recommandé: non en v1. La session persiste le scrollback texte et résout le backend courant au restore; l'identité n'est qu'un diagnostic runtime.
- **Config:** comment préserver une config partagée entre OS? Recommandé: enum globale auto/ghostty/alacritty, résolution ghostty limitée à Linux, warning unique et fallback Alacritty ailleurs.
- **Migration:** peut-on supprimer Alacritty dès la première release Linux? Recommandé: non. Le conserver au moins une release stable et 30 jours après promotion, sans planifier sa suppression dans ce PRD.
- **Upgrade:** comment accepter un nouveau SHA Ghostty? Recommandé: PR explicite qui repasse bindings, layouts ABI, corpus, fuzz, benchmarks, licences et build info; aucun bot d'update automatique.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Divergences de snapshot | Ghostty non intégré | 0 non documentée sur >= 100 streams et tous les goldens | Avant US-018 | Harness différentiel seedé |
| Débit parser | Alacritty = 100 % | Ghostty >= 95 % | Avant promotion puis chaque SHA | Benchmark release, même matériel/payload |
| p95 input-to-frame | Mesurée en US-003 | Régression <= 5 %, p99 <= 33 ms | Avant promotion | Instrumentation locale 8 panes |
| CPU et RSS | Mesurés en US-003 | <= 110 % de la baseline | Avant promotion | Profil release 8 panes, 10 000 lignes |
| Durée de verrou | Mesurée en US-003 | p95 < 2 ms, <= 1 lock/frame/pane | Avant promotion | Compteurs backend locaux |
| Lifecycle PTY | Smokes Alacritty actuels | 200 cycles, 0 crash/deadlock/orphelin/zombie | Avant promotion | Stress test automatisé |
| Fiabilité dogfood | N/A pour Ghostty | 30 jours, >= 200 sessions, 0 crash natif/deadlock/fallback inexpliqué | Month 1-2 | Rapport local sans PII |
| Couverture Linux | Ubuntu x64 + ARM64 partielle | 5 familles distro, x86_64/ARM64, Wayland + X11/XWayland | Avant release défaut | CI, package smoke et runbook |
| Taille binaire | Release Alacritty mesurée en US-003 | Delta stripé <= 15 MiB | Chaque release | Comparaison artefacts |
| Résolution auto Linux | Alacritty 100 % | Ghostty 100 % des nouvelles sessions auto | Après US-018 | Test config + diagnostics |
| Non-régression autres OS | Alacritty 100 % | 100 % des jobs macOS/Windows verts, 0 dépendance Zig/Ghostty | Chaque PR | Matrice CI existante |

## Open Questions

- **Q1 - portable-pty valide-t-il signaux et teardown Paneflow?** Owner: US-001. Deadline: avant US-008. Fallback décidé: host Unix privé derrière la même interface.
- **Q2 - EP-003 OSC 133 est-il DONE sur la branche d'implémentation?** Owner: mainteneur CLI Cockpit. Deadline: avant US-014. Bloque uniquement US-014 et la promotion.
- **Q3 - libghostty publiera-t-il une première release C versionnée avant l'implémentation?** Owner: mainteneur dépendances. Vérifier à chaque upgrade; un tag stable ne supprime aucune gate ABI.
- **Q4 - quand supprimer le fallback Alacritty sous Linux?** Owner: futur PRD après au moins une release stable et 30 jours post-promotion. Hors scope actuel.
[/PRD]
