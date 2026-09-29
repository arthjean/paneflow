[PRD]
# PRD: Correctifs issus de l'audit du fork theaamgroup/PaneFlow

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-28 | Arthur Jean | PRD initial : correction des défauts confirmés dans Paneflow par l'audit du fork, regroupés par cause racine en 6 epics et 47 stories, livraisons R0 à R4. |
| 1.1 | 2026-09-29 | Arthur Jean | Le critère d'undo-close d'US-023 devient US-048 : libghostty n'émet pas d'OSC 8 en sortie VT et la replay n'atteignait plus l'écran ; l'undo-close rattache désormais la session gardée 5 s. US-040 dépend d'US-048. 48 stories. |
| 1.2 | 2026-09-29 | Arthur Jean | Le critère de reproduction manuelle d'US-016 devient une reproduction à l'exécution sur un vrai PTY : le test automatisé a reproduit l'état exited sous Linux avant correctif, et le rendu GPUI du libellé n'ajoute rien à la cause. |
| 1.3 | 2026-09-29 | Arthur Jean | Les critères chiffrés de performance d'US-024 (frames de 50 ms) et d'US-026 (première frame à 250 ms au-dessus de la baseline) deviennent des preuves structurelles déjà testées : aucune requête runtime ni sonde de chemin sur le thread GPUI, et abandon au délai d'un chemin qui bloque. Les deux mesures passent en vérification facultative à la qualification d'une release, sans bloquer les stories. |

## Problem Statement

Le fork privé theaamgroup/PaneFlow a fait tourner des agents d'audit sur son code pendant un mois : 708 issues, 352 PR et 937 commits depuis la v0.8.2. Son code descend en grande partie de celui de Paneflow. Le 2026-09-28, dix sous-agents ont confronté chaque issue et chaque commit de fix à Paneflow `main` @ `8c3dd2ca` (v0.17.4), par lecture de code uniquement. Environ 330 éléments du fork désignent un défaut encore présent, soit environ 210 défauts distincts une fois les doublons retirés. Une trentaine de ces affirmations a été revérifiée ligne à ligne.

1. **Perte de données silencieuse.** Sept scénarios détruisent du travail sans avertir :
   - « Remove worktree » dans le menu d'onglet supprime un checkout sous des agents vivants, en un clic ([tab_worktree.rs:820](src-app/src/app/tab_worktree.rs)).
   - `is_clean` croit propre un worktree qui contient des fichiers non suivis quand `status.showUntrackedFiles=no`, puis `worktree remove --force` le supprime.
   - Fermer un onglet ou un workspace, ou quitter, jette les buffers non sauvegardés du dock.
   - Ctrl+S sous le bandeau de conflit écrase l'écriture d'un agent.
   - Le Revert de l'onglet Changes greffe un hunk dans un contenu plus récent.
   - Plusieurs écrivains remplacent des symlinks par des fichiers, dont `CLAUDE.md -> AGENTS.md`.
2. **Code exécuté sans action de l'utilisateur, et verrous volés.** Aucun des 15 sites `Command::new("git")` n'isole la config du dépôt. Or les sondes tournent à chaque changement de cwd (OSC 7) et toutes les 30 s : un `core.fsmonitor` local à un dépôt s'exécute donc sans action de l'utilisateur. Ces mêmes sondes réécrivent `.git/index`, si bien que les `git commit` des agents lancés en parallèle échouent sur `index.lock`. C'est le cas d'usage central de Paneflow.
3. **Thread de rendu bloqué.** C'est une violation directe de AGENTS.md. `surface.read`, `surface.search` et `workspace.current` construisent tout le scrollback sur le thread GPUI, avec jusqu'à 1 s par requête, alors que `paneflow wait` interroge toutes les 500 ms. La résolution de chemins au survol, l'import du PATH du login shell et l'ouverture d'URL sous Linux bloquent aussi.
4. **Terminal qui ment sur son état.** Plus de 256 BEL dans un même chunk font afficher « exited -1 » sur un shell vivant et purgent ses sessions agent ; un `cat` de binaire suffit. Le host répond aux requêtes de couleur avec un thème sombre codé en dur. « Reset terminal » envoie `ESC c` comme une frappe, ce qui interrompt Claude ou Codex.
5. **Intégration agents cassée ou trop permissive.** L'entrée MCP écrite pour Codex ne transmet aucune variable `PANEFLOW_*`. La portée MCP échoue en mode ouvert côté host. `CLAUDE_CONFIG_DIR` n'est respecté que pour les hooks. Un Paneflow lancé depuis un pane peut faire boucler ses shims.

**Why now:** Ces défauts ont été confirmés dans le code en une seule passe, avec pour chacun le fichier, la ligne et souvent un correctif de référence dans le fork. Les corriger maintenant coûte une lecture ; les corriger plus tard coûtera un signalement utilisateur, souvent après une perte de données. La v0.17 a déplacé le PTY dans `paneflow-host` et les hooks vers une installation globale : plusieurs défauts (couleurs du host, scope MCP côté host, Cmd+K) sont nés de cette architecture récente et ne feront que s'étendre.

## Overview

Le PRD corrige les défauts par cause racine plutôt qu'un par un. Chaque story commence par un test qui échoue sur `8c3dd2ca` ; un défaut que ce test ne reproduit pas est annulé avec la preuve, sans correctif spéculatif. Trois défauts à fort impact mais confirmés seulement par lecture reçoivent une story de reproduction dédiée avant leur correctif : le bridge MCP sous Codex, la boucle des shims en Paneflow imbriqué, et le pane déclaré mort par un flot de BEL.

Quatre abstractions partagées portent l'essentiel des correctifs :
- **Un builder git unique à deux profils.** `Probe` sert aux lectures automatiques : config isolée, verrous optionnels désactivés, locale C. `UserAction` sert aux opérations demandées par l'utilisateur et garde hooks et filtres, pour ne pas casser Git LFS.
- **Un écrivain atomique partagé qui écrit à travers les symlinks** sans jamais les remplacer.
- **Une machine d'état de synchronisation disque de l'éditeur** qui distingue la version observée sur disque de celle à laquelle le buffer correspond.
- **Un mécanisme de réponse IPC différée** qui sort les requêtes runtime du thread GPUI.

Les nouvelles commandes envoyées au host (apparence, clear, reset) doivent se dégrader proprement face à un host plus ancien, puisque les hosts survivent aux mises à jour de l'app.

Cinq livraisons :

| Livraison | Contenu | Stories | Condition de passage |
|-----------|---------|---------|----------------------|
| R0 : reproductions | Trois défauts à confirmer à l'exécution | 3 | Preuve consignée pour chacun ; un défaut non reproduit reclasse sa story de correctif |
| R1 : intégrité et sécurité | Toutes les stories P0 hors reproduction | 13 | Aucun scénario de perte de données reproductible ; builder git et écrivain symlink-safe en place |
| R2 : robustesse du cœur | Stories P1 des EP-001 à EP-004 | 17 | Aucune requête runtime, I/O ou sous-processus bloquant sur le thread GPUI dans les chemins listés |
| R3 : agents, UI et outillage | Stories P1 des EP-005 et EP-006 | 11 | Intégrations agents et accessibilité conformes aux critères |
| R4 : finitions | Stories P2 | 3 | Résultats git exacts, sémantique IPC secondaire, tests et bench fiables |

L'état et les preuves de l'audit sont conservés localement dans `tasks/fork-audit/`. Ce dossier n'est pas suivi par git et n'existe que sur la machine d'Arthur : un autre agent ne peut pas le lire. Chaque story cite donc ses sources directement : fichiers upstream et références du fork (numéro d'issue ou sha dans `theaamgroup/PaneFlow`).

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|----------------|----------------|
| Éliminer la perte de données silencieuse | 7/7 scénarios corrigés, chacun couvert par un test de régression | 0 nouveau signalement de perte de données |
| Sécuriser les sondes automatiques | 100 % des spawns git de production passent par le builder ; 0 exécution de `core.fsmonitor` local dans les tests | 0 régression Git LFS ou hooks signalée |
| Libérer le thread de rendu | 0 frame > 50 ms due à un handler IPC dans le scénario de US-024 | 100 % des chemins de l'EP-004 hors du thread GPUI |
| Rétablir l'intégration agents | Bridge MCP fonctionnel sous Codex sur Linux, macOS et Windows | 0 clé utilisateur perdue par une réinstallation MCP |
| Clore l'audit | R0 et R1 DONE (16 stories) | ≥ 45/48 stories DONE |

## Target Users

### Développeur qui fait travailler plusieurs agents en parallèle
- **Role:** Utilisateur principal de Paneflow. Il lance Claude Code, Codex, Gemini ou OpenCode dans des panes et des worktrees séparés, et relit leur travail dans le dock.
- **Behaviors:** Il garde de nombreux onglets ouverts, bascule entre workspaces, édite des fichiers pendant qu'un agent les réécrit, et ferme des onglets par raccourci.
- **Pain points:** Il perd des éditions non sauvegardées, voit disparaître des fichiers non suivis au démontage d'un worktree, subit des `git commit` d'agents qui échouent sur `index.lock` et des panes marqués terminés alors qu'ils tournent. L'interface se fige quand un script interroge Paneflow.
- **Current workaround:** Il enregistre souvent, évite de fermer depuis le menu d'onglet, relance les commits échoués et redémarre le pane.
- **Success looks like:** Aucune action de fermeture, de revert ou de suppression ne détruit de travail sans le lui dire, et ses agents n'échouent plus à cause de Paneflow.

### Orchestrateur scripté
- **Role:** Un script, un autre agent ou le skill `paneflow-conductor` qui pilote Paneflow par la CLI `paneflow`, l'IPC ou le bridge MCP.
- **Behaviors:** Il interroge les panes par `paneflow wait`, `surface.read` et `surface.search`, crée des workspaces et des splits, et envoie du texte.
- **Pain points:** Un id mal typé cible le pane actif ; un index mal typé ferme le workspace actif ; un timeout renvoie un texte vide présenté comme vrai ; `flow` met plusieurs unités dans le même checkout ; le bridge MCP ne démarre pas sous Codex.
- **Current workaround:** Il valide ses paramètres lui-même, relance les commandes et évite Codex pour l'orchestration.
- **Success looks like:** Toute entrée invalide est rejetée en -32602, toute erreur est une erreur, et le bridge fonctionne sous chaque agent supporté.

### Mainteneur de Paneflow
- **Role:** Arthur : release, CI, benchmarks, développement de Paneflow depuis un pane Paneflow.
- **Behaviors:** Il lance `scripts/dev.sh` ou `dev.ps1` depuis un pane, publie par tag, mesure avec les scripts de bench et vérifie Windows sur une machine réelle.
- **Pain points:** Il risque une boucle de shims en instance imbriquée ; les actions de release ne sont pas épinglées ; les benchs macOS sont inutilisables ; des tests écrivent dans les vrais dossiers helpers ; les erreurs de config sont muettes sur macOS et Windows.
- **Current workaround:** Il surveille les runs manuellement et relance les jobs.
- **Success looks like:** Une CI épinglée, des benchs exploitables sur les trois OS, et des logs qui expliquent les erreurs de config.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **VS Code :** son extension git pose `GIT_OPTIONAL_LOCKS=0` sur chaque processus git ([git.ts](https://github.com/microsoft/vscode/blob/main/extensions/git/src/git.ts)). Workspace Trust désactive git dans une fenêtre non fiable. Save est bloqué quand le fichier est plus récent sur disque (Compare, Overwrite ou Revert), et le hot-exit préserve les buffers sales ([docs](https://code.visualstudio.com/docs/editing/codebasics#_hot-exit)).
- **Zed :** demande confirmation au quit s'il reste des buffers sales, et signale un conflit disque avant d'écraser.
- **lazygit et GitHub Desktop :** interrogent git avec `--no-optional-locks` ou `GIT_OPTIONAL_LOCKS=0`.
- **Ghostty, WezTerm, kitty :** répondent à OSC 10/11 avec les couleurs courantes et implémentent le rapport de schéma `CSI ? 996 n` / `997` avec le mode 2031 ([extension contour](https://contour-terminal.org/vt-extensions/color-palette-update-notifications/)). kitty fusionne les notifications qui partagent un même id.
- **Copilot CLI :** avis [GHSA-9ccr-r5hg-74gf](https://github.com/github/copilot-cli/security/advisories/GHSA-9ccr-r5hg-74gf), où un dépôt bare imbriqué déclenche `core.fsmonitor`. La même classe de défaut touche plusieurs CLI d'agents.
- **Market gap :** un terminal pensé pour les agents doit laisser ses sondes git cohabiter avec les écritures git des agents, et ne jamais détruire le travail d'un agent au nom du confort de l'humain. Aucun des produits cités ne cible ce couple.

### Best Practices Applied
- Lectures git d'arrière-plan : `git --no-optional-locks` ou `GIT_OPTIONAL_LOCKS=0` pour `status` ([git-status, BACKGROUND REFRESH](https://git-scm.com/docs/git-status)), et `diff.autoRefreshIndex=false` pour `diff` ([git-config](https://git-scm.com/docs/git-config#Documentation/git-config.txt-diffautoRefreshIndex)).
- Git sur un dossier non fiable : neutraliser `core.fsmonitor`, `core.hooksPath`, `diff.external`, textconv, et poser `safe.bareRepository=explicit` ([CVE-2022-24765](https://nvd.nist.gov/vuln/detail/CVE-2022-24765), [analyse des dépôts bare enfouis](https://github.com/justinsteven/advisories/blob/main/2022_git_buried_bare_repos_and_fsmonitor_various_abuses.md)). Sous Windows il n'y a pas de `/dev/null` : un dossier de hooks vide est la seule valeur portable.
- Écriture atomique qui préserve les symlinks : résoudre la cible, créer le fichier temporaire dans le dossier de la cible, `fsync`, puis renommer sur la cible. `rename` et `NamedTempFile::persist` remplacent le lien lui-même ([tempfile](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist)).
- Codex ne transmet à un serveur MCP stdio qu'une liste fixe de variables (`DEFAULT_ENV_VARS` dans codex-rs), plus celles nommées dans `env_vars` ([doc MCP Codex](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)).
- MCP 2025-06-18 réserve `params._meta` sur toute requête ([spec](https://modelcontextprotocol.io/specification/2025-06-18/basic/index)).
- ureq 3 : `timeout_global` couvre tout le transfert et il n'existe pas de délai d'inactivité ; un gros téléchargement a besoin de délais de connexion et de réponse plus un délai de stagnation implémenté autour du lecteur.
- `open` 5.4.2 : `that` attend la fin du lanceur, `that_detached` non (sous macOS, `that_detached` appelle `that`, ce qui ne bloque pas).

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- Codex ne transmet à un serveur MCP que `DEFAULT_ENV_VARS` et les noms listés dans `env_vars`. C'est vérifié dans la source codex-rs, pas à l'exécution ; US-032 le valide.
- `safe.bareRepository=explicit` ne casse pas les worktrees liés à un dépôt bare ; un critère de US-008 le valide.
- Neutraliser `core.fsmonitor` dans les sondes ne ralentit pas `git status` de plus de 2x sur un dépôt de 100 000 fichiers ; US-008 le mesure.
- Une fois l'apparence fournie au host, libghostty répond lui-même à OSC 10/11/12 et à `CSI ? 996 n` (le test `constructor.rs:334-385` prouve OSC 10/11) ; US-018 le valide.
- Les affirmations du fork ne sont confirmées que par lecture statique ; chaque story les valide par un test rouge avant correction.
- Un host lancé par une version antérieure de l'app peut rester attaché après une mise à jour ; les nouvelles commandes host doivent le tolérer.

### Hard Constraints
- Linux (Wayland et X11, x86_64 et aarch64), macOS (aarch64) et Windows 10/11 (x86_64) : chaque story fonctionne sur les trois OS, ou fournit un repli documenté. Le comportement Windows est vérifié nativement sur la machine d'Arthur ; une plateforme non vérifiée est déclarée comme telle.
- Règles de AGENTS.md :
  - aucun commentaire dans le code source ;
  - le thread de rendu ne bloque jamais ;
  - tout item déclaré avant un `mod tests` existant ;
  - `--locked` partout ;
  - `cargo fmt --check` avant chaque commit.
- Aucune nouvelle dépendance dans `paneflow-shim`, `paneflow-ai-hook` et `paneflow-mcp`, dont les tailles restent sous leurs plafonds (512 KiB, environ 375 KB, 512 KiB).
- libghostty reste le seul moteur ; les types moteur ne sortent pas de la frontière `TerminalSessionBackend`.
- Code, messages et textes d'interface en anglais US.
- `docs/user/` est un miroir généré depuis paneflow-web : les changements de documentation utilisateur se font dans paneflow-web.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage, obligatoire avant chaque commit et push
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lint, cibles de test incluses
- `cargo test --workspace --locked` - suite de tests complète
- `cargo deny check advisories licenses sources` - uniquement quand une dépendance change

Pour une story qui touche du code `#[cfg(windows)]` : `cargo test --workspace --no-fail-fast` exécuté nativement sous Windows, et ordre des items relu par rapport à tout `mod tests`.

Pour une story UI : vérification visuelle manuelle par Arthur sur un build debug. L'agent livre le changement sans lancer l'app pour la vérifier.

Pour une story qui revendique un gain de performance : mesure par `scripts/bench-terminal`, `scripts/bench-editor` ou `scripts/bench-startup`, comparée aux baselines de `bench/`.

## Epics & User Stories

| Livraison | Stories |
|-----------|---------|
| R0 | US-016, US-032, US-034 |
| R1 | US-001, US-002, US-003, US-004, US-006, US-008, US-009, US-012, US-017, US-018, US-024, US-033, US-035 |
| R2 | US-005, US-007, US-011, US-013, US-014, US-015, US-019, US-020, US-021, US-022, US-023, US-048, US-025, US-026, US-027, US-028, US-029, US-031 |
| R3 | US-036, US-037, US-038, US-039, US-040, US-041, US-042, US-043, US-044, US-045, US-046 |
| R4 | US-010, US-030, US-047 |

Chaque story commence par un test qui échoue sur `8c3dd2ca`. Un critère que ce test ne reproduit pas est retiré de la story avec la preuve consignée dans la PR ; il n'est jamais corrigé à l'aveugle. Les dépendances du JSON font foi.

### EP-001: Protéger les données de l'utilisateur

Corrige chaque chemin qui détruit du travail sans prévenir : suppression et démontage de worktrees, buffers sales, synchronisation disque de l'éditeur, Revert et réinitialisations de réglages.

**Definition of Done:** Aucun des sept scénarios de perte de données de l'audit n'est reproductible, chacun est couvert par un test de régression automatisé, et toute action destructive restante demande une confirmation explicite.

#### US-001: Protéger la suppression d'un worktree depuis le menu d'onglet
**Description:** En tant que développeur dont un agent travaille dans un worktree, je veux que « Remove worktree » vérifie les sessions vivantes et demande confirmation afin de ne jamais perdre le checkout sous un agent actif. Sources : fork #348 (2e803b98, 5e1168cc) ; `src-app/src/app/tab_worktree.rs:820-900`, `src-app/src/app/sidebar/context_menu.rs:588-598`, `src-app/src/app/worktree_remove.rs:211`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un onglet lié à un worktree dont un pane a une session hébergée vivante (`terminal::host_link::live_sessions`), when l'utilisateur choisit Remove worktree, then le dialogue bloquant de `worktree_remove.rs` s'affiche et rien n'est supprimé.
- [ ] Given aucun pane vivant, when l'utilisateur choisit Remove worktree, then une confirmation nommant le chemin et la branche est exigée avant `snapshot_and_remove`.
- [ ] Les bloqueurs (workspace ouvert sur ce chemin, sessions vivantes) sont réévalués après la confirmation et immédiatement avant `snapshot_and_remove` ; un bloqueur apparu entre-temps annule la suppression avec un toast.
- [ ] Un test vérifie qu'avec une session vivante, puis avec un bloqueur apparu après confirmation, `snapshot_and_remove` n'est jamais appelé.
- [ ] Échec : given une suppression partielle sous Windows (cwd encore ouvert), then un toast nomme le chemin restant et l'onglet ne perd pas son rattachement au worktree.

#### US-002: Garantir le snapshot avant tout démontage de worktree
**Description:** En tant que développeur, je veux que la détection de worktree propre voie tous les fichiers non suivis et les sous-modules afin qu'un démontage ne supprime jamais de travail sans snapshot. Sources : fork #651 (c233bcd7), #938 (80b7c07d) ; `src-app/src/workspace/worktree.rs:658-686`, `:1052-1054`, `:1063-1065`, `src-app/src/app/session.rs:540-557`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `is_clean` exécute `git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none`.
- [ ] Given `status.showUntrackedFiles=no` dans la config du dépôt et un fichier non suivi, when le worktree est démonté (fermeture de workspace, `enforce_worktree_limit` ou Settings), then un snapshot est écrit avant `worktree remove --force` (test sur un dépôt temporaire réel).
- [ ] Given un sous-module modifié avec `submodule.<name>.ignore=all`, then le worktree n'est pas considéré comme propre.
- [ ] Le `git worktree prune` exécuté au restore de session ne supprime pas l'entrée d'un worktree dont le dossier est absent mais qui a été utilisé dans les 90 derniers jours (test : dossier déplacé, restore, entrée conservée).
- [ ] Échec : given `git status` en erreur ou hors délai, then `is_clean` renvoie une erreur et le démontage est annulé avec un toast, jamais traité comme propre.

#### US-003: Ne jamais fermer ni quitter en jetant un buffer éditeur non sauvegardé
**Description:** En tant que développeur qui édite dans le dock, je veux que fermer un onglet, un workspace, la fenêtre ou l'app me propose d'enregistrer les fichiers modifiés afin de ne jamais perdre une édition. Sources : fork #396 (82060c2a), #397 (72e010c2, 44a82b84), 7da1085d, 7799e2b4 ; `src-app/src/app/cli_diff_dock.rs:74-117`, `src-app/src/app/close_policy.rs:150-243`, `src-app/src/app/quit_dialog.rs:206-216`, `src-app/src/app/window.rs:228-233`, `src-app/src/app/diff_dock/tabs.rs:199-218,287-293`, `src-app/src/app/workspace_ops/tab.rs:268,602-604`.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given une `CodeView` sale dans le slot actif ou dans un slot garé (`diff_dock.parked`) d'un onglet, when l'onglet ou son workspace est fermé depuis l'UI (raccourci, sidebar, menu contextuel), then le dialogue de fermeture liste chaque fichier sale avec Save, Don't Save et Cancel.
- [ ] Given un fichier sale dans n'importe quel onglet, when Quit, fermeture de fenêtre (`on_window_should_close`) ou redémarrage de mise à jour, then le même choix est proposé avant `quit_plan` ; Cancel laisse l'app ouverte et intacte.
- [ ] Given Save, when l'écriture d'un fichier échoue ou entre en conflit, then la fermeture est interrompue et le fichier reste ouvert et sale.
- [ ] Given un fichier sale dans le workspace ciblé, when l'IPC `workspace.close` est appelé, then la réponse est une erreur qui nomme les fichiers sales et rien n'est fermé.
- [ ] La fermeture d'un onglet dock sale n'est confirmée qu'à partir de 400 ms après l'armement, et l'armement expire après 4 s ; un double-clic rapide ne ferme pas l'onglet.
- [ ] Déplacer le seul pane d'un onglet vers un autre workspace ne détruit ni le dock de cet onglet ni son terminal sans passer par la même vérification.
- [ ] Échec : given `park_live_diff_dock` sans propriétaire vivant, then aucun slot contenant une vue sale n'est abandonné par `drop` (test).

#### US-004: Rendre l'enregistrement sûr face aux écritures externes
**Description:** En tant que développeur dont un agent réécrit le fichier ouvert, je veux qu'un enregistrement ne puisse jamais écraser une version plus récente sur disque sans choix explicite afin de conserver le travail de l'agent. Sources : fork #376 (f0ba7ffd), #428 et #402 (1b6a0057, 4449261a), #134 ; `src-app/src/app/diff_dock/code/view/disk_sync.rs:59-127`, `:194-240`, `src-app/src/app/diff_dock/code/save.rs:44-64`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `disk == Conflict`, when Ctrl/Cmd+S, then aucun octet n'est écrit et le bandeau reste affiché ; seul « Keep mine » autorise l'écrasement.
- [ ] La vue garde deux stamps distincts : celui observé sur disque, et celui auquel le buffer correspond. Ce dernier n'est mis à jour que dans `finish_disk_reload`, quand les splices sont appliqués.
- [ ] Given un reload en cours de calcul, when Save, then l'enregistrement est différé jusqu'à la fin du reload ou refusé ; il n'est jamais comparé à un stamp que le buffer n'a pas encore absorbé.
- [ ] Une sonde de reload lancée avant un Save réussi et terminée après est ignorée grâce à un jeton de génération.
- [ ] Le stamp est revérifié juste avant `persist` ; un changement détecté à ce moment annule l'écriture et place la vue en `Conflict`.
- [ ] Des tests `#[gpui::test]` couvrent Ctrl+S en `Conflict`, Save pendant un reload et la sonde périmée.
- [ ] Échec : given un fichier modifié par un autre processus après la sauvegarde de l'utilisateur, then le prochain Save détecte le conflit au lieu d'écraser.

#### US-005: Fiabiliser le rechargement et le Revert de gouttière de l'éditeur
**Description:** En tant que développeur, je veux que le rechargement d'un fichier ouvert applique les mêmes gardes que l'ouverture et que le Revert de gouttière soit exact afin que l'éditeur n'altère jamais un fichier par surprise. Sources : fork #271 (0b9fa454), #378 (f0ba7ffd), #457 et #462 (c61842d0), #432 (07f9f3e7, 0c10683b), #434 ; `src-app/src/app/diff_dock/code/document.rs:14-19,83,288-294`, `disk_sync.rs:128-166,194-202,269-329`, `src-app/src/app/diff_dock/code/view/change_markers.rs:226-232,415-437`, `src-app/src/widgets/editor_scrollbar.rs`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Le rechargement applique les gardes de `load.rs:72-123` : un fichier de plus de 10 MiB, binaire, avec une ligne de plus de 10 000 caractères ou en lecture seule produit l'état correspondant au lieu d'un splice.
- [ ] Given un fichier réécrit en UTF-8 invalide, then la vue affiche l'état illisible et non Deleted.
- [ ] Given un buffer propre, when le fichier passe de LF à CRLF sur disque, then le reload met à jour la fin de ligne détectée et le Save suivant conserve CRLF.
- [ ] Après l'enregistrement du watcher, le fichier est re-stat ; une écriture survenue entre le chargement et l'enregistrement du watcher déclenche un reload.
- [ ] Le popup de marqueur se ferme à toute édition du buffer ; son Revert agit sur le bloc affiché à l'ouverture ou ne fait rien.
- [ ] Revert d'une nouvelle ligne finale ajoutée et d'une dernière ligne non terminée restaure le texte de base octet pour octet.
- [ ] Le drag de la scrollbar verticale est annulé au remplacement du handle, à la fermeture du dock et au changement d'onglet.
- [ ] Échec : given un fichier ouvert qui grossit à 11 MiB, then aucun `read_to_string` non borné n'est exécuté et la vue passe à l'état « trop volumineux ».

#### US-006: Rendre le Revert de l'onglet Changes exact et non destructif
**Description:** En tant que développeur, je veux que Revert n'agisse que sur le contenu affiché et ne modifie rien hors du hunk afin de ne jamais réécrire le travail d'un agent. Sources : fork #458, #459, #460, #463, #464, #468 (c61842d0, 28227fe7), #941 (bd5e3c9e), #432 (0e89cbde, 4cf03d32) ; `src-app/src/app/diff_dock/revert.rs:16-96,302-321`, `src-app/src/app/diff_dock/git.rs:31-56`, `src-app/src/diff/git.rs:140-146,188-196,241-246`, `src-app/src/app/diff_dock/code/save.rs:52-62`, `change_markers.rs:18`.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] Au clic, les octets lus sont normalisés comme le diff puis comparés à `FileDiff::new_text` ; toute différence refuse le Revert avec le message de fichier périmé et relance le diff.
- [ ] Le stamp vérifié est pris sur le même handle que la lecture ; le dock prend le stamp de chaque fichier au moment de sa lecture, pas après la boucle.
- [ ] `parse_name_status_z` reconnaît `T` ; Revert est refusé pour un changement de type et pour un symlink avec un message qui nomme la raison, et aucun symlink n'est remplacé par un fichier régulier.
- [ ] Le splice conserve la fin de ligne propre à chaque ligne hors du hunk et ne transforme aucun CR isolé ; un test sur un fichier mixte LF/CRLF avec CR isolé produit un diff limité au hunk.
- [ ] Sous Unix, un fichier en 0444 ou 0555 conserve ses bits de mode après Revert ; sous Windows, l'attribut lecture seule est conservé ou le Revert est refusé avec un message.
- [ ] Revert est refusé si une vue sale du fichier existe dans un slot actif ou garé ; la fin d'un Revert rafraîchit le dock de l'onglet d'origine.
- [ ] Deux mouvements rapides de HEAD n'installent jamais la base la plus ancienne dans la gouttière.
- [ ] Échec : given un agent qui écrit le fichier entre la construction du dock et le clic, then le fichier sur disque est inchangé après le clic (test).

#### US-007: Ordonner les écritures de réglages et protéger les réinitialisations
**Description:** En tant que développeur, je veux que la dernière valeur choisie soit celle enregistrée et qu'une réinitialisation ne puisse pas partir par accident afin de ne jamais perdre ma configuration. Sources : fork #27 (d8db591b), #135, #242 (a62c085a), #59 (ce0bfa6c), #729 (430842f1), #300 (2a6fa322), 5d0bd7db ; `src-app/src/app/settings.rs:112-177,268-365,371-400`, `src-app/src/config_writer.rs:4-10,84-103,231-243,283-310`, `src-app/src/settings/tabs/shortcuts.rs:540-600`, `src-app/src/app/window.rs:195-208`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Chaque `persist_setting` reçoit un numéro de séquence ; sous `CONFIG_WRITE_LOCK`, une écriture plus ancienne que la dernière écrite est ignorée.
- [ ] Test : deux clics rapides sur un stepper laissent la valeur la plus récente sur disque et en mémoire après le reload du watcher.
- [ ] Le reload déclenché par une écriture de l'app ne remplace pas en mémoire une valeur plus récente.
- [ ] « Reset all to defaults » n'accepte la confirmation qu'après 400 ms et écrit `paneflow.json.before-reset` avant de modifier le fichier.
- [ ] Un reload de config pendant l'enregistrement d'un raccourci désarme la ligne ; la cible de l'enregistrement est identifiée par nom d'action.
- [ ] Donner le focus au champ de recherche des raccourcis désarme l'enregistrement ; une lettre tapée dans ce champ n'est jamais enregistrée comme raccourci.
- [ ] `with_field` et `with_agent_panel_field` renvoient une erreur journalisée et n'écrivent pas le disque quand la conversion serde échoue.
- [ ] Échec : given `paneflow.json` en lecture seule, when Reset all, then le toast d'échec de `save_shortcut_checked` s'affiche et les raccourcis en mémoire restent inchangés.

---

### EP-002: Durcir git, le système de fichiers et les worktrees

Remplace les spawns git dispersés par un builder unique, fait écrire tous les fichiers à travers les symlinks, et empêche toute lecture ou journalisation de bloquer ou de suivre un fichier spécial.

**Definition of Done:** Un test de garde prouve que 100 % des spawns git de production passent par le builder ; aucun écrivain ne remplace un symlink ; aucune FIFO à un chemin d'état ne bloque plus de 100 ms ; les sondes ne prennent jamais `index.lock`.

#### US-008: Introduire un builder git unique avec un profil sonde isolé
**Description:** En tant que développeur qui ouvre des dossiers de sources variées, je veux que les commandes git lancées automatiquement par Paneflow n'exécutent jamais de code configuré dans le dépôt afin qu'ouvrir un dossier ou y faire `cd` reste sans danger. Sources : fork #170 (c0d6f34c), #246 (bf24e2b2), #1019 (c7e62359, d6bb6572, ae58323d), #145, #525 (14d16e23) ; `src-app/src/workspace/git.rs:91-103`, `src-app/src/diff/git.rs:49-71`, `src-app/src/workspace/worktree.rs:446,460,740-754`, `src-app/src/app/clone_repo.rs:267`, `src-app/src/app/diff_dock/branch.rs:428,456`, `src-app/src/app/diff_dock/code/base.rs:139`, `src-app/src/app/files_git.rs:223`, `src-app/src/app/diff_dock/revert.rs:542`, `src-app/src/cli/up_cmd.rs:850`.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un constructeur unique crée toutes les commandes git de production. Il a deux profils : `Probe`, pour les lectures automatiques, et `UserAction`, pour les opérations demandées par l'utilisateur (worktree add et remove, switch, clone, checkout et restore, snapshot).
- [ ] Les deux profils posent `GIT_TERMINAL_PROMPT=0` et retirent de l'environnement hérité `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES` et `GIT_COMMON_DIR`.
- [ ] `Probe` ajoute `-c core.fsmonitor=false -c core.hooksPath=<dossier vide géré par Paneflow> -c safe.bareRepository=explicit -c diff.external=`, et `--no-ext-diff --no-textconv` sur les diffs ; `UserAction` conserve hooks et filtres.
- [ ] Le dossier de hooks vide est un vrai dossier sous `~/.paneflow/cache`, valide sous Linux, macOS et Windows.
- [ ] Un test de garde échoue si un `Command::new("git")` hors `#[cfg(test)]` existe ailleurs que dans le builder.
- [ ] Test : dans un dépôt dont `.git/config` pointe `core.fsmonitor` vers un script qui crée un fichier marqueur, les sondes stats, diff, ls-files et status ne créent jamais ce marqueur.
- [ ] `git status` par `Probe` sur un dépôt de 100 000 fichiers suivis prend au plus 2x le temps de la même commande sans isolation ; la mesure est consignée.
- [ ] Échec : given un worktree lié à un dépôt bare, then les sondes `Probe` fonctionnent ; sinon la story applique et documente un repli pour ce cas.

#### US-009: Empêcher les sondes git d'arrière-plan de prendre le verrou de l'index
**Description:** En tant que développeur dont les agents commitent en parallèle, je veux que les sondes de Paneflow ne réécrivent jamais `.git/index` afin que les `git commit` des agents n'échouent pas sur `index.lock`. Sources : fork #525 (14d16e23), #943 (c2e5ee68), #56 et #689 (1eb733d5) ; `src-app/src/workspace/git.rs:24,34-61`, `src-app/src/diff/git.rs:321`, `src-app/src/app/files_git.rs:142-157`, `src-app/src/app/workspace_ops/git_watch.rs:61-188`, `crates/paneflow-process/src/lib.rs:380-427`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] `Probe` pose `GIT_OPTIONAL_LOCKS=0` et `-c diff.autoRefreshIndex=false`, ainsi que `LC_ALL=C` et `LANGUAGE=C`.
- [ ] Test : dans un dépôt dont des fichiers ont été touchés sans changement de contenu, les sondes `diff --shortstat HEAD`, `diff --name-status HEAD` et `status` laissent le mtime de `.git/index` inchangé.
- [ ] Test de concurrence : 500 itérations `git add` puis `git commit` d'un script, pendant que les sondes tournent en boucle, produisent 0 erreur `index.lock`.
- [ ] Avec `LANG=fr_FR.UTF-8`, le parsing de `--shortstat` renvoie les nombres corrects.
- [ ] Échec : given une sonde tuée à son délai, then aucun `.git/index.lock` ne subsiste.

#### US-010: Rendre les résultats git exacts ou explicitement en erreur
**Description:** En tant que développeur, je veux que les compteurs, branches et statuts git affichés soient exacts ou signalés en erreur afin de ne jamais prendre une sonde échouée pour un état réel. Sources : fork #691 et #913 (1eb733d5, 618e77dd), #646 (00bdbb46), #307 (3cf49754), #301, #921 (e78212eb), #928 (0a4505e9), #393 (a84fabc6, 97e951c1, 339bb000), #394 (06ea1318), #525 (14d16e23), #540 (20c163f1, 5ae3e42a) ; `src-app/src/workspace/git.rs:10,67-88,271-289`, `src-app/src/diff/git.rs:73-78,118-124,305-334`, `src-app/src/app/diff_dock/branch.rs:430`, `src-app/src/workspace/worktree.rs:809-826`, `src-app/src/app/diff_dock/git.rs:90`, `src-app/src/app/files_git.rs:131-160`, `src-app/src/app/files_sidebar/worker.rs:84-180`, `src-app/src/app/files_sidebar/row.rs:27,163-167`.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Le compteur de fichiers non suivis affiche le nombre exact, ou le plafond suivi de « + » quand il est atteint ; il ne vaut jamais 1001 ni 0 par dépassement du plafond de 256 KiB.
- [ ] Un échec de `rev-parse HEAD` est affiché comme une erreur ; l'arbre vide n'est utilisé que pour une branche non née.
- [ ] Les listes de branches utilisent `%(refname:lstrip=2)` ; une branche et un tag du même nom ne produisent plus `heads/<name>`.
- [ ] La branche d'un dépôt reftable est affichée correctement.
- [ ] Le dock affiche l'état « Not a Git repository » pour un dossier hors dépôt, jamais la sortie d'usage de git.
- [ ] Les stats d'un workspace ouvert sur un sous-dossier comptent les fichiers non suivis de tout le dépôt, comme le diff.
- [ ] Le Files tree conserve les statuts précédents quand une sonde échoue, active le polling de secours si le watch de `.git` échoue, ne supprime plus l'espace initial d'un chemin, et met à jour les points d'un dossier jamais déplié au moment où il est déplié.
- [ ] Échec : given une sonde qui dépasse son délai, then l'UI montre l'état précédent ou une erreur, jamais « 0 fichier » présenté comme exact.

#### US-011: Donner aux commandes utilisateur un budget de capture adapté
**Description:** En tant que développeur qui utilise `paneflow up` et `flow`, je veux que mes commandes `setup` et les commandes git bavardes ne soient pas tuées pour trop de sortie afin que mes worktrees soient correctement préparés. Sources : fork #95 (ae5762e1), #144 (400f1b19) ; `src-app/src/cli/up_cmd.rs:389`, `src-app/src/cli/flow_cmd.rs:634`, `crates/paneflow-process/src/lib.rs:12,107-131,388,527,583-598`, `src-app/src/workspace/worktree.rs:451`, `src-app/src/diff/git.rs:55`, `src-app/src/app/diff_dock/branch.rs:463`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Le `setup` de `paneflow up` et `flow` s'exécute avec un budget séparé : stdout jusqu'à 8 MiB, et seuls les 64 derniers KiB de stderr conservés, sans échec en cas de dépassement.
- [ ] Test : un `setup` qui écrit 1 MiB sur stderr puis réussit se termine avec le statut 0 sans être tué.
- [ ] Le `git add -A` du snapshot, le `git diff` du dock et `git switch` n'échouent plus au-delà de 64 KiB de stderr ; seule la fin de stderr est conservée.
- [ ] `spawn_detached` journalise l'échec d'envoi au reaper et attend lui-même l'enfant quand le thread reaper n'existe pas.
- [ ] Échec : given un `setup` qui dépasse son délai, then il est tué, le message nomme le délai et affiche les 20 dernières lignes de stderr.

#### US-012: Écrire à travers les symlinks sans jamais les remplacer
**Description:** En tant que développeur qui gère ses dotfiles avec stow, chezmoi ou home-manager, je veux que Paneflow écrive dans la cible d'un symlink sans remplacer le lien afin que ma configuration versionnée reste la source de vérité. Sources : fork #660 (107d0eb4), #875 (39a588af), #1026 (81140984), #360 (97dbb4ea), #231 et #205 (da43470d, ec0b79e0, 14acdfb1), #243 (631613cf), #432 (25dd720f, b17e95bc) ; `src-app/src/config_writer.rs:43-78`, `src-app/src/window_state.rs:89-108`, `src-app/src/app/session.rs:1021-1053`, `src-app/src/app/diff_dock/code/save.rs:44-64`, `src-app/src/markdown/state.rs:163-185`, `crates/paneflow-mcp-install/src/io.rs:28-46`, `crates/paneflow-config/src/watcher.rs:63-116`, `src-app/src/theme/watcher.rs:190,213`.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un helper partagé suit cet ordre : résoudre la cible d'un chemin symlink, créer le fichier temporaire dans le dossier de la cible, `sync_all`, puis renommer sur la cible en préservant son mode.
- [ ] Ce helper est utilisé pour `paneflow.json`, `window-state.json`, `session.json`, `markdown_state.json`, les fichiers enregistrés par le dock et les configs d'agents écrites par `paneflow-mcp-install`.
- [ ] Test Unix : chacun de ces fichiers, créé comme symlink, reste un symlink après écriture, et sa cible contient la nouvelle valeur.
- [ ] Un symlink pendant ou une cible en lecture seule (magasin Nix) produit une erreur affichée à l'utilisateur ; le lien n'est jamais remplacé par un fichier.
- [ ] Le watcher de config surveille aussi le dossier de la cible résolue et re-résout le lien à chaque événement ; une édition de la cible déclenche un reload en moins de 1 s.
- [ ] Le comportement Windows est vérifié nativement sur un symlink de fichier créé en Developer Mode.
- [ ] Échec : given un symlink vers un fichier sur un autre volume, then l'écriture réussit, car le fichier temporaire est créé à côté de la cible.

#### US-013: Ne jamais bloquer ni suivre un fichier spécial en lisant ou en journalisant
**Description:** En tant que développeur, je veux que Paneflow refuse proprement les FIFO, les symlinks et les fichiers démesurés à ses chemins d'état et de log afin qu'un fichier inattendu ne fige jamais l'app. Sources : fork #241 (91696201), #379, #380 et #381 (f0ba7ffd), #235 (9449e3f4), #732 (aadf9a87), #166, #1058, #1028 et #257 (ed812e00, c658c8af), #272 (3016e7fb), #737 (8f47a8b1), #258 (7a2c397f), #143 ; `crates/paneflow-config/src/loader.rs:47-66`, `src-app/src/app/session.rs:677-691,994,1035-1038,1112,1130`, `src-app/src/markdown/view.rs:737-758`, `crates/paneflow-mcp-install/src/merge.rs:6-14,84-96`, `crates/paneflow-mcp-install/src/io.rs:52-61`, `crates/paneflow-mcp-install/src/agents/support.rs:191-197`, `src-app/src/diff/git.rs:188-211,285-289`, `src-app/src/ai_hooks/mod.rs:5-18`, `crates/paneflow-ai-hook/src/runtime.rs:253-267`, `src-app/src/window_state.rs:113-135`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Sous Unix, les fichiers suivants sont ouverts avec `O_NONBLOCK`, vérifiés comme fichiers réguliers sur le descripteur, puis lus avec `take(cap + 1)` : `paneflow.json`, `session.json` et `.pending`, `window-state.json`, les fichiers Markdown, les configs d'agents et les fichiers du working tree lus par le diff.
- [ ] Test : une FIFO sans écrivain à chacun de ces chemins fait échouer la lecture en moins de 100 ms ; le démarrage continue avec l'état par défaut et journalise la raison.
- [ ] Les writers `PANEFLOW_HOOK_LOG` de l'app et de `paneflow-ai-hook` exigent un chemin absolu, ouvrent avec `O_NOFOLLOW | O_NONBLOCK` et n'écrivent que sur un fichier régulier.
- [ ] Test des writers : un chemin relatif ne crée aucun fichier, une cible symlink reste inchangée, et une FIFO rend la main en moins de 100 ms.
- [ ] Sous Unix, `session.json`, ses backups et ses fichiers temporaires sont créés en 0600, et `~/.paneflow` en 0700.
- [ ] La taille d'un symlink suivi est mesurée sur sa cible ; un fichier suivi remplacé par une FIFO apparaît comme illisible dans le dock au lieu de bloquer un worker.
- [ ] Un échec de listing dans `rotate_corruption_backups` est journalisé avec le chemin.
- [ ] Échec : given un `opencode.jsonc` qui est une FIFO, when Settings > MCP s'ouvre, then une erreur est affichée et `agent-config.lock` n'est pas conservé.

#### US-014: Copier les fichiers d'include d'un worktree sans suivre les symlinks
**Description:** En tant que développeur, je veux que la création d'un worktree ne copie jamais le contenu d'une cible de symlink située hors du dépôt afin qu'aucun secret ne se retrouve dans le worktree ou dans son snapshot. Sources : fork #173 (c0d6f34c), #940 (05e87518) ; `src-app/src/workspace/worktree.rs:1092-1141`, `src-app/src/cli/up_cmd.rs:364`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `copy_tree` utilise `symlink_metadata` à chaque niveau : un symlink source est recréé comme symlink vers la même cible, jamais copié comme contenu.
- [ ] La destination est créée avec `create_new` (et `O_NOFOLLOW` sous Unix) ; un symlink pendant à la destination n'est jamais suivi.
- [ ] Une boucle de symlinks de dossiers ne fait pas boucler la copie.
- [ ] Échec : given une copie qui échoue au milieu d'un fichier, then le fichier partiel est supprimé et l'erreur est affichée dans le toast de création.

#### US-015: Garder l'état git d'un onglet frais et rattaché au bon checkout
**Description:** En tant que développeur qui navigue entre worktrees, je veux que le badge git et le worktree lié à un onglet suivent réellement mes panes afin de ne jamais lancer un agent dans le mauvais checkout. Sources : fork #724 (44ed2b7c), #887 (bfe50d02), #937 (72614cf6), #1025 (2ec5a26e), #366 (1258635d, 98723b29), #347 et #348 (f0688146, 5e1168cc), #54 (ff403769), #709 (5580ecec), #359 et #350 (f121dce3, ca814faf) ; `src-app/src/app/event_handlers/cwd_tracking.rs:29-115`, `src-app/src/app/tab_worktree.rs:249-296`, `src-app/src/workspace/mod.rs:382-400`, `src-app/src/workspace/tab.rs:49-57`, `src-app/src/theme/watcher.rs:241-244`, `src-app/src/app/workspace_ops/git_watch.rs:92-199`, `src-app/src/app/pull_request.rs:86-146,232-273`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Un pane qui sort du dépôt du workspace puis y revient rétablit le watch du `.git` du workspace ; les stats d'un dépôt étranger ne sont jamais enregistrées sous le cwd du workspace.
- [ ] Les sondes de cwd portent un numéro de séquence par pane ; seule la plus récente peut lier l'onglet.
- [ ] Un checkout de branche lent ne remplace pas un rattachement choisi après lui ; l'état « checkout en attente » est tenu par onglet.
- [ ] Un pane qui revient au checkout principal délie l'onglet de son worktree.
- [ ] Les chemins de worktree d'onglet sont persistés sans perte ; un chemin non UTF-8 est restauré.
- [ ] Le confinement du cwd compare des chemins canonisés des deux côtés et refuse `..`.
- [ ] Les watchers de thème et de HEAD/index ont un plafond de debounce de 2 s ; les sondes de badge d'un workspace partagent un délai global de 10 s.
- [ ] Un échec `gh` place le dépôt en attente de 10 min au lieu de le désactiver jusqu'au redémarrage ; un succès ou un changement de branche lève l'attente.
- [ ] Échec : given `cd /tmp && cd -` dans l'onglet actif, then le badge git du workspace est correct en moins de 2 s sans attendre le poll de 30 s.

---

### EP-003: Fiabiliser le terminal et son host

Fait en sorte que la sortie d'un programme ne puisse plus faire croire à la mort d'un pane, que le host rapporte le thème et la géométrie réels, et que les commandes du terminal agissent sur l'émulateur et non sur le programme.

**Definition of Done:** Un flot de 10 000 BEL laisse le pane vivant ; une requête OSC 11 sur un thème clair reçoit le fond du thème ; Reset et Clear n'écrivent rien dans le PTY et effacent aussi le host ; chaque défaut terminal de l'audit a un test.

#### US-016: Reproduire le passage d'un pane vivant à « exited » sur un flot de BEL
**Description:** En tant que mainteneur, je veux reproduire à l'exécution le défaut de dépassement d'effets afin de corriger la bonne cause avant US-017. Sources : fork #797 (9a8dffd1), #805 (c3f222f7) ; `crates/paneflow-terminal-ghostty/src/callbacks.rs:12-155`, `src-app/src/terminal/ghostty_session/events.rs:281-289`, `src-app/src/terminal/ghostty_session/attached_runtime.rs:430-447`, `src-app/src/terminal/pty_session.rs:600-605`.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un test automatisé alimente le miroir avec 300 BEL dans un seul chunk et vérifie sur `8c3dd2ca` l'apparition de `RuntimeFailed` puis de `exited = Some(-1)`.
- [ ] Le même test avec 20 OSC 9, 40 OSC 52 et 40 séquences inconnues consigne le seuil qui déclenche l'état exited.
- [ ] Une reproduction à l'exécution sur un vrai PTY est consignée avec la plateforme : 100 000 octets contenant 10 000 BEL passent par `cat` dans un shell, et le pane passe à `exited = Some(-1)` avant la vraie sortie du programme.
- [ ] Échec : si aucun cas ne reproduit l'état exited, la preuve est consignée et US-017 est reclassée avant d'être commencée.

#### US-017: Absorber les rafales d'effets terminal sans tuer le pane
**Description:** En tant que développeur, je veux qu'une rafale de bells, de notifications ou de séquences inconnues soit absorbée sans marquer le pane comme terminé afin que mon shell et mes sessions agent restent visibles. Sources : fork #797, #805, #909 (9fe909f0), #1029 (0e6295bd), #898 (9ac54348), #240, #701 (c6f8e05b) ; `crates/paneflow-terminal-ghostty/src/callbacks.rs:12-203`, `crates/paneflow-terminal-ghostty/src/batch.rs:38-45`, `src-app/src/terminal/ghostty_session/events.rs:281-289`, `src-app/src/agents/notifications.rs:132-145`, `src-app/src/app/event_handlers/mod.rs:638-662`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] Un dépassement du plafond d'effets est coalescé ou ignoré avec au plus un `log::warn!` par pane et par minute ; il ne produit jamais `RuntimeFailed`.
- [ ] Test : 100 000 octets contenant 10 000 BEL, puis `echo ok` ; le pane reste vivant, affiche `ok`, et ses sessions agent ne sont pas purgées.
- [ ] Les bells sont limitées à une par 250 ms et par pane.
- [ ] Les notifications OSC 9/777 sont limitées à 3 par 60 s et par surface, les doublons identiques (titre et corps) étant fusionnés.
- [ ] Les notifications de programme respectent le réglage « Native OS notifications » ; un échec de livraison est journalisé une fois par surface.
- [ ] Le message d'un échec batch libghostty n'est plus présenté comme une incompatibilité d'ABI.
- [ ] Échec : given un cluster de graphèmes de 2 000 points de code, then le pane n'est pas marqué terminé.

#### US-018: Faire rapporter au host le thème et la géométrie réels
**Description:** En tant que développeur sur un thème clair, je veux que les programmes qui interrogent le terminal reçoivent les vraies couleurs et le vrai schéma afin que Neovim, bat, delta et les CLI d'agents s'affichent correctement. Sources : fork #897 (936f4113, 1d00fcd3), #872 (e0f8da3a), #677 (347c0cea), #343 (295d7c88) ; `crates/paneflow-host/src/runtime.rs:40-41,786-790,1106-1148,1642-1643`, `crates/paneflow-host/src/host/types.rs:37-56`, `src-app/src/terminal/ghostty_session/commands.rs:239-251`, `src-app/src/theme/model.rs:404-421,698-708`, `src-app/src/terminal/element/paint/cursor.rs:10-18`, `src-app/src/terminal/element/mod.rs:684,769,893,1058`.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `CreateSession` transmet au host l'apparence complète : premier plan, fond, curseur, palette de 256 couleurs et schéma clair ou sombre.
- [ ] Une commande host met à jour l'apparence à chaque changement de thème et émet le rapport 997 vers les programmes qui ont activé le mode 2031.
- [ ] Test : sur un thème clair, une requête OSC 11 reçoit la couleur de fond du thème, et `CSI ? 996 n` reçoit `CSI ? 997 ; 2 n`.
- [ ] Le host reçoit la taille réelle des cellules en pixels à la création et à chaque redimensionnement.
- [ ] Un host plus ancien, qui ignore la commande d'apparence, continue de fonctionner ; l'app journalise une fois que le rapport de thème n'est pas disponible pour cette session.
- [ ] `selection_foreground` est calculé contre le fond composé `background.blend(selection)`, et le test d'invariant utilise ce composite pour tous les thèmes livrés.
- [ ] Le glyphe sous un curseur bloc atteint un contraste APCA Lc d'au moins 45 sur Vercel Light et Tailwind Light (test calculé).
- [ ] La clé de cache de rendu lit la génération de thème avant le thème.
- [ ] Échec : given un reattach à une session créée avant la mise à jour, then aucun crash et aucune sortie parasite n'apparaissent dans le pane.

#### US-019: Faire agir Reset et Clear sur l'émulateur et le host
**Description:** En tant que développeur, je veux que « Reset terminal » et « Clear scroll history » agissent sur le terminal et jamais sur le programme en cours afin de ne plus interrompre un agent ni garder un secret effacé côté host. Sources : fdf67050, fork #896 (ed9d8b62) ; `src-app/src/terminal/search.rs:30-39`, `crates/paneflow-terminal-ghostty/src/engine.rs:12,93-97`, `src-app/src/terminal/ghostty_session/commands.rs:219-226`, `crates/paneflow-host/src/runtime.rs:1106-1148`, `crates/paneflow-host/src/server.rs:716-810`, `crates/paneflow-host/src/cold_text.rs:7-15`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] « Reset terminal » réinitialise l'émulateur du host et celui du miroir, et n'écrit aucun octet dans le PTY ; un test vérifie qu'aucun `ESC c` n'atteint le programme.
- [ ] « Clear scroll history » efface le scrollback du host et celui du miroir.
- [ ] Après un reattach ou un redémarrage de l'app, l'historique effacé ne revient pas et n'apparaît pas dans `final-output.txt`.
- [ ] Le clear n'injecte plus d'octets bruts entre deux chunks ; une séquence d'échappement coupée entre deux chunks reste intacte (test).
- [ ] Pendant l'écran alternatif (modes 47, 1047, 1049), Clear n'efface pas l'écran du programme.
- [ ] Échec : given un host plus ancien sans ces commandes, then un toast indique que l'historique côté host n'a pas pu être effacé, et rien n'est écrit dans le PTY.

#### US-020: Corriger la géométrie du pointeur, du survol et de la sélection
**Description:** En tant que développeur, je veux que clics, survols et copies visent exactement ce qui est affiché et ne figent jamais la fenêtre afin de pouvoir utiliser les TUI et les liens en confiance. Sources : fork #789 (e31e0f45, eda3f88f), #696 (2d60ac16), #149, #168 et #707 (400f1b19, c0d6f34c, bab04fba), #652 et #654 (08b30e9b), #721 (b67b682d), #704 (4bc96616) ; `src-app/src/terminal/input.rs:486-532,792-866,956-1000`, `src-app/src/terminal/element/mod.rs:710-716`, `crates/paneflow-terminal-ghostty/src/encode.rs:94-126`, `crates/paneflow-terminal-ghostty/src/engine.rs:26-45,111`, `src-app/src/terminal/ghostty_session/mod.rs:792-799,873-906`, `src-app/src/terminal/element/hyperlink.rs:125-330,457-520`, `src-app/src/terminal/view.rs:739-771`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les coordonnées souris transmises à libghostty sont mises à l'échelle par le rapport entre la taille de cellule arrondie et la taille mesurée ; test : avec une cellule de 8,5 px, un clic au centre de la colonne 80 est rapporté en colonne 80.
- [ ] Le texte de lien sous Cmd/Ctrl-hover est lu sur la ligne affichée quand la vue est remontée de 50 lignes (test).
- [ ] La résolution de chemins de fichiers au survol (`canonicalize`, `is_file`) s'exécute hors du thread GPUI ; son résultat n'est appliqué que si la cellule survolée n'a pas changé.
- [ ] Un mouse-up sans sélection n'envoie aucune requête runtime synchrone ; la lecture de sélection passe par une tâche d'arrière-plan.
- [ ] Une copie refusée pour dépassement des 400 000 octets affiche un toast et conserve la sélection.
- [ ] Un drag commencé sur un lien et étendu en sélection n'ouvre pas le lien.
- [ ] Le cache de l'encodeur souris distingue les modes 9, 1000, 1015 et 1016.
- [ ] Échec : given un résolveur de chemin simulé qui bloque 5 s, then aucune frame ne dépasse 50 ms pendant le survol.

#### US-021: Rendre l'IME, la barre de recherche et les champs texte corrects
**Description:** En tant que développeur qui compose du texte avec un IME ou utilise la recherche, je veux voir ma composition et garder mes raccourcis afin de saisir et chercher sans surprise. Sources : fork #324 (ef184e02), #1059, #364 (6fb8f618), 9a978db5, #429 (9190fed4), #278 (5f1a941a) ; `src-app/src/terminal/element/paint/overlay.rs:88,114`, `src-app/src/widgets/text_input.rs:436-449`, `src-app/src/widgets/text_area.rs:536-567`, `src-app/src/terminal/view.rs:1366-1368,1472-1474`, `src-app/src/app/macos_menu.rs:76-77`, `src-app/src/terminal/input.rs:325,1003`, `src-app/src/markdown/view.rs:335-365,582-590`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le texte de preedit IME est peint avec la couleur de texte et un fond opaque du thème.
- [ ] `character_index_for_point` passe `line_point.x` à `index_for_x`.
- [ ] Test de `character_index_for_point` : sur un champ éloigné de l'origine, deux positions interrogées donnent des offsets UTF-16 distincts et attendus, avec un texte non ASCII.
- [ ] Les bornes IME de `TextArea` viennent de la mise en page réelle, pas d'une grille fixe de 7x20.
- [ ] La vue terminal pose un seul `KeyContext` qui contient `Terminal` et `Search` ; les raccourcis du terminal restent actifs quand la barre de recherche est ouverte.
- [ ] `handle_paste` et `handle_copy` n'agissent que si le terminal a le focus ; sur macOS, Edit > Paste avec le champ de recherche focalisé colle dans ce champ.
- [ ] Une touche qui rend le curseur visible déclenche `cx.notify()`.
- [ ] La recherche Markdown utilise le `TextInput` partagé.
- [ ] Échec : given un champ vide, then `character_index_for_point` renvoie `None` sans panique.

#### US-022: Fiabiliser l'intégration shell et l'environnement des panes
**Description:** En tant que développeur, je veux que chaque pane démarre avec son intégration shell complète et un environnement fidèle afin que le cwd, les marques de prompt et ma configuration shell fonctionnent partout. Sources : 92dce813, fork #703, #383 et #384 (f0ba7ffd), 8bc3860e, #23 (b5662a20), #922 (ca3e8525), #369 (8d01d8b0), 19c81752, 3ce1d640 ; `src-app/src/terminal/shell.rs:11,38,57,194-224,502-573`, `crates/paneflow-terminal-ghostty/src/osc7.rs:1-71`, `src-app/src/terminal/pty_session/spawn_env.rs:169-205,242,269`, `schemas/paneflow.schema.json:292`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les scripts d'intégration ne sont réécrits que si leur contenu diffère, par fichier temporaire puis rename dans le même dossier.
- [ ] Test de concurrence : pendant la restauration de 10 panes, aucun shell ne lit un script d'intégration vide ou partiel.
- [ ] Les hooks zsh, bash et fish encodent le chemin OSC 7 en pourcentage, octet par octet ; un dossier nommé `a%20b` est rapporté tel quel.
- [ ] Un OSC 7 dont l'hôte n'est ni vide, ni `localhost`, ni le nom d'hôte local (MSYS compris) n'est pas utilisé comme cwd du pane.
- [ ] `ZDOTDIR` et `PANEFLOW_ORIG_ZDOTDIR` sont protégés contre `terminal.env` ; le `ZDOTDIR` de l'utilisateur est restauré par le `.zshenv` d'intégration.
- [ ] `LANG` n'est défini que si `LANG`, `LC_ALL` et `LC_CTYPE` sont tous absents.
- [ ] Chaque clé de `terminal.env` filtrée est journalisée une fois ; un test vérifie que la liste du schéma correspond à la liste filtrée par le code.
- [ ] `default_shell` accepte `~/` ; sous Unix, le test d'exécutabilité utilise `access(X_OK)`.
- [ ] Échec : given un `default_shell` inexistant, then le pane démarre avec le shell de la plateforme et un toast nomme le chemin refusé.

#### US-023: Durcir le presse-papiers, le collage et les images
**Description:** En tant que développeur, je veux qu'un programme ne puisse pas remplir mon presse-papiers ni faire exécuter un collage multi-lignes à mon insu, afin que mon terminal reste sûr et prévisible. Le critère d'undo-close est passé à US-048 (v1.1). Sources : fork #315 (bc617eea), #885 (1039826e), #237 (96619f4f), #195 (eb775e04) ; `src-app/src/terminal/pty_session.rs:224,239,818`, `src-app/src/terminal/view.rs:75-79`, `src-app/src/terminal/kitty.rs:72-88`, `crates/paneflow-terminal-ghostty/src/formatter.rs:38-67,216-230`, `crates/paneflow-terminal-ghostty/src/terminal_ops.rs:108-110`, `src-app/src/app/workspace_ops/mod.rs:137,193-202`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un réglage `terminal.osc52_clipboard` accepte `copy` (par défaut, comportement actuel) et `off` ; avec `off`, toute écriture OSC 52 est refusée. Le schéma `schemas/paneflow.schema.json` documente la clé.
- [ ] Un collage multi-lignes dans un programme sans bracketed paste demande une confirmation qui affiche le nombre de lignes ; Cancel n'envoie rien, et `allow_unsafe` n'est plus forcé à `true`.
- [ ] Le décodage PNG kitty impose la limite de 8192x8192 par les limites du décodeur, avant toute allocation ; test : un PNG de 1 Ko qui déclare 60000x60000 est refusé sans allocation de plus de 64 MiB.
- [ ] Échec : given un collage d'une seule ligne, then aucune confirmation n'apparaît.

#### US-048: Rouvrir un pane fermé en rattachant sa session
**Description:** En tant que développeur, je veux qu'annuler la fermeture d'un pane ou d'un onglet me rende la même session, avec son processus, son texte, ses styles, ses liens et son curseur, afin qu'une fausse manœuvre ne me coûte rien. Remplace le critère d'undo-close d'US-023 : libghostty n'émet pas d'OSC 8 en sortie VT (`src/terminal/formatter.zig:1462` au pin `0c2a290d`), et depuis `73c01bf5` la replay n'atteint plus l'écran, car le runtime attaché ignore `WriteOutput` (`src-app/src/terminal/ghostty_session/attached_runtime.rs:791`). Modèle : l'`undo-timeout` de Ghostty, 5 s par défaut (`src/config/Config.zig:2733`). Sources : `src-app/src/app/close_policy.rs:236-330`, `src-app/src/app/workspace_ops/mod.rs:66-206,622-716`, `src-app/src/app/workspace_ops/tab.rs:239-346`, `src-app/src/app/hosted_sessions.rs:150-330`, `src-app/src/app/quit_dialog.rs:218-510`, `src-app/src/terminal/pty_session.rs:883-920`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Une fermeture sans dialogue, donc sans agent actif, garde la session 5 s au lieu de l'arrêter ; un undo dans ce délai rattache la même session, avec le même processus, le texte, les styles, les liens OSC 8 et le curseur. Test : une session détachée puis rattachée garde son PID, son lien OSC 8, son gras et son curseur.
- [ ] Après 5 s, ou quand l'entrée sort de la pile des 5 fermetures, la session est arrêtée ; l'undo rouvre alors un shell neuf dans le même cwd.
- [ ] Stop dans le dialogue de fermeture arrête immédiatement, et l'undo rouvre un shell neuf ; Detach n'a pas de délai, et l'undo rattache la session. Une fermeture qui ne laisse rien à annuler arrête immédiatement.
- [ ] Une session gardée n'apparaît ni dans la barre latérale ni dans le décompte du dialogue de sortie, et quitter l'app l'arrête, quel que soit `on_quit`.
- [ ] Le toast de réouverture d'un onglet reste affiché pendant tout le délai de garde.
- [ ] `capture_replay`, `restore_replay`, `TerminalExtra::replay` et le budget de 2 MiB des fermetures sont supprimés ; fermer un pane n'envoie plus de requête synchrone au runtime.
- [ ] Échec : given une session détachée déjà rouverte depuis la barre latérale, when on annule sa fermeture, then aucun second rattachement n'a lieu, un shell neuf s'ouvre et un toast l'explique.

---

### EP-004: Libérer le thread de rendu et fiabiliser l'IPC et la CLI

Sort du thread GPUI toutes les requêtes runtime, I/O et sondes, et donne à l'IPC et à la CLI un contrat strict : entrée mal typée rejetée, erreur signalée comme erreur, mutations IPC alignées sur celles de l'UI.

**Definition of Done:**
- Aucun handler IPC ni gestionnaire d'entrée n'exécute de requête runtime, d'I/O fichier ou de sous-processus sur le thread GPUI dans les chemins listés.
- Toute entrée IPC mal typée reçoit -32602.
- `paneflow wait` et `flow` distinguent un échec transitoire d'une vraie panne.

#### US-024: Répondre aux lectures IPC hors du thread GPUI
**Description:** En tant qu'orchestrateur qui interroge des panes en continu, je veux que mes lectures ne figent jamais l'interface et qu'un timeout soit une erreur afin de ne dégrader ni l'humain ni mes décisions. Sources : fork #29 (3d1446af), #362 et #363 (5f43ee4d, 8bacf38c), #704 ; `src-app/src/app/ipc_handler/surface_methods.rs:637-668,750-765`, `src-app/src/app/ipc_handler/workspace_methods.rs:494`, `src-app/src/terminal/ghostty_session/mod.rs:934-1109`, `src-app/src/layout/serde.rs:47-49`, `src-app/src/ipc.rs:1091-1123`, `src-app/src/app/ipc_handler/mod.rs:77-92`, `src-app/src/app/workspace_ops/mod.rs:137`.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le dispatcher IPC permet à un handler de répondre de façon différée.
- [ ] `surface.read`, `surface.search` et `workspace.current` résolvent leur cible sur le thread GPUI et exécutent leurs requêtes runtime dans une tâche d'arrière-plan.
- [ ] `surface.read` ne demande au runtime que la fenêtre de lignes demandée ; `surface.search` transmet `max_matches` au moteur.
- [ ] `workspace.current` utilise `serialize_layout_without_scrollback`.
- [ ] Un timeout runtime renvoie une erreur JSON-RPC documentée, jamais un texte vide avec `eof: true` ni une liste de résultats vide.
- [ ] La capture de scrollback à la fermeture d'un pane et à la sauvegarde du layout ne bloque plus le thread GPUI.
- [ ] Test : avec un runtime terminal bloqué, la partie de `surface.read` et `surface.search` exécutée sur le thread GPUI se termine en moins de 50 ms, et la réponse arrive depuis l'arrière-plan une fois le runtime libéré (v1.3 ; la mesure sous 10 MiB de sortie devient une vérification facultative de qualification de release).
- [ ] Échec : given un client qui se déconnecte avant la réponse différée, then la tâche est abandonnée sans panique ni fuite.

#### US-025: Respecter le contrat des réponses read et search
**Description:** En tant qu'orchestrateur, je veux que `truncated`, les lignes renvoyées et la taille des réponses soient fiables afin de ne jamais conclure à tort qu'un motif est absent. Sources : fork #653 et #654 (08b30e9b), #884 (78a2ecc5), #920 (e11aa60b), #1060 ; `src-app/src/terminal/ghostty_session/mod.rs:961-1034`, `crates/paneflow-ipc-client/src/scrollback.rs:1-39`, `crates/paneflow-ipc-client/src/lib.rs:28,135-139`, `src-app/src/app/ipc_handler/surface_methods.rs:689-698,750-765`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [ ] `truncated` n'est vrai que s'il existe d'autres correspondances au-delà de `max_matches`.
- [ ] Chaque ligne renvoyée par `surface.search` est le texte capturé pendant le scan ; une sortie survenue entre-temps ne décale pas les lignes (test avec sortie concurrente).
- [ ] Une recherche remplacée par une autre renvoie une erreur d'annulation, pas `truncated`.
- [ ] Le budget de `surface.read` et de `surface.search` porte sur l'enveloppe JSON sérialisée ; test : 240 KiB de guillemets et d'antislashs donnent une réponse sous 256 KiB marquée tronquée.
- [ ] Échec : given un échec runtime pendant une recherche, then la réponse est une erreur, pas un résultat vide.

#### US-026: Sortir les sondes de fichiers et de git du thread GPUI
**Description:** En tant que développeur dont certains dossiers sont sur un montage réseau, je veux que créer, restaurer ou scinder un workspace ne fige jamais l'interface afin qu'un montage mort ne bloque pas Paneflow. Sources : fork #358 (f886b03c), #400 (b056d111), #291 et #705 (ef8a9e4a, 7d30b638), #254, #403, #310 ; `src-app/src/app/ipc_handler/workspace_methods.rs:47-51,137-155,515-531`, `src-app/src/app/ipc_handler/surface_methods.rs:980-986`, `src-app/src/workspace/mod.rs:141-152`, `src-app/src/workspace/git.rs:156-238`, `src-app/src/app/session.rs:463-564,734-761,902-905`, `src-app/src/app/bootstrap.rs:51-67`, `src-app/src/app/welcome.rs:121`, `src-app/src/app/recents.rs:75`, `src-app/src/diff/git.rs:188-211`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] La canonisation du cwd de `workspace.create`, `workspace.up` et `surface.split` s'exécute en arrière-plan avec un délai de 2 s ; un délai dépassé renvoie une erreur IPC.
- [ ] La découverte git et la lecture de HEAD à la création d'un workspace s'exécutent hors du thread GPUI ; le workspace apparaît immédiatement et ses informations git arrivent ensuite.
- [ ] La restauration de session sonde les cwd et worktrees sauvegardés hors du thread GPUI, avec un délai global de 2 s.
- [ ] Un chemin non résolu dans ce délai garde sa valeur sauvegardée et est sondé de nouveau plus tard.
- [ ] Les listes de dossiers récents ne font aucun `is_dir` synchrone au rendu.
- [ ] Le confinement du cwd d'un nouveau pane dans le worktree de son onglet (`Tab::confine_cwd`, appelé par `new_terminal_cwd` et `surface.split`) ne canonise plus de chemin sur le thread GPUI ; la résolution a lieu hors du thread, et un chemin non résolu dans le délai retombe sur la racine du worktree.
- [ ] Les lectures de fichiers du working tree du dock ont un délai de 10 s.
- [ ] Test : la restauration ne sonde aucun chemin sur le thread GPUI, et la sonde d'arrière-plan abandonne au délai de 2 s un chemin qui bloque 30 s (v1.3 ; la mesure `scripts/bench-startup` avec un chemin injoignable devient une vérification facultative de qualification de release).
- [ ] Échec : given un chemin sauvegardé qui n'existe plus, then le workspace est restauré sur le dossier personnel avec un toast, comme aujourd'hui.

#### US-027: Sortir les I/O de config, les ouvertures externes et l'import du PATH du thread GPUI
**Description:** En tant que développeur, je veux que les réglages, les ouvertures de liens et l'import du PATH ne bloquent ni le rendu ni le démarrage afin que Paneflow reste réactif et que mes outils soient dans le PATH des panes. Sources : fork #908 (8aaaf0ff), #298 (c7f2b51f), #314 (b20fc8db), #906 (c63617e7), #698 (16594629), #712 (afa8681b), #683 (f71749e6, 2f16f80f), #313 (91037b23), #156 ; `src-app/src/app/sidebar/customize_menu.rs:53`, `src-app/src/app/theme_selection.rs:63`, `src-app/src/app/quit_dialog.rs:431`, `src-app/src/app/settings.rs:76,94,322,361`, `src-app/src/settings/chrome.rs:594`, `src-app/src/pane.rs:348`, `src-app/src/terminal/view.rs:468`, `src-app/src/terminal/pty_session/spawn_env.rs:242`, `src-app/src/markdown/state.rs:38-44,121-146`, `src-app/src/markdown/view.rs:62,181-209`, `src-app/src/external_open.rs:83-85`, `src-app/src/editor.rs:147-158,230-275`, `src-app/src/login_shell_env.rs:55-123`, `src-app/src/launch.rs:268-277`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Aucun `load_config()` ni aucune écriture de `paneflow.json` n'a lieu sur le thread GPUI dans les chemins listés ; les nouveaux panes lisent `cached_config`, y compris les valeurs pas encore écrites.
- [ ] `markdown_state.json` est lu et écrit hors du thread GPUI et plafonné à 1 000 entrées, les moins récemment utilisées étant évincées ; un fichier trop grand est réduit à ses entrées récentes, jamais remis à zéro.
- [ ] Sous Linux, l'ouverture d'URL et de fichiers utilise `open::that_detached` ; un code de sortie non nul du lanceur, quand il est observable, est signalé par un toast.
- [ ] Le repli d'éditeur ne lance plus un éditeur terminal (vim, nvim, hx, emacs) sans terminal et passe au gestionnaire de l'OS.
- [ ] L'import du PATH attend la fin du processus shell (délai 5 s) plutôt que la fermeture de stdout, et lit au plus 1 MiB.
- [ ] Une ligne `PATH=` complète déjà reçue est adoptée même quand le délai expire.
- [ ] Test : un rc qui lance `sleep 30 &` n'ajoute pas plus de 500 ms au démarrage, et le PATH du login shell est adopté.
- [ ] Échec : given un rc qui écrit sans fin sur stdout, then la lecture s'arrête à 1 MiB, le PATH hérité est conservé et un avertissement est journalisé.

#### US-028: Rejeter toute entrée IPC mal typée
**Description:** En tant qu'orchestrateur, je veux qu'un paramètre mal typé soit rejeté au lieu d'être remplacé par une valeur par défaut afin qu'une erreur de script ne ferme ni ne pilote jamais le mauvais workspace ou pane. Sources : fork #1020, #1021, #1022 et #1023 (3726879a, ba4ed6c7), #1046 (c090e70f), #281 (d430d96a), #285 (749dc84e), #286 (60b01e1c), #279 (dfc61380), #284 (fe04b566), #53 (1cfee6c7), 07360a6d ; `src-app/src/app/ipc_handler/surface_methods.rs:267-307,502-543,645-688,826-953,1005-1017`, `src-app/src/app/ipc_handler/workspace_methods.rs:25-42,563-615`, `src-app/src/app/ipc_handler/jsonrpc.rs:62-68`, `src-app/src/ipc.rs:110,670,758-773`, `src-app/src/cli/read_cmds.rs:74`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Des helpers typés renvoient -32602 pour les paramètres présents mais mal typés, avant toute barrière d'autorisation. Paramètres concernés : `surface_id`, `workspace_id`, `index`, `text`, `submit`, `lines`, `offset`, `keystroke`, `env`, `profile` et `name`.
- [ ] `workspace.close` et `workspace.select` sans `index` valide renvoient -32602 et ne ciblent jamais le workspace actif par défaut.
- [ ] Un `surface_id` en chaîne, négatif ou décimal renvoie -32602 au lieu de cibler le pane actif.
- [ ] `env` doit être un objet dont toutes les valeurs sont des chaînes ; `profile` doit être une valeur connue.
- [ ] Les erreurs imputables au client (index hors bornes, dernier workspace, id de focus inconnu) renvoient -32602 au lieu de -32603.
- [ ] Les erreurs de paramètres de `events.subscribe` reprennent l'id de la requête.
- [ ] `PANEFLOW_ALLOW_MULTIPLE` n'est actif que pour la valeur `1`.
- [ ] Échec : given `{"text": 5, "submit": true}`, then aucun octet n'est écrit dans le PTY et la réponse est -32602.

#### US-029: Aligner les mutations IPC sur le comportement de l'UI
**Description:** En tant qu'orchestrateur, je veux que fermer, créer ou scinder par IPC respecte les mêmes garde-fous que l'UI afin qu'un script ne contourne ni la confirmation, ni la barrière d'orchestration, ni l'isolation des worktrees. Sources : fork #21 (4164b027), #596 (8f387338), #347 (f0688146), #130 (400f1b19), 6e3e6cd7, #46 (a00b159a), 19c81752, #236 (fc2c0211) ; `src-app/src/app/ipc_handler/workspace_methods.rs:370,433,506-615`, `src-app/src/app/workspace_ops/layout.rs:79-129`, `src-app/src/app/session.rs:566-627`, `src-app/src/app/ipc_handler/surface_methods.rs:865-873,910-956,1025-1072`, `src-app/src/workspace/tab.rs:49-57,99-121`, `src-app/src/app/event_handlers/mod.rs:279-317`, `src-app/src/terminal/pty_session.rs:725-830`, `src-app/src/terminal/view.rs:658-716`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-028

**Acceptance Criteria:**
- [ ] `workspace.close` passe par le même pipeline que l'UI : `request_close` et `perform_close`, `stop_terminals`, ajustement de `active_idx` et nettoyage des états.
- [ ] Quand une confirmation serait requise (agents vivants, buffers sales), `workspace.close` renvoie une erreur au lieu de fermer.
- [ ] Les surfaces d'un `layout` fourni à `workspace.create` ou `workspace.restore_layout` passent par la même barrière que les specs de pane (`env`, `session`, `scrollback`) ; sans `PANEFLOW_IPC_ORCHESTRATION=1`, ces champs sont refusés.
- [ ] `workspace.restore_layout` ne supprime aucun pane de l'onglet actif sans passer par la politique de fermeture.
- [ ] Un `cwd` explicite de `surface.split` est honoré, ou refusé en -32602 s'il sort du worktree lié ; la comparaison porte sur des chemins canonisés des deux côtés.
- [ ] `surface.split` et le dépôt d'une session sur un onglet zoomé sont refusés (« Unzoom before splitting panes ») ; `pane_count` tient compte de `saved_layout`.
- [ ] `send_text` et `send_keystroke` renvoient une erreur quand l'écriture est rejetée (`BackendInputResult::Rejected`) ou quand aucun octet n'est produit.
- [ ] Un `prompt` contenant CR ou LF est livré par bracketed paste.
- [ ] Échec : given `paneflow flow run` avec 3 unités sur 3 worktrees, lancé depuis un onglet lié au worktree A, then chaque unité démarre dans son propre worktree (test).

#### US-030: Corriger la sémantique secondaire de l'IPC et les métadonnées de pane
**Description:** En tant qu'orchestrateur, je veux que chaque méthode IPC fasse ce que sa documentation annonce et que les panes soient nommés d'après l'agent qu'ils exécutent afin de piloter la flotte sans contournement. Sources : fork #44 (75bb8101), #40 (56ce6524), #1052, #283 (6876e9d2, b7c0511a), #280 (45b77e93), #38 (6813489d), #441 et #720 (20bd297e, 514fb874), #929 (2370a89a), #679, #687 (267d6f2a), 4125ac98 ; `src-app/src/app/ipc_handler/workspace_methods.rs:86-101,277,322,523-555`, `src-app/src/app/ipc_handler/surface_methods.rs:132-151,238,444-447,572,631-635,780-813`, `src-app/src/ipc.rs:712-719,1091-1123`, `src-app/src/workspace/ports.rs:219-236,419-461,561-581`, `src-app/src/terminal/pty_session.rs:704-722`, `src-app/src/terminal/service_detector.rs:287-332`, `src-app/src/app/event_handlers/pane_scan.rs:186-191`.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-028

**Acceptance Criteria:**
- [ ] `workspace.create` avec un layout ne pré-crée pas de pane par défaut ; le premier leaf respecte sa spec.
- [ ] `workspace.up` avec `focus = true` donne le focus au pane demandé, quel que soit le preset.
- [ ] `surface.focus` sur un pane masqué par le zoom sort du zoom avant de donner le focus.
- [ ] `system.capabilities.scripting` reflète la barrière effective, `ai_unrestricted` compris.
- [ ] `surface.list` filtré par workspace renvoie l'index et le nombre de panes de ce workspace.
- [ ] L'annulation et le démarrage d'une requête IPC partagent un état atomique mis à jour par compare-and-swap ; l'attente après démarrage est bornée.
- [ ] Le nom de surface privilégie l'agent confirmé par le scan, sinon le processus de premier plan du terminal.
- [ ] La détection de ports macOS n'ignore plus les descripteurs au-delà du 1 024e.
- [ ] Une puce de service n'emprunte pas le label d'un autre port, n'accepte que des adresses loopback valides et ouvre le port détecté.
- [ ] Échec : given une requête qui expire exactement au délai de 5 s, then aucune mutation n'est exécutée après la réponse -32002 (test).

#### US-031: Rendre `paneflow wait` et `flow` robustes aux erreurs transitoires
**Description:** En tant qu'orchestrateur, je veux que les attentes et les flows tolèrent un hoquet de l'IPC sans conclure à tort, afin qu'un script ne s'arrête ni ne continue au mauvais moment. Sources : fork #25 (cc7a165b), #131, #132 et #133 (400f1b19), #252 (0b60e7f0), #162, #164 et #165 (c0d6f34c), #282 (f181a766), #26 (e57d6e7a) ; `src-app/src/cli/wait_cmd.rs:51-54,65,152-173,272-356`, `src-app/src/cli/flow_cmd.rs:298-300,444,743-758`, `src-app/src/cli/surface_read.rs:28-42`, `src-app/src/cli/read_cmds.rs:74`, `crates/paneflow-ipc-client/src/lib.rs:53-99`, `crates/paneflow-ipc-client/src/line_wire.rs:78-83`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [ ] Seul un pane disparu (`Ok(None)`) permet de se passer de la baseline ; une erreur de lecture de la baseline est réessayée 3 fois, puis fait échouer la commande.
- [ ] Le client IPC distingue un échec de connexion, un timeout et une réponse busy (-32000).
- [ ] Busy et timeout sont réessayés 3 fois avec un backoff de 250 ms à 2 s et comptent comme un poll sauté ; un échec de connexion reste une erreur immédiate.
- [ ] `wait --idle` sur un pane disparu échoue avec un code non nul.
- [ ] Le nouveau texte est calculé par un diff ordonné par chevauchement : un second « DONE » imprimé dans le même pane est détecté, et le test existant de suppression de l'écho du prompt passe toujours.
- [ ] `paneflow search --human` affiche les numéros de ligne négatifs du scrollback.
- [ ] La connexion Unix du client respecte le délai demandé.
- [ ] Échec : given Paneflow arrêté, then `paneflow wait` échoue immédiatement avec « IPC unreachable », sans réessai.

---

### EP-005: Réparer l'intégration agents, MCP et hooks

Rend le bridge MCP fonctionnel sous chaque agent, empêche les shims de se lancer mutuellement, ferme la portée MCP, respecte les emplacements de config des agents et rend exact l'état des sessions agent.

**Definition of Done:**
- Le bridge MCP démarre sous Claude Code et sous Codex sur les trois OS.
- Un Paneflow imbriqué ne crée pas de boucle de processus.
- Sans preuve de pane, la portée MCP est refusée.
- Une réinstallation ne perd aucune clé de l'utilisateur.

#### US-032: Reproduire l'échec du bridge MCP sous Codex
**Description:** En tant que mainteneur, je veux mesurer à l'exécution ce que le bridge MCP reçoit sous Codex afin de confirmer le défaut avant de modifier les entrées MCP. Sources : ed4a3aad, fork #411 (6506e187) ; `crates/paneflow-mcp-install/src/agents/support.rs:201-216`, `crates/paneflow-mcp/src/main.rs:28-66`, `crates/paneflow-mcp/src/scope.rs:44-80`, `crates/paneflow-ipc-client/src/host_control.rs:78-99`.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Après `paneflow mcp install`, Codex lancé dans un pane Paneflow avec la fenêtre ouverte liste les outils Paneflow ou montre l'erreur du bridge ; le stderr du bridge est capturé.
- [ ] La même vérification est faite fenêtre fermée (chemin host), sur au moins deux OS parmi Linux, macOS et Windows.
- [ ] La liste des noms de variables reçues par le bridge est consignée, sans aucune valeur.
- [ ] Échec : si le bridge fonctionne dans tous les cas, la preuve est consignée et US-033 est réduite à la préservation des clés utilisateur, en P1.

#### US-033: Écrire des entrées MCP complètes sans écraser celles de l'utilisateur
**Description:** En tant que développeur qui utilise Codex et règle ses serveurs MCP, je veux que Paneflow écrive une entrée qui fonctionne et conserve mes réglages afin que le bridge démarre sans perdre ma configuration. Sources : ed4a3aad, fork #411 (6506e187), #42 (8a970f2e), #93 (31a76f5a), #214, #648, #680 ; `crates/paneflow-mcp-install/src/merge.rs:98-131`, `crates/paneflow-mcp-install/src/integrations.rs:548-582`, `crates/paneflow-mcp-install/src/agents/support.rs:201-216`, `crates/paneflow-agent-config/src/jsonc.rs:62-66,214-216`, `crates/paneflow-serve/src/worker.rs:165-189`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-032

**Acceptance Criteria:**
- [ ] L'entrée Codex `[mcp_servers.paneflow]` contient `env_vars` avec `PANEFLOW_SOCKET_PATH`, `PANEFLOW_HOST_ENDPOINT`, `PANEFLOW_HOME`, `PANEFLOW_WORKSPACE_ID`, `PANEFLOW_SURFACE_ID` et `PANEFLOW_SESSION_ID`, fusionnés avec les entrées existantes de l'utilisateur.
- [ ] `paneflow mcp status` rapporte « needs repair » quand cette liste manque ou est incomplète.
- [ ] Install, Repair et le rafraîchissement du worker mettent à jour `command` et `args` en place et conservent toutes les autres clés (`startup_timeout_sec`, `tool_timeout_sec`, `env`, `enabled`), en TOML comme en JSON (tests).
- [ ] `enabled = false` posé par l'utilisateur n'est pas réactivé par le rafraîchissement du worker.
- [ ] Les opérations JSONC ciblent la dernière occurrence d'une clé dupliquée ; une assertion après chaque splice vérifie que seule l'entrée visée a changé sémantiquement.
- [ ] Échec : given une config Codex invalide, then l'installation est refusée sans modifier le fichier.

#### US-034: Reproduire la boucle des shims dans un Paneflow imbriqué
**Description:** En tant que mainteneur qui développe Paneflow depuis un pane, je veux reproduire à l'exécution la résolution mutuelle des shims afin de corriger la cause réelle. Sources : fork #871 (49bcd918) ; `crates/paneflow-shim/src/detect.rs:40-80`, `crates/paneflow-host/src/helpers.rs:62-72`, `crates/paneflow-host/src/host/spawn_env.rs:138-156`, `src-app/src/terminal/pty_session/spawn_env.rs:45-60`, `src-app/src/runtime_paths.rs:92-108`.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Depuis un pane d'un Paneflow release, `scripts/dev.sh --tag nested` (ou `dev.ps1 -Tag nested`) lance une instance. Dans un de ses panes, `claude --version` s'exécute sous un plafond de processus : `ulimit -u` sous Unix, Job Object sous Windows.
- [ ] Le PATH du pane imbriqué et le nombre de processus shim créés en 10 s sont consignés.
- [ ] Le test est exécuté sous Windows et sur au moins un OS Unix.
- [ ] Échec : si aucune boucle n'apparaît, le PATH observé est consigné et US-035 conserve uniquement ses critères d'hygiène des helpers.

#### US-035: Empêcher un shim de lancer un autre shim et assainir les dossiers helpers
**Description:** En tant que mainteneur, je veux qu'un shim trouve toujours le vrai binaire de l'agent et que les dossiers helpers restent propres afin qu'une instance imbriquée ne sature jamais la session. Sources : fork #871 (49bcd918), #894 (96f02cfd), 3ce1d640, b8aba9a1, #442 (5ed99746, 3f2dface, 2d155bee) ; `crates/paneflow-shim/src/detect.rs:55-73`, `src-app/src/ai_hooks/extract.rs:75,197-265`, `src-app/src/runtime_paths.rs:92-108`, `crates/paneflow-host/src/helpers.rs:62-72`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-034

**Acceptance Criteria:**
- [ ] `find_real_binary_in` ignore tout dossier du PATH qui contient aussi `paneflow-ai-hook` (`.exe` sous Windows).
- [ ] Un shim refuse d'exécuter un binaire quand la variable exportée par `run_real` montre qu'il a été lancé par un autre shim.
- [ ] Test : deux dossiers helpers distincts dans le PATH, chacun avec un shim `claude`, puis un vrai `claude` plus loin ; le shim résout le vrai binaire.
- [ ] L'abandon du contexte d'instance retire aussi du PATH l'entrée de `PANEFLOW_BIN_DIR` héritée.
- [ ] Sous Unix, le test d'exécutabilité utilise `access(X_OK)`.
- [ ] Un helper extrait dont le contenu est à jour mais qui a perdu son bit exécutable retrouve le mode 0755.
- [ ] Les anciens dossiers `cache/bin/<version>` sont supprimés au démarrage, sauf ceux qui figurent dans le PATH d'une session hébergée vivante.
- [ ] La taille de `paneflow-shim` reste sous 512 KiB.
- [ ] Échec : given aucun vrai binaire dans le PATH, then le shim échoue avec le code 127 et un message qui nomme l'outil, sans boucle.

#### US-036: Faire échouer la portée MCP en mode fermé et respecter le protocole
**Description:** En tant que développeur, je veux qu'un agent ne voie que les panes de son workspace et que le bridge accepte les requêtes MCP conformes afin que l'isolation annoncée soit réelle et que chaque client fonctionne. Sources : fork #20 (fc625f3d, 0ccf6f30), #412 (4f234813), #250 (0cd118a7), #249, #251, #312 (08034286), #151, #61 (39c9d52a) ; `crates/paneflow-mcp/src/scope.rs:44-80`, `crates/paneflow-mcp/src/main.rs:55-66`, `crates/paneflow-mcp/src/bridge.rs:149-220`, `crates/paneflow-mcp/src/tools.rs:74-146`, `crates/paneflow-mcp/src/mcp.rs:55-65,163`, `crates/paneflow-mcp/src/resources.rs:46-69`, `crates/paneflow-host/src/control.rs:139,245-313`, `src-app/src/terminal/pty_session/spawn_env.rs:145-158`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Sur le chemin host, sans preuve de pane hébergé (`PANEFLOW_SESSION_ID`), la portée est refusée au lieu de valoir « tous les workspaces ».
- [ ] Le host filtre `surface.list`, `surface.read` et `surface.search` par workspace côté serveur ; une cible numérique hors portée renvoie une erreur.
- [ ] La portée d'un bridge lancé dans un pane est dérivée côté serveur de la surface appelante et de son appartenance actuelle ; après le déplacement de l'onglet ou un redémarrage de l'app, le bridge voit les panes du workspace actuel.
- [ ] `tools/call` et `resources/read` acceptent et ignorent `params._meta`.
- [ ] `resources/templates/list` est implémenté.
- [ ] Un outil inconnu renvoie une erreur JSON-RPC.
- [ ] `resources/read` accepte une pagination.
- [ ] La boucle stdio lit des lignes bornées à 1 MiB, répond -32700 à une ligne trop longue et continue de servir.
- [ ] La taille de `paneflow-mcp` reste sous 512 KiB.
- [ ] Échec : given un bridge lancé hors de tout pane avec la fenêtre fermée, then `list_panes` renvoie une erreur de portée.

#### US-037: Respecter les emplacements et formats de config des agents
**Description:** En tant que développeur qui utilise plusieurs comptes ou des dossiers de config personnalisés, je veux que hooks, MCP et sessions suivent le dossier de config réel de chaque agent afin que chaque profil soit intégré. Sources : fork #31 (af91213f), #253, #292 (36d10049), #233 (403f7ea9), #874 (35c77058), #1054, #60 (a04ad9bd), #216 (805c2c7a), #544, #662 ; `crates/paneflow-mcp-install/src/integrations.rs:79-89,705-706`, `crates/paneflow-mcp-install/src/agents/support.rs:12-92`, `crates/paneflow-mcp-install/src/hook_command.rs:17-42`, `src-app/src/claude_sessions.rs:38-42`, `crates/paneflow-agent-config/src/io.rs:5-13`, `crates/paneflow-agent-config/src/jsonc.rs:23-28,265-351`, `crates/paneflow-mcp-install/src/merge.rs:16-41`, `src-app/src/runtime_paths.rs:208-215`, `src-app/src/agent_launcher.rs:452-463`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-033

**Acceptance Criteria:**
- [ ] Un résolveur unique calcule le dossier de config Claude à partir d'une table d'environnement ; les hooks, l'entrée MCP et la lecture des sessions l'utilisent.
- [ ] Pour chaque `CLAUDE_CONFIG_DIR` distinct déclaré par un profil d'agent, hooks et entrée MCP sont installés dans ce dossier, et Settings affiche l'état de chaque dossier.
- [ ] Une valeur relative de `CLAUDE_CONFIG_DIR`, `CODEX_HOME` ou `OPENCODE_CONFIG_DIR` est rejetée avec un avertissement.
- [ ] Avec `OPENCODE_CONFIG_DIR`, l'entrée est écrite dans `$OPENCODE_CONFIG_DIR/opencode.json(c)`, sans segment `opencode/` supplémentaire.
- [ ] Le `settings.json` de Gemini est lu et modifié comme du JSONC.
- [ ] Le parser JSONC refuse une profondeur de plus de 128 niveaux avec une erreur, sans débordement de pile.
- [ ] Un build debug refuse un `mcp install` durable, sauf avec le drapeau explicite `--force-dev`.
- [ ] Les commandes de hook Codex utilisent le quoting de `hook_command.rs` ; test avec un chemin de profil Windows qui contient un espace.
- [ ] Les blocs de hooks écrits par les versions antérieures à 0.17 dans `<projet>/.claude/settings.local.json` sont retirés pour les workspaces présents dans `session.json`.
- [ ] Échec : given un `opencode.jsonc` imbriqué sur 10 000 niveaux, when Settings s'ouvre, then l'app reste ouverte et affiche une erreur d'analyse.

#### US-038: Rendre l'état des sessions agent exact
**Description:** En tant que développeur qui fait tourner plusieurs agents du même outil, je veux que chaque ligne d'agent reflète son agent et son workspace réels afin que la sidebar, l'attention et les dialogues de fermeture disent vrai. Sources : fork #886 (9a98b4cd), #831, 1b9b8c2d, f6681018, #488 (ed5a5302), #934 (8b38aeab, 1e22d40e), #28 (a5234a06), 07360a6d, #414 (16f00e8e), #515 (87c506a4), #196 (a7d93fc8) ; `src-app/src/app/ipc_handler/agent_frames.rs:102-238,344-354,597-613,899-1022`, `src-app/src/app/event_handlers/session_reaper.rs:36-97,179`, `src-app/src/app/host_agents.rs:235-261,455-503`, `src-app/src/app/workspace_ops/tab.rs:479-535`, `src-app/src/workspace/mod.rs:34-81`, `crates/paneflow-ipc-client/src/agent.rs:69-79`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `ai.session_end` avec un PID réel sans ligne correspondante ne retire aucune autre ligne ; le repli ne s'applique qu'aux frames sans PID ou aux lignes synthétiques.
- [ ] Une frame sans PID se lie d'abord par `surface_id` validé et n'accepte qu'un candidat unique.
- [ ] Une ligne Errored est évincée quand un autre outil se lie à la même surface ; une ligne Errored qui se lie tardivement n'évince pas un jumeau vivant.
- [ ] Le minuteur de 5 s après un stop porte un compteur de tour et ne retire pas une ligne Finished plus récente.
- [ ] L'heure de démarrage du processus est comparée : un PID recyclé ne rejoint pas une ligne morte, et une heure absente n'est plus traitée comme la preuve d'un processus vivant.
- [ ] Un agent en attente sans message affiche l'anneau et le point d'attention.
- [ ] Une ligne en attente d'entrée n'est remplacée par une source de rang inférieur qu'après être sortie de cet état.
- [ ] Déplacer un onglet déplace ses lignes d'agent et ses marques de complétion non lues vers le workspace de destination ; les dialogues de fermeture et de quit ne comptent jamais un agent deux fois.
- [ ] Échec : given deux sessions Claude dans un workspace, when le pane A est fermé, then la ligne de B reste affichée (test).

#### US-039: Fiabiliser la lecture et le scan des sessions d'agents
**Description:** En tant que développeur qui reprend des sessions depuis la sidebar, je veux que toutes mes sessions récentes soient listées sans scans en double afin de pouvoir les reprendre de façon fiable. Sources : fork #888 (10c719a3), #713 (a2f1ff7e), #718 (648786f1), #889 (9df048c5), #907 (ef0e1460), #311 (8296d9de), #302 (cdb876fe), #137 (400f1b19), #905 (46259575) ; `src-app/src/claude_sessions.rs:84-117,165-193`, `src-app/src/pi_sessions.rs:40-72,152-199`, `src-app/src/command_sessions.rs:139-162`, `src-app/src/agent_sessions.rs:461-483`, `src-app/src/app/sessions_sidebar.rs:19-111`, `src-app/src/opencode_sessions.rs:11-31`, `src-app/src/agent_launcher.rs:285-322`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les lecteurs Claude et Pi lisent avec `read_until` puis décodent en lossy ; une ligne coupée au milieu d'un caractère UTF-8 à la frontière des 64 KiB ne fait plus disparaître la session (test avec un emoji).
- [ ] Une fois l'enveloppe trouvée, une fin de fichier au milieu d'une ligne trop longue n'annule pas la session.
- [ ] Le cache Claude est indexé par l'empreinte prise avant le scan.
- [ ] Les sessions Gemini sont triées des plus récentes aux plus anciennes avant l'application du plafond.
- [ ] Un scan en cours pour le même agent et le même cwd n'est pas relancé au changement de workspace.
- [ ] La découverte Pi est plafonnée à 10 000 entrées et 8 MiB lus.
- [ ] `opencode session list` s'exécute avec le cwd du workspace, et une sortie invalide est journalisée.
- [ ] La sonde des agents installés a un délai de 5 s ; un délai dépassé termine le scan avec l'état déjà connu.
- [ ] Échec : given un fichier de session illisible, then les autres sessions restent listées et une ligne de log nomme le fichier.

---

### EP-006: Corriger UI, focus, accessibilité, config, mise à jour et CI

Fait porter les actions sur tous les onglets et rend le focus là où il était. Rend les contrôles accessibles et les erreurs de config visibles. Fiabilise la mise à jour et durcit la chaîne de release.

**Definition of Done:**
- Aucune action multi-agents n'ignore un onglet inactif.
- Tout overlay rend le focus à son origine.
- Chaque helper interactif partagé exige un libellé accessible.
- Les erreurs de config sont journalisées sur les trois OS.
- Les téléchargements de mise à jour survivent à une connexion lente.
- Toutes les actions de release sont épinglées.

#### US-040: Faire porter les actions multi-agents sur tous les onglets
**Description:** En tant que développeur qui répartit ses agents dans plusieurs onglets, je veux que broadcast, sauts, file d'attention et recherche voient tous les onglets afin qu'aucun agent en attente ne soit invisible. Sources : fork #293 (c68a17d4), #294 (db37310e), #601, #602, #722 (84a4609d), #723 (d5b316f9), #48 (5c775339), #702 (4818e7d0), #140 (400f1b19), #890 (7c230395), #935 (dd5f9a6b), #936 (4fd2f318), #113 (6461ba8e), #347 et #348 ; `src-app/src/app/broadcast.rs:93-130,315-326`, `src-app/src/app/workspace_ops/focus.rs:118-154`, `src-app/src/app/attention_queue.rs:41-61`, `src-app/src/app/fleet_search.rs:42,164`, `src-app/src/app/composer.rs:313-324`, `src-app/src/app/window.rs:112-116,143-146`, `src-app/src/app/workspace_ops/mod.rs:183-206,665-713,802-981`, `src-app/src/app/workspace_ops/tab.rs:172-189,289-349`, `src-app/src/app/sidebar/tab_row.rs:207-211`, `src-app/src/app/sidebar/mod.rs:231-260`, `src-app/src/main.rs:163`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** US-048

**Acceptance Criteria:**
- [ ] Broadcast, jump-to-waiting, attention queue, fleet search, badges et chips parcourent tous les onglets, y compris les panes masqués par le zoom.
- [ ] Un saut active l'onglet cible et sort du zoom avant de donner le focus.
- [ ] La synchronisation du broadcast ne retire plus les membres situés dans un onglet inactif.
- [ ] L'undo-close d'un pane résout le workspace par id et refuse, avec un toast, un onglet zoomé ou au plafond de panes.
- [ ] L'undo-close d'un pane confine le cwd au worktree lié à l'onglet.
- [ ] Le renommage d'onglet, le menu d'onglet et le dialogue de fermeture référencent des ids stables.
- [ ] Fermer ou insérer un workspace ou un onglet ne redirige jamais un renommage, un menu ou un dialogue vers un autre onglet.
- [ ] Cmd/Ctrl+1 à 9 et Next Workspace suivent l'ordre affiché par la sidebar.
- [ ] Échec : given un renommage ouvert et la fermeture du workspace par Cmd/Ctrl+Shift+Q, then aucun autre onglet ne reçoit le titre (test).

#### US-041: Restaurer le focus et rendre les notifications in-app fiables
**Description:** En tant que développeur, je veux que la frappe revienne toujours au pane d'où je suis parti et que chaque toast soit visible afin de ne jamais taper dans le mauvais agent ni manquer une erreur. Sources : fork #584 (e76f3883), #110 (2b7c6530), #299 (01aca4af, 3a08ef8f), #471 et #472 (5d616f5d), #506 (8ce939ed), #508 (78bb2c96), #697 (6490b6a6), #910 (915c18e1) ; `src-app/src/app/close_policy.rs:67-81,279-285`, `src-app/src/app/quit_dialog.rs:361-370`, `src-app/src/app/attention_queue.rs:102-111`, `src-app/src/app/fleet_search.rs:176-181,231,271`, `src-app/src/app/render.rs:216-237,706-708`, `src-app/src/main.rs:258,314`, `src-app/src/terminal/input.rs:262-267`, `src-app/src/app/workspace_ops/swap.rs:6-31`, `src-app/src/app/cli_diff_dock.rs:181-262`, `src-app/src/app/notifications.rs:107-147,266,295`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Chaque overlay ou modal mémorise son handle de focus d'origine et le restaure à toute fermeture : attention queue, fleet search, dialogues de fermeture et de quit, broadcast, theme picker, pane palette.
- [ ] Si le handle d'origine n'existe plus, le focus va au pane actif de l'onglet actif.
- [ ] Un listener `on_focus_lost` enregistré au montage rend le focus au pane actif.
- [ ] Masquer la sidebar pendant un renommage valide ou annule ce renommage.
- [ ] Le mode swap est indiqué visuellement, limité à sa fenêtre, tient le pane source par référence faible et se termine quand ce pane se ferme ; hors de ce mode, Escape n'est plus avalé.
- [ ] Le menu contextuel de pane tient une référence faible et se ferme si le pane disparaît.
- [ ] La restauration d'un dock maximisé donne le focus au pane à la fin de l'animation et ignore un pane fermé.
- [ ] Chaque toast a un id unique ; un toast en file s'affiche entièrement.
- [ ] La file de toasts est limitée à 5 et fusionne les doublons consécutifs.
- [ ] Échec : given Cmd/Ctrl+W sur le pane C puis Cancel, then le focus revient sur C (test).

#### US-042: Corriger les défauts macOS, des raccourcis et de l'aide CLI
**Description:** En tant que développeur sur macOS, Linux ou Windows, je veux que les menus, les raccourcis par défaut et l'aide fonctionnent comme annoncé afin de ne plus tomber sur des commandes inertes. Sources : fork #708 (634a8bd1), #10 (0cdaca29), #739 (d16d2a6c), #218 (7736e7e6), #915 (dd335dfc), #112 (cd65f086), #34 (04ab140e) ; `src-app/src/app/macos_menu.rs:49-127`, `src-app/src/keybindings/defaults.rs:68-71,314,415-442`, `src-app/src/keybindings/display.rs:58-164`, `src-app/src/app/settings.rs:226-253`, `src-app/src/settings/tabs/terminal.rs:373`, `src-app/src/app/render.rs:35-85`, `src-app/src/launch.rs:209-235,327`, `src-app/src/cli/mod.rs:26-56`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les actions du menu macOS, Quit compris, fonctionnent fenêtre minimisée, en se rabattant sur la première fenêtre `PaneFlowApp`.
- [ ] Sous macOS, Next Workspace est lié par défaut à `ctrl-tab` et non plus à `cmd-tab`.
- [ ] Un test vérifie qu'aucun raccourci macOS par défaut n'utilise Cmd+Tab ni Cmd+Space.
- [ ] La touche moins s'affiche `-` dans les libellés de raccourcis.
- [ ] Quand une action a plusieurs raccourcis utilisateur, tous sont affichés, dans un ordre stable.
- [ ] Escape avec le menu Preset ouvert ne ferme que ce menu.
- [ ] La saisie de recherche de police ignore `\n` et `\t`, et Entrée sélectionne la police mise en évidence.
- [ ] Le bouton de sidebar, pendant que Settings est ouvert, ne change pas l'état masqué de façon invisible.
- [ ] `paneflow --help` liste les verbes CLI et `paneflow help` fonctionne.
- [ ] Les raccourcis affichés dans l'aide utilisent la touche secondaire de la plateforme.
- [ ] Échec : given un verbe inconnu, then le message d'erreur renvoie vers `paneflow --help`, qui liste effectivement les verbes.

#### US-043: Rendre les contrôles accessibles, lisibles et conformes à Reduce motion
**Description:** En tant que développeur qui utilise un lecteur d'écran, le clavier seul ou Reduce motion, je veux que chaque contrôle ait un nom, un rôle et un contraste suffisant afin de pouvoir utiliser Paneflow sans souris ni animation. Sources : fork #275 (b217fcc7), #316, #317, #320, #321 et #340 (9a6b6719, 58ed4bf4, ebb7b27b, 1583f54e), #361 (e5d20e97), #659 (49523d97), #881 (c8af8490), #882 (221d7b75), #918 (6a0c0158), #658 (daf0712b), #274 (01848e83), #676 (3fc06edc), #322 (d5c32349), #323 (b7fbbb39), #919 (7ad3d1f0), #325 (092082e0), ae5c5f23, #318 (4171e0e0), #916 (2a17a459), #276 (3c7e8df7), #1034 (163d4640), #719 (4a67f369) ; `src-app/src/settings/components.rs:118-185,502-690`, `src-app/src/pane.rs:937-1121,1760-1773`, `src-app/src/ui_primitives.rs:549-815`, `src-app/src/app/window_chrome/csd.rs:319-373`, `src-app/src/app/window_chrome/title_bar.rs:336-365`, `src-app/src/widgets/text_input.rs:645-650`, `src-app/src/widgets/text_area.rs:616-618`, `src-app/src/app/custom_buttons_modal.rs:243-616`, `src-app/src/app/diff_dock/branch.rs:193-203`, `src-app/src/terminal/view.rs:915-997,1483-1491`, `src-app/src/terminal/element/mod.rs:1700`, `src-app/src/app/clone_repo.rs:424-434`, `src-app/src/settings/tabs/mcp.rs:35-51`, `src-app/src/settings/tabs/terminal.rs:627-631`, `src-app/tests/svg_icon_color_policy.rs:24-28`, `src-app/src/app/diff_dock/file_chrome.rs:86-91`, `src-app/src/app/settings.rs:482-486`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les helpers partagés de toggle, select, bouton icône, champ de filtre et contrôles de fenêtre exigent un libellé, qui fournit le rôle, le nom accessible et le tooltip.
- [ ] Les toggles exposent leur état et répondent à Espace et Entrée ; les selects s'ouvrent au clavier.
- [ ] `TextInput` et `TextArea` exposent le rôle de champ texte, un nom et une valeur ; chaque `TextArea` a un id unique.
- [ ] Le point d'état du pane et le badge de zoom ont un libellé textuel ; aucun état n'est transmis par la couleur seule.
- [ ] Le modal des boutons personnalisés et le chip de branche sont utilisables au clavier ; Edit et Delete sont visibles au focus.
- [ ] Les boutons de la barre de recherche du terminal, le badge COPY et la bascule regex ont un rôle, un nom et, pour la bascule, un état pressé.
- [ ] Le spinner d'état vide, la barre de progression du clone et le survol de l'en-tête de pane respectent Reduce motion.
- [ ] Le libellé du bouton Install MCP atteint un contraste d'au moins 4,5:1 sur tous les thèmes livrés (test calculé) ; le chip « uses theme » utilise l'accent du thème.
- [ ] Le test de couleur d'icône SVG ne lit que la chaîne de l'icône elle-même, et le chevron du breadcrumb a sa propre couleur.
- [ ] Les toasts de télémétrie sont en anglais US.
- [ ] Échec : un test énumère les helpers partagés et échoue si l'un d'eux peut être construit sans libellé.

#### US-044: Rendre la config, les journaux et les chemins runtime cohérents
**Description:** En tant que développeur et mainteneur, je veux que les erreurs de config soient journalisées sur chaque OS et que tous les composants trouvent le même endpoint IPC afin de diagnostiquer vite et de ne jamais voir l'IPC « injoignable » à tort. Sources : fork #841, #850 (ece5e037), #867 (fe4ef865), #148 (400f1b19), #33 (4c9642a0), #217 (82b87975), #289 (9259ba3f), #880 (78484f9e) ; `Cargo.toml:57`, `src-app/src/launch.rs:248-251`, `src-app/src/theme/watcher.rs:30-32,263-266`, `src-app/src/app/bootstrap.rs:507`, `src-app/src/terminal/element/font.rs:148`, `crates/paneflow-config/src/schema/layout.rs:335-346`, `crates/paneflow-config/src/schema.rs:212-226`, `schemas/paneflow.schema.json`, `src-app/src/runtime_paths.rs:45-59`, `crates/paneflow-ipc-client/src/lib.rs:629-665`, `crates/paneflow-home/src/lib.rs:155-167`, `src-app/src/settings/search.rs:139-143`, `src-app/src/app/ipc_handler/surface_methods.rs:892,921-960`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les événements `tracing` de paneflow-config et de l'app apparaissent dans le log `env_logger` sous Linux, macOS et Windows.
- [ ] Test : un `paneflow.json` invalide produit une ligne « using defaults » sur chacun des trois OS.
- [ ] Un `paneflow.json` invalide ne change pas le thème actif ; le thème ne suit que le callback du watcher de config.
- [ ] L'avertissement de repli de `font_family` est émis une fois par valeur.
- [ ] Le schéma `surface` contient `path` et `session`, et le test de dérive remplit chaque champ optionnel.
- [ ] La GUI, la CLI, le bridge et le host résolvent l'endpoint IPC par une seule fonction de `paneflow-home`.
- [ ] Sous macOS, cette fonction ignore `XDG_RUNTIME_DIR`.
- [ ] Une valeur inutilisable de `XDG_RUNTIME_DIR` bascule vers le repli documenté, identiquement côté client et côté serveur.
- [ ] Le texte de consentement « AI free access » ne promet plus de journal ; les écritures `surface.send_text` et `surface.send_keystroke` sont journalisées au niveau info.
- [ ] Échec : given `XDG_RUNTIME_DIR` pointant vers un dossier appartenant à root, then l'IPC reste disponible par le repli, et `paneflow send` lancé depuis un terminal externe le trouve.

#### US-045: Fiabiliser la mise à jour
**Description:** En tant qu'utilisateur sur une connexion lente ou une installation non standard, je veux que la mise à jour se télécharge ou me renvoie vers la bonne page afin de ne pas rester sur « Update keeps failing ». Sources : fork #49 (4cca4f8e), #4, #58 (af7f5642) ; `src-app/src/update/verified_download.rs:12-22`, `src-app/src/update/macos/dmg.rs:11,152-157`, `src-app/src/update/windows/msi.rs:14,918-923`, `src-app/src/update/linux/targz.rs:10,90-95`, `src-app/src/update/linux/appimage.rs:11,143-145`, `src-app/src/update/checker.rs:15-56,185-209,303-327,428`, `src-app/src/app/self_update_flow.rs:396-463,565-569`, `src-app/src/update/install_method.rs:525-540`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le téléchargement d'un asset utilise un délai de connexion de 30 s, un délai de réponse de 30 s, un délai de stagnation de 60 s entre deux lectures et un plafond global de 15 min, aligné sur le watchdog ; le flux JSON et `.minisig` gardent 30 s.
- [ ] Test : un serveur local qui sert 60 MB à 256 Ko/s mène le téléchargement à terme, et un serveur qui cesse d'envoyer le fait échouer en 60 s au plus.
- [ ] `TarGz` n'est choisi que sous Linux ; une installation Unknown ou non gérée sous macOS et Windows ouvre la page des releases sans enregistrer d'échec.
- [ ] `html_url` n'est ouvert que s'il est en https sur un hôte GitHub ; sinon, l'URL fixe des releases est utilisée.
- [ ] Les redirections du flux sont limitées et leur cible finale est vérifiée par rapport à la liste des hôtes autorisés.
- [ ] Échec : given une signature minisign invalide, then l'installation est refusée comme aujourd'hui (test de non-régression).

#### US-046: Durcir les workflows de release et de CI
**Description:** En tant que mainteneur, je veux que la chaîne de release n'exécute que des actions épinglées, sans jeton persistant, et que la CI teste chaque changement qui peut casser un test, afin qu'une release signée ne puisse être ni détournée ni cassée en silence. Sources : fork #924, #207, #208, #716 (4dbf074d), #923, #52, #35, #700, #715 (e0469a92), #914 (4d0283ff), #547 (0984da93), #152, #901 (8f6c7c4f), #902 (981f2802) ; `.github/workflows/release.yml:40-41,78,94-105,189-194,244,301-310,526-527,1840,2367-2369,3281-3283`, `.github/workflows/audit.yml:36-38,45,61,90-123`, `.github/workflows/run_tests.yml:80-88,191-197,441-446,812-816`, `.github/workflows/repo_publish.yml:134`, `.github/workflows/update_cask.yml:116`, `scripts/notarize-macos.sh:22-85`, `scripts/sign-macos.sh:104-135`, `scripts/create-dmg.sh:56-66`, `scripts/fetch-libghostty.sh:87-89`, `scripts/fetch-libghostty.ps1:49`, `scripts/bundle-appimage.sh:49`, `scripts/bench-terminal.sh:38-39`, `scripts/bench-startup.sh:40-41`, `scripts/bundle-macos.sh:59-83`, `src-app/build.rs:58`.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Les actions tierces de `release.yml`, `audit.yml`, `repo_publish.yml` et `update_cask.yml` sont épinglées par SHA.
- [ ] Chaque checkout pose `persist-credentials: false`, et `contents: write` est limité au job de publication.
- [ ] Le ref de checkout du tag de release est qualifié (`refs/tags/<tag>`).
- [ ] `audit.yml` n'ouvre pas de nouvelle issue pour un avis déjà ouvert, grâce à un titre stable par avis, et installe `cargo-deny` à une version épinglée.
- [ ] La release échoue dès le premier job si la clé publique minisign est vide.
- [ ] Le filtre de chemins `rust` de `run_tests.yml` inclut `schemas/**`, `docs/user/configuration/schema.md`, `src-app/src/diff/queries/**`, `src-app/src/diff/fixtures/**`, `protocol/**`, `examples/**`, `src-app/tests/fixtures/**` et `runtimes/**`.
- [ ] La notarisation, le staple, le timestamp de signature et `hdiutil` sont réessayés 3 fois avec backoff, et les téléchargements des scripts ont un délai.
- [ ] `PANEFLOW_SKIP_EMBED_BUILD` n'est actif que pour la valeur `1`.
- [ ] `bench-terminal` et `bench-startup` appliquent le contrôle de contention avant `--set-baseline`.
- [ ] `bundle-macos.sh` vérifie la version des binaires qu'il embarque.
- [ ] Échec : given une action ajoutée sans épinglage par SHA, then un contrôle CI échoue.

#### US-047: Rendre les tests et les benchmarks fiables
**Description:** En tant que mainteneur, je veux que chaque test vérifie réellement ce qu'il nomme, n'écrive jamais dans mes vrais dossiers, et que les benchs soient exploitables sur macOS, afin que la CI et les mesures de performance soient dignes de confiance. Sources : fork #346 et #425 (51a408f1, 846d063c), fef05bba, #66 (9e7655c6), #516 (7cada6d3), #305 (337c0869), #927 (9a2d7c20), #477 (c930b310), #740 (ba1d0fe6), #395 (8d0c8576), #399 (50292a28), #67 (c1e33d71), #304 (18c21dc5), #308 (3f5c503f), #876, #1049, #926, #117 (ec7a2f0f), #484 (cc864b7e) ; `src-app/src/bench_harness.rs:115-140,570-628`, `src-app/src/runtime_paths.rs:314-425`, `src-app/src/workspace/git.rs:461-465,631-661`, `src-app/src/app/diff_dock/revert.rs:551-575`, `src-app/src/diff/git.rs:474-476`, `src-app/src/app/diff_dock/code/base.rs:165-172`, `src-app/src/terminal/element/hyperlink.rs:857-895`, `src-app/tests/ghostty_stress.rs:461-463`, `src-app/src/app/session.rs:2160-2175`, `src-app/src/app/ipc_handler/surface_methods.rs:1243-1255`, `src-app/src/terminal/ghostty_session/convert.rs:434-470`, `src-app/src/terminal/view.rs:1586-1595`, `src-app/src/keybindings/display.rs:450-459`, `src-app/src/keybindings/apply.rs:205-225`, `src-app/src/layout/presets.rs:59-104`, `src-app/src/ai_hooks/extract.rs:509-560`, `crates/paneflow-ai-hook/tests/integration.rs:22`, `src-app/src/app/diff_dock/code/view/disk_sync.rs:864-877`, `crates/paneflow-agent-config/src/lock.rs:8-78`.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `resident_set_bytes` et `process_cpu_time` sont implémentés sous macOS (libproc et `mach_timebase_info`) ; un run de bench macOS n'est plus marqué comme sous contention à tort.
- [ ] Les tests de `runtime_paths` passent l'environnement en paramètre au lieu de modifier `TMPDIR` et `PANEFLOW_HOME` du processus.
- [ ] La sonde mémoire tree-sitter est une `unsafe fn` à précondition documentée et s'exécute dans un processus enfant lancé avec `--exact`.
- [ ] Les fixtures git échouent au lieu de passer quand `git init` échoue ; elles pointent `GIT_CONFIG_GLOBAL` vers un fichier vide temporaire, désactivent la signature et ont un délai.
- [ ] Chaque test cité dans les sources devient rouge sous une mutation ciblée du comportement qu'il nomme ; la mutation est consignée dans la PR.
- [ ] Les tests d'extraction de helpers utilisent un `PANEFLOW_HOME` temporaire.
- [ ] La borne de sortie du test d'intégration de `paneflow-ai-hook` passe à 10 s.
- [ ] `ConfigLock` implémente `Drop` en appelant `unlock()`.
- [ ] `LayoutTree::tiled` a des tests de grille.
- [ ] Échec : given `cargo test --workspace --locked` lancé depuis un pane d'un Paneflow release, then le hash de `~/.paneflow/bin/paneflow-mcp` est inchangé.

## Functional Requirements

- FR-01 : Aucune action de fermeture, de suppression, de démontage, de Revert ou de réinitialisation ne détruit de données sans confirmation explicite ou sans snapshot vérifié. (US-001 à US-007)
- FR-02 : L'éditeur n'écrit jamais sur un fichier plus récent sur disque que la version à laquelle correspond son buffer, sauf après « Keep mine ». (US-004)
- FR-03 : Chaque commande git de production est construite par le builder unique ; les lectures automatiques n'exécutent ni hook, ni fsmonitor, ni filtre, ni diff externe, et ne prennent aucun verrou optionnel. (US-008, US-009)
- FR-04 : Toute écriture de fichier d'état ou de config suit un symlink existant jusqu'à sa cible, sans le remplacer. (US-012)
- FR-05 : Toute lecture de fichier d'état, de config ou de log refuse un fichier non régulier sans bloquer et respecte un plafond de taille. (US-013)
- FR-06 : Une sortie de programme, même abondante ou malformée, ne marque jamais un pane vivant comme terminé. (US-016, US-017)
- FR-07 : Le host reçoit et rapporte l'apparence et la géométrie réelles du terminal, et les commandes Reset et Clear agissent sur l'émulateur du host et du miroir sans écrire dans le PTY. (US-018, US-019)
- FR-08 : Le thread GPUI n'exécute ni requête runtime synchrone, ni I/O fichier, ni sous-processus, ni sonde de chemin dans les chemins listés par l'EP-004. (US-020, US-024, US-026, US-027)
- FR-09 : Toute entrée IPC mal typée reçoit -32602, et toute défaillance runtime est renvoyée comme erreur, jamais comme résultat vide. (US-024, US-025, US-028)
- FR-10 : Une mutation IPC respecte la même politique de fermeture, la même barrière d'orchestration et le même confinement de worktree que l'UI. (US-029)
- FR-11 : Le bridge MCP démarre sous chaque agent supporté, et sa portée est refusée sans preuve de pane. (US-032, US-033, US-036)
- FR-12 : Une réinstallation MCP ou un rafraîchissement d'intégration ne supprime aucune clé que l'utilisateur a posée. (US-033)
- FR-13 : Hooks, entrée MCP et lecture des sessions utilisent le dossier de config réel de chaque agent et de chaque profil. (US-037, US-039)
- FR-14 : Les actions multi-agents couvrent tous les onglets, et chaque overlay rend le focus à son origine. (US-040, US-041)
- FR-15 : Les workflows de release n'exécutent que des actions épinglées par SHA, sans jeton persistant. (US-046)

### Priorités MoSCoW

| Niveau | Capacités | Livraison |
|--------|-----------|-----------|
| Must Have | Perte de données, isolation git, verrous de l'index, écritures symlink-safe, flots terminal, thème du host, lectures IPC hors GPUI, entrée MCP Codex, boucle des shims | R0, R1 (P0) |
| Should Have | Robustesse de l'éditeur, des fichiers spéciaux, du terminal, de l'IPC, de la CLI, de l'intégration agents, de l'UI, de l'accessibilité, de la mise à jour et de la CI | R2, R3 (P1) |
| Could Have | Exactitude git secondaire, sémantique IPC secondaire, fiabilité des tests et benchs | R4 (P2) |
| Won't Have | Les fonctionnalités du fork et les choix produit hors défauts | Section Non-Goals |

## Non-Functional Requirements

- **Performance :**
  - Aucun handler IPC n'attend un runtime terminal sur le thread GPUI (US-024). À la qualification d'une release, facultatif : dans le scénario poll `paneflow wait` à 500 ms et 10 MiB de sortie, 0 frame de plus de 50 ms imputable à un handler IPC.
  - Le p95 de latence clavier vers pixel de `scripts/bench-terminal` reste à ±5 % de `bench/baseline.json` après l'EP-003.
  - Un chemin sauvegardé injoignable ne bloque pas le thread GPUI à la restauration (US-026). À la qualification d'une release, facultatif : première frame à moins de 250 ms au-dessus de `bench/startup-baseline.json`.
  - Un rc de login shell qui lance un job en arrière-plan n'ajoute pas plus de 500 ms au démarrage.
- **Sécurité :**
  - 100 % des spawns git de production passent par le builder (test de garde).
  - 0 exécution d'un `core.fsmonitor` local au dépôt dans les tests des sondes.
  - La portée MCP est refusée sans preuve de pane.
  - 100 % des actions tierces des workflows de release sont épinglées par SHA.
  - `session.json` est en 0600 et `~/.paneflow` en 0700 sous Unix.
- **Fiabilité :**
  - 0 erreur `index.lock` sur 500 itérations `git add` puis `git commit` concurrentes aux sondes.
  - Un pane reste vivant après 10 000 BEL.
  - Au plus 1 bell par 250 ms et 3 notifications OS par 60 s par surface.
  - Un téléchargement de 60 MB à 256 Ko/s aboutit, et une connexion bloquée échoue en 60 s au plus.
  - Une FIFO à un chemin d'état fait échouer la lecture en moins de 100 ms.
- **Accessibilité :**
  - 100 % des helpers interactifs partagés exigent un libellé accessible.
  - Le texte des boutons atteint un contraste d'au moins 4,5:1 sur tous les thèmes livrés.
  - Tous les contrôles de Settings sont utilisables au clavier seul.
  - Les animations listées en US-043 sont statiques quand Reduce motion est actif.
- **Compatibilité :**
  - Chaque story vaut pour Linux x86_64 et aarch64, macOS aarch64 et Windows x86_64 ; sa PR déclare, par OS, si la vérification a été faite nativement ou par lecture.
  - `paneflow-shim` ≤ 512 KiB, `paneflow-ai-hook` ≤ 375 KB et `paneflow-mcp` ≤ 512 KiB après toute modification.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Fermeture avec buffer sale dans un slot garé | Fermer un onglet dont le dock garé contient un fichier modifié | Dialogue listant le fichier, avec Save, Don't Save et Cancel | "You have unsaved changes in 1 file." |
| 2 | Enregistrement pendant un conflit | Ctrl+S alors que le bandeau de conflit est affiché | Aucune écriture, le bandeau reste | "The file changed on disk. Choose Keep mine or Reload." |
| 3 | Revert sur un fichier périmé | Un agent écrit le fichier entre l'affichage et le clic | Revert refusé, diff relancé | "This file changed since it was displayed. Review the new diff." |
| 4 | Revert d'un symlink ou d'un changement de type | Hunk sur un lien retargeté ou un `T` | Revert refusé | "Revert is not supported for symlinks or type changes." |
| 5 | Symlink pendant ou cible en lecture seule | Écriture de `paneflow.json` lié vers le magasin Nix | Erreur, lien intact | "Could not save: the settings file points to a read-only location." |
| 6 | FIFO à un chemin d'état | `session.json` est une named pipe | Lecture refusée en < 100 ms, état par défaut | Log : "session.json is not a regular file" |
| 7 | Suppression de worktree avec agent vivant | Remove worktree depuis le menu d'onglet | Dialogue bloquant, rien supprimé | "A session is still running in this worktree." |
| 8 | Sonde git hors délai | Montage réseau figé | État précédent conservé, erreur visible | "Git status is unavailable." |
| 9 | Flot de BEL ou de notifications | `cat` d'un binaire | Pane vivant, bells et notifications limitées | (aucun) |
| 10 | Host plus ancien que l'app | Reattach après une mise à jour | Fonctionnement normal, fonctions host nouvelles désactivées | Log une fois : "host does not support appearance updates" |
| 11 | Paramètre IPC mal typé | `"surface_id": "42"` | -32602, aucune action | "Invalid params: surface_id must be an integer" |
| 12 | Timeout runtime IPC | Pane très occupé | Erreur JSON-RPC, pas de texte vide | "Terminal runtime did not answer in time" |
| 13 | Codex sans variables Paneflow | Bridge lancé par Codex | `env_vars` transmet les variables ; `status` signale une entrée incomplète | "needs repair" |
| 14 | Paneflow imbriqué | `dev.sh` lancé depuis un pane | Le shim résout le vrai binaire | (aucun) |
| 15 | Connexion lente pendant la mise à jour | 256 Ko/s | Téléchargement mené à terme | (aucun) |
| 16 | Connexion bloquée pendant la mise à jour | Aucun octet pendant 60 s | Échec en 60 s au plus, nouvel essai possible | "Download stalled. Try again." |
| 17 | Collage multi-lignes sans bracketed paste | Collage de 5 lignes à un prompt | Confirmation avant envoi | "Paste 5 lines into the terminal?" |
| 18 | Chemin de profil avec espace sous Windows | `C:\Users\Jean Dupont` | Hooks Codex quotés et fonctionnels | (aucun) |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | L'isolation git casse Git LFS ou des hooks attendus par l'utilisateur | Med | High | Deux profils : `UserAction` conserve hooks et filtres ; test avec un dépôt LFS avant la fin de US-008 |
| 2 | Neutraliser fsmonitor ralentit fortement les sondes sur de gros dépôts | Med | Med | Mesure sur 100 000 fichiers dans US-008 ; au-delà de 2x, question d'ingénierie sur l'autorisation du daemon intégré |
| 3 | Une affirmation du fork est un faux positif | Med | Low | Test rouge obligatoire avant correction ; critère non reproduit retiré avec preuve |
| 4 | Les nouvelles commandes host cassent un host plus ancien encore attaché | Med | High | Négociation ou tolérance explicite, testée par un reattach à une session créée avant la mise à jour (US-018, US-019) |
| 5 | Le mécanisme de réponse IPC différée change la sémantique vue par les clients (timeouts, ordre) | Med | Med | Format de fil inchangé, délai client de 10 s conservé, tests CLI `wait` et `flow` existants rejoués (US-024) |
| 6 | Le code `#[cfg(windows)]` n'est pas compilable hors Windows et casse la CI Windows | Med | Med | Tests natifs sur la machine Windows d'Arthur ; relecture de l'ordre des items par rapport à `mod tests` |
| 7 | 47 stories font dériver le calendrier et diluent les correctifs P0 | High | Med | Livraisons R0 à R4 ; R1 (P0) passe avant tout le reste ; toute réduction de périmètre passe d'abord par ce PRD |
| 8 | Des changements de comportement surprennent les utilisateurs (prompt au quit, confirmation de collage, notifications de programme) | Med | Low | Notes de release explicites, défauts conservateurs (`osc52_clipboard = copy`) |
| 9 | Des modifications locales en cours touchent les mêmes fichiers (terminal, raccourcis) | Med | Med | Chaque story repart du `main` à jour et rebase avant de commencer ; les lignes citées se rapportent à `8c3dd2ca` |

## Non-Goals

Explicit boundaries: what this version does NOT include.

- Les fonctionnalités construites par le fork et qui ne corrigent aucun défaut sont renvoyées à un PRD dédié :
  - Pane Overview (grille de miniatures de tous les panes) ;
  - filtre de la sidebar des sessions ;
  - « Continue in another agent » ;
  - inventaire « Agent setup » ;
  - redimensionnement des splits au clavier ;
  - Cmd+M et Zoom dans le menu Window de macOS ;
  - lien `paneflow` dans le dossier helper des panes ;
  - compteur d'agents et de sous-agents par workspace ;
  - incitation à installer le bridge ;
  - notifications natives activées par défaut ;
  - branche de départ par workspace ;
  - titre fixé par l'agent sur les onglets de pane ;
  - raccourci et persistance de la sidebar.
- Hot-exit des buffers sales, c'est-à-dire leur restauration après un quit : ce PRD garantit seulement qu'aucun buffer n'est perdu sans choix.
- Navigation par mot et par ligne dans `TextInput` et `TextArea` : c'est une fonctionnalité, pas un défaut de l'audit.
- Chargement des bases du dock par `git cat-file --batch`, gain de latence sous Windows : amélioration de performance, à mesurer dans un travail séparé.
- Changement de la politique des schémas de lien ouvrables au Cmd-click (ssh, git, ftp, magnet, ipfs) : décision produit sans exploit démontré.
- Suppressions choisies par le propriétaire du fork (API de tâches, verbes CLI `new`, `select`, `split`, `focus`, `ls`, `read`, `search`), refonte Review du fork, Sentry, sidecar Foundation Models.
- Tout défaut introduit par le fork lui-même ou confiné à ses scripts, sa CI macOS-only ou sa notarisation.
- Doublon du symbole `_memset` dans l'archive libghostty macOS : dépend de l'amont et n'est pas un défaut de Paneflow.

## Files NOT to Modify

- `src-app/Cargo.toml`, les quatre valeurs `rev` de GPUI et la feature `font-kit` de `gpui_platform` : révision Zed épinglée ; un changement casse le rendu macOS.
- `native/libghostty/` (manifest, archives prebuilt, bindings générés) : frontière du moteur unique, gérée par le runbook libghostty.
- `src-app/build.rs`, sauf la vérification de `PANEFLOW_SKIP_EMBED_BUILD` (US-046) : les budgets de taille des helpers et le plafond combiné restent inchangés.
- `docs/user/` : miroir généré de paneflow-web ; les changements de documentation se font dans le dépôt du site.
- `bench/*.json` : baselines de référence ; une nouvelle baseline n'est enregistrée qu'explicitement, par les scripts de bench.
- Les dépendances de `crates/paneflow-shim`, `crates/paneflow-ai-hook` et `crates/paneflow-mcp` : aucune nouvelle dépendance, pour respecter les plafonds de taille.
- `Cargo.lock`, sauf quand une story ajoute une dépendance, avec `cargo deny` à l'appui.

## Technical Considerations

Frame as questions for engineering input, not mandates.

- **Architecture (écrivain symlink-safe) :** placer le helper partagé dans `paneflow-home`, dont dépendent toutes les crates concernées et qui dépend déjà de `tempfile`. Engineering doit confirmer que ce module n'augmente pas la taille des helpers qui dépendent de `paneflow-home`, puisque le code mort est éliminé.
- **Architecture (builder git) :** un module unique dans `src-app`, puisque tous les spawns git de production y vivent. Il remplacerait les trois wrappers actuels (`workspace/git.rs`, `diff/git.rs`, `workspace/worktree.rs`) et les six sites bruts. Faut-il déplacer `run_with_timeout` et le budget de capture de US-011 dans ce même module ?
- **Architecture (réponses IPC différées) :** option A, le handler renvoie une réponse différée qui porte une tâche GPUI. Option B, le thread GPUI résout la cible et renvoie un handle runtime `Send` que le thread de connexion interroge directement. La B semble plus simple et garde le budget de 5 s côté connexion. Engineering doit choisir avant US-024.
- **Protocole host :** les commandes d'apparence, de clear et de reset doivent-elles passer par une version de protocole négociée à l'attache, ou par des commandes optionnelles ignorées par un host ancien ? Les hosts survivent aux mises à jour de l'app.
- **Git et fsmonitor :** si la mesure de US-008 dépasse 2x sur un gros dépôt, faut-il autoriser le daemon intégré (`core.fsmonitor=true`) tout en refusant les chemins de hook ?
- **Journaux :** activer la feature `log` de `tracing` dans la dépendance du workspace (une ligne, sans nouvelle crate) ou brancher un subscriber vers `env_logger` ? La première option est recommandée.
- **Téléchargement :** ureq 3 n'a pas de délai d'inactivité ; le délai de stagnation doit envelopper le lecteur du corps. Un lecteur maison, ou `timeout_recv_body` par morceaux ?
- **Dialogue des buffers sales :** étendre les lignes de `close_dialog_rows`, ou créer un dialogue dédié réutilisé par quit et par la fermeture d'onglet ? Réutiliser `close_policy` est recommandé.
- **Migration :** aucune migration de données. `terminal.osc52_clipboard` est une nouvelle clé optionnelle, rétrocompatible. Les entrées MCP existantes sont réparées en place au prochain rafraîchissement du worker.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Défauts confirmés encore ouverts | ≈210 (audit du 2026-09-28) | 0 défaut P0 ; ≤ 15 défauts P2 | Month-1 / Month-6 | `tasks/prd-fork-audit-fixes-status.json` et sources citées par story |
| Scénarios de perte de données reproductibles | 7 | 0 | Month-1 | Tests de régression de l'EP-001 |
| Erreurs `index.lock` dans le test de concurrence | Non mesuré (défaut confirmé par lecture) | 0 sur 500 | Month-1 | Test de US-009 |
| Frames de plus de 50 ms imputables à l'IPC pendant `paneflow wait` | Non mesuré (défaut confirmé par lecture) | 0 | Month-6 | Qualification de release, facultative (v1.3) |
| Panes marqués terminés par un flot de BEL | À mesurer par US-016 | 0 | Month-1 | Test de US-017 |
| OS où le bridge MCP fonctionne sous Codex | À mesurer par US-032 | 3/3 | Month-1 | Protocole de US-032 rejoué après US-033 |
| Spawns git de production hors builder | ≈15 sites | 0 | Month-1 | Test de garde de US-008 |
| Stories DONE | 0/48 | 16/48 (R0 et R1) ; ≥ 45/48 | Month-1 / Month-6 | Fichier de statut |
| Tests de régression ajoutés | 0 | ≥ 47, au moins un par story | Month-6 | Revue des PR |

## Open Questions

- Le défaut `copy` de `terminal.osc52_clipboard` est-il le bon, ou faut-il `off` ? Arthur tranche avant de commencer US-023 ; seule cette story en dépend.
- Les notifications de programme (OSC 9/777) doivent-elles suivre le réglage « Native OS notifications », comme décidé ici, ou garder un réglage séparé ? Arthur, avant US-017.
- Au démontage d'un worktree, faut-il prévenir que les fichiers ignorés seront supprimés, ou les inclure dans le snapshot ? Décision actuelle : ils restent jetables, conformément à `.worktreeinclude`. Arthur, avant la clôture de US-002.
- Si US-032 ou US-034 ne reproduisent pas leur défaut, que deviennent US-033 et US-035 ? Les critères sont déjà prévus dans les stories ; Arthur valide leur reclassement.
- Est-il souhaitable de citer les numéros d'issues et les sha d'un fork privé dans un document suivi par git ? Arthur, avant le premier commit de ce PRD.
- Faut-il ajouter à `workspace.close` un paramètre `force` qui ignore la confirmation (buffers sales, agents vivants) pour les orchestrateurs ? Engineering, pendant US-029.
[/PRD]
