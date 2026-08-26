[PRD]

# PRD: Éditeur de fichier du dock diff - 2026-Q3

## Changelog

| Version | Date | Auteur | Changement |
|---------|------|--------|------------|
| 1.0 | 2026-08-25 | Arthur Jean | Version initiale, périmètre read-only. 4 epics, 18 stories. |
| 2.1 | 2026-08-25 | Arthur Jean | Ajout de la section Références Zed (ancrages `zed:file:line` vérifiés par story). Trois valeurs corrigées contre le code de Zed : budget de reparse synchrone 1 ms au lieu de 2 ms, groupage d'annulation 300 ms au lieu de 500 ms, écriture atomique nommée sur `tempfile`. |
| 2.0 | 2026-08-25 | Arthur Jean | Périmètre étendu à l'édition (Codex App, Cursor). Rope `ropey`, coloration incrémentale, saisie IME, undo/redo, sauvegarde, conflit d'écriture externe. Arborescence retirée du périmètre : `app/files_tree.rs` et `app/files_sidebar/` existent déjà, seule l'ouverture des fichiers non-markdown reste à lever. 6 epics, 20 stories. |

## Problem Statement

1. **L'entrée "File" du menu `+` est un no-op.** Le strip d'onglets du dock diff expose deux surfaces au menu de création (`src-app/src/app/agents_diff/new_tab_menu.rs:60-82`), mais seule "Terminal" fait quelque chose : son handler appelle `open_diff_terminal_tab` (`tabs.rs:17`). Le handler de "File" se contente de `close_diff_new_tab_menu` (`new_tab_menu.rs:66-69`). L'utilisateur voit une entrée de menu avec un raccourci affiché (`Ctrl+G`, `new_tab_menu.rs:63`) qui ne produit rien.

2. **Le raccourci annoncé n'existe pas.** `Ctrl+G` et `Ctrl+J` sont rendus dans le menu (`new_tab_menu.rs:63` et `:75`) mais aucun des deux n'est enregistré dans `keybindings/defaults.rs`. Le menu ment sur deux lignes.

3. **`DiffDockTab` n'a que deux variantes.** L'enum (`src-app/src/app/agents_diff/model.rs:31-35`) ne modélise que `Changes` et `Terminal(Entity<TerminalView>)`. Le switch de corps (`mod.rs:322`) et l'étiquetage des chips (`render.rs:131-134`) sont exhaustifs sur ces deux cas : rien ne peut porter un fichier aujourd'hui.

4. **Corriger ce qu'un agent vient d'écrire oblige à quitter Paneflow.** C'est le vrai coût. Le dock montre le diff, l'utilisateur repère la ligne fautive, et doit alors ouvrir Zed ou VSCode pour changer trois caractères, puis revenir. Le cockpit d'agents perd la main précisément au moment où l'humain reprend la main.

5. **La sidebar Files rend les fichiers non-markdown délibérément inertes.** `app/files_sidebar/row.rs:27` calcule `is_md`, `:30` documente que "every other file reads greyed", et seul le markdown déclenche `open_markdown_in_active_pane` (`row.rs:116`). Les rows non-markdown ne sont ni cliquables ni draggables (`row.rs:122-123`). L'arborescence existe donc en entier, avec son modèle (`app/files_tree.rs:20-262`), son watcher (`files_sidebar/watch.rs:46` et `:144`) et son filtrage gitignore, mais elle ne sait ouvrir qu'un seul type de fichier.

6. **L'infrastructure de saisie existe mais ne passe pas à l'échelle d'un fichier.** `widgets/text_area.rs` implémente déjà `EntityInputHandler` (`:914`), `replace_text_in_range` (`:954`), la navigation graphème via `unicode-segmentation` (`:46`), le curseur et la sélection en offsets d'octets, le clic-pour-positionner, la sélection au drag, le double-clic mot et le triple-clic ligne (`text_area.rs:1-23`). Mais son `request_layout` shape **toutes** les lignes pour calculer la hauteur après soft-wrap (`:1256-1302`) : sur un fichier de 10 000 lignes, chaque frame paie le shaping du fichier entier. Le widget est correct pour un composer de quelques lignes, inutilisable pour un fichier.

7. **Le moteur de coloration ne survit pas à une édition.** `highlight_lines` (`src-app/src/diff/highlighter.rs:167`) prend un texte complet, parse, produit des plages par ligne, et jette l'arbre tree-sitter. Une frappe invalide tout le résultat et impose un reparse complet. Le commentaire d'architecture de `src-app/Cargo.toml:155-215` assume ce choix explicitement : "fast enough (ms-scale parse) that we highlight at build time off-thread, no lazy-viewport machinery needed". Vrai pour un diff immuable, faux dès que le texte mute sous les doigts.

8. **Les agents écrivent dans les fichiers que l'humain édite.** C'est la contrainte propre à Paneflow, qu'aucun éditeur généraliste n'a à résoudre aussi frontalement : pendant que l'utilisateur corrige une ligne, un agent CLI peut réécrire le même fichier. Sans garde, on perd soit le travail de l'humain, soit celui de l'agent, silencieusement.

**Why now :** les trois incréments précédents du dock (strip d'onglets, menu `+` style Codex App, onglet Terminal) sont livrés et ont posé toute la mécanique d'onglets. L'entrée "File" est la seule case vide du menu. Le coût marginal est plus faible qu'il n'y paraît parce que trois briques coûteuses existent et sont éprouvées : les grammaires tree-sitter et la palette de couleurs (`diff/syntax.rs:29`, `diff/rows.rs:522`), le protocole de saisie natif avec IME (`widgets/text_area.rs:914`), et l'arborescence de fichiers complète (`app/files_tree.rs`). Le net-neuf se concentre sur un élément de rendu virtualisé et sur la couche d'édition.

## Overview

Un éditeur de fichier, ouvert comme un onglet du dock diff au même titre qu'un terminal. Il affiche un fichier du worktree avec gouttière de numéros de ligne, ligne courante, curseur, sélection, scrollbars, défilement horizontal, et coloration syntaxique identique à celle du diff parce qu'elle sort des mêmes grammaires et de la même palette. On peut y taper, sélectionner, annuler, coller, indenter et sauvegarder.

Deux mots d'ordre gouvernent chaque arbitrage. **Minimalisme de l'IDE** : pas d'autocomplétion, pas de LSP, pas de formatage automatique, pas de minimap, pas de repli de code, pas de multi-curseur. C'est un éditeur pour corriger ce qu'un agent vient d'écrire, pas pour développer une journée entière. **Performance maximale** : aucun travail par frame proportionnel à la taille du fichier, et aucune frappe qui déclenche un reparse complet.

La ligne de démarcation avec Zed et VSCode est assumée : Paneflow n'essaie pas de les remplacer. Il ferme la boucle "l'agent a écrit, je relis, je corrige, je sauve" sans quitter le cockpit.

## Goals

| Objectif | Métrique | Baseline | Cible Mois-1 | Cible Mois-6 |
|----------|----------|----------|--------------|--------------|
| Fermer la boucle de correction dans le cockpit | Sorties vers un éditeur externe pour une correction de moins de 10 lignes | 100 % | < 30 % | < 10 % |
| Tenir le budget de frame en scroll | Temps de `paint` du `CodeElement`, fichier 5 000 lignes | n/a | < 4 ms p95 | < 3 ms p95 |
| Latence de frappe | Délai frappe vers pixel, fichier 5 000 lignes | n/a | < 16 ms p95 | < 10 ms p95 |
| Ne pas bloquer le thread de rendu | Travail sur le thread GPUI à l'ouverture d'un fichier de 1 Mo | n/a | < 2 ms | < 2 ms |
| Coloration identique au diff | Divergences de couleur constatées entre un fichier ouvert et son diff | n/a | 0 | 0 |
| Zéro perte de travail sur conflit agent | Éditions humaines perdues par écrasement silencieux | n/a | 0 | 0 |

## Target Users

**Rôle : l'opérateur d'agents (Arthur, et le profil visé par Paneflow).**

- Comportements : lance plusieurs CLIs d'agents en parallèle, surveille le dock diff pour voir ce que les agents modifient, corrige à la main les petites erreurs plutôt que de relancer un tour d'agent.
- Points de douleur : le diff ne montre que 3 lignes de contexte (`rows.rs:213`) ; corriger une ligne impose d'ouvrir un autre éditeur, de retrouver le fichier, de changer trois caractères, de revenir. La rupture de contexte coûte plus cher que la correction.
- Contournement actuel : Zed ou VSCode ouvert en parallèle sur le même worktree, ou un onglet Terminal avec `sed`/`vim` pour les cas les plus courts.
- Le succès ressemble à : voir la ligne fautive dans le diff, ouvrir le fichier dans le dock, corriger, `Ctrl+S`, revenir à l'onglet Changes et voir le diff se mettre à jour, sans jamais changer de fenêtre.

## User Stories

### EP-001 - Document éditable (`CodeDocument`)

**Definition of done :** un chemin de fichier produit, hors du thread de rendu, un document mutable prêt à peindre et à éditer, avec une coloration qui survit à chaque frappe sans reparse complet ; aucun fichier hostile ne peut geler l'application.

#### US-001 - `CodeDocument` sur une rope

**En tant qu'** opérateur, **je veux** que l'insertion d'un caractère au milieu d'un gros fichier soit instantanée, **afin que** la frappe ne rame jamais.

Priorité : P0. Taille : M (3). Dépendances : aucune.

Critères d'acceptation :
- [ ] `CodeDocument` porte le texte dans une `ropey::Rope`, plus le chemin absolu et le nombre de lignes.
- [ ] `insert(byte_offset, &str)` et `remove(Range<usize>)` s'exécutent en O(log n) et ne reconstruisent aucun index linéaire.
- [ ] Les conversions octet vers ligne et ligne vers octet passent par les compteurs maintenus par la rope, jamais par un rescan.
- [ ] `line(&self, i: usize) -> Option<RopeSlice>` retourne la ligne sans son `\n` final, et `None` hors bornes (pas de panic).
- [ ] Un fichier vide produit `line_count == 1` et une ligne vide, jamais `line_count == 0`.
- [ ] Un fichier sans `\n` final ne gagne pas de ligne fantôme.
- [ ] Les fins de ligne CRLF sont préservées à la sauvegarde : un fichier CRLF réécrit reste CRLF, un fichier LF reste LF.
- [ ] La plus longue ligne est maintenue à jour de façon incrémentale, sans rescan complet à chaque frappe ; un recalcul complet n'est autorisé qu'au chargement.
- [ ] Tests couvrant : fichier vide, une seule ligne sans `\n`, CRLF préservé aller-retour, index hors bornes, insertion au milieu d'un fichier de 100 000 lignes.

**Décision, rope contre `String` plat :** la version 1.0 de ce PRD retenait `String` plus `Vec<u32>`, justifié *par* le read-only. L'édition retire cette justification : chaque insertion au milieu impose un memmove O(n) sur le texte et une reconstruction O(lignes) de l'index. Sur un fichier de 20 000 lignes, c'est 80 Ko d'écritures par frappe, plus la pression de cache. `ropey` est retenu contre un `SumTree` maison : c'est la rope de référence en Rust (Helix, Lapce l'utilisent en production), elle maintient les compteurs de lignes dans l'arbre, et elle expose des `RopeSlice` par chunks directement consommables par le callback de `Parser::parse_with` de tree-sitter (US-004). Une seule dépendance, contre plusieurs centaines de lignes d'arbre équilibré à écrire et à tester.

#### US-002 - Chargement off-thread avec garde de génération

**En tant qu'** opérateur, **je veux** que l'ouverture d'un gros fichier ne gèle pas la fenêtre, **afin que** l'UI reste réactive pendant la lecture disque et le parse initial.

Priorité : P0. Taille : M (3). Dépendances : US-001.

Critères d'acceptation :
- [ ] La lecture disque, la construction de la rope et le parse initial se font dans `smol::unblock`, sur le patron de `markdown/view.rs:155-157`.
- [ ] Un compteur de génération est incrémenté à chaque demande d'ouverture ; un résultat asynchrone dont la génération ne correspond plus à la génération courante est ignoré sans `cx.notify()`.
- [ ] Ouvrir deux fichiers coup sur coup affiche le second, jamais le premier, quel que soit l'ordre d'arrivée des tâches.
- [ ] Fermer l'onglet pendant le chargement n'entraîne ni panic ni écriture d'état : la `WeakEntity` morte fait échouer l'`update` silencieusement.
- [ ] Un état `Loading` est rendu pendant la tâche, remplacé par le contenu ou par une erreur.
- [ ] Le mode d'ouverture du fichier (permissions POSIX, attribut lecture seule Windows) est relevé au chargement et expose un document non éditable si l'écriture est impossible.
- [ ] Test : deux appels de chargement concurrents, le résultat de génération obsolète est rejeté.

#### US-003 - Garde-fous : taille, ligne géante, contenu non textuel

**En tant qu'** opérateur, **je veux** qu'un fichier pathologique dégrade proprement, **afin qu'** un binaire ou un bundle minifié n'immobilise jamais l'application.

Priorité : P0. Taille : M (3). Dépendances : US-001, US-002.

Critères d'acceptation :
- [ ] Un fichier au-delà de 10 Mo est refusé avec un message nommant la taille et la limite, aligné sur `MAX_INPUT_BYTES` du markdown (`markdown/parser.rs:19`).
- [ ] Une ligne au-delà de 10 000 caractères ouvre le fichier en lecture seule, avec un bandeau expliquant pourquoi l'édition est désactivée.
- [ ] Un fichier dont le contenu n'est pas de l'UTF-8 valide est refusé avec un message explicite, sans panic (`from_utf8` géré par `match`, jamais `unwrap`).
- [ ] Un fichier contenant un octet nul dans ses 8 premiers Ko est traité comme binaire et refusé avec le message "Binary file".
- [ ] Un fichier supprimé entre le clic et le chargement produit une erreur "File not found" et non un onglet vide.
- [ ] Un fichier ouvert en lecture seule refuse toute frappe et le signale visuellement, sans avaler la touche silencieusement.
- [ ] Tests couvrant les six cas ci-dessus.

**Justification :** le vrai mur de performance d'un éditeur n'est pas la taille du fichier mais la ligne unique géante, qui casse la virtualisation verticale parce qu'elle concentre tout le coût dans une seule ligne toujours visible. La rendre lecture seule plutôt que de refuser le fichier permet quand même de la lire.

#### US-004 - Coloration incrémentale, couleurs identiques au diff

**En tant qu'** opérateur, **je veux** que les couleurs restent justes pendant que je tape et identiques à celles du diff, **afin de** ne jamais douter de ce que je lis.

Priorité : P0. Taille : L (5). Dépendances : US-001, US-002.

Critères d'acceptation :
- [ ] Le driver de coloration réutilise `DiffSyntax` (`diff/syntax.rs:29`) pour les grammaires et les requêtes, et `crate::diff::rows::palette(ui)` (`rows.rs:522`) pour les couleurs. Aucune grammaire ni couleur n'est dupliquée.
- [ ] L'arbre tree-sitter est conservé vivant dans le document, pas jeté après le parse initial.
- [ ] Chaque édition alimente `Tree::edit` avec un `InputEdit` correct, puis un reparse incrémental via `Parser::parse_with` lisant la rope par chunks.
- [ ] Une frappe remappe d'abord les plages de l'arbre existant sans reparser, sur le modèle de `SyntaxMap::interpolate` (`zed:crates/language/src/syntax_map.rs:291`), pour que la coloration reste plausible dans la même frame.
- [ ] Le reparse est ensuite tenté **synchrone sous budget**, et bascule hors du thread de rendu seulement si le budget est dépassé. Le budget est de 1 ms, la valeur de production de Zed (`zed:crates/language/src/buffer.rs:1138`), pas une valeur choisie au jugé. Le mécanisme suit `reparse_with_timeout` (`zed:crates/language/src/syntax_map.rs:469`).
- [ ] Le reparse asynchrone porte une garde de génération ; le texte reste éditable pendant ce temps, coloré avec le résultat interpolé.
- [ ] Aucune frappe ne déclenche un parse complet du fichier.
- [ ] Un test de parité compare, sur un corpus de fichiers couvrant les 15 grammaires (`Cargo.toml:170-215`), la sortie du driver incrémental à celle de `highlight_lines` (`highlighter.rs:167`) : les plages colorées doivent être identiques.
- [ ] Un fichier au-delà de `MAX_HIGHLIGHT_BYTES` (`highlighter.rs:32`, 300 000 octets) reste éditable, rendu en texte brut non coloré.
- [ ] Une extension sans grammaire connue reste éditable, rendue en texte brut.

**Contrainte :** `highlighter.rs` n'est pas modifié. Le diff en dépend, et faire diverger les deux surfaces est le seul échec inacceptable de cette story. Le driver incrémental est un module séparé qui consomme les mêmes grammaires.

### EP-002 - `CodeElement` : rendu virtualisé

**Definition of done :** un élément GPUI custom peint le fichier avec gouttière et deux axes de défilement, sans aucun travail par frame proportionnel à la taille du fichier.

#### US-005 - `CodeElement` : trois phases et virtualisation

**En tant qu'** opérateur, **je veux** que le défilement et la frappe restent fluides sur un gros fichier, **afin que** l'éditeur ne devienne jamais le maillon lent.

Priorité : P0. Taille : L (5). Dépendances : US-001, US-004.

Critères d'acceptation :
- [ ] `CodeElement` implémente `Element` avec `request_layout` / `prepaint` / `paint`, sur le modèle documenté en `diff/element.rs:1-12`.
- [ ] `request_layout` déclare une hauteur de contenu égale à `line_count * ROW_HEIGHT` (`rows.rs:22`, 18.0) sans shaper aucune ligne. C'est la divergence explicite avec `TextArea`, qui shape tout le contenu au layout (`text_area.rs:1256-1302`).
- [ ] La première et la dernière ligne visibles sont dérivées de `window.content_mask()` par division entière, en O(1).
- [ ] Seules les lignes visibles sont shapées, via `text_system().shape_line`.
- [ ] Le nombre d'appels à `shape_line` par frame est borné par la hauteur du viewport divisée par `ROW_HEIGHT`, indépendamment de `line_count`.
- [ ] L'élément est hôte dans un div `overflow_y_scroll` avec `track_scroll`, conformément à la recette deux-axes documentée dans `CLAUDE.md`.
- [ ] Un changement de thème invalide le rendu via `theme_generation` (la clé du `LineLayoutCache` de GPUI inclut les `runs`, donc les couleurs, et le cache ne couvre que 2 frames).
- [ ] Test : sur un fichier de 100 000 lignes, `prepaint` ne touche pas plus de lignes que le viewport n'en contient.

#### US-006 - Gouttière de numéros de ligne

**En tant qu'** opérateur, **je veux** des numéros de ligne alignés à droite, **afin de** pouvoir référencer une ligne exactement comme dans le diff.

Priorité : P0. Taille : S (2). Dépendances : US-005.

Critères d'acceptation :
- [ ] La largeur de la gouttière est dérivée du nombre de chiffres de `line_count`, recalculée seulement quand ce nombre de chiffres change, jamais par frame.
- [ ] Les numéros sont alignés à droite avec le même `NUM_GAP` que le diff (6.0) pour que les deux onglets s'alignent visuellement.
- [ ] Le numéro de la ligne du curseur utilise `ui.text` ; les autres `ui.muted`.
- [ ] La gouttière ne défile pas horizontalement avec le code : elle reste collée au bord gauche.
- [ ] Ajouter une ligne qui fait passer le fichier de 999 à 1 000 lignes élargit la gouttière sans décaler le curseur d'une ligne.

#### US-007 - Défilement vertical et scrollbar

**En tant qu'** opérateur, **je veux** une scrollbar verticale visible, **afin de** savoir où je suis dans le fichier.

Priorité : P0. Taille : M (3). Dépendances : US-005.

Critères d'acceptation :
- [ ] La molette fait défiler verticalement via le gestionnaire natif du div hôte.
- [ ] Une scrollbar verticale est peinte au bord droit, avec un pouce dont la hauteur est proportionnelle au ratio viewport sur contenu, en couleur `ui.scrollbar_thumb` (`theme/model.rs:30`).
- [ ] Le pouce est draggable et suit le curseur au pixel.
- [ ] Un fichier plus court que le viewport ne peint pas de scrollbar.
- [ ] Le défilement est clampé : pas de surdéfilement au-delà de la dernière ligne ni au-dessus de la première.
- [ ] Le curseur reste visible : toute édition ou navigation qui le sort du viewport déclenche un défilement automatique qui le ramène, avec une marge d'au moins deux lignes.

#### US-008 - Défilement horizontal, sans soft-wrap

**En tant qu'** opérateur, **je veux** que les lignes longues défilent horizontalement plutôt que de s'enrouler, **afin que** la structure du code reste lisible et que les numéros de ligne restent alignés.

Priorité : P1. Taille : M (3). Dépendances : US-005, US-006.

Critères d'acceptation :
- [ ] Aucun soft-wrap : une ligne logique occupe exactement une ligne visuelle. C'est la seconde divergence explicite avec `TextArea` (`text_area.rs:12`), et elle est ce qui rend la virtualisation par division entière possible.
- [ ] Le décalage horizontal maximal est dérivé de la plus longue ligne maintenue par US-001, jamais d'un rescan par frame.
- [ ] `Shift+molette` défile horizontalement en lisant `delta.x` ; le code ne branche jamais sur `modifiers.shift` (la plateforme a déjà permuté l'axe, voir `CLAUDE.md`).
- [ ] `restrict_scroll_to_axis` est posé sur l'hôte pour empêcher la molette verticale de saigner dans l'axe horizontal.
- [ ] La largeur du viewport de texte est calculée depuis la largeur réelle de l'élément moins la gouttière dérivée ; les constantes `-92.0` et `-55.0` de `hscroll.rs:42-48` ne sont pas réutilisées, elles encodent la géométrie du diff.
- [ ] Une scrollbar horizontale n'apparaît que si la plus longue ligne dépasse le viewport.
- [ ] La gouttière reste fixe pendant le défilement horizontal.

### EP-003 - Curseur, sélection, navigation

**Definition of done :** l'utilisateur peut placer un curseur, sélectionner à la souris et au clavier, et naviguer dans le fichier avec les raccourcis attendus, sur les trois plateformes.

#### US-009 - Focus, curseur et ligne courante

**En tant qu'** opérateur, **je veux** voir où je vais taper, **afin de** ne pas éditer à l'aveugle.

Priorité : P0. Taille : M (3). Dépendances : US-005, US-006.

Critères d'acceptation :
- [ ] La vue possède un `FocusHandle` et implémente `Focusable` ; le contexte de touches `"CodeEditor"` scope les raccourcis de l'éditeur.
- [ ] Le curseur est stocké en offset d'octets, comme `TextArea` (`text_area.rs:441`), et rendu comme une barre verticale de 2 px en couleur `ui.cursor`.
- [ ] Le curseur clignote à la cadence système et cesse de clignoter pendant la frappe, reprenant après une pause.
- [ ] La ligne du curseur est peinte avec un fond dérivé de `ui.text` à faible alpha, calculé dans `palette()`, sans ajouter de slot aux 6 fichiers de thème.
- [ ] Perdre le focus masque le curseur et atténue le fond de ligne courante ; le retrouver les restaure à la même position.
- [ ] Un clic au-delà de la dernière ligne place le curseur en fin de fichier, sans panic.
- [ ] Le curseur ne peut jamais se retrouver au milieu d'un graphème : toute position est validée sur une frontière de graphème via `unicode-segmentation`.

#### US-010 - Sélection à la souris

**En tant qu'** opérateur, **je veux** sélectionner du texte à la souris comme dans n'importe quel éditeur, **afin de** copier ou remplacer un fragment.

Priorité : P0. Taille : M (3). Dépendances : US-009.

Critères d'acceptation :
- [ ] Un clic place le curseur ; un drag étend la sélection ; le relâchement la fige. Le comportement copie celui déjà implémenté dans `TextArea` (`text_area.rs:1-23`).
- [ ] Le double-clic sélectionne le mot, le triple-clic la ligne.
- [ ] La sélection est peinte avec `ui.selection` et le texte sélectionné avec `ui.selection_foreground` (`theme/model.rs:22` et `:29`), qui sont déjà validés APCA mais aujourd'hui câblés au seul terminal.
- [ ] Un drag qui sort du viewport fait défiler automatiquement dans la direction du drag.
- [ ] `Ctrl+A` (`Cmd+A` sur macOS) sélectionne tout le document.
- [ ] Une sélection vide n'est pas peinte et se comporte comme un simple curseur.
- [ ] Cliquer ailleurs efface la sélection sans effacer le contenu.

#### US-011 - Navigation clavier

**En tant qu'** opérateur, **je veux** les raccourcis de déplacement standard, **afin de** ne pas réapprendre un éditeur.

Priorité : P1. Taille : M (3). Dépendances : US-009.

Critères d'acceptation :
- [ ] Flèches, `Home`, `End`, `PageUp`, `PageDown` déplacent le curseur ; `Shift` étend la sélection au lieu de la remplacer.
- [ ] `Ctrl+Flèche` (`Alt+Flèche` sur macOS) se déplace par mot, en réutilisant la segmentation de `TextArea`.
- [ ] `Ctrl+Home` et `Ctrl+End` (`Cmd+Haut`/`Cmd+Bas` sur macOS) vont au début et à la fin du document.
- [ ] Haut et bas conservent la colonne visée quand ils traversent une ligne plus courte, comme le fait `TextArea` avec son déplacement par ligne logique (`text_area.rs:17`).
- [ ] Chaque raccourci est déclaré une fois avec ses variantes par plateforme ; aucun modificateur n'est codé en dur pour une seule plateforme.
- [ ] Une touche non liée n'est pas avalée : elle remonte au dispatch parent.

### EP-004 - Édition

**Definition of done :** l'utilisateur peut taper, annuler, coller, indenter et sauvegarder, et le travail concurrent d'un agent ne peut pas écraser le sien silencieusement.

#### US-012 - Saisie native et IME

**En tant qu'** opérateur, **je veux** taper du texte, y compris avec une méthode de saisie, **afin que** l'éditeur fonctionne dans ma langue.

Priorité : P0. Taille : L (5). Dépendances : US-001, US-004, US-009.

Critères d'acceptation :
- [ ] La vue implémente `EntityInputHandler` et enregistre un `ElementInputHandler` dans `paint`, sur le modèle de `text_area.rs:914` et `:1406`.
- [ ] `replace_text_in_range` insère dans la rope, alimente `Tree::edit` (US-004) et remappe curseur et sélection à travers l'édition.
- [ ] La composition IME affiche le preedit souligné à la position du curseur et ne mute le document qu'à la validation, comme le fait déjà le terminal (`terminal/element/mod.rs:1744-1792`).
- [ ] Taper avec une sélection active remplace la sélection.
- [ ] `Retour arrière` et `Suppr` suppriment un graphème complet, jamais un demi-caractère UTF-8.
- [ ] `Entrée` insère un saut de ligne et reprend l'indentation de la ligne précédente.
- [ ] Une frappe sur un document en lecture seule (US-002, US-003) ne mute rien et le signale visuellement.
- [ ] Tests : insertion avec sélection active, retour arrière sur un emoji composé, saut de ligne avec indentation.

#### US-013 - Annuler et rétablir

**En tant qu'** opérateur, **je veux** annuler une erreur, **afin de** ne jamais craindre de taper.

Priorité : P0. Taille : L (5). Dépendances : US-012.

Critères d'acceptation :
- [ ] `Ctrl+Z` annule, `Ctrl+Shift+Z` et `Ctrl+Y` rétablissent (`Cmd+Z` et `Cmd+Shift+Z` sur macOS).
- [ ] Les frappes consécutives sont groupées en une transaction ; une pause de 300 ms, un déplacement de curseur ou un collage ferment le groupe. Les 300 ms sont le `group_interval` de production de Zed (`zed:crates/text/src/text.rs:229`), et la règle de groupage suit la sienne : comparer l'horodatage de première frappe du groupe courant à la dernière frappe du précédent (`zed:crates/text/src/text.rs:294`).
- [ ] Annuler restaure le texte, la position du curseur et la sélection tels qu'ils étaient avant la transaction.
- [ ] Chaque annulation alimente `Tree::edit` en sens inverse : la coloration reste juste après un `undo`, sans reparse complet.
- [ ] L'historique est plafonné à 1 000 transactions ; au-delà, les plus anciennes sont écartées.
- [ ] Annuler jusqu'à l'état sauvegardé remet le document à l'état propre, sans point de modification.
- [ ] Rétablir après une nouvelle frappe est impossible : la branche de rétablissement est vidée.
- [ ] Tests : groupage temporel, annulation d'un collage multi-ligne, curseur restauré.

#### US-014 - Presse-papier et indentation

**En tant qu'** opérateur, **je veux** couper, copier, coller et indenter, **afin de** faire les manipulations courantes sans souris.

Priorité : P1. Taille : M (3). Dépendances : US-010, US-012.

Critères d'acceptation :
- [ ] `Ctrl+C`, `Ctrl+X`, `Ctrl+V` (variantes `Cmd` sur macOS) opèrent sur la sélection via `ClipboardItem`, comme `TextArea` le fait déjà.
- [ ] Copier sans sélection copie la ligne courante entière, saut de ligne compris.
- [ ] Coller un texte multi-ligne insère toutes les lignes et place le curseur à la fin de l'insertion.
- [ ] Coller compte comme une seule transaction d'annulation.
- [ ] `Tab` insère une indentation ; avec une sélection multi-ligne, il indente toutes les lignes touchées.
- [ ] `Shift+Tab` désindente, sans jamais retirer de caractère non blanc.
- [ ] L'unité d'indentation est détectée depuis le fichier (tabulation ou nombre d'espaces dominant) et non imposée.
- [ ] Coller un texte contenant des caractères de contrôle ou des marqueurs bidirectionnels les neutralise avant insertion.

#### US-015 - Sauvegarde et état modifié

**En tant qu'** opérateur, **je veux** sauvegarder explicitement et voir ce qui ne l'est pas, **afin de** garder le contrôle de ce qui touche le disque.

Priorité : P0. Taille : M (3). Dépendances : US-012.

Critères d'acceptation :
- [ ] `Ctrl+S` (`Cmd+S` sur macOS) écrit le fichier ; il n'y a pas de sauvegarde automatique.
- [ ] L'écriture est atomique : fichier temporaire créé dans le répertoire parent de la cible puis persisté par renommage, pour qu'une coupure ne laisse jamais un fichier tronqué. Même forme que `Fs::atomic_write` chez Zed, qui utilise `tempfile::NamedTempFile::new_in(parent)` puis `persist(path)` (`zed:crates/fs/src/fs.rs:927-935`). Le répertoire parent, et non le répertoire temporaire du système, parce qu'un renommage entre systèmes de fichiers n'est pas atomique.
- [ ] L'écriture s'exécute dans `smol::unblock`, jamais sur le thread de rendu.
- [ ] Les permissions et le mode du fichier d'origine sont préservés sur les trois plateformes.
- [ ] Le style de fin de ligne détecté au chargement (US-001) est réappliqué à l'écriture.
- [ ] Un document modifié affiche un point de modification sur son chip d'onglet ; sauvegarder l'efface.
- [ ] Un échec d'écriture (disque plein, permission refusée, fichier devenu lecture seule) affiche une erreur rédigée et **conserve** les modifications en mémoire, sans jamais prétendre avoir sauvegardé.
- [ ] Sauvegarder un fichier suivi par git déclenche le rafraîchissement du diff du dock.

#### US-016 - Conflit d'écriture avec un agent

**En tant qu'** opérateur, **je veux** être averti si un agent modifie le fichier que j'édite, **afin de** ne perdre ni mon travail ni le sien.

Priorité : P0. Taille : L (5). Dépendances : US-015.

Critères d'acceptation :
- [ ] Le mtime et la taille du fichier sont relevés au chargement et à chaque sauvegarde.
- [ ] Une modification externe détectée pendant que le document est **propre** recharge le contenu silencieusement et préserve la position de défilement ainsi que le curseur si le nombre de lignes n'a pas changé.
- [ ] Une modification externe détectée pendant que le document est **modifié** n'écrase rien : un bandeau signale le conflit et propose de conserver la version en mémoire ou de recharger depuis le disque.
- [ ] Une sauvegarde sur un fichier modifié entre-temps est refusée avant écriture ; le même choix est proposé.
- [ ] Choisir de recharger place l'état antérieur dans l'historique d'annulation, pour qu'un `Ctrl+Z` récupère le travail perdu.
- [ ] La suppression externe du fichier ouvert passe le document en état "supprimé sur disque" ; sauvegarder le recrée.
- [ ] La détection réutilise l'infrastructure de surveillance existante plutôt que d'ouvrir un second watcher sur le même répertoire (`files_sidebar/watch.rs:46` et `:144`, `markdown/view.rs:486-579`).
- [ ] Tests : écriture externe sur document propre, sur document modifié, suppression externe, sauvegarde refusée.

**Pourquoi cette story est P0 :** c'est la seule contrainte de ce PRD qu'aucun éditeur généraliste n'a à résoudre avec cette acuité. Dans Paneflow, un agent CLI écrit dans le fichier pendant que l'humain l'édite : c'est le cas nominal, pas le cas limite.

### EP-005 - Intégration au dock

**Definition of done :** l'entrée "File" du menu `+` ouvre un onglet d'édition fonctionnel, avec chip, point de modification, garde de fermeture et raccourci clavier réel.

#### US-017 - Variante `DiffDockTab::File` et cycle de vie de l'onglet

**En tant qu'** opérateur, **je veux** qu'un fichier ouvert soit un onglet comme un autre, **afin de** basculer entre Changes, un terminal et un fichier de la même façon.

Priorité : P0. Taille : M (3). Dépendances : US-005, US-015.

Critères d'acceptation :
- [ ] `DiffDockTab` (`model.rs:31-35`) gagne une variante `File(Entity<CodeEditorView>)`.
- [ ] Le switch de corps (`mod.rs:322`) rend l'éditeur pour cette variante.
- [ ] `render_diff_tab` (`render.rs:124`) étiquette le chip avec le nom de base du fichier, tronqué si nécessaire, et une icône dérivée de l'extension avec repli sur `icons/file-text.svg`.
- [ ] Un document modifié affiche un point à la place ou à côté du bouton de fermeture, comme le fait Cursor.
- [ ] Fermer un onglet modifié demande confirmation ; l'onglet `Changes` reste le seul permanent (`render.rs:178-201`).
- [ ] Fermer un onglet fichier ajuste `diff_active_tab` (`main.rs:877`) comme le fait `close_diff_tab` (`tabs.rs:63`), sans jamais laisser un index hors bornes.
- [ ] Ouvrir un fichier déjà ouvert active l'onglet existant au lieu d'en créer un second.
- [ ] Le nombre d'onglets fichiers simultanés est plafonné à 8 ; au-delà, le plus ancien onglet **non modifié** et non actif est fermé, jamais un onglet modifié.

#### US-018 - Raccourcis, en-tête de fichier et états

**En tant qu'** opérateur, **je veux** que "File" et son raccourci affiché fonctionnent, et savoir quel fichier je lis, **afin que** le menu ne mente plus et que je ne confonde pas deux homonymes.

Priorité : P0. Taille : M (3). Dépendances : US-017.

Critères d'acceptation :
- [ ] Le handler de la ligne "File" (`new_tab_menu.rs:66-69`) ferme le menu puis ouvre la sidebar Files pour choisir un fichier.
- [ ] Une action GPUI est déclarée dans `app/actions.rs` et liée à `Ctrl+G` dans `keybindings/defaults.rs` ; `Ctrl+J` est liée à l'ouverture d'un onglet Terminal, corrigeant la seconde promesse non tenue (`new_tab_menu.rs:75`).
- [ ] Les deux liaisons sont vérifiées libres de conflit avec les 87 actions existantes, et testées sur Linux, macOS et Windows.
- [ ] Une barre sous le strip d'onglets affiche le chemin relatif au worktree, tronqué par la gauche si trop long, alignée sur la hauteur de `render_diff_files_toolbar` (`render.rs:245`, 36 px), plus la position du curseur en ligne et colonne.
- [ ] L'état de chargement réutilise `diff_panel_centered` avec `icons/loader-circle.svg` (`render.rs:295-308`) ; chaque erreur d'US-003 s'affiche via le même composant.
- [ ] Aucun état n'affiche un message technique brut de `std::io::Error` : chaque cas a un libellé rédigé et, quand c'est pertinent, un bouton de rechargement.
- [ ] Le raccourci n'agit que lorsque le dock diff est ouvert et focalisé ; il ne capture pas la frappe dans un terminal.

### EP-006 - Ouverture depuis la sidebar Files existante

**Definition of done :** la sidebar Files déjà livrée ouvre n'importe quel fichier texte dans l'éditeur, et se filtre à la frappe.

#### US-019 - Lever le verrou markdown de la sidebar Files

**En tant qu'** opérateur, **je veux** cliquer n'importe quel fichier de l'arborescence, **afin de** ne plus être limité au markdown.

Priorité : P0. Taille : S (2). Dépendances : US-017.

Critères d'acceptation :
- [ ] `files_sidebar/row.rs:27-30` cesse de griser et d'inhiber les fichiers non-markdown : tout fichier texte est cliquable et lu à pleine couleur.
- [ ] Un clic sur un fichier non-markdown ouvre un onglet `DiffDockTab::File` dans le dock diff ; le markdown conserve son comportement actuel d'ouverture dans le pane actif (`row.rs:116`).
- [ ] Un fichier binaire ou trop grand reste visuellement atténué et affiche l'erreur d'US-003 s'il est ouvert quand même.
- [ ] Le modèle d'arbre (`app/files_tree.rs`), le tri dossiers d'abord (`files_tree.rs:124`), le filtrage gitignore et le watcher (`files_sidebar/watch.rs:46` et `:144`) sont réutilisés sans modification de leur logique.
- [ ] Le drag vers un pane reste réservé au markdown (`row.rs:122-123`) : cette story n'y touche pas.
- [ ] Un second clic sur un fichier déjà ouvert active son onglet au lieu d'en créer un doublon.

#### US-020 - Filtre à la frappe dans la sidebar Files

**En tant qu'** opérateur, **je veux** taper quelques lettres pour trouver un fichier, **afin de** ne pas parcourir l'arbre à la souris.

Priorité : P1. Taille : M (3). Dépendances : US-019.

Critères d'acceptation :
- [ ] Un champ de saisie en tête de sidebar filtre les entrées sur le chemin relatif, insensible à la casse, en réutilisant `workspace_relative_path` (`files_tree.rs:226`).
- [ ] Le filtrage produit un vecteur distinct de `flatten_visible` (`files_tree.rs:213`) ; l'état de pliage de l'arbre n'est pas muté par le filtre.
- [ ] Effacer le champ restaure exactement l'état de pliage antérieur.
- [ ] Le filtrage d'un arbre de 50 000 entrées s'exécute en moins de 16 ms, sinon il est déporté off-thread.
- [ ] Une requête sans résultat affiche "No matching files", pas une liste vide muette.
- [ ] `Échap` vide le champ et rend le focus à l'arbre.
- [ ] Les segments correspondants sont mis en évidence via `StyledText::with_highlights`, pas par des spans imbriqués.

**Note sur l'absence de modèle Zed :** le `ProjectPanel` de Zed n'a pas de filtre à la frappe ; c'est son file finder séparé qui filtre, sur le crate `zed:crates/fuzzy/`. Cette story n'a donc pas d'ancrage Zed à copier. Si un scoring flou devient nécessaire au-delà du simple sous-chaîne, `zed:crates/fuzzy/src/matcher.rs` est la référence, mais le critère ci-dessus n'en demande pas.

## Functional Requirements

| ID | Exigence |
|----|----------|
| FR-1 | Aucun travail par frame ne peut être proportionnel à `line_count`. |
| FR-2 | Aucune frappe ne peut déclencher un parse tree-sitter complet. |
| FR-3 | Les couleurs sortent des mêmes grammaires et de la même palette que le diff ; un test de parité le prouve. |
| FR-4 | Toute lecture disque, toute écriture disque et tout parse coûteux s'exécutent hors du thread GPUI. |
| FR-5 | Aucune écriture disque n'est implicite : la sauvegarde est toujours une action de l'utilisateur. |
| FR-6 | Aucune modification externe ne peut écraser silencieusement une modification en mémoire, ni l'inverse. |
| FR-7 | Chaque état d'erreur porte un libellé rédigé, jamais un message technique brut. |
| FR-8 | Le code compile et se comporte à l'identique sur Linux, macOS et Windows ; chaque raccourci a ses variantes par plateforme. |
| FR-9 | Aucun nouveau slot de thème n'est ajouté aux 6 fichiers de thème ; les couleurs manquantes sont dérivées dans `palette()`. |

## Non-Goals

- Autocomplétion, LSP, diagnostics, aller à la définition, renommage symbolique.
- Formatage automatique à la sauvegarde, actions de code, correctifs rapides.
- Multi-curseur et sélection par colonne.
- Recherche et remplacement dans le fichier (le dock a déjà une recherche côté diff ; l'unifier est un travail distinct).
- Soft-wrap : rejeté explicitement, c'est ce qui rend la virtualisation par division entière possible (US-008).
- Repli de code, minimap, lentille de code, blame en gouttière.
- Ligatures de police : désactivées, seule divergence assumée avec la capture Codex App. La fast-path monospace conditionne le calcul incrémental de la plus longue ligne.
- Onglets fichiers persistants entre sessions.
- Coloration au-delà des 15 grammaires déjà embarquées (`Cargo.toml:170-215`).
- Refonte de `widgets/text_area.rs` : le composer Agents en dépend, et son modèle soft-wrap non virtualisé est correct pour son usage.
- Édition depuis la vue diff elle-même : ce PRD n'ajoute pas d'édition en place dans les hunks.

## Design Considerations

L'éditeur copie la capture Codex App pour la géométrie : gouttière fine, numéros alignés à droite en teinte atténuée, pas de bordure entre gouttière et code, ligne courante en fond à peine perceptible, scrollbar fine sans piste visible, curseur en barre verticale fine.

Il hérite de la géométrie du diff là où l'alignement entre onglets compte : `ROW_HEIGHT` à 18.0 (`rows.rs:22`) et `NUM_GAP` à 6.0, pour que basculer de Changes vers un fichier ne fasse pas sauter les lignes.

Le chip d'onglet suit le gabarit des chips existants (`render.rs:139-176`) : hauteur 26, rayon 7, gap 6, fond `ui.text` à 0.05 quand actif. Le point de modification remplace la croix au repos et cède la place à la croix au survol, comme dans Cursor.

Deux couleurs manquent au modèle de thème et sont dérivées plutôt qu'ajoutées : le fond de ligne courante, dérivé dans `palette()` (`rows.rs:522`) comme l'est déjà `sticky_header_bg`. En revanche `ui.selection`, `ui.selection_foreground` et `ui.cursor` (`theme/model.rs:21-29`) existent, sont déjà validés APCA, et sont simplement recâblés : ils ne servaient jusqu'ici qu'au terminal.

## Technical Considerations

**Ce qui est réutilisé.** Les grammaires et requêtes tree-sitter via `DiffSyntax` (`syntax.rs:29`), la palette via `palette()` (`rows.rs:522`), le protocole de saisie et la navigation graphème de `TextArea` (`text_area.rs:914`, `:46`), le patron de chargement off-thread de `MarkdownView` (`view.rs:155-157`), l'architecture d'élément virtualisé du diff (`element.rs:1-12`), et l'intégralité de l'arborescence de fichiers (`app/files_tree.rs`, `app/files_sidebar/`).

**Ce qui est net-neuf.** Le `CodeElement` virtualisé, le driver de coloration incrémentale, l'historique d'annulation transactionnel, la sauvegarde atomique et la détection de conflit.

**Pourquoi ne pas réutiliser `TextArea` directement.** Deux blocages structurels, pas des détails d'implémentation. Son `request_layout` shape chaque ligne du contenu pour calculer la hauteur après soft-wrap (`text_area.rs:1256-1302`) : sur un fichier de 10 000 lignes, le coût de layout est celui du fichier entier, à chaque frame. Et son modèle soft-wrap fait qu'une ligne logique occupe un nombre variable de lignes visuelles, ce qui interdit la virtualisation par division entière. Le composer Agents n'a ni l'un ni l'autre problème, donc `TextArea` reste tel quel et n'est pas refactorisé (voir Non-Goals). Le `CodeEditorView` reprend la forme de son `EntityInputHandler` et ses helpers de segmentation.

**Piège GPUI documenté.** Le `LineLayoutCache` de GPUI n'a que deux frames et sa clé inclut les `runs`, donc les couleurs : un changement de thème invalide tout le cache. La garde `theme_generation` (déjà utilisée par `AgentsDiffData`, `model.rs:99`) sert de déclencheur de recalcul.

**Piège inotify documenté.** Un watcher récursif sur la racine d'un projet Rust épuise les watches inotify et gèle le thread GPUI si le `WalkDir` s'exécute dessus (`diff/view/watcher.rs:115-141`). US-016 ne crée pas de watcher supplémentaire : elle se branche sur celui de la sidebar Files.

**Constantes non transposables.** `h_text_viewport` (`hscroll.rs:42-48`) code en dur `-92.0` et `-55.0`, qui encodent la largeur de la barre de hunk, de la gouttière et des paddings du diff. L'éditeur calcule sa largeur de viewport depuis sa propre gouttière dérivée.

**Nouvelle dépendance.** `ropey` (voir US-001). C'est la seule ajoutée par ce PRD. `tree-sitter`, `unicode-segmentation`, `smol` et `notify` sont déjà présents.

## Références Zed

Le codebase de Zed est le socle factuel de ce PRD : les décisions d'architecture ci-dessus viennent d'une exploration ciblée de son éditeur, et chacune est vérifiable. Cette section existe pour qu'un agent qui implémente puisse retrouver la décision d'origine plutôt que de la redériver.

**Checkout local :** `/home/arthur/dev/zed`.

**Révision vérifiée :** branche `main`, commit `f42c6e873eee375b94ec8684001a1d6eba8c3a2b` (2026-08-25). Toutes les lignes citées ci-dessous ont été lues à cette révision.

**Mise en garde, à lire avant de suivre un numéro de ligne.** Ce checkout n'est **pas** la révision que Paneflow épingle. Les huit dépendances git de `src-app/Cargo.toml` pointent `arthjean/zed@3aaba57b95c22f4d21bbbf9f4b10b513173209db`, basé sur `zed-industries/zed@afc13dc8`. Le checkout local est donc en avance sur le fork épinglé. Deux conséquences : les numéros de ligne dérivent à chaque `git pull`, et l'API GPUI vue localement peut différer de celle que Paneflow compile. Traiter les lignes comme un point d'entrée, jamais comme une adresse stable : chercher le nom de symbole, pas la ligne. Toute API GPUI relevée ici doit être revérifiée contre le rev épinglé avant d'être appelée.

**Zed est GPL-3.0.** Ces références servent à comprendre un mécanisme et à valider un ordre de grandeur, pas à copier du code. Se reporter à la note de licence déjà suivie sur ce dépôt avant tout emprunt littéral.

### Ancrage par story

| Story | Mécanisme | Référence Zed |
|-------|-----------|---------------|
| US-001 | Rope sur `SumTree<Chunk>`, conversions offset/point, chunks bornés | `crates/rope/src/rope.rs:26-27`, `:361` (`chunks_in_range`), `:397` (`offset_to_point`) |
| US-002 | Statut de parse observable pendant le chargement | `crates/language/src/buffer.rs:1140` (`parse_status`) |
| US-004 | Remappage sans reparse, puis reparse incrémental | `crates/language/src/syntax_map.rs:291` (`interpolate`), `:414-438` (`InputEdit` puis `tree.edit`), `:459` (`reparse`) |
| US-004 | Budget de reparse synchrone avant bascule async | `crates/language/src/syntax_map.rs:469` (`reparse_with_timeout`), `crates/language/src/buffer.rs:121` et `:1138` (`sync_parse_timeout`, 1 ms en production) |
| US-004 | Requête de coloration bornée à une plage d'octets | `crates/language/src/syntax_map.rs:1222` (`set_byte_range`), `:1553` (`parse_with_options`) |
| US-005 | Layout des seules lignes visibles | `crates/editor/src/element.rs:3072` (`layout_lines`), `:9025` (`visible_row_range`) |
| US-005 | Cache de layout de ligne à deux frames, clé incluant les runs | `crates/gpui/src/text_system/line_layout.rs:455-456`, `:507-514` |
| US-006 | Gouttière et numéros de ligne | `crates/editor/src/element.rs:2748` (`layout_line_numbers`), `:6588` (`shape_line_number`) |
| US-007 | Ancre de défilement et position | `crates/editor/src/scroll.rs:35` (`ScrollAnchor`), `:48` (`scroll_position`) |
| US-012 | Contrat de saisie native et IME | `crates/gpui/src/input.rs:12` (`EntityInputHandler`), `:31` (`marked_text_range`), `:48` (`replace_text_in_range`) |
| US-013 | Annulation transactionnelle et groupage temporel | `crates/text/src/text.rs:135` (`Transaction`), `:229` (`group_interval`, 300 ms en production), `:294` (règle de groupage), `:1313`-`:1352` (`start_transaction` à `undo`) |
| US-015 | Écriture atomique | `crates/fs/src/fs.rs:136` (contrat `atomic_write`), `:927-935` (`NamedTempFile::new_in` puis `persist`) |
| US-015 | État modifié mémoïsé par version | `crates/language/src/buffer.rs:137` (`has_unsaved_edits`), `:1558` (`did_save`) |
| US-016 | Détection de conflit par mtime, rechargement annulable | `crates/language/src/buffer.rs:109` et `:1453` (`saved_mtime`), `:1574` (`reload`, qui retourne une `Transaction`) |
| US-020 | Pas de modèle : le `ProjectPanel` de Zed n'a pas de filtre à la frappe | `crates/fuzzy/src/matcher.rs` seulement si un scoring flou devient nécessaire |

### Trois décisions que l'inspection a corrigées

Ces valeurs étaient posées au jugé dans la version 2.0 du PRD et ont été remplacées par les valeurs de production de Zed. Elles sont listées ici pour qu'on ne les « réarrondisse » pas plus tard en croyant les améliorer.

1. **Budget de reparse synchrone : 1 ms, pas 2 ms.** Zed tente le reparse sur le thread appelant sous un budget de 1 ms et ne bascule en asynchrone qu'au-delà (`crates/language/src/buffer.rs:1138`). Le seuil est bas parce qu'il est comparé à un budget de frame, pas à une latence perçue.
2. **Groupage d'annulation : 300 ms, pas 500 ms** (`crates/text/src/text.rs:229`).
3. **Écriture atomique : temporaire nommé dans le répertoire parent.** Pas dans le répertoire temporaire du système, parce qu'un renommage entre systèmes de fichiers n'est pas atomique (`crates/fs/src/fs.rs:933`).

### Deux endroits où ce PRD diverge délibérément de Zed

Ces divergences sont des choix, pas des omissions. Ne pas les « corriger » vers le modèle Zed sans rouvrir la décision.

1. **Pas de soft-wrap.** Zed maintient une `wrap_map` complète (`crates/editor/src/display_map/wrap_map.rs`) qui traduit lignes logiques en lignes visuelles. Ce PRD refuse le soft-wrap (US-008), ce qui rend la ligne visible dérivable par simple division entière et supprime toute cette couche. C'est le principal gain de simplicité de l'éditeur face à celui de Zed.
2. **Pas de `DisplayMap`.** Zed empile fold, tab, wrap, block et inlay maps (`crates/editor/src/display_map.rs`, 4364 lignes) entre le buffer et le rendu. Aucune de ces transformations n'est dans le périmètre (voir Non-Goals), donc le `CodeElement` lit la rope directement.

## Success Metrics

| Métrique | Baseline | Cible | Horizon | Mesure |
|----------|----------|-------|---------|--------|
| Temps de `paint` du `CodeElement`, fichier 5 000 lignes | n/a | < 4 ms p95 | Mois-1 | `cargo flamegraph` selon `tasks/heaptrack-runbook.md` |
| Latence frappe vers pixel, fichier 5 000 lignes | n/a | < 16 ms p95 | Mois-1 | `PANEFLOW_LATENCY_PROBE=1` |
| Durée de reparse incrémental après une frappe | n/a | < 1 ms p95 en synchrone, sinon basculé async | Mois-1 | Assertion de test plus trace |
| Travail sur le thread GPUI à l'ouverture d'un fichier de 1 Mo | n/a | < 2 ms | Mois-1 | Trace de frame |
| Appels à `shape_line` par frame | n/a | <= hauteur viewport / 18 | Mois-1 | Assertion de test |
| Mémoire résidente supplémentaire, fichier 1 Mo ouvert | n/a | < 12 Mo | Mois-1 | Diff heaptrack |
| Divergences de coloration éditeur contre diff | n/a | 0 | Mois-1 | Test de parité (US-004) |
| Éditions humaines perdues sur conflit agent | n/a | 0 | Mois-1 | Tests d'US-016 |

## Edge Cases

| Catégorie | Cas | Comportement attendu | Story |
|-----------|-----|----------------------|-------|
| Frontière | Fichier vide | Une ligne vide éditable, gouttière à un chiffre | US-001, US-018 |
| Frontière | Fichier sans `\n` final | Pas de ligne fantôme, `\n` non ajouté à la sauvegarde | US-001, US-015 |
| Frontière | Ligne de plus de 10 000 caractères | Ouvert en lecture seule avec bandeau explicatif | US-003 |
| Frontière | Passage de 999 à 1 000 lignes | Gouttière élargie sans décalage du curseur | US-006 |
| Validation | Contenu non UTF-8 | Erreur "Invalid encoding", pas de panic | US-003 |
| Validation | Fichier binaire (octet nul) | Erreur "Binary file" | US-003 |
| Validation | Fichier de plus de 10 Mo | Erreur nommant taille et limite | US-003 |
| Validation | Fichier en lecture seule sur disque | Ouvert non éditable, frappe signalée et refusée | US-002, US-012 |
| Unicode | Retour arrière sur un emoji composé | Le graphème entier est supprimé | US-012 |
| Unicode | Composition IME interrompue par un clic | Le preedit est validé ou abandonné, jamais laissé en suspens | US-012 |
| Unicode | Collage contenant des marqueurs bidirectionnels | Neutralisés avant insertion | US-014 |
| Concurrence | Deux ouvertures rapprochées | La génération obsolète est ignorée | US-002 |
| Concurrence | Onglet fermé pendant le chargement | Échec silencieux de l'`update`, pas de panic | US-002 |
| Concurrence | Agent écrit pendant que le document est propre | Rechargement silencieux, défilement préservé | US-016 |
| Concurrence | Agent écrit pendant que le document est modifié | Bandeau de conflit, aucun écrasement | US-016 |
| Concurrence | Sauvegarde d'un fichier modifié entre-temps | Refusée avant écriture, choix proposé | US-016 |
| Système de fichiers | Fichier supprimé après le clic | Erreur "File not found" | US-003 |
| Système de fichiers | Fichier ouvert supprimé sur disque | État "supprimé sur disque", sauvegarder le recrée | US-016 |
| Système de fichiers | Disque plein à la sauvegarde | Erreur rédigée, modifications conservées en mémoire | US-015 |
| État UI | Fermer un onglet modifié | Confirmation demandée | US-017 |
| État UI | Neuvième fichier ouvert | Fermeture du plus ancien onglet non modifié, jamais un modifié | US-017 |
| État UI | Changement de thème, fichier ouvert | Recoloration via `theme_generation` | US-005 |
| État UI | Perte de focus avec sélection active | Curseur masqué, sélection conservée | US-009 |
| Cross-platform | Chemins Windows | `PathBuf` partout, aucun `/` codé en dur | US-015, US-019 |
| Cross-platform | Fins de ligne CRLF | Préservées à l'aller-retour | US-001, US-015 |
| Cross-platform | Raccourcis macOS | Variantes `Cmd` déclarées pour chaque liaison | US-011, US-013, US-014, US-015 |

## Quality Gates

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

`cargo fmt --check` est obligatoire avant chaque commit et chaque push touchant du Rust (voir `CLAUDE.md`) : le pipeline de release l'exécute sur les quatre jambes de matrice et un seul écart fait échouer les quatre.

Toute story marquée UI exige en plus une vérification visuelle manuelle : les gates automatiques ne couvrent pas le rendu, le clignotement du curseur ni la composition IME.

## Open Questions

1. L'unité d'indentation (US-014) est détectée depuis le fichier. Faut-il un repli configurable quand le fichier est vide ou ambigu ? Proposition : 4 espaces, sans réglage, jusqu'à ce qu'une demande concrète apparaisse.
2. Le rechargement silencieux d'un document propre (US-016) préserve le curseur "si le nombre de lignes n'a pas changé". Un remappage plus fin par diff textuel serait meilleur mais coûte une story entière. Proposition : garder la règle simple en v1.
3. `Ctrl+G` est conventionnellement "aller à la ligne" dans plusieurs éditeurs. Le menu l'affiche déjà pour "File" (`new_tab_menu.rs:63`), donc la liaison suit le menu, mais si "aller à la ligne" arrive un jour il faudra réarbitrer.

## Files to Modify

| Fichier | Nature |
|---------|--------|
| `src-app/src/app/agents_diff/model.rs` | Variante `File` de `DiffDockTab` |
| `src-app/src/app/agents_diff/mod.rs` | Switch de corps, déclaration des modules |
| `src-app/src/app/agents_diff/render.rs` | Étiquette, icône et point de modification du chip |
| `src-app/src/app/agents_diff/new_tab_menu.rs` | Handler de la ligne "File" |
| `src-app/src/app/agents_diff/tabs.rs` | Ouverture, activation et fermeture d'onglet fichier |
| `src-app/src/app/actions.rs` | Actions d'ouverture, de sauvegarde et d'annulation |
| `src-app/src/keybindings/defaults.rs` | Liaisons `Ctrl+G`, `Ctrl+J`, `Ctrl+S`, `Ctrl+Z` et variantes macOS |
| `src-app/src/main.rs` | État de l'éditeur sur `AgentsViewState` |
| `src-app/src/app/bootstrap.rs` | Initialisation de cet état |
| `src-app/src/diff/rows.rs` | Dérivation de la couleur de ligne courante dans `palette()` |
| `src-app/src/app/files_sidebar/row.rs` | Lever le verrou markdown (US-019) |
| `src-app/src/app/files_sidebar/view.rs` | Champ de filtre (US-020) |
| `src-app/Cargo.toml` | Ajout de `ropey` |

## Files NOT to Modify

| Fichier | Raison |
|---------|--------|
| `src-app/src/diff/highlighter.rs` | Le diff en dépend ; faire diverger les deux surfaces est le seul échec inacceptable d'US-004 |
| `src-app/src/diff/syntax.rs` | Consommé tel quel pour les grammaires et les requêtes |
| `src-app/src/diff/element.rs` | Le `CodeElement` est un élément séparé, pas une variante de `DiffBody` |
| `src-app/src/diff/hscroll.rs` | Ses constantes encodent la géométrie du diff |
| `src-app/src/widgets/text_area.rs` | Le composer Agents en dépend ; son modèle soft-wrap non virtualisé est correct pour son usage |
| `src-app/src/app/files_tree.rs` | Modèle d'arbre réutilisé sans changement de logique |
| `src-app/src/app/files_sidebar/watch.rs` | Watcher réutilisé ; US-016 s'y branche au lieu d'en créer un second |
| `src-app/assets/themes/*.json` | Aucun nouveau slot de thème (FR-9) |

## New Files

| Fichier | Contenu |
|---------|---------|
| `src-app/src/app/agents_diff/code/document.rs` | `CodeDocument` sur `ropey`, garde-fous, détection de fin de ligne |
| `src-app/src/app/agents_diff/code/load.rs` | Chargement et sauvegarde off-thread, génération, détection de conflit |
| `src-app/src/app/agents_diff/code/highlight.rs` | Driver de coloration incrémentale sur `DiffSyntax` |
| `src-app/src/app/agents_diff/code/element.rs` | `CodeElement` virtualisé |
| `src-app/src/app/agents_diff/code/input.rs` | `EntityInputHandler`, curseur, sélection, navigation |
| `src-app/src/app/agents_diff/code/history.rs` | Historique d'annulation transactionnel |
| `src-app/src/app/agents_diff/code/view.rs` | `CodeEditorView`, en-tête, états, bandeau de conflit |

[/PRD]
