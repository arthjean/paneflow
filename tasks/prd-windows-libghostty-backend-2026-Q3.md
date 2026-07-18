[PRD]

# Backend libghostty natif pour Paneflow sous Windows

**Status:** READY  
**Version:** 1.1  
**Author:** Arthur Jean  
**Created:** 2026-07-18  
**Target:** 2026-Q3  
**Scope:** Windows 10 1809+ et Windows 11, x86_64-pc-windows-msvc  
**Related:** tasks/prd-linux-libghostty-backend-2026-Q3.md, tasks/prd-linux-libghostty-promotion-2026-Q3.md  
**Ghostling reference codebase:** C:\dev\ghostling  
**Ghostty upstream codebase:** C:\dev\ghostty

## Changelog

| Version | Date | Status | Changes |
|---|---|---|---|
| 1.1 | 2026-07-18 | READY | Ajout des emplacements locaux officiels utilisés pour explorer Ghostling et la source Ghostty épinglée. |
| 1.0 | 2026-07-18 | READY | PRD initial fondé sur l’intégration Linux livrée, l’audit de Paneflow et Ghostling, les sources Ghostty épinglées et les contrats ConPTY. |

## Problem Statement

Paneflow utilise déjà libghostty-vt avec succès sur Linux, mais le backend Ghostty reste entièrement exclu de la compilation et de la sélection runtime sous Windows. Les utilisateurs Windows restent donc sur le backend Alacritty et ne bénéficient pas du moteur VT Ghostty, alors que l’architecture Paneflow possède déjà une abstraction de backend, un renderer GPUI neutre et une couche PTY portable.

Le blocage n’est pas un manque de capacité de libghostty-vt: la révision Ghostty épinglée par Paneflow sait produire une bibliothèque Windows. Le blocage se trouve dans quatre frontières Paneflow encore Linux-only:

1. La distribution d’un artefact natif MSVC reproductible, vérifié et compatible avec le runtime C Windows.
2. Les gates Cargo, FFI et build scripts qui n’acceptent aujourd’hui que Linux.
3. L’observation et l’arrêt du processus Ghostty, actuellement fondés sur des primitives POSIX.
4. La qualification fonctionnelle Windows, notamment ConPTY, AltGr, dead keys, IME, clipboard, resize, final drain, descendants et packaging MSI.

Une simple démonstration affichant PowerShell ne suffirait pas. Le résultat doit pouvoir devenir le backend Windows par défaut sans créer de régression sur les workflows réels, sans processus orphelins, sans dépendance DLL fragile, sans fuite de données terminal et avec un rollback Alacritty immédiat avant le spawn d’un child.

## Overview

Ce projet étend l’intégration libghostty-vt existante de Paneflow à Windows x64. Il ne porte ni l’application Ghostling ni l’interface Ghostty complète. Paneflow conserve GPUI pour la fenêtre, le rendu, l’IME, les panes, la configuration et les interactions produit. Il conserve portable-pty comme host PTY, ce qui sélectionne ConPTY sur Windows. Libghostty-vt reste responsable du parsing VT, de l’état terminal, des snapshots et de l’encodage des protocoles terminal.

Le rollout comporte deux états successifs au sein du même PRD:

1. Qualification: libghostty est compilé dans les builds Windows standards, mais seul un choix explicite active Ghostty. La valeur auto continue de sélectionner Alacritty.
2. Promotion: après passage de tous les quality gates, auto sélectionne Ghostty sous Windows. Alacritty reste disponible comme rollback explicite. Une session ne change jamais de backend après le spawn de son processus.

La cible de production est ghostty-vt-static.lib pour x86_64-pc-windows-msvc, avec SIMD. Un build sans SIMD peut servir au diagnostic du linkage, mais ne devient pas la configuration de production sans décision documentée et sans passage des budgets de performance. Une DLL n’est qu’un plan de contingence si le linkage statique est démontré impossible.

## Goals

| ID | Goal | Target |
|---|---|---|
| G-001 | Produire et consommer un artefact libghostty-vt Windows natif, hermétique et traçable. | Build x64 MSVC statique au SHA Ghostty ae52f97dcac558735cfa916ea3965f247e5c6e9e et Zig 0.15.2, hashé et vérifié en CI. |
| G-002 | Exécuter un terminal Ghostty complet sur ConPTY sans modifier le renderer GPUI. | PowerShell 7, Windows PowerShell 5.1, cmd.exe, Git Bash et WSL lorsqu’installé passent la matrice fonctionnelle. |
| G-003 | Atteindre la parité de comportement avec le backend Windows Alacritty sur les fonctions Paneflow supportées. | Zéro divergence inexpliquée dans le corpus différentiel et 100 % des cas input, IME, clipboard, resize et lifecycle validés. |
| G-004 | Garantir un lifecycle sûr et déterministe. | Zéro deadlock, zéro child ou descendant orphelin et zéro double-spawn sur les campagnes de stress définies dans les quality gates. |
| G-005 | Distribuer Ghostty dans le MSI Paneflow sans dépendance native fragile. | Paneflow.exe contient le linkage statique, aucune ghostty-vt.dll n’est requise, installée ou chargée. |
| G-006 | Promouvoir Ghostty sans rendre le rollback risqué. | auto sélectionne Ghostty après qualification; alacritty reste sélectionnable par configuration et le fallback automatique n’arrive qu’avant tout spawn. |
| G-007 | Préserver les autres plateformes. | Les comportements et gates Linux restent verts; macOS continue son chemin existant sans compiler de branche Windows. |

## Target Users

### Utilisateur primaire: développeur Paneflow sous Windows

Il utilise Windows 10 ou 11 x64 comme machine de développement principale, ouvre plusieurs panes et mélange PowerShell, cmd, Git Bash, WSL, agents de code et outils TUI. Il attend un terminal rapide, correct sur Unicode, compatible avec ses raccourcis, son clavier local, l’IME et le clipboard. Il ne doit pas connaître l’existence du linkage natif pour utiliser l’application.

### Utilisateur secondaire: utilisateur avancé et contributeur Paneflow

Il veut choisir explicitement Ghostty ou Alacritty, diagnostiquer un échec de backend sans exposer le contenu de son terminal, reproduire un problème et revenir à Alacritty sans réinstaller Paneflow.

### Mainteneur Paneflow

Il doit pouvoir reconstruire l’artefact natif à partir du SHA épinglé, vérifier sa provenance, détecter une dérive ABI, exécuter une matrice Windows reproductible et publier un MSI sans installer Zig ou Ghostty sur la machine de consommation standard.

## Research Findings

### Codebase Paneflow et Ghostling

| Finding | Evidence | Product implication |
|---|---|---|
| Paneflow possède déjà TerminalSessionBackend et un snapshot Content backend-neutre. | src-app/src/terminal/pty_session.rs et src-app/src/terminal/types.rs | Il faut étendre le backend existant, pas créer un troisième terminal ni brancher le renderer sur l’OS. |
| Le worker Ghostty sérialise déjà moteur, PTY, input, resize et shutdown. | src-app/src/terminal/ghostty_session.rs | La structure runtime est réutilisable; seules les primitives host doivent être séparées par OS. |
| Le lifecycle Ghostty actuel emploie getpgid, waitid, kill et des signaux de groupe. | src-app/src/terminal/ghostty_session.rs | Windows exige une implémentation dédiée fondée sur Child, ConPTY et la gestion d’arbre de processus existante. |
| portable-pty 0.9.0 sélectionne ConPtySystem dans native_pty_system sous Windows. | Source locale portable-pty 0.9.0 et [documentation portable-pty](https://docs.rs/portable-pty/0.9.0/portable_pty/) | Paneflow ne doit pas réimplémenter CreatePseudoConsole dans le runtime Ghostty. |
| IME commit écrit déjà de l’UTF-8 de façon backend-neutre, mais le clavier et la souris n’utilisent pas encore les encodeurs Ghostty. | src-app/src/terminal/view.rs, src-app/src/terminal/input.rs, crates/paneflow-terminal-ghostty/src/encode.rs | L’input Windows doit être une story de parité de premier rang, pas une validation visuelle tardive. |
| Paneflow possède déjà une terminaison d’arbre Windows et un Job Object process-wide. | src-app/src/terminal/pty_session.rs et src-app/src/agents/parent_guard.rs | Le port doit réutiliser ou extraire ces invariants sans dupliquer une troisième politique de processus. |
| Le sys crate vérifie déjà header, bindings, archive et build-info, sans téléchargement au build Cargo. | crates/paneflow-libghostty-sys/build.rs et native/libghostty | La chaîne Windows doit rester hermétique et reproduire ce contrat. |
| Ghostling est une démonstration POSIX basée sur forkpty, ioctl, waitpid et kill. | C:\dev\ghostling\main.c | Ghostling sert de référence d’intégration libghostty, pas de couche PTY à porter dans Paneflow. |
| Paneflow et Ghostling épinglent le même SHA Ghostty. | native/libghostty/manifest.toml et C:\dev\ghostling | Les résultats ABI et comportement peuvent être comparés sans ambiguïté de version. |

### Sources primaires externes

- La révision Ghostty épinglée expose un build lib-vt Windows, un header C avec dllexport/dllimport et des cibles DLL, import library et static library: [ghostty-org/ghostty](https://github.com/ghostty-org/ghostty).
- ConPTY est disponible à partir de Windows 10 version 1809. Microsoft documente des pipes synchrones UTF-8/VT, un drain continu, ResizePseudoConsole et un ordre strict de fermeture: [Creating a pseudoconsole session](https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session) et [CreatePseudoConsole](https://learn.microsoft.com/en-us/windows/console/createpseudoconsole).
- Les Job Objects permettent de gérer un arbre de processus, mais l’appartenance à un job parent ou imbriqué peut limiter l’assignation: [Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).
- Cargo attend qu’un build script déclare explicitement les chemins et bibliothèques de linkage natif et recommande la clé links pour coordonner une bibliothèque native: [Cargo build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html).
- Une DLL chargerait une surface de packaging et de recherche supplémentaire. Microsoft recommande des chemins absolus ou des modes de recherche restreints pour réduire le DLL planting: [Dynamic-link library security](https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-security).

### Competitive context

Windows Terminal, WezTerm, Alacritty et Rio démontrent que ConPTY, Unicode, GPU rendering et les protocoles clavier modernes sont devenus des attentes de base. La différenciation de Paneflow ne vient pas d’une nouvelle fenêtre terminal, mais de l’intégration d’un moteur VT moderne et embeddable à son produit multi-pane et agent-first. Le choix libghostty-vt maximise cette profondeur sans abandonner GPUI ni les workflows Paneflow.

## Assumptions & Constraints

1. La base de référence est libghostty-vt au commit ae52f97dcac558735cfa916ea3965f247e5c6e9e, API 0.1.0, Ghostty 1.3.2-dev et Zig 0.15.2. Tout changement de SHA, header, bindings ou flags invalide les hashes et relance la qualification.
2. La v1 cible x86_64-pc-windows-msvc. Windows ARM64 reste architecturalement possible, mais son artefact et sa matrice sont hors périmètre de cette livraison.
3. Windows 10 antérieur à 1809, Windows 8.1 et Windows 7 ne sont pas supportés, car ConPTY n’y fournit pas le contrat requis.
4. La consommation standard du workspace et le build Cargo ne téléchargent rien et ne requièrent ni Zig ni un checkout Ghostty. La reconstruction de provenance se déroule dans une lane CI dédiée.
5. Le linkage statique avec SIMD est la cible. Un résultat no-SIMD ou DLL ne peut pas remplacer cette cible silencieusement.
6. Paneflow reste propriétaire de GPUI, des panes, de l’IME, du clipboard, des keybindings, de la configuration, de la télémétrie, du PTY et du packaging.
7. Libghostty-vt reste propriétaire du parsing VT, de l’état terminal, des snapshots et de l’encodage terminal spécifique au backend.
8. Alacritty reste compilé et disponible sur Windows pendant et après la promotion. Une session ayant créé un child ne peut pas changer de backend.
9. Le fallback automatique est permis seulement si Ghostty échoue avant le spawn. Après spawn, l’erreur est visible et la session est arrêtée proprement au lieu de créer un second child.
10. Le contenu terminal, les commandes, le clipboard et les séquences OSC ne sont jamais ajoutés à la télémétrie ou aux logs de production.
11. Les changements Windows sont isolés par cfg et ne modifient pas le comportement Linux ou macOS.
12. Les scripts, artefacts et workflows natifs doivent fonctionner depuis un chemin Windows contenant des espaces et des caractères non ASCII.

## Quality Gates

Tous les gates ci-dessous doivent passer avant US-018. Ils constituent la définition unique de qualification du PRD.

| ID | Gate | Pass condition |
|---|---|---|
| QG-001 | Format, lint et tests workspace sur Windows | Les commandes cargo fmt --check, cargo clippy --workspace --all-targets --locked -- -D warnings et cargo test --workspace --locked passent sur la lane x64 MSVC. |
| QG-002 | Build release Ghostty Windows | cargo build -p paneflow-app --release --target x86_64-pc-windows-msvc --features libghostty-windows --locked passe depuis un checkout standard sans Zig ni Ghostty local. |
| QG-003 | Provenance et ABI | Le SHA source, la version Zig, les flags, le header, les bindings, le build-info et l’archive correspondent au manifest; les tests de taille, alignement, symboles et allocation passent; aucune récupération réseau n’est effectuée par build.rs. |
| QG-004 | Linkage statique | L’inspection COFF et des dépendances de paneflow.exe confirme l’architecture x64, les bibliothèques système attendues et l’absence de ghostty-vt.dll ou de chemin de chargement Ghostty dynamique. |
| QG-005 | Corpus différentiel | Tous les chunks du corpus backend passent sous Windows avec zéro divergence inexpliquée. Une différence intentionnelle possède une fixture distincte et une justification liée à libghostty-vt. |
| QG-006 | Lifecycle et ressources | 200 cycles spawn-resize-close consécutifs et un scénario de 32 panes concurrents terminent sans deadlock, double-spawn ni processus orphelin. Après warmup, le nombre de handles et le RSS résiduel reviennent à moins de 5 % du niveau de référence. |
| QG-007 | Performance | Sur le même runner release, le débit médian du corpus Ghostty n’est pas inférieur de plus de 10 % à Alacritty, le P95 de création du host avant init shell reste sous 500 ms, et le binaire release augmente de 15 MiB maximum face au build Alacritty-only. |
| QG-008 | Input et protocoles | La matrice US-010 et US-011 passe à 100 % sur clavier US et au moins un clavier AltGr, avec IME, dead keys, Kitty keyboard, bracketed paste, souris, focus, OSC 52 et hyperlinks. |
| QG-009 | Shells et workflows Paneflow | PowerShell 7, Windows PowerShell 5.1, cmd.exe et Git Bash passent la matrice complète. WSL passe lorsqu’il est installé; son absence est détectée comme skip explicite. Les hooks et agents Paneflow conservent cwd, env, sortie et fermeture. |
| QG-010 | Compatibilité Windows | Le runbook passe sur Windows 10 22H2 x64 et Windows 11 x64, avec resize storms, gros débit, Unicode, Ctrl-C, fermeture de pane, fermeture d’application, veille/reprise et chemins utilisateur non ASCII. |
| QG-011 | Packaging MSI | Installation propre, upgrade depuis la dernière release Alacritty-only, lancement, rollback Alacritty et désinstallation passent sur une VM propre. Aucun fichier Ghostty natif résiduel ou DLL non signé n’est installé. |
| QG-012 | Sécurité et confidentialité | OSC 52 est limité, policy-gated et focus-gated; les URI ne sont jamais exécutées implicitement; les queues et payloads sont bornés; les logs de test et production ne capturent aucun contenu terminal utilisateur. |

## Epics & User Stories

### EP-001: Établir la fondation native MSVC x64

**Goal:** Produire une frontière libghostty-vt Windows statique, hermétique, sûre et testable avant toute intégration UI.

**Definition of Done:** US-001 à US-004 sont DONE, l’artefact canonique et ses métadonnées sont vérifiés, et un smoke headless consomme le wrapper Windows.

#### US-001: Fermer le spike de linkage statique MSVC avec SIMD

**Description:** En tant que mainteneur Paneflow, je veux prouver que la révision Ghostty épinglée produit une ghostty-vt-static.lib x64 MSVC avec SIMD et une fermeture complète de ses dépendances, afin d’éviter de bâtir le port sur une hypothèse native non vérifiée.

**Priority:** P0  
**Size:** L  
**Dependencies:** None

**Acceptance Criteria:**

- [ ] Un build propre produit une archive COFF x64 statique depuis le SHA et la version Zig épinglés, avec les flags et commandes consignés dans la documentation native Paneflow.
- [ ] Les symboles C libghostty-vt attendus sont présents et un exécutable MSVC minimal lie, initialise, parse puis libère un terminal.
- [ ] Les dépendances SIMD et Windows transitives, le modèle CRT et les exigences de linkage sont listés avec leur origine.
- [ ] Deux builds propres utilisant la même recette produisent le même inventaire de symboles et le hash canonique attendu, ou toute source de non-déterminisme est supprimée avant clôture.
- [ ] **Unhappy path:** si le build SIMD échoue, la story capture le symbole ou objet manquant et un reproducer minimal; elle ne valide ni no-SIMD ni DLL comme production par défaut.

#### US-002: Distribuer l’artefact Windows de façon hermétique

**Description:** En tant que contributeur, je veux que le sys crate sélectionne un artefact Windows x64 vérifié depuis le repository, afin qu’un build Paneflow standard n’ait besoin ni de Zig, ni de Ghostty, ni du réseau.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**

- [ ] native/libghostty contient une entrée Windows x64 MSVC avec archive, header, bindings, build-info, SHA source, version Zig, flags et SHA-256 de chaque entrée vérifiée.
- [ ] Le build script sélectionne l’artefact par target triple et émet les directives Cargo de linkage statique et système requises sans modifier le chemin Linux.
- [ ] Une reconstruction CI depuis la source épinglée régénère l’artefact canonique et compare ses métadonnées au manifest.
- [ ] Un checkout standard compile avec l’artefact préconstruit sans accès réseau et sans Ghostty ou Zig installés.
- [ ] **Unhappy path:** archive absente, corrompue, d’une mauvaise architecture ou avec un build-info incohérent provoque un échec immédiat contenant target triple, fichier attendu et action corrective, sans fallback silencieux.

#### US-003: Rendre la frontière FFI sûre sur Windows

**Description:** En tant que développeur backend, je veux activer le sys crate et le wrapper libghostty-vt sur Windows avec les mêmes invariants RAII que Linux, afin qu’aucune différence d’ABI, de heap ou de callback ne traverse la frontière Rust/C.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**

- [ ] Les gates cfg exposent les crates Ghostty sur Linux et Windows uniquement, tandis qu’un build macOS sans feature Windows reste inchangé.
- [ ] Les tailles, alignements, discriminants, signatures et symboles requis sont validés contre le header épinglé sur la cible MSVC.
- [ ] Toute mémoire allouée par libghostty-vt est libérée par l’API Ghostty correspondante; Rust ou le CRT MSVC ne libère jamais directement une allocation Zig.
- [ ] Les handles opaques possèdent un ownership unique, un Drop idempotent et aucune donnée empruntée à durée limitée n’est conservée après le callback.
- [ ] Les callbacks FFI empêchent tout unwind Rust de traverser la frontière native.
- [ ] **Unhappy path:** pointeur nul, enum inconnu, buffer invalide ou initialisation native en échec retourne une erreur structurée ou une absence explicite, sans panic ni accès mémoire.

#### US-004: Ajouter les tests de contrat et le smoke headless Windows

**Description:** En tant que mainteneur, je veux valider libghostty-vt sans GPUI ni ConPTY, afin de séparer les défauts d’artefact et d’ABI des défauts de session ou de rendu.

**Priority:** P0  
**Size:** M  
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**

- [ ] Un test Windows headless crée un terminal, injecte un fixture VT déterministe, produit un snapshot, encode un input puis détruit toutes les ressources.
- [ ] Les tests couvrent création/destruction répétée, resize, palette, Unicode, alternate screen et lecture de snapshot.
- [ ] Un contrôle de dérive compare les bindings et constantes utilisés au header canonique.
- [ ] Les tests Linux existants continuent de passer sans changement de leurs résultats attendus.
- [ ] **Unhappy path:** fixture malformée, taille zéro ou initialisation volontairement refusée ne bloque pas le runner et ne laisse aucun handle ou allocation native vivant.

### EP-002: Porter le host Ghostty sur ConPTY et sécuriser son lifecycle

**Goal:** Réutiliser le worker Ghostty et portable-pty avec une implémentation Windows déterministe pour spawn, I/O, resize, exit et descendants.

**Definition of Done:** Un pane Ghostty Windows exécute un shell réel, termine proprement dans tous les ordres de fermeture et passe les campagnes de stress.

#### US-005: Généraliser la session Ghostty et créer le host ConPTY

**Description:** En tant qu’utilisateur Windows, je veux ouvrir un pane Ghostty qui lance mon shell via ConPTY, afin d’utiliser le backend sans modifier le modèle de pane ou le renderer Paneflow.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**

- [ ] La feature libghostty-windows active les modules et dépendances Ghostty sur x64 MSVC sans compiler de primitive POSIX.
- [ ] GhosttySession utilise portable_pty::native_pty_system, CommandBuilder, le cwd, l’environnement et la sélection de shell déjà définis par Paneflow.
- [ ] Le master possède un reader clonable et un writer unique; l’I/O reste hors du thread GPUI et alimente le moteur Ghostty en bytes.
- [ ] Un succès crée exactement un child et publie le backend Ghostty au modèle de session sans changer le format de session persisté.
- [ ] **Unhappy path:** si artefact, ConPTY ou spawn échoue avant la création du child, la vue peut revenir à Alacritty une seule fois avec un diagnostic sans contenu terminal; après création du child, aucun second backend n’est lancé.

#### US-006: Implémenter l’observation et l’arrêt de processus Windows

**Description:** En tant qu’utilisateur multi-pane, je veux que fermer un pane Ghostty arrête son shell et ses descendants sans affecter les autres panes, afin d’éviter processus orphelins, blocages et perte de contrôle.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**

- [ ] Les opérations host Windows couvrent try_wait, attente bornée, kill, exit status et fermeture idempotente sans getpgid, waitid, kill POSIX ou signal de groupe.
- [ ] L’ordre de shutdown arrête les writes, demande ou force la fin du child, attend ou reap, draine la sortie finale, puis libère PTY et moteur.
- [ ] La gestion d’arbre réutilise ou extrait la politique Windows existante de Paneflow et respecte le Job Object process-wide.
- [ ] Fermer un pane termine ses descendants tandis que les processus des autres panes restent vivants.
- [ ] Kill, wait, EOF et fermeture de fenêtre concurrents convergent vers un seul état terminal.
- [ ] **Unhappy path:** si l’assignation à un Job Object ou l’ouverture d’un process échoue, un fallback borné tente la terminaison disponible, journalise seulement les métadonnées de processus nécessaires et ne bloque pas l’arrêt de Paneflow.

#### US-007: Garantir drain final, backpressure et resize déterministe

**Description:** En tant qu’utilisateur de TUI et de commandes bavardes, je veux que Ghostty conserve toute la sortie et suive les resizes ConPTY, afin d’éviter troncature, corruption Unicode et écran désynchronisé.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-005, US-006

**Acceptance Criteria:**

- [ ] Le reader draine continuellement la sortie hors du thread UI et accepte les séquences UTF-8 ou VT découpées entre plusieurs reads.
- [ ] Le final drain traite tous les bytes disponibles avant de publier l’état exited et de détruire le moteur.
- [ ] Les resizes lignes/colonnes sont coalescés, ordonnés et transmis à ConPTY et libghostty-vt sans reordering visible.
- [ ] Les dimensions pixel ignorées par ConPTY n’altèrent pas les dimensions cellule ou le snapshot Paneflow.
- [ ] Les queues d’input, output et commandes appliquent les caps de NFR-005 et conservent la réactivité du thread GPUI.
- [ ] **Unhappy path:** broken pipe, EOF précoce, resize zéro, resize pendant shutdown ou consumer en retard se termine par un état borné et observable, sans deadlock ni croissance mémoire non bornée.

#### US-008: Couvrir stress multi-pane, crash et ressources Windows

**Description:** En tant que mainteneur, je veux une suite déterministe de stress ConPTY, afin de détecter les races de lifecycle et les fuites avant la qualification produit.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-006, US-007

**Acceptance Criteria:**

- [ ] La suite automatise 200 cycles spawn-resize-output-close avec comptage des children, descendants, handles et mémoire avant et après warmup.
- [ ] Un scénario ouvre 32 panes, injecte des resize storms et du gros débit, puis ferme panes et application dans plusieurs ordres.
- [ ] Les scénarios couvrent shell qui quitte immédiatement, shell bloqué, descendant long-lived, Ctrl-C, crash simulé du worker et fermeture brutale de l’application.
- [ ] Les échecs produisent des métadonnées diagnostiques, timings et compteurs suffisants sans capturer les commandes ou la sortie utilisateur.
- [ ] **Unhappy path:** timeout ou fuite détectée fait échouer la suite, conserve les identifiants de process de test utiles et force un cleanup borné du runner.

### EP-003: Atteindre la parité terminal et workflow Windows

**Goal:** Prouver que le backend Ghostty se comporte comme un terminal Paneflow complet sur le rendu, l’input, le clipboard et les shells Windows.

**Definition of Done:** Le corpus, la matrice input/protocoles et les workflows shell passent sans branche backend dans le renderer GPUI.

#### US-009: Étendre le corpus rendu et interaction à Windows

**Description:** En tant qu’utilisateur, je veux que les contenus complexes s’affichent et se manipulent correctement avec Ghostty, afin que le changement de moteur n’altère pas mon travail.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-004, US-005

**Acceptance Criteria:**

- [ ] Le corpus couvre ASCII, UTF-8 fragmenté, combining marks, emoji, CJK, wide cells, true color, palette, styles, tabs, wrapping, reflow et alternate screen.
- [ ] Les snapshots couvrent cursor, scrollback, viewport, selection, search, hyperlinks et damage après resize.
- [ ] Les chunks différentiels Windows s’exécutent dans le plan CI existant et distinguent divergence Ghostty intentionnelle de régression Paneflow.
- [ ] terminal/element/paint reste backend-neutre et aucune branche Windows/Ghostty n’est ajoutée à la géométrie, aux couleurs ou aux fonts.
- [ ] **Unhappy path:** séquence VT inconnue, OSC surdimensionné, grapheme incomplet ou resize extrême ne panic pas, ne lit pas hors limites et respecte les caps de ressources.

#### US-010: Intégrer le clavier Ghostty, AltGr, dead keys et IME

**Description:** En tant qu’utilisateur d’un clavier international, je veux que raccourcis, texte composé et protocoles clavier soient encodés correctement, afin que Ghostty soit utilisable au quotidien hors clavier US.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**

- [ ] Les événements clavier destinés au terminal Ghostty utilisent une voie d’encodage backend-aware couvrant legacy VT et Kitty keyboard sans contourner les keybindings Paneflow.
- [ ] La matrice couvre lettres, chiffres, fonctions, navigation, Ctrl, Alt, Shift, Ctrl+Alt, AltGr, répétition, NumPad et modes application.
- [ ] Les dead keys et au moins un layout AltGr produisent exactement le texte ou la séquence attendue sans générer de raccourci Ctrl+Alt parasite.
- [ ] IME preedit reste visuel dans GPUI et seul commit écrit l’UTF-8 final une fois dans le PTY.
- [ ] Le focus et la priorité des keybindings Paneflow restent identiques au backend Alacritty.
- [ ] **Unhappy path:** touche non mappable, séquence IME annulée, changement de layout ou encodeur Ghostty refusant un événement n’écrit aucun byte corrompu et ne déclenche pas de panic.

#### US-011: Valider paste, clipboard, souris, focus et liens

**Description:** En tant qu’utilisateur de TUI, je veux que les protocoles d’interaction terminal fonctionnent avec les mêmes politiques Paneflow, afin de conserver sécurité et ergonomie.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-005, US-010

**Acceptance Criteria:**

- [ ] Normal paste et bracketed paste préservent bytes, retours de ligne et Unicode selon les conventions Windows existantes.
- [ ] Les modes souris, wheel, drag, motion et focus report sont encodés par le backend actif sans doubler les événements GPUI.
- [ ] Une écriture OSC 52 n’est acceptée que pour un terminal focalisé, selon la policy Paneflow, et avec un payload décodé maximal de 100 KiB.
- [ ] Les hyperlinks sont exposés comme données et ne déclenchent jamais une ouverture ou exécution sans action utilisateur explicite et validation de protocole.
- [ ] La selection et la copie Paneflow restent fonctionnelles dans main screen, alternate screen et scrollback.
- [ ] **Unhappy path:** base64 invalide, OSC 52 hors focus ou surdimensionné, URI invalide, paste pendant shutdown ou événement souris hors viewport est ignoré ou rejeté proprement sans modifier le clipboard.

#### US-012: Qualifier les shells et workflows Paneflow réels

**Description:** En tant que développeur Windows, je veux utiliser Ghostty avec mes shells, WSL, TUIs et agents habituels, afin que le backend couvre le produit plutôt qu’une démo synthétique.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-007, US-009, US-010, US-011

**Acceptance Criteria:**

- [ ] PowerShell 7, Windows PowerShell 5.1, cmd.exe et Git Bash valident lancement, cwd, environnement, Unicode, couleurs, Ctrl-C, exit code, resize, scrollback et fermeture.
- [ ] WSL valide la même matrice lorsqu’il est installé, y compris chemins Windows/WSL et resize d’une TUI.
- [ ] Les hooks shell, OSC 133, commandes Paneflow, agents et injection de contexte conservent leur comportement et n’écrivent pas deux fois.
- [ ] La restauration de workspace relance un nouveau child avec le backend configuré sans changer le format de session persistant.
- [ ] Les chemins utilisateur contenant espaces et caractères non ASCII fonctionnent pour cwd, config, shell et assets.
- [ ] **Unhappy path:** shell configuré absent, WSL non installé, hook en échec ou agent qui ne termine pas produit un état explicite et récupérable sans second child ni blocage du pane.

### EP-004: Industrialiser CI, sécurité, performance et MSI

**Goal:** Transformer le backend fonctionnel en composant publiable, reproductible et observable dans la release Windows.

**Definition of Done:** La CI reconstruit et qualifie l’intégration, les budgets sont respectés et un MSI propre installe la version statique avec ses notices.

#### US-013: Ajouter la lane CI native et les contrôles supply-chain

**Description:** En tant que mainteneur release, je veux reconstruire et vérifier l’artefact Ghostty Windows séparément de sa consommation Cargo, afin de détecter toute dérive de source, outil ou ABI.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-002, US-004

**Acceptance Criteria:**

- [ ] Une lane Windows x64 reconstruit libghostty-vt depuis le SHA épinglé avec Zig épinglé et publie logs, inventaire de symboles, build-info et hash.
- [ ] Une lane consumer distincte utilise uniquement l’artefact du repository et exécute les quality gates Cargo Windows.
- [ ] Les caches sont clés par SHA Ghostty, version Zig, target triple et flags; aucun cache d’une autre architecture ou configuration n’est accepté.
- [ ] Header, bindings, archive, licences et notices sont vérifiés avant le build Paneflow.
- [ ] Les artefacts de release permettent de retracer chaque binaire au manifest sans inclure de contenu terminal ou secret CI.
- [ ] **Unhappy path:** dérive de hash, binding, licence, symbole, architecture ou toolchain fait échouer la lane avant packaging et interdit toute substitution automatique.

#### US-014: Enforcer corpus, performance et budgets de ressources

**Description:** En tant que mainteneur, je veux comparer Ghostty à la référence Alacritty sur le même runner, afin que la promotion repose sur des seuils et non sur une impression visuelle.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-008, US-009, US-010

**Acceptance Criteria:**

- [ ] Le corpus différentiel, les benchmarks de parsing/snapshot, le temps de création host, la taille binaire et le stress ressources sont exécutés en release sur un runner contrôlé.
- [ ] Les baselines Alacritty et Ghostty utilisent fixtures, taille, runner et nombre d’itérations identiques avec médiane et P95 publiés.
- [ ] Les seuils QG-005, QG-006 et QG-007 sont évalués automatiquement et historisés par commit.
- [ ] Toute variance runner supérieure à la tolérance documentée déclenche un rerun borné unique, puis un échec si elle persiste.
- [ ] **Unhappy path:** régression au-delà d’un budget bloque la promotion; elle ne peut être masquée par une mise à jour de baseline sans justification versionnée et review.

#### US-015: Intégrer le linkage statique au MSI et aux notices

**Description:** En tant qu’utilisateur, je veux installer Paneflow Ghostty avec le MSI normal, afin de ne gérer ni runtime Ghostty séparé, ni DLL, ni chemin système.

**Priority:** P0  
**Size:** L  
**Dependencies:** Blocked by US-013, US-014

**Acceptance Criteria:**

- [ ] Le build release MSI embarque un paneflow.exe x64 lié statiquement à libghostty-vt et n’ajoute aucune ghostty-vt.dll.
- [ ] L’inspection des imports et fichiers installés confirme uniquement les dépendances système et runtime explicitement approuvées.
- [ ] Les licences, notices et éléments SBOM de Ghostty et de ses dépendances natives sont présents dans les artefacts de release.
- [ ] Installation propre, upgrade depuis la dernière release Windows, lancement Ghostty, lancement Alacritty et désinstallation sont automatisés sur VM propre.
- [ ] La signature, le trust de l’updater et le format de package existants ne sont pas modifiés.
- [ ] **Unhappy path:** dépendance DLL Ghostty, architecture incorrecte, fichier résiduel après uninstall ou licence manquante fait échouer le package smoke et bloque la publication.

#### US-016: Exécuter la matrice Windows et publier le runbook de diagnostic

**Description:** En tant que mainteneur, je veux une qualification reproductible sur les Windows supportés et un runbook de rollback, afin de diagnostiquer un échec sans dépendre de la machine d’origine.

**Priority:** P0  
**Size:** M  
**Dependencies:** Blocked by US-012, US-015

**Acceptance Criteria:**

- [ ] Le runbook couvre Windows 10 22H2 x64 et Windows 11 x64, GPU supporté, session distante si disponible, veille/reprise et chemins non ASCII.
- [ ] Chaque shell obligatoire et chaque interaction de QG-008 à QG-011 possède une étape, un attendu et une preuve enregistrable sans contenu utilisateur.
- [ ] Le diagnostic expose backend demandé, backend effectif, phase d’échec, version Ghostty, target et code OS, sans commande, sortie, clipboard ou chemin sensible complet.
- [ ] Le rollback vers Alacritty est documenté, testable en une modification de configuration et ne requiert ni réinstallation ni suppression de données.
- [ ] Les limites connues et skips conditionnels, comme WSL absent, sont distingués des échecs.
- [ ] **Unhappy path:** machine incompatible, ConPTY indisponible, GPU/driver défaillant ou antivirus bloquant produit un diagnostic actionnable et conserve Alacritty comme chemin de récupération.

### EP-005: Qualifier, promouvoir et rendre le rollback explicite

**Goal:** Exposer Ghostty aux utilisateurs avancés, accumuler les preuves de qualification puis basculer le choix auto de Windows sans supprimer Alacritty.

**Definition of Done:** Le backend est configurable, la qualification est complète, auto sélectionne Ghostty sur Windows et le rollback Alacritty reste vérifié.

#### US-017: Ajouter la sélection Windows et le mode de qualification

**Description:** En tant que contributeur Windows, je veux activer explicitement Ghostty et voir le backend effectif, afin de qualifier la feature avant sa promotion générale.

**Priority:** P0  
**Size:** M  
**Dependencies:** Blocked by US-005, US-007

**Acceptance Criteria:**

- [ ] La configuration Windows accepte auto, ghostty et alacritty avec validation, sérialisation et migration backward-compatible.
- [ ] Pendant la qualification, auto conserve Alacritty tandis que ghostty demande le backend Ghostty.
- [ ] Le backend demandé, le backend effectif et un échec pré-spawn sont visibles dans les diagnostics sans contenu terminal.
- [ ] Un fallback Ghostty vers Alacritty ne peut arriver qu’avant child spawn et au plus une fois.
- [ ] Linux conserve sa sélection Ghostty actuelle et macOS conserve son comportement actuel.
- [ ] **Unhappy path:** valeur inconnue, feature native absente, artefact refusé ou échec ConPTY pré-spawn retourne vers Alacritty avec raison structurée; un échec post-spawn ferme la session sans lancer Alacritty.

#### US-018: Promouvoir Ghostty comme backend auto sous Windows

**Description:** En tant qu’utilisateur Windows, je veux que Paneflow choisisse Ghostty automatiquement après qualification, afin de bénéficier du moteur moderne sans configuration manuelle tout en gardant un rollback sûr.

**Priority:** P0  
**Size:** M  
**Dependencies:** Blocked by US-012, US-013, US-014, US-015, US-016, US-017

**Acceptance Criteria:**

- [ ] Tous les quality gates QG-001 à QG-012 possèdent une preuve verte sur le commit candidat.
- [ ] Sur Windows x64 supporté, auto sélectionne Ghostty pour toute nouvelle session; ghostty reste explicite et alacritty force le backend historique.
- [ ] La promotion ne change ni la sélection Linux, ni le comportement macOS, ni le format des workspaces et sessions persistés.
- [ ] La documentation utilisateur, les release notes et le runbook décrivent support Windows, minimum 1809, sélection explicite, limites et rollback.
- [ ] La release candidate MSI passe une dernière installation, upgrade, smoke Ghostty et smoke Alacritty avant publication.
- [ ] **Unhappy path:** si un gate régresse, si l’artefact natif n’est pas vérifiable ou si le smoke MSI échoue, auto reste ou revient à Alacritty avant publication; aucune exception manuelle non versionnée ne permet la promotion.

## Functional Requirements

| ID | Requirement | Stories |
|---|---|---|
| FR-001 | Paneflow doit consommer une libghostty-vt statique x64 MSVC issue du SHA épinglé et vérifiée par manifest. | US-001, US-002, US-013 |
| FR-002 | Le sys crate et le wrapper doivent exposer la même API sûre sur Linux et Windows sans allocation traversant le mauvais heap. | US-003, US-004 |
| FR-003 | Un build Windows standard doit fonctionner sans réseau, Zig ou checkout Ghostty. | US-002, US-013 |
| FR-004 | GhosttySession doit utiliser portable-pty et ConPTY avec un seul child par session. | US-005 |
| FR-005 | Spawn, exit, kill, wait, drain et fermeture doivent être ordonnés et idempotents sur Windows. | US-006, US-007 |
| FR-006 | Fermer un pane ou Paneflow doit terminer les descendants concernés sans affecter les autres panes. | US-006, US-008 |
| FR-007 | Output fragmenté, Unicode, resize et final drain doivent produire un snapshot complet et cohérent. | US-007, US-009 |
| FR-008 | Le renderer GPUI doit rester backend-neutre et consommer Content sans branche Windows spécifique. | US-009 |
| FR-009 | Clavier, AltGr, dead keys, IME et Kitty keyboard doivent suivre une voie backend-aware et respecter les keybindings Paneflow. | US-010 |
| FR-010 | Paste, mouse, focus, clipboard OSC 52, selection et hyperlinks doivent conserver les policies Paneflow. | US-011 |
| FR-011 | PowerShell 7, Windows PowerShell 5.1, cmd.exe, Git Bash et WSL lorsqu’installé doivent couvrir la matrice de workflow. | US-012 |
| FR-012 | La CI doit reconstruire la source épinglée, vérifier supply-chain, ABI, corpus, ressources et performance. | US-013, US-014 |
| FR-013 | Le MSI doit distribuer Paneflow sans ghostty-vt.dll et inclure licences, notices et SBOM. | US-015 |
| FR-014 | Les diagnostics doivent identifier backend et phase d’échec sans journaliser le contenu terminal. | US-008, US-016, US-017 |
| FR-015 | La configuration doit supporter auto, ghostty et alacritty; le fallback automatique doit rester pré-spawn. | US-017, US-018 |
| FR-016 | La promotion de auto vers Ghostty doit être conditionnée à tous les gates et conserver un rollback Alacritty. | US-018 |

## Non-Functional Requirements

| ID | Category | Requirement | Measurement |
|---|---|---|---|
| NFR-001 | Compatibility | Supporter Windows 10 1809+ et Windows 11 x64 avec target x86_64-pc-windows-msvc. | Build CI plus runbook Windows 10 22H2 et Windows 11. |
| NFR-002 | Performance | Le débit médian Ghostty ne doit pas être inférieur de plus de 10 % à Alacritty sur le corpus release identique. | Benchmark comparatif, minimum 20 itérations après warmup. |
| NFR-003 | Startup | Le P95 de création du host Ghostty avant initialisation du shell doit rester inférieur à 500 ms sur le runner de référence. | 100 créations séquentielles en release. |
| NFR-004 | Binary size | Le binaire Paneflow release Ghostty ne doit pas dépasser de plus de 15 MiB le build Alacritty-only équivalent. | Taille du même commit et même target après packaging comparable. |
| NFR-005 | Memory bounds | Chaque queue runtime est bornée; output en attente maximal 8 MiB, input en attente maximal 1 MiB, OSC 52 décodé maximal 100 KiB. | Tests de saturation et assertions de configuration. |
| NFR-006 | Lifecycle reliability | 200 cycles et 32 panes doivent produire zéro deadlock, double-spawn ou processus orphelin. | Suite US-008 avec timeout global et inventaire de processes. |
| NFR-007 | Resource recovery | Après warmup et cleanup, handles et RSS résiduels doivent revenir à moins de 5 % de la baseline. | Compteurs avant/après campagne QG-006. |
| NFR-008 | UI responsiveness | Aucun read, wait ou kill bloquant ne s’exécute sur le thread GPUI; une rafale output/resize ne doit pas bloquer une frame plus de 16,7 ms au P95 sur le runner de référence. | Instrumentation test et trace de frame pendant stress. |
| NFR-009 | Supply-chain | 100 % des artefacts natifs, headers, bindings et build-info doivent être hashés et reliés au SHA source et à la version Zig. | Vérification manifest en CI. |
| NFR-010 | Privacy | Zéro byte de commande, output, clipboard ou séquence OSC utilisateur dans les logs et événements de télémétrie de production. | Tests de redaction avec canaries et audit des champs émis. |
| NFR-011 | Security | Aucun chargement ghostty-vt.dll; OSC 52 focus-gated et policy-gated; aucune URI terminal exécutée implicitement. | Inspection imports, tests protocoles et security review ciblée. |
| NFR-012 | Regression safety | Les suites Linux existantes et le build macOS sans feature Windows doivent rester inchangés et verts. | Matrice CI multi-plateforme du commit candidat. |

## Edge Cases & Error States

| Case | Expected behavior | Coverage |
|---|---|---|
| Artefact absent, hash invalide ou mauvaise architecture | Échec de build immédiat et actionnable, aucune substitution. | US-002, US-013 |
| Incompatibilité ABI ou CRT | Smoke natif et contrat FFI échouent avant intégration runtime. | US-001, US-003, US-004 |
| ConPTY indisponible ou CreatePseudoConsole échoue | Fallback Alacritty unique si aucun child n’existe, sinon session en erreur. | US-005, US-017 |
| Shell quitte avant que la vue soit prête | Exit et final drain sont publiés une fois, sans hang ni fallback post-spawn. | US-006, US-007 |
| Shell lance des descendants long-lived | Fermeture du pane termine l’arbre concerné selon la politique Windows. | US-006, US-008 |
| Job Object parent interdit une assignation | Fallback de terminaison borné et diagnostic, sans bloquer Paneflow. | US-006 |
| UTF-8 ou VT découpé entre reads | Les bytes sont conservés dans l’ordre et parsés sans remplacement prématuré. | US-007, US-009 |
| Resize zéro, storm ou resize pendant shutdown | Coalescing, clamp et arrêt idempotent, sans appel invalide ni deadlock. | US-007, US-008 |
| Output dépasse la capacité du consumer | Backpressure bornée, thread UI réactif, erreur explicite si la policy de saturation s’active. | US-007 |
| IME annulé, dead key ou changement de layout | Aucun byte partiel ou raccourci parasite n’est envoyé. | US-010 |
| OSC 52 invalide, hors focus ou trop grand | Rejet sans changement du clipboard et sans log du payload. | US-011 |
| URI malformée ou protocole non approuvé | Affichage possible comme texte, jamais d’exécution implicite. | US-011 |
| Shell optionnel absent, dont WSL | Skip ou diagnostic explicite; le shell par défaut reste utilisable. | US-012 |
| Paneflow crash ou fermeture forcée | Le Job Object process-wide nettoie les descendants; le prochain lancement reste sain. | US-006, US-008 |
| Upgrade depuis une release sans Ghostty Windows | Config migrée sans perte, auto suit la règle de la nouvelle version, alacritty reste sélectionnable. | US-015, US-017, US-018 |
| Gate régresse après qualification | Promotion bloquée ou auto rebasculé avant publication; aucune release partielle. | US-014, US-018 |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation | Trigger / owner |
|---|---|---|---|---|
| Les dépendances SIMD ne ferment pas dans ghostty-vt-static.lib sous MSVC. | Medium | Critical | Spike US-001 avant toute dépendance runtime; reproducer minimal; escalade upstream; no-SIMD uniquement comme diagnostic. | Premier symbole non résolu, owner EP-001. |
| Une allocation Zig est libérée par le CRT MSVC. | Low | Critical | Wrapper RAII, ghostty_free exclusif, tests allocate/free répétés et audit ABI. | Crash heap ou sanitizer, owner US-003. |
| ConPTY deadlock pendant shutdown ou output non drainé. | Medium | Critical | I/O hors UI, ordre de fermeture explicite, timeouts, final drain, stress multi-pane. | Timeout US-008, owner EP-002. |
| Des descendants survivent à la fermeture d’un pane. | Medium | High | Réutiliser la politique process-tree, intégrer Job Object et tester children/grandchildren. | Process encore vivant après cleanup, owner US-006. |
| AltGr, dead keys ou IME sont interprétés comme raccourcis. | Medium | High | Matrice layout réelle, séparation preedit/commit, encodeur backend-aware et tests bytes exacts. | Divergence US-010, owner EP-003. |
| Les snapshots Ghostty divergent et poussent à modifier le renderer. | Medium | High | Corpus avant UI, adaptateur Content, fixtures distinctes seulement pour différences justifiées. | Branche backend détectée dans paint, owner US-009. |
| Le binaire ou le runtime régresse fortement. | Medium | Medium | Budgets automatiques taille, parsing, startup, frames, handles et RSS. | Seuil QG-006/QG-007 dépassé, owner US-014. |
| Une DLL apparaît comme contournement rapide. | Low | High | Gate statique et inspection MSI; toute DLL exige une décision de scope séparée et une security review. | ghostty-vt.dll dans imports/package, owner US-015. |
| Le SHA Ghostty ou le C API change avant release. | Medium | Medium | Pin strict, drift checks, pas d’upgrade dans ce PRD. | Manifest modifié, owner US-013. |
| Le scope ARM64 retarde x64. | Medium | Medium | ARM64 hors v1, chemins et manifest target-aware, PRD séparé après promotion x64. | Demande d’artefact ARM64, owner produit. |
| La promotion masque des problèmes rares de machine réelle. | Medium | High | Phase explicite, Alacritty maintenu, diagnostic privacy-safe, rollback en une config. | Régression release candidate, owner US-018. |

## Non-Goals

1. Porter l’application Ghostling, Raylib ou son host POSIX sur Windows.
2. Remplacer GPUI, le renderer Paneflow, le système de panes ou la persistence de workspace.
3. Construire le terminal GUI Ghostty officiel ou reproduire sa configuration complète.
4. Supprimer Alacritty du binaire ou du codebase Windows.
5. Changer le backend d’une session après le spawn de son child.
6. Supporter Windows 7, Windows 8.1 ou Windows 10 antérieur à 1809.
7. Livrer Windows ARM64 dans cette version.
8. Mettre à jour le SHA Ghostty, Zig, GPUI ou Alacritty sans nécessité directe pour ce PRD.
9. Introduire une ghostty-vt.dll de production tant que le spike statique n’a pas démontré un blocage irréductible.
10. Modifier le modèle de signature, le trust de l’updater ou le format MSI au-delà des métadonnées et notices nécessaires.
11. Ajouter de la télémétrie de contenu terminal, commandes, clipboard ou OSC.
12. Garantir le support d’un shell tiers non présent dans la matrice, tout en conservant un comportement générique via ConPTY.

## Files NOT to Modify

| Path | Protection |
|---|---|
| src-app/src/terminal/element/paint/** | Ne pas ajouter de branche Ghostty ou Windows au renderer, aux fonts, aux couleurs ou à la géométrie. |
| src-app/src/terminal/element/golden/** | Ne pas réécrire en masse les goldens pour masquer une divergence. Ajouter une fixture ciblée et justifiée si nécessaire. |
| src-app/src/app/session.rs | Ne pas changer le format persistant pour représenter un handle ou un type Ghostty concret. |
| src-app/src/update/signature.rs et src-app/src/update/verified_download.rs | Ne pas modifier le trust, les clés ou le protocole updater pour distribuer le backend. |
| native/libghostty/prebuilt/*-unknown-linux-gnu/** | Ne pas remplacer ou régénérer les artefacts Linux pendant l’ajout Windows. |
| Entrées GPUI de Cargo.toml et src-app/Cargo.toml | Ne pas remplacer les dépendances GPUI locales ou leur révision dans ce projet. |
| C:\dev\ghostling\** | Référence externe read-only; aucun port ou patch Ghostling n’est requis. |
| C:\dev\ghostty\** | Checkout source de référence read-only; toute correction upstream éventuelle suit un travail séparé. |

## Technical Considerations

1. **Frontière host:** recommander une petite abstraction interne pour child observation, process-tree termination et shutdown, plutôt que disperser cfg(target_os = "windows") dans le worker. La meilleure profondeur de module doit être décidée pendant US-006 à partir des helpers Windows déjà présents.
2. **Features Cargo:** recommander libghostty-windows comme feature explicite et target-specific, en conservant libghostty-linux. Une généralisation future en libghostty-native peut être évaluée seulement si elle réduit réellement les cfg sans masquer les artefacts par cible.
3. **Linkage:** vérifier avec les outils MSVC si les dépendances SIMD doivent être fusionnées dans l’archive canonique ou liées séparément. Le consommateur Cargo devrait recevoir une liste stable issue du manifest plutôt qu’une détection implicite.
4. **Job Objects:** déterminer si un Job Object par session est fiable sous les jobs parents de CI. Si l’assignation imbriquée est refusée, recommander la politique process-tree existante pour la fermeture de pane et le Job Object process-wide pour le crash de l’application.
5. **Input:** évaluer une voie unique transformant un événement GPUI en key event Ghostty pour le backend Ghostty, tout en gardant IME commit comme bytes UTF-8 et les keybindings Paneflow en priorité.
6. **Snapshots:** recommander de comparer les modèles Content plutôt que les pixels pour le corpus automatisé. Les smoke visuels doivent seulement vérifier l’intégration GPUI et le packaging.
7. **Backpressure:** décider si saturation output doit suspendre la lecture, compacter les damage events ou fermer la session. La solution retenue doit rester bornée et ne jamais bloquer GPUI ou ConPTY.
8. **Artefact canonique:** recommander de stocker target triple, SHA source, Zig, flags, CRT, SIMD, dépendances système et hashes dans une seule entrée manifest validée par build.rs et CI.
9. **DLL contingency:** si US-001 démontre que le statique est impossible, ouvrir une décision technique séparée avant implémentation DLL. Cette décision devra couvrir chemin de chargement absolu, signature, WiX, updater, SBOM et rollback.
10. **Observabilité:** recommander des erreurs structurées avec phase, backend, code OS et version native. Les chemins utilisateur doivent être réduits ou hashés et tout contenu PTY doit rester absent.
11. **ARM64:** conserver les sélections et chemins indexés par target triple dès maintenant, mais ne créer aucun faux artefact ou gate vert ARM64 sans runner et bibliothèque correspondants.

## Success Metrics

| Metric | Baseline | Target | Timeframe | Measurement |
|---|---|---|---|---|
| Sessions Windows auto utilisant Ghostty | 0 % | 100 % des nouvelles sessions sur Windows x64 supporté après promotion | Release contenant US-018 | Test d’intégration de sélection backend et diagnostic backend effectif. |
| Quality gates Windows Ghostty | 0 sur 12 | 12 sur 12 verts sur le commit candidat | Avant release candidate | Statuts CI et runbook signés par commit. |
| Divergences corpus inexpliquées | Non mesuré sur Windows Ghostty | 0 | Avant US-018 | Rapport de corpus différentiel par chunk. |
| Fiabilité lifecycle | Aucun scénario Ghostty Windows | 200 cycles et 32 panes, zéro deadlock, double-spawn ou orphan | Avant MSI release candidate | Suite US-008 avec inventaire process/handles. |
| Overhead parsing | Non mesuré | Régression médiane inférieure ou égale à 10 % face à Alacritty | Avant US-018 et à chaque upgrade Ghostty | Benchmark release sur runner contrôlé. |
| Overhead binaire | 0 MiB pour Ghostty Windows | Inférieur ou égal à 15 MiB | Avant packaging final | Comparaison des binaires du même commit. |
| Dépendance Ghostty dynamique | Aucune, backend absent | 0 ghostty-vt.dll importée ou installée | Chaque MSI | Inspection imports et inventaire WiX/install. |
| Confidentialité diagnostic | Logs backend Windows inexistants | 0 canary terminal/clipboard détectée | Avant promotion et en CI continue | Tests de redaction avec canaries. |
| Rollback utilisateur | Alacritty seul | Retour à Alacritty en une modification de configuration, sans réinstallation | Release candidate | Étape automatisée du smoke MSI. |
| Régressions critiques confirmées | Baseline à établir pendant qualification | 0 issue P0 non corrigée et au plus 2 issues P1 confirmées | 30 jours après release | Issues GitHub labelisées Windows/Ghostty, crash telemetry opt-in sans contenu terminal. |
| Stabilité après adoption | Baseline à établir sur la release candidate | Taux de sessions Ghostty sans crash supérieur ou égal à 99,9 % | 6 mois après promotion | Télémétrie opt-in limitée au backend, version et code d’arrêt, jamais au contenu. |

## Open Questions

| Question | Current decision | Owner | Deadline / dependency |
|---|---|---|---|
| Le build statique SIMD ferme-t-il toutes ses dépendances sous MSVC sans archive auxiliaire fragile? | La cible reste statique SIMD; aucune alternative n’est promue par défaut. | EP-001 | Résolution obligatoire dans US-001 avant US-002. |
| Un Job Object par pane fonctionne-t-il dans les environnements CI déjà placés dans un job parent? | Tester l’assignation imbriquée; conserver process-tree termination et Job Object process-wide comme stratégie de repli. | US-006 | Avant validation du lifecycle. |
| Quel mécanisme de backpressure préserve le mieux ConPTY sans perte silencieuse? | Queue bornée obligatoire; politique exacte mesurée entre suspension, batching et erreur contrôlée. | US-007 | Avant US-008. |
| Une différence de snapshot Ghostty doit-elle devenir une fixture spécifique? | Seulement si elle est conforme au protocole, intentionnelle et documentée; le renderer ne change pas pour égaliser artificiellement. | US-009 | Pendant chaque chunk corpus. |
| Quand ouvrir la cible Windows ARM64? | Après promotion x64 et disponibilité d’un artefact natif et d’un runner vérifiable. | Product / release | PRD séparé, ne bloque pas US-018. |

[/PRD]
