[PRD]
# PRD: Orchestration v2 — Flow, Worktrees & Grid Awareness (2026-Q3)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-09 | Arthur Jean | Draft initial — 4 epics / 20 stories couvrant flow engine, worktree-per-agent, awareness grille, primitives CLI/IPC |
| 1.1 | 2026-06-09 | Arthur Jean | US-013 AC3 harmonisé avec US-011 AC3 (dépendants d'un échec = SKIPPED, jamais FAILED) — les deux AC se contredisaient |

## Problem Statement

1. **L'orchestration multi-agents locale est de la glue bash.** Paneflow expose `up` / `wait` / `send` mais chaîner « lance A, attends `tests passed`, fan-out 3 agents, fan-in, feed le résumé au reviewer » exige un script shell fragile écrit à la main. Warp Oz formalise ce pattern mais cloud-only (lancé 2026-02) ; aucun outil local-first cross-platform ne le fait ([warp.dev/blog/oz](https://www.warp.dev/blog/oz-orchestration-platform-cloud-agents)).
2. **La parallélisation par worktrees décourage avant de servir.** Setup manuel : `git worktree add`, recopie des `.env` gitignorés, install des deps, collision de ports (3000/5432 repris par tous les agents), teardown oublié → worktrees orphelins. Documenté par 6+ articles Q1-Q2 2026 ; constat transverse : « Every tool assumes someone else will solve the environment problem. Nobody does. » ([dev.to/rohansx](https://dev.to/rohansx/every-ai-agent-tool-creates-git-worktrees-none-of-them-make-worktrees-actually-work-3ae9)).
3. **À 5+ panes, l'opérateur ne sait pas qui attend quoi.** Le message `ai.notification` est reçu, tronqué à 512 chars… puis ignoré (`let _ = message;`, `ipc_handler.rs:1308`). La notif desktop dit « agent finished » sans contexte. Aucun indicateur visuel par pane ne signale `WaitingForInput`. Friction n°1 du marché : « constantly jumping between terminals to see which session needed input » ([HN ccmux](https://news.ycombinator.com/item?id=47223142), [issue #24537 claude-code](https://github.com/anthropics/claude-code/issues/24537), 20+ issues consolidées).
4. **Des primitives IPC manquent pour piloter finement la grille** : pas de `surface.focus`, `surface.split` tape toujours la first leaf, pas de broadcast, `surface.send_keystroke` n'a pas de CLI, et rien ne peut soumettre un prompt (`send_text` sans `\r`, `send_keystroke` rejette `\r`/`\n`).

**Why now :** la branche `feat/cli-agent-orchestration` vient de livrer le socle (`up`/`wait`/`send`, hooks, prefill `output_generation`). C'est la fenêtre pour transformer ces primitives en moteur d'orchestration avant que Warp ne descende son orchestration cloud en local. Le positionnement Paneflow (« agent cockpit cross-platform », seul moat vs cmux mac-only) dépend de ce saut.

## Overview

Quatre chantiers complémentaires sur la surface terminal (mode CLI) uniquement — ni Diff ni Agents :

**EP-001 — Primitives CLI/IPC.** Compléter le plan de contrôle : `surface.focus`, `surface.split` ciblé, `send --broadcast`, CLI pour `send_keystroke`, et la primitive de soumission explicite double-gatée (`PANEFLOW_IPC_SCRIPTING=1` + opt-in par appel). Ces primitives débloquent le flow engine et restent utiles seules.

**EP-002 — Worktree-per-agent.** Un champ `worktree = "branch"` dans `paneflow.workspace.toml` : Paneflow crée le worktree en sibling (`<repo>.worktrees/<branch>`), copie les `.env*` gitignorés, exécute une commande `setup` optionnelle, injecte `${port_offset}`, et tear down proprement (worktree seulement, jamais la branche).

**EP-003 — Flow engine.** `paneflow flow run <file>` : DAG déclaratif TOML de steps (spawn pane → barrier `ready.pattern` → feed du suivant), avec `needs`, fan-out `foreach`, fan-in, `capture` des lignes matchées réinjectables, timeouts par step, fail-fast/continue, dry-run. Sémantique calquée sur process-compose (`process_log_ready`) + GitHub Actions (`needs`, outputs, timeout par step) — les deux formats les plus éprouvés.

**EP-004 — Awareness grille.** Matérialiser le lien PID agent → pane (marche d'ancêtres cross-platform), stocker le message `ai.notification`, glow accent sur les panes `WaitingForInput` (amplifier l'actif, jamais dégrader l'inactif), action « jump to next waiting agent », peek overlay pour lire la question de l'agent sans focus, et notif desktop contextuelle (la question réelle, pas « needs input »).

Décisions structurantes : soumission scriptée = double gate explicite (env var instance + opt-in par step dans un fichier rédigé par l'humain) — la règle human-in-loop gouverne les actions IA initiées par la GUI, pas le scripting user-authored ; worktrees en sibling dir pour ne pas polluer les watchers récursifs ; toutes les opérations git/FS hors render-thread via `paneflow_process::run_with_timeout`.

## Goals

| Goal | Phase-1 Target | Phase-2 Target |
|------|---------------|----------------|
| Orchestrer un pipeline 3 agents (impl → fan-out → review) sans script shell | `flow run` exécute le pipeline de démo en < 3 commandes utilisateur | `flow run` utilisé dans le dogfooding Paneflow (≥ 1 flow réel dans le repo) |
| Réduire le setup d'un agent isolé sur worktree | 1 ligne de TOML (vs ≥ 5 commandes manuelles) | teardown propre vérifié : 0 worktree orphelin après 20 cycles up/close |
| Identifier la pane qui attend en < 2 s | glow visible + jump-to-waiting en 1 raccourci | notif desktop contient la question de l'agent (≥ 1 ligne de contexte) |
| Compléter le plan de contrôle IPC | 4 méthodes/flags ajoutés (focus, split ciblé, broadcast, key) | flow engine consomme uniquement des méthodes IPC publiques |

## Target Users

### Le dev orchestrateur (Arthur et profils similaires)
- **Role :** solo dev / indie maker qui pilote 3-8 agents CLI (Claude Code, Codex, OpenCode) en parallèle dans Paneflow.
- **Behaviors :** lance les agents via `paneflow up`, supervise dans la grille, review au fil de l'eau. Scripte ponctuellement via `send`/`wait`.
- **Pain points :** glue bash fragile pour chaîner les agents ; setup worktree manuel dissuasif ; ne sait pas quelle pane attend une réponse sans scanner la grille.
- **Current workaround :** scripts shell `up && wait && send`, worktrees à la main, notifications binaires `notify-send`.
- **Success looks like :** décrire un workflow en TOML, le lancer, et n'être interrompu que quand un agent a réellement besoin de lui — avec la question affichée.

### L'agent CLI lui-même (consommateur IPC)
- **Role :** Claude Code / Codex tournant dans une pane, utilisant le MCP bridge et la CLI `paneflow`.
- **Behaviors :** lit les autres panes (`read_pane`), pilote des workspaces via la CLI quand le gate scripting est actif.
- **Pain points :** ne peut ni cibler une pane pour split/focus, ni soumettre un prompt à un autre agent (orchestration agent-of-agents impossible).
- **Current workaround :** aucun — capacités absentes.
- **Success looks like :** un agent « conductor » peut exécuter un flow complet via la CLI publique, sous le gate scripting.

## Research Findings

### Competitive Context
- **Warp Oz** (2026-02) : orchestration parent/child agents, fan-out par shards, agrégation — mais cloud-only, pricing opaque. Paneflow fait l'équivalent local-first.
- **cmux** : cockpit d'agents sur Ghostty, notifications par projet — macOS-only, non scriptable.
- **Claude Squad / amux / ccmux** : tmux + worktrees, TUI de supervision — friction tmux, pas de moteur déclaratif, < 500 stars chacun (besoin réel, offre faible).
- **process-compose** : la référence sémantique pour un DAG de process locaux (`depends_on` à 5 conditions, `ready_log_line`, restart policies) — mais pas orienté agents ni terminal.
- **Market gap :** aucun format déclaratif d'orchestration d'agents CLI n'existe (Claude Code dynamic workflows = JS généré, non déclaratif ; claude-workflow = phases séquentielles sans DAG). Territoire vierge.

### Best Practices Applied
- Détection de cycles au parse, pas à l'exécution (process-compose).
- `ready.pattern` = équivalent `process_log_ready`, fiabilisé par le contrôle du PTY (pas de buffering stdout, le piège n°1 documenté).
- Timeout par step (gap connu de process-compose, repris de GitHub Actions `timeout-minutes`).
- Passage de données entre steps inspiré de `$GITHUB_OUTPUT` (seul format avec outputs typés).
- Worktrees : sibling dir (watchers récursifs), branche jamais supprimée au teardown, `git worktree prune` pour les orphelins, vérifier `git worktree list` avant création (branche verrouillable dans un seul worktree).

*Sources complètes tracées dans la conversation de recherche (Warp, process-compose docs, dev.to/rohansx, Nimbalyst, GitHub issues #24537/#15487, HN amux/ccmux).*

## Assumptions & Constraints

### Assumptions (to validate)
- Le PID rapporté par les hooks `ai.*` est un descendant du `terminal.child_pid` de la pane où l'agent tourne (vrai pour `up` qui lance l'agent directement ; à valider pour un agent lancé depuis un shell interactif imbriqué) — US-017 inclut la validation.
- Le pattern-match sur le scrollback (`ready.pattern`) est suffisamment fiable pour les CLI agents dont Paneflow contrôle le PTY (line-buffering garanti par le PTY).
- Les utilisateurs acceptent qu'un flow `submit = true` soumette des prompts — dès lors qu'ils ont écrit le fichier et activé le gate.

### Hard Constraints
- **Cross-platform Linux/macOS/Windows obligatoire** (CLAUDE.md) : chaque story a un chemin par OS ou un stub documenté cohérent avec l'existant (ex. notif desktop Windows = stub comme `fire_turn_end_notification`).
- **Human-in-loop** : aucun prompt soumis sans (1) `PANEFLOW_IPC_SCRIPTING=1` ET (2) opt-in explicite par appel/step. Le défaut reste pré-rempli sans `\r`.
- `MAX_PANES = 32`, `MAX_WORKSPACES = 20` — `foreach` et flows les respectent (échec au parse si dépassement statique, à l'exécution sinon).
- Exit codes CLI existants : 0 OK, 1 runtime, 3 target, 4 timeout. `flow` les réutilise.
- Jamais d'I/O bloquant sur le render-thread GPUI (audit 2026-06-04) : git/FS via `paneflow_process::run_with_timeout` ou `smol::unblock`.
- Jamais `git branch -d` automatique. Jamais de suppression de worktree contenant des changements non commités.
- Convention commits : `feat(module): US-NNN — description`, atomiques par story.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique (gate CI release, 4 legs)
- `cargo clippy --workspace -- -D warnings` - zéro warning
- `cargo test --workspace` - tous les tests workspace

Pour les stories UI (US-018, US-019, US-020) :
- Vérification visuelle manuelle dans l'app (GPUI non testable headless) — noter « UI non vérifiée GUI » dans le commit si la passe visuelle n'a pas eu lieu.

## Epics & User Stories

### EP-001: Primitives CLI/IPC du plan de contrôle

Compléter les méthodes IPC et sous-commandes manquantes pour piloter finement la grille. Débloque le flow engine (EP-003) et l'orchestration agent-of-agents.

**Definition of Done:** `paneflow focus|split --target|send --broadcast|key` fonctionnent contre une instance live ; la soumission explicite est double-gatée et testée ; toutes les méthodes documentées dans `docs/`.

#### US-001: `surface.focus` IPC + `paneflow focus <target>`
**Description:** As a dev orchestrateur, I want donner le focus à une pane ciblée par selector so that je saute instantanément vers l'agent concerné (et que le flow engine puisse diriger l'attention).

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given une instance avec 3 panes, when `paneflow focus backend`, then la pane nommée `backend` reçoit le focus GPUI (`focus_handle.focus()`) et son workspace devient actif si nécessaire
- [x] Given un selector ambigu (préfixe matchant 2 panes), when `paneflow focus ba`, then exit code 3 avec la liste des candidats (parité `resolve_target` existant, `selector.rs:33`)
- [x] Given un selector sans match, when `paneflow focus zzz`, then exit code 3
- [x] La méthode `surface.focus` est dispatchée sur le main thread GPUI comme les autres méthodes stateful (`ipc_handler.rs`)
- [x] Pas de gate scripting requis (lecture/navigation, pas d'injection)

#### US-002: `surface.split` ciblé + `paneflow split <h|v> [--target <sel>]`
**Description:** As a dev orchestrateur, I want splitter une pane désignée (pas seulement la first leaf) so that je construis un layout précis depuis la CLI et que `flow` place les panes où il faut.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given 3 panes, when `paneflow split v --target backend`, then la pane `backend` est splittée verticalement (nouvelle leaf adjacente dans le layout tree), pas la first leaf
- [x] Given `--target` omis, when `paneflow split h`, then comportement actuel inchangé (first leaf — rétrocompatibilité)
- [x] Given `MAX_PANES` (32) atteint, when split, then exit code 1 avec message explicite (parité comportement existant `control_cmds.rs:49`)
- [x] Given un target ambigu ou sans match, when split, then exit code 3
- [x] Le param `surface_id` de `surface.split` est optionnel côté IPC (absent = first leaf, rétrocompatible pour les clients existants)

#### US-003: `paneflow send --broadcast <selector> <text>`
**Description:** As a dev orchestrateur, I want envoyer un même texte à toutes les panes matchant un selector so that je pré-remplis un ordre commun (« commitez votre travail ») sur N agents en une commande.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given 3 panes `shard-api`, `shard-ui`, `shard-db`, when `paneflow send --broadcast shard <text>`, then le texte est injecté (sans `\r`) dans les 3 panes via `resolve_all` (`selector.rs:73`) + N appels `surface.send_text`
- [x] Given le gate `PANEFLOW_IPC_SCRIPTING` absent, when broadcast, then exit code 1 avec le message actionnable existant (traduction `-32601`, parité `send_cmd.rs`)
- [x] Given un selector sans match, when broadcast, then exit code 3, aucun envoi partiel
- [x] Given un envoi qui échoue au milieu (pane fermée entre resolve et send), when broadcast, then les panes restantes sont quand même servies et le rapport JSON liste `{sent: [...], failed: [...]}` avec exit code 1
- [x] Sans `--broadcast`, un selector multi-match reste une erreur (comportement single inchangé)

#### US-004: `paneflow key <target> <keystroke>` (CLI pour `surface.send_keystroke`)
**Description:** As a dev orchestrateur, I want envoyer une keystroke nommée (ex. `escape`, `ctrl-c`, `tab`) à une pane so that je débloque un agent TUI coincé sans toucher la souris.

**Priority:** P1
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given une pane `backend`, when `paneflow key backend escape`, then la keystroke est envoyée via `surface.send_keystroke` existant (`ipc_handler.rs:1087`)
- [x] Given le gate scripting absent, when `key`, then exit code 1 avec message actionnable
- [x] Given une keystroke `enter`/`return`, when `key`, then refus côté serveur conservé (rejet `\r`/`\n` existant) et exit code 1 expliquant le chemin `--submit` (US-005)
- [x] Given un target invalide, when `key`, then exit code 3

#### US-005: Soumission explicite double-gatée (`send --submit` / param `submit` IPC)
**Description:** As a dev orchestrateur, I want pouvoir soumettre un prompt (texte + retour chariot) sous double gate explicite so that un flow user-authored peut enchaîner des agents sans intervention, sans jamais ouvrir la porte à une soumission implicite.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `PANEFLOW_IPC_SCRIPTING=1` actif, when `paneflow send --submit backend "run tests"`, then le texte est injecté puis soumis (`\r`) dans la pane
- [x] Given le gate scripting absent, when `send --submit`, then exit code 1, rien n'est injecté (ni texte ni `\r`)
- [x] Given `--submit` absent, when `send`, then comportement actuel strict (injection sans `\r`) — le défaut ne change jamais
- [x] Le param IPC `submit: true` sur `surface.send_text` est rejeté avec une erreur dédiée si le gate scripting est off (jamais de soumission silencieuse)
- [x] `surface.send_keystroke` continue de rejeter `\r`/`\n` inconditionnellement — l'unique chemin de soumission est `send_text` + `submit`
- [x] Un test couvre le cas « texte 64 KiB + submit » (limite existante respectée, `\r` envoyé après le dernier chunk)

---

### EP-002: Worktree-per-agent dans `paneflow up`

Une ligne de TOML pour isoler chaque agent sur son worktree git, avec environnement reproductible et teardown propre. Élimine la friction n°1 d'onboarding à la parallélisation.

**Definition of Done:** `worktree = "branch"` dans un `paneflow.workspace.toml` crée/réutilise le worktree, copie les `.env*`, exécute `setup`, injecte `${port_offset}` ; la fermeture du workspace tear down les worktrees propres ; `git worktree prune` au démarrage ; zéro opération git sur le render-thread.

#### US-006: Champ `worktree` + création du worktree git
**Description:** As a dev orchestrateur, I want déclarer `worktree = "feat/x"` sur une pane de mon workspace spec so that Paneflow crée le worktree et y lance l'agent, sans commande git manuelle.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given une pane avec `cwd` dans un repo git et `worktree = "feat/x"`, when `paneflow up`, then `git worktree add <repo>.worktrees/feat-x -b feat/x` (branche créée si absente, slug filesystem-safe pour le path) est exécuté via `paneflow_process::run_with_timeout` hors render-thread, et la pane démarre avec `cwd` = le worktree
- [x] Given la branche existe déjà sans worktree, when up, then `git worktree add <path> feat/x` (sans `-b`) réutilise la branche
- [x] Given la branche est déjà checked-out dans un autre worktree (verrou git), when up, then le spawn de CETTE pane échoue avec un message explicite citant le path du worktree existant, et `--dry-run` le détecte sans rien créer (vérif `git worktree list` au plan)
- [x] Given le worktree path existe déjà et pointe sur la bonne branche, when up, then réutilisation idempotente (pas d'erreur, pas de doublon)
- [x] Given `cwd` hors d'un repo git, when up avec `worktree`, then erreur de validation au parse du spec (exit 1, atomique — aucune pane spawnée, parité fail-atomique existant `up_cmd.rs`)
- [x] `worktree` + `cwd` relatif/`~` : résolution via les helpers existants `find_git_dir`/`resolve_repo_root` (`git.rs:101`, `git.rs:211`) — chemins `PathBuf`, jamais de séparateur hardcodé (Windows OK)

#### US-007: Environnement reproductible — copie `.env*` + commande `setup`
**Description:** As a dev orchestrateur, I want que le worktree reçoive mes fichiers d'env gitignorés et exécute ma commande d'install so that l'agent démarre dans un environnement fonctionnel, pas un squelette.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [x] Given un repo avec `.env` et `.env.local` gitignorés à la racine, when le worktree est créé, then ces fichiers sont copiés à la racine du worktree (top-level uniquement, pas de récursion — pas de copie de `node_modules`)
- [x] Given `copy_env = false` sur la pane, when up, then aucune copie (opt-out explicite ; défaut = true)
- [x] Given `setup = "bun install"` sur la pane, when le worktree est créé, then la commande s'exécute dans le worktree (cwd) AVANT le spawn de l'agent, avec timeout configurable (`setup_timeout_secs`, défaut 300) via `run_with_timeout`
- [x] Given `setup` échoue (exit ≠ 0 ou timeout), when up, then la pane démarre quand même MAIS un warning best-effort est émis (stdout CLI + log) — l'échec d'install ne doit pas bloquer l'humain qui peut corriger dans la pane
- [x] Given un fichier `.env` absent, when up, then aucune erreur (copie best-effort, silencieuse si rien à copier)
- [x] Paneflow ne devine jamais le gestionnaire de paquets — sans `setup`, aucune install n'est tentée

#### US-008: Variable `${port_offset}` dans `env`
**Description:** As a dev orchestrateur, I want une variable `${port_offset}` substituée dans les valeurs `env` de la pane so that chaque agent worktree reçoive une plage de ports distincte sans arithmétique manuelle.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [x] Given 3 panes worktree avec `env = { PORT = "${port_offset}" }` et `port_base = 3000` au niveau workspace (défaut 3000), when up, then les panes reçoivent PORT=3000, 3010, 3020 (stride 10, indexé par ordre de déclaration)
- [x] Given un port de la plage déjà occupé sur la machine, when up, then l'offset saute à la plage libre suivante (réutilise la détection de ports existante : Linux `/proc/net/tcp`, macOS `libproc`, `ports.rs`) ; sur Windows (stub ports existant), allocation arithmétique pure sans vérification, documentée
- [x] Given `${port_offset}` utilisé sans `worktree`, when up, then la substitution fonctionne aussi (utile hors worktree) — la variable est par-pane, pas par-worktree
- [x] Given une valeur env sans `${port_offset}`, when up, then aucune substitution ni altération (passthrough exact)
- [x] La substitution n'introduit aucune autre variable magique (`${...}` inconnu = erreur de validation au parse, message citant les variables supportées)

#### US-009: Teardown propre + prune des orphelins
**Description:** As a dev orchestrateur, I want que la fermeture du workspace retire les worktrees créés par Paneflow s'ils sont propres so that je n'accumule ni gigaoctets ni références git mortes — sans jamais perdre du travail non commité.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [x] Given un worktree créé par Paneflow (marqué dans l'état du workspace) SANS changements non commités (`git status --porcelain` vide), when le workspace est fermé, then `git worktree remove <path>` est exécuté hors render-thread — la branche n'est JAMAIS supprimée
- [x] Given des changements non commités dans le worktree, when close, then le worktree est conservé et un toast/log signale « worktree conservé : changements non commités » (jamais de perte de données)
- [x] Given `worktree_teardown = "keep"` sur la pane, when close, then aucun teardown (opt-out ; défaut = "auto" = remove si propre)
- [x] Given un worktree dont le répertoire a été supprimé manuellement (référence orpheline), when Paneflow démarre, then `git worktree prune` est exécuté sur les repos des workspaces restaurés (best-effort, hors render-thread, timeout 10 s)
- [x] Given Paneflow crash avec un agent actif dans un worktree, when relance, then le prune au démarrage ne touche PAS un worktree dont le répertoire existe encore (prune ne nettoie que les références mortes — comportement git natif, testé)
- [x] Un worktree NON créé par Paneflow (préexistant, pointé par `cwd`) n'est jamais tear down

---

### EP-003: Flow engine — `paneflow flow`

Le DAG déclaratif local-first : décrire un pipeline d'agents en TOML, l'exécuter avec barriers, fan-out/fan-in et passage de données. Le différenciant structurel vs cmux (non scriptable) et Warp Oz (cloud-only).

**Definition of Done:** `paneflow flow run <file>` exécute un pipeline multi-étapes réel (spawn → ready → feed → fan-out → fan-in) ; `--dry-run` valide sans toucher l'instance ; cycles et dépassements de caps détectés au parse ; échecs gérés (fail-fast/continue) ; exit codes cohérents.

#### US-010: Schéma `flow.toml` — parse, validation, cycles, dry-run
**Description:** As a dev orchestrateur, I want un format TOML validé statiquement (steps, `needs`, `ready`, caps, cycles) so that mes erreurs de pipeline sont attrapées avant de toucher l'instance.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given un fichier valide, when `paneflow flow run <file> --dry-run`, then le plan résolu est imprimé (steps topologiquement triés, panes, barriers, variables) sans aucun appel IPC mutateur — parité `up --dry-run`
- [x] Le schéma supporte : `[[step]]` avec `id` (unique), `needs = ["id", ...]`, `pane = { ...PaneSpec... }` (réutilise `workspace_spec.rs:67` — agent XOR command, cwd, env, name, worktree d'EP-002), `ready = { pattern, timeout_secs }`, `send = { target, text, submit }`, `foreach`, `capture` ; `[defaults]` avec `timeout_secs`, `on_failure = "fail_fast" | "continue"`
- [x] Given un cycle dans `needs` (A→B→A), when parse, then erreur au parse citant le cycle (détection statique, pattern process-compose) — exit 1
- [x] Given un `needs` référençant un id inexistant, when parse, then erreur listant les ids connus — exit 1
- [x] Given un flow dont les steps statiques + `foreach` dépassent `MAX_PANES` (32) pour un même workspace, when parse, then erreur explicite avant exécution — exit 1
- [x] `deny_unknown_fields` actif (parité `workspace_spec.rs`) — un champ inconnu est une erreur citant le champ
- [x] Un step avec `ready` sans `timeout_secs` ni défaut global est une erreur de validation (timeout obligatoire — un barrier sans timeout bloque indéfiniment)

#### US-011: Exécuteur — spawn + barriers `ready.pattern`
**Description:** As a dev orchestrateur, I want que le moteur lance les steps dont les dépendances sont satisfaites et attende leurs patterns de complétion so that le pipeline avance seul, au rythme réel des agents.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given `step B` avec `needs = ["A"]` et `A.ready.pattern = "tests passed"`, when `flow run`, then B ne démarre qu'après que le scrollback de la pane de A matche le regex (réutilise la machinerie poll de `wait_cmd.rs:53` — fenêtre 500 lignes, 500 ms)
- [x] Given deux steps sans dépendance mutuelle, when run, then ils démarrent concurremment (spawn parallèle, parité `workspace.up` multi-panes)
- [x] Given `ready.timeout_secs` dépassé sans match, when run, then le step est marqué FAILED ; en `fail_fast` (défaut) le flow s'arrête (steps en cours laissés vivants — jamais de kill de pane), exit 4 ; en `continue`, les steps ne dépendant pas du failed continuent et les dépendants sont SKIPPED
- [x] Given une pane d'un step fermée par l'humain pendant le barrier, when run, then le step est FAILED immédiatement (parité fail-fast de `wait` quand les panes cibles se ferment) — pas de poll fantôme
- [x] Given un step sans `ready`, when run, then il est considéré satisfait dès le spawn + prefill réussis (équivalent `process_started`)
- [x] Le moteur tourne côté CLI (process `paneflow flow`), pilote l'instance via IPC public uniquement — Ctrl-C sur le process flow arrête l'orchestration sans tuer les panes (les agents restent vivants, état imprimé)
- [x] Cross-platform : aucun chemin POSIX-only ; le poll IPC est identique sur les 3 OS

#### US-012: Steps `send` — feed et soumission gatée
**Description:** As a dev orchestrateur, I want des steps qui injectent un texte (avec variables) dans une pane existante, soumis seulement si je l'ai explicitement écrit so that le pipeline enchaîne les agents en respectant le contrat human-in-loop.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011, US-005

**Acceptance Criteria:**
- [x] Given un step `send = { target = "review", text = "...", submit = true }` et le gate scripting actif, when le step s'exécute, then le texte est injecté et soumis dans la pane `review` (via US-005)
- [x] Given `submit` absent ou false, when le step s'exécute, then injection SANS soumission (pré-rempli, l'humain valide) — le défaut du format est non-soumis
- [x] Given le gate `PANEFLOW_IPC_SCRIPTING` absent et un flow contenant ≥ 1 step `submit = true`, when `flow run` (et `--dry-run`), then erreur AVANT toute exécution, message expliquant le gate — jamais de dégradation silencieuse en non-soumis
- [x] Given `target` référençant le `name` d'une pane d'un step précédent, when run, then résolution via le selector existant ; cible fermée = step FAILED
- [x] Le prefill attend l'inactivité de la pane cible (réutilise le mécanisme `output_generation`, `ipc_handler.rs:707`) avant d'injecter — pas d'injection au milieu d'un output

#### US-013: Fan-out `foreach` + fan-in
**Description:** As a dev orchestrateur, I want déclarer un step template instancié N fois (`foreach`) et des steps aval qui attendent toutes les instances so that je shard une tâche sur N agents et j'agrège en une barrier.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given `foreach = ["api", "ui", "db"]` sur un step `shard`, when run, then 3 panes sont spawnées avec `${item}` substitué dans `prompt`, `name`, `cwd`, `worktree` et valeurs `env` (ex. `name = "shard-${item}"`)
- [x] Given un step `merge` avec `needs = ["shard"]`, when run, then il attend que TOUTES les instances aient satisfait leur `ready` (fan-in barrier ; sémantique « groupe » de process-compose)
- [x] Given une instance FAILED en mode `continue`, when run, then le fan-in et ses steps aval sont SKIPPED avec une erreur citant la dépendance morte (un fan-in exige toutes ses instances ; sémantique unifiée avec US-011 AC3 — un step qui n'a jamais tourné est SKIPPED, pas FAILED) ; les autres instances continuent jusqu'à leur ready
- [x] Given `foreach` vide, when parse, then erreur de validation — exit 1
- [x] Given `${item}` utilisé hors d'un step `foreach`, when parse, then erreur de validation
- [x] Le dépassement dynamique de `MAX_PANES` à l'exécution (workspace déjà peuplé) échoue le step avec message explicite, pas de spawn partiel silencieux

#### US-014: `capture` — passage de données entre steps
**Description:** As a dev orchestrateur, I want capturer les dernières lignes du scrollback d'un step au moment du match `ready` dans une variable so that le step suivant reçoive le résumé de l'agent précédent dans son prompt.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given `capture = { var = "summary", lines = 20 }` sur le step A, when le `ready` de A matche, then les 20 dernières lignes du scrollback (fenêtre `surface.read` existante) sont stockées dans `${summary}`
- [x] Given un step aval avec `text = "Review ceci : ${summary}"`, when il s'exécute, then la variable est substituée (et la limite 64 KiB de `send_text` respectée — troncature head avec marqueur `[truncated]` si dépassement)
- [x] Given `${var}` non défini (step capture non exécuté/skipped), when le step consommateur démarre, then le step est FAILED avec un message citant la variable manquante — jamais de substitution vide silencieuse
- [x] Given un step `foreach` avec capture, when fan-in, then les captures sont exposées en `${var.api}`, `${var.ui}`… (suffixe = item) et `${var}` seul est une erreur de validation au parse
- [x] `lines` est clampé à la fenêtre max de lecture existante (500) ; valeur 0 ou absente = erreur de validation
- [x] Le contenu capturé est du texte terminal UNTRUSTED : il est substitué verbatim, jamais interprété/exécuté par le moteur

#### US-015: Reporting, exit codes et reprise d'état
**Description:** As a dev orchestrateur, I want un état lisible du flow (live + final JSON) et des exit codes cohérents so that je scripte par-dessus et je diagnostique un échec en 10 secondes.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given un flow en cours, when run (mode humain, défaut TTY), then une ligne de statut par step est affichée et mise à jour : `PENDING | RUNNING | READY | FAILED | SKIPPED` avec durée
- [x] Given `--json`, when le flow se termine, then un rapport JSON final : `{flow, status, steps: [{id, status, duration_ms, pane_ids, error?}]}` sur stdout, statuts lisibles machine
- [x] Exit codes : 0 = tous READY ; 4 = ≥ 1 timeout de barrier ; 1 = échec runtime (spawn, IPC, validation runtime) ; 3 = erreur de target (selector) — réutilise la grille existante
- [x] Given Ctrl-C pendant le run, when interruption, then le rapport partiel est imprimé (steps et leur état au moment de l'arrêt), exit 1, panes laissées vivantes
- [x] Given l'instance Paneflow tuée pendant le flow, when le poll IPC échoue, then le moteur abandonne proprement avec le rapport partiel et un message « instance unreachable » — pas de retry infini

---

### EP-004: Awareness — la grille qui route l'attention

Matérialiser « quel agent attend quoi » directement dans la grille terminale : lien PID→pane, glow, jump, peek, notification contextuelle. Tier ressenti au quotidien.

**Definition of Done:** une pane `WaitingForInput` est identifiable en < 2 s (glow), atteignable en 1 raccourci (jump), sa question lisible sans focus (peek) et reprise dans la notif desktop ; zéro régression de la règle « amplifier l'actif, jamais dégrader l'inactif ».

#### US-016: Stocker le message `ai.notification` + notif desktop contextuelle
**Description:** As a dev orchestrateur, I want que la question de l'agent (déjà transmise par les hooks) soit conservée et injectée dans la notification desktop so that je sache QUOI on me demande sans retourner à l'écran.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given un event `ai.notification` avec message, when traité, then le message (déjà tronqué 512 chars, `ipc_handler.rs:1296`) est stocké dans `AgentSession` (nouveau champ `message: Option<String>`) au lieu d'être ignoré (`let _ = message;` supprimé)
- [x] Given une transition `WaitingForInput` fenêtre inactive, when la notif desktop part, then le body contient le titre du workspace + le message de l'agent (ex. « backend : Allow `cargo test` ? ») — extension de `fire_turn_end_notification` (`ipc_handler.rs:94`) en fonction partagée
- [x] Given un message vide/absent, when notif, then fallback au comportement actuel (« needs input ») — jamais de body vide
- [x] Le message est sanitizé par le chemin existant (`sanitize_applescript_body` macOS, terminateur `--` Linux) ; Windows reste le stub existant (cohérence plateforme documentée)
- [x] Given `ai.stop` ou `ai.prompt_submit` sur la même session, when transition, then le message stocké est cleared (pas de question fantôme)
- [x] Le message vient de texte terminal untrusted : jamais interprété, uniquement affiché

#### US-017: Mapping PID agent → surface (socle du Tier 2)
**Description:** As a dev orchestrateur, I want que Paneflow sache dans quelle pane tourne chaque session agent so that l'état `WaitingForInput` soit attribuable à une pane précise (glow, jump, peek en dépendent).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given un event `ai.*` portant un `pid`, when traité, then le PID est résolu vers un `surface_id` par remontée de la chaîne parent-PID jusqu'à un `terminal.child_pid` connu (`pty_session.rs:239`) — Linux via `/proc/<pid>/stat` (ppid), macOS via `libproc` (pattern `ports.rs`), Windows via Toolhelp32 snapshot
- [x] Given un agent lancé directement par `up` (PID = child_pid), when résolution, then mapping direct sans marche (fast path)
- [x] Given un PID non résoluble (process mort entre l'event et la résolution, ou ancêtre hors Paneflow), when résolution, then la session reste au niveau workspace (comportement actuel) — dégradation gracieuse, jamais d'erreur visible
- [x] Le mapping est caché dans `AgentSession` (nouveau champ `surface_id: Option<u64>`) et invalidé par le stale-PID sweep existant (`workspace/mod.rs:797`)
- [x] La résolution s'exécute hors render-thread (la marche `/proc` est de l'I/O) et son résultat est déposé sur le main thread (pattern event-driven existant)
- [x] Un test unitaire couvre la marche d'ancêtres avec une chaîne simulée (pid→ppid mockée) ; le test de la plateforme réelle est `#[cfg]`-gated par OS

#### US-018: Glow `WaitingForInput` sur la pane + indicateur tab
**Description:** As a dev orchestrateur, I want que les panes dont l'agent attend une réponse attirent l'œil dans la grille so that je repère en < 2 s qui me sollicite, sans scanner 8 scrollbacks.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [x] Given une pane dont une session mappée est `WaitingForInput`, when rendu, then sa bordure passe à une couleur d'attention thémée (slot `UiColors`, pas de hex hardcodé) — mécanisme : le `border_color` conditionnel existant du ring actif (`pane.rs:1617`), étendu d'un état ; AUCUNE altération des panes inactives (règle : amplifier, jamais dégrader)
- [x] Given la pane est à la fois active (focus) et waiting, when rendu, then l'état focus prime visuellement (le ring actif reste lisible) — priorité documentée dans le code
- [x] Given la transition vers `Thinking`/`Finished`, when rendu, then le glow disparaît au prochain frame (piloté par l'état, pas par timer)
- [x] Given une session NON mappée à une surface (fallback workspace d'US-017), when rendu, then aucun glow erroné sur une pane arbitraire — le glow exige un mapping résolu
- [x] L'onglet de la pane dans la tab bar porte un point d'attention de même slot couleur (parité info quand la pane est cachée derrière un tab inactif)
- [x] Vérification visuelle manuelle sur les thèmes One Dark et PaneFlow Light (contraste suffisant sur les deux)

#### US-019: Action « jump to next waiting agent »
**Description:** As a dev orchestrateur, I want un raccourci qui me téléporte vers la prochaine pane en attente d'input (cross-workspace) so that je traite la file des agents bloqués sans chercher.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [x] Given 2 panes waiting dans 2 workspaces différents, when je déclenche `JumpNextWaiting`, then le focus va à la première (ordre stable : workspace index puis ordre layout), et le workspace est switché si nécessaire ; un second déclenchement va à la suivante (cycle)
- [x] L'action est enregistrée selon le triptyque existant : `actions!` (`app/actions.rs:9`), `ActionMeta` (`registry.rs:40`), `DefaultBinding` (`defaults.rs:11`) avec `key = "secondary-shift-j"` (`secondary` = convention cross-platform existante), `context: None` (global), remappable via `shortcuts` config
- [x] Given aucune pane waiting, when déclenché, then no-op silencieux (pas de toast d'erreur — l'absence de file est la bonne nouvelle)
- [x] Given la pane waiting est dans un tab non visible d'une pane multi-tabs, when jump, then le tab est activé (pas seulement la pane)
- [x] Le cycle ignore les sessions non mappées à une surface (cohérence US-018)

#### US-020: Peek overlay — lire la question sans focus
**Description:** As a dev orchestrateur, I want voir la question posée par l'agent en overlay sur sa pane so that je décide (urgent ou pas) sans quitter ma pane courante.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016, US-017

**Acceptance Criteria:**
- [x] Given une pane `WaitingForInput` avec message stocké, when rendu, then un badge overlay compact apparaît sur la pane (pattern `absolute().top_1().right_1()` du search overlay, `terminal/view.rs:936`) affichant la première ligne du message (tronquée ~80 chars, ellipsis)
- [x] Given un hover (ou focus clavier) sur le badge, when interaction, then le message complet (≤ 512 chars) s'affiche dans un panneau étendu — fond `ui.overlay` (pattern theme_picker), `whitespace_nowrap`/`overflow_hidden` respectés (gotcha uniform_list documenté)
- [x] Given la transition hors `WaitingForInput`, when rendu, then l'overlay disparaît (même cycle de vie que le glow US-018)
- [x] Given un message absent (session waiting sans message), when rendu, then badge générique « waiting for input » — pas d'overlay vide
- [x] Le texte affiché est inerte : pas de liens cliquables, pas d'interprétation ANSI — texte brut sanitizé (untrusted)
- [x] Vérification visuelle manuelle : l'overlay ne masque pas la dernière ligne du prompt de l'agent (placement top-right, pane ≥ 80 px)

## Functional Requirements

- FR-01: La CLI doit exposer `focus`, `split --target`, `send --broadcast`, `key`, `send --submit`, `flow run|--dry-run` avec la grille d'exit codes existante (0/1/3/4).
- FR-02: Quand un spec de pane déclare `worktree`, le système doit créer ou réutiliser le worktree git correspondant avant le spawn du PTY, et refuser atomiquement le `up` si la validation échoue.
- FR-03: Le système ne doit JAMAIS soumettre de texte (`\r`) dans une pane sans la conjonction du gate `PANEFLOW_IPC_SCRIPTING=1` ET d'un opt-in explicite (`--submit` CLI ou `submit = true` dans un fichier user-authored).
- FR-04: Le système ne doit JAMAIS supprimer une branche git, ni un worktree contenant des modifications non commitées, ni un worktree qu'il n'a pas créé.
- FR-05: Le moteur flow doit détecter cycles, références inconnues et dépassements de caps au parse, avant tout appel IPC mutateur.
- FR-06: L'arrêt du moteur flow (Ctrl-C, crash, instance injoignable) doit laisser les panes et agents vivants — le moteur orchestre, il ne possède pas les processus.
- FR-07: Les events `ai.*` portant un PID doivent être attribués à une `surface_id` quand la chaîne d'ancêtres le permet, avec dégradation gracieuse au niveau workspace sinon.
- FR-08: Tout texte issu du terminal (captures, messages de notification) est UNTRUSTED : affiché ou substitué verbatim, jamais interprété ni exécuté.
- FR-09: Toute opération git/FS déclenchée par ces features s'exécute hors du render-thread GPUI.

## Non-Functional Requirements

- **Performance :** parse + validation d'un `flow.toml` de 32 steps < 50 ms ; poll des barriers = réutilisation du cycle `wait` existant (500 ms, fenêtre 500 lignes) sans nouveau poll ajouté ; glow/overlay = zéro coût de rendu quand aucune session agent n'existe (early-return avant tout calcul par-pane).
- **Latence :** résolution PID→surface < 100 ms hors render-thread ; notif desktop émise < 1 s après réception du hook (parité existant).
- **Sécurité :** gate scripting inchangé pour toute injection ; soumission = double gate (FR-03) ; messages sanitizés par les chemins existants (AppleScript strip, `--` Linux) ; `git` invoqué avec args en tableau (jamais de shell-interpolation des noms de branches) ; slugs de path filesystem-safe.
- **Fiabilité :** `up` avec worktrees reste fail-atomique à la validation ; teardown best-effort avec timeout 10 s par opération git ; `worktree prune` au démarrage best-effort (échec silencieux loggé) ; 0 worktree orphelin créé par Paneflow après 20 cycles up/close en conditions nominales.
- **Cross-platform :** chaque story compile et a un comportement défini sur Linux/macOS/Windows ; les stubs Windows (notif desktop, vérification de ports) sont documentés et cohérents avec les stubs existants ; aucun test ne dépend d'un chemin POSIX hardcodé.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Branche verrouillée | `worktree = "x"` mais branche checked-out ailleurs | Échec du spawn de la pane, path du worktree existant cité ; détecté en `--dry-run` | "branch 'x' already checked out at <path>" |
| 2 | Teardown sale | Workspace fermé, worktree avec changements non commités | Worktree conservé, jamais supprimé | "worktree kept: uncommitted changes in <path>" |
| 3 | Cycle DAG | `needs` circulaire dans flow.toml | Erreur au parse, exit 1, aucun appel IPC | "dependency cycle: A → B → A" |
| 4 | Pattern jamais matché | `ready.pattern` introuvable | FAILED au `timeout_secs` du step, fail-fast ou continue selon config, exit 4 | "step 'X' timed out after Ns waiting for /pattern/" |
| 5 | Pane fermée mid-flow | Humain ferme une pane pendant un barrier | Step FAILED immédiat, pas de poll fantôme | "step 'X' failed: pane closed" |
| 6 | Caps dépassés | `foreach` × steps > MAX_PANES=32 | Erreur au parse (statique) ou step FAILED (dynamique), jamais de spawn partiel silencieux | "flow needs N panes, exceeds MAX_PANES (32)" |
| 7 | Gate scripting absent | flow avec `submit = true`, env var absente | Refus avant exécution (run ET dry-run) | "this flow submits prompts: launch Paneflow with PANEFLOW_IPC_SCRIPTING=1" |
| 8 | PID non mappé | Agent lancé via shell imbriqué exotique, chaîne d'ancêtres cassée | Session au niveau workspace (comportement actuel), pas de glow erroné | — (silencieux, loggé debug) |
| 9 | Variable de capture manquante | Step consommant `${var}` d'un step SKIPPED | Step FAILED, variable citée | "undefined variable ${var} (step 'X' was skipped)" |
| 10 | Instance injoignable | Paneflow quitté pendant un flow | Rapport partiel, exit 1, pas de retry infini | "paneflow instance unreachable, aborting (partial report above)" |
| 11 | `.env` absent | `copy_env = true` (défaut), aucun fichier `.env*` | Copie silencieusement vide, aucun warning | — |
| 12 | Setup échoue | `setup` exit ≠ 0 ou timeout | Pane démarre quand même, warning émis | "setup failed in <path> (exit N) — agent started anyway" |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | `ready.pattern` fragile (l'output des agents CLI change entre versions) | High | Med | Timeout obligatoire par step ; PTY contrôlé = pas de buffering ; documenter des patterns robustes (ex. ancrer sur les marqueurs stables des CLI) ; `capture` pour diagnostiquer |
| 2 | La soumission scriptée érode le contrat human-in-loop | Med | High | Double gate (FR-03), défaut non-soumis partout, refus de `\r` conservé sur `send_keystroke`, erreur dès le parse si gate absent — jamais de dégradation silencieuse |
| 3 | Opérations git (worktree add/remove/status) lentes sur gros repos → jank | Med | Med | Tout via `run_with_timeout` hors render-thread (pattern audit 2026-06-04), timeouts 10-300 s selon op, états UI optimistes avec rollback |
| 4 | Marche d'ancêtres PID racée (process mort/réutilisé entre event et résolution) | Med | Low | Résolution best-effort + invalidation par le stale-PID sweep existant ; jamais de glow sans mapping confirmé |
| 5 | Scope : 20 stories = plafond de PRD | Med | Med | EP-004 découplé (livrable indépendamment), EP-003 P1 partiel (US-013/014/015 différables après US-010/011/012) ; pas de nouvelle story sans en retirer une |
| 6 | Windows : moins testé (pas de CI GUI, stubs ports/notif) | Med | Med | Allocation de ports arithmétique documentée, notif stub cohérent ; revue d'inspection dédiée (le host Linux ne compile pas les branches cfg(windows) GPUI — limite connue) |

## Non-Goals

- **Pas de kill/restart d'agents par le flow engine** : le moteur orchestre (spawn, wait, feed) mais ne tue jamais un process — l'humain ferme les panes. Pas de `restart_policy` à la process-compose en v1 (revisitable si le dogfooding le réclame).
- **Pas d'isolation conteneur** (Docker/namespaces à la container-use) : worktrees = isolation filesystem git uniquement. Hors scope durablement (philosophie léger/natif).
- **Pas de dashboard de supervision séparé** : l'awareness vit DANS la grille (glow, peek, jump). Un control plane dédié relève du mode Agents, hors scope de ce PRD.
- **Pas de tracking coût/tokens par agent** : les hooks n'exposent pas les tokens (issue claude-code #22625 ouverte) ; parser le scrollback serait brittle. Revisitable quand les CLI exposeront la donnée.
- **Pas de remote/push notifications** (Telegram, mobile) : produit séparé, hors scope.
- **Pas d'auto-install de dépendances dans les worktrees** sans `setup` explicite : Paneflow ne devine jamais le gestionnaire de paquets.
- **Pas de modification des modes Diff et Agents** : surface terminal uniquement.

## Files NOT to Modify

- `src-app/src/terminal/element/**` — pipeline de rendu bas niveau (block-char fix indépendant), aucun besoin pour ces features
- `crates/paneflow-mcp/**` — bridge MCP read-only par design ; le flow engine passe par l'IPC, pas par le MCP
- `src-app/src/diff/**` et `src-app/src/agents*/**` — modes Diff et Agents explicitement hors scope
- `src-app/src/update/**`, `crates/paneflow-shim/**` (hors ajout de tests) — self-update et shim audités/clos (EP-001/EP-006 audit), ne pas rouvrir
- `src-app/src/workspace/git.rs` helpers existants (`find_git_dir`, `resolve_repo_root`) — réutiliser tels quels, étendre dans un module worktree dédié plutôt que modifier les fonctions auditées

## Technical Considerations

- **Architecture flow engine :** moteur côté process CLI (`paneflow flow`) pilotant l'instance via IPC public uniquement — recommandé (testable sans GUI, crash-isolé, dogfoode l'IPC). Alternative : moteur in-app sur le main thread GPUI — rejetée (couplage, jank, pas de Ctrl-C naturel). Engineering à confirmer : le cycle poll 500 ms × 32 steps reste-t-il sous le budget de l'IPC dispatch (10 ms) ?
- **Module worktree :** nouveau `src-app/src/workspace/worktree.rs` (création/copie/teardown/prune) appuyé sur `git.rs` helpers + `paneflow_process`. Question : marquage « créé par Paneflow » — fichier sentinelle dans le worktree vs état dans `session.json` ? Recommandé : état dans le workspace persisté (survit au crash via session restore), sentinelle en backup.
- **Schéma flow :** `[[step]]` + `needs` plat (pas de phases imbriquées) — recommandé pour la détection statique. `pane` embarque le `PaneSpec` existant (deny_unknown_fields). Question : un flow peut-il cibler un workspace EXISTANT (panes déjà ouvertes) ou crée-t-il toujours le sien ? Recommandé v1 : toujours son propre workspace (sémantique `workspace.up`, plus simple à raisonner).
- **Mapping PID→surface :** la marche d'ancêtres réutilise les patterns per-OS de `ports.rs` (`/proc`, libproc, Toolhelp32). Question : cache du mapping invalidé par sweep — TTL additionnel nécessaire si un PID est réutilisé par l'OS entre deux sweeps ? (fenêtre courte, risque faible — à trancher en implémentation).
- **Glow :** étendre le `border_color` conditionnel de `pane.rs:1617` avec une priorité d'états (focus > waiting > rien) et un slot `UiColors::attention` thémé. Pas de nouvelle passe de rendu.
- **`submit` IPC :** param booléen sur `surface.send_text` (rétrocompatible, absent = false) plutôt qu'une méthode `surface.submit_text` séparée — recommandé (un seul chemin de validation du gate). Engineering à confirmer côté revue sécurité (le refus `\r` de `send_keystroke` reste l'invariant).

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Commandes pour un pipeline 3 agents chaînés | ~15+ lignes de bash maison | 1 (`flow run`) | livraison EP-003 | démo reproductible dans `docs/` |
| Setup d'un agent isolé sur worktree | ≥ 5 commandes manuelles | 1 ligne de TOML | livraison EP-002 | démo reproductible |
| Worktrees orphelins après 20 cycles up/close | N/A (feature absente) | 0 | livraison US-009 | test manuel scripté (`git worktree list`) |
| Temps pour identifier la pane qui attend (8 panes) | scan visuel complet (~10-30 s) | < 2 s (glow) + 1 raccourci (jump) | livraison US-018/019 | vérification manuelle chronométrée |
| Notif desktop avec contexte | 0 % (body fixe « agent finished ») | 100 % des `ai.notification` avec message | livraison US-016 | inspection des notifs en dogfooding |
| Régressions CI | 0 | 0 (4 legs release verts) | chaque story | pipeline release existant |

## Open Questions

- **Patterns `ready` recommandés par agent CLI** : quels marqueurs stables ancrer pour Claude Code / Codex / OpenCode (fin de tour, prompt d'input) ? À documenter en EP-003 via dogfooding dans `docs/user/scripting.md` et `docs/user/scripting/reference.md` (US-015).
- **`UiColors::attention`** : réutiliser un slot existant du thème ou ajouter un slot aux 6 thèmes bundlés ? Décision au moment d'US-018 (impact : fichiers thème + hot-reload).
- **Raccourci `JumpNextWaiting`** : `secondary-shift-j` proposé — vérifier la collision avec les 57 actions existantes au moment de l'implémentation (US-019).
- **Flow ciblant un workspace existant** (vs toujours créer le sien) : différé v2 — réévaluer après le premier dogfooding réel d'EP-003.
[/PRD]
