[PRD]
# PRD: CLI d'orchestration d'agents (2026-Q3)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-09 | Arthur Jean | Initial draft. Construit une vraie surface CLI au-dessus du serveur IPC JSON-RPC existant : socle client (clap), `paneflow up` (workspaces d'agents déclaratifs), `paneflow wait` (orchestration), `paneflow hooks setup` (adoption). Recherche codebase `file:line`-sourcée + état de l'art (WezTerm/Zellij/Kitty/tmux/cmux). |

## Problem Statement

Paneflow expose déjà un serveur IPC JSON-RPC riche (`src-app/src/ipc.rs`, `src-app/src/app/ipc_handler.rs`) : `workspace.create/list/select/close/restore_layout`, `surface.list/read/search/rename/split/send_text/send_keystroke`, et toute la surface `ai.*`. Mais il n'a **aucun client CLI**.

1. **La surface scriptable est inatteignable depuis un shell.** La seule CLI existante est `paneflow mcp install|uninstall|status` + `--update-and-exit` + `--help/--version`, parsées à la main dans `main.rs:1199-1313` sans aucun crate d'arguments (`grep clap|argh|pico-args` sur tous les `Cargo.toml` = 0 résultat). Tout le reste exige `socat`/`nc` avec un framing JSON-RPC manuel, inutilisable par un développeur ou un script.
2. **Asymétrie inversée read/write.** Via le bridge MCP read-only (`crates/paneflow-mcp`), les agents IA peuvent LIRE les autres panes (`list_panes`/`read_pane`/`search_pane`), mais le développeur humain ne peut PAS piloter son propre workspace en ligne de commande. Celui qui orchestre est moins équipé que les agents qu'il orchestre.
3. **Pas de primitive d'orchestration multi-agents.** Lancer N agents sur N repos puis enchaîner sur la complétion exige aujourd'hui de cliquer dans la GUI pane par pane. Aucun équivalent de "attends que ce pane affiche X" (la primitive `wait` que tmux/Zellij/WezTerm rendent scriptable et que les workflows d'agents réclament).
4. **Adoption à friction.** Le pipeline AI-hook (notifications d'état d'agent vers la sidebar) existe de bout en bout mais son installation est **éphémère** : le shim écrit les hooks dans le CWD du projet et les retire à la fin du process (`crates/paneflow-shim/src/hooks.rs:285-346`). Il n'existe aucune commande qui installe des hooks persistants user-scope en une fois, là où le concurrent direct (cmux `hooks setup`) en a fait son pattern d'adoption le plus frictionless.

**Why now:** le serveur IPC, le moteur d'install idempotent (`crates/paneflow-mcp-install`), le launcher d'agents (`src-app/src/agent_launcher.rs`) et le binaire de hook (`crates/paneflow-ai-hook`) sont tous déjà là et stables. Le travail restant est majoritairement du plumbing de surface : exposer ce qui existe. C'est le levier le plus élevé du repositionnement "cockpit d'agents cross-platform" — un terrain où cmux est mac-only et où tmux/WezTerm sont des multiplexeurs génériques sans intégration agent. La fenêtre est ouverte tant que la base de code reste petite, avant le durcissement du port macOS/Windows.

## Overview

Ce PRD construit une CLI d'**orchestration d'agents**, pas un multiplexeur scriptable de plus. La ligne directrice : minimale sur le contrôle de terminal générique, maximale sur l'orchestration d'agents. Quatre epics, du socle vers la valeur différenciante.

Le socle (EP-001) extrait le client IPC portable (`ipc_client.rs`) dans une crate partagée `paneflow-ipc-client` sans dépendance GPUI, introduit `clap` 4.5 (derive) dans `main.rs` avec `Option<Commands>` pour préserver le fallthrough GUI quand aucune sous-commande n'est passée, et expose des commandes thin sur les méthodes IPC existantes : `ls`, `read`, `search`, `new`, `select`, `split`, `send`. Les sous-commandes tournent avant l'init GPUI (comme `mcp` aujourd'hui), parlent à l'instance en cours via la socket, et sortent. Une couche transverse de **sélecteurs de cible** (`<id>` | `name` | `cmdline:<substr>` | `cwd:<path>`) est partagée par `read`/`search`/`wait`.

La feature phare (EP-002) est `paneflow up <file>` : un workspace d'agents déclaratif ("docker-compose pour agents"). Un fichier TOML décrit N panes, leur layout, leur cwd/repo, et l'agent + prompt pré-rempli de chacun ; `paneflow up` matérialise le tout en un appel. La recherche a révélé que ceci exige une **nouvelle méthode IPC** : aucune méthode actuelle ne lance un agent dans un pane (le launch est GUI-only), et le chemin de matérialisation de layout IPC (`apply_layout_from_json`) ignore les cwd/command par-surface. La nouvelle méthode route via le chemin `spawn_pane_from_surfaces` (le seul qui honore les `SurfaceDefinition`), résout `agent: claude` vers `launch_command` (réutilise `TerminalAgent`, honore le bypass), et pré-remplit le prompt avec `send_text` (sans `\r`, jamais auto-soumis) après une attente de disponibilité bornée.

L'orchestration (EP-003) ajoute `paneflow wait --match <sel> --pattern <regex> [--timeout]` : un poll côté client sur `surface.search` (qui existe déjà), avec codes de sortie distincts (0 = match, non-zéro = timeout). C'est la primitive "Playwright-for-terminals" qui rend les pipelines multi-agents scriptables.

L'adoption (EP-004) ajoute `paneflow hooks setup|status|uninstall <agent>` : un installateur de hooks **persistants user-scope** qui réutilise le moteur idempotent no-clobber + backup + atomic-write de `paneflow-mcp-install`, en pointant le binaire `paneflow-ai-hook` à son **path stable non-versionné**. Une story P2 distincte re-câble la notification desktop de fin de tour, qui a été retirée avec le chat ACP (voir Open Questions — décision utilisateur requise avant de la réactiver).

Décisions clés : crate client partagée plutôt que dépendance sur le binaire MCP ; `clap` avec `Option<Commands>` + intercept `mcp` manuel conservé avant clap ; toml 0.8 (déjà présent) pas 0.9 ; `paneflow up` route via `spawn_pane_from_surfaces` et non `apply_layout_from_json` ; prefill via readiness-poll borné et non timer fixe ; launch-agent IPC gardé same-UID (pas de gate scripting) tandis que `send` reste gaté.

## Goals

| Goal | Phase-1 Target (P0, EP-001/002) | Phase-2 Target (tout) |
|------|---------------------------------|-----------------------|
| Sous-commandes CLI parlant à une instance en cours (baseline : 0) | ≥ 7 (`ls`/`read`/`search`/`new`/`select`/`split`/`send`) | ≥ 11 (+ `up`/`wait`/`hooks setup`/`hooks status`/`hooks uninstall`) |
| Spawn d'un workspace d'agents complet via une commande | `paneflow up` matérialise ≤ 8 panes (cwd + agent + prompt par pane) en 1 appel | idem + `--dry-run` |
| Latence d'une commande read-only (instance lancée) | P95 < 150 ms (1 aller-retour socket, plafond timeout 10 s) | idem sur les 3 OS |
| Primitive d'orchestration `wait` | `wait` bloque sur regex avec timeout + exit code distinct | + matching cross-platform documenté |
| Hooks persistants installés en 1 commande (baseline : éphémère shim only) | — | `hooks setup` idempotent (re-run = diff 0 octet) sur 4 agents |
| Invariant human-in-loop préservé (prompts jamais auto-soumis) | 100 % (aucune méthode n'émet de `\r` pour le compte de l'utilisateur) | 100 % |
| CI verte sur les 4 legs de la matrice de release | requis | requis |

## Target Users

### Paneflow power user / orchestrateur d'agents (dont Arthur)
- **Role:** développeur qui fait tourner plusieurs agents de code (Claude Code, Codex, opencode, Gemini) en parallèle dans des panes, et qui scripte ses workflows.
- **Behaviors:** ouvre les mêmes constellations de panes/repos chaque jour ; enchaîne des étapes "lance l'agent → attends qu'il finisse → lance la suivante" ; vit dans le shell autant que dans la GUI.
- **Pain points:** doit recréer son workspace à la main à chaque session ; n'a aucun moyen scriptable de savoir quand un agent a fini ; le pilotage CLI exige `socat` + JSON-RPC manuel.
- **Current workaround:** clics manuels dans la GUI ; wrappers `tmux` ad hoc (qui perdent l'intégration agent et la sidebar) ; polling `read_pane` via MCP depuis un autre agent.
- **Success looks like:** `paneflow up dev.toml` reconstruit son cockpit d'agents en une commande ; `paneflow wait` enchaîne les étapes ; tout est scriptable sans quitter le terminal.

### Auteur de scripts / CI locale orchestrant des agents
- **Role:** développeur qui écrit des scripts shell/Makefile pilotant des agents pour des tâches répétables (revue, refactor de masse, batch sur N repos).
- **Behaviors:** consomme de la sortie machine (JSON), branche sur des codes de sortie, veut du déterminisme.
- **Pain points:** la sortie de terminal est polluée par les séquences ANSI ; pas de sortie JSON ; pas de primitive de synchronisation.
- **Current workaround:** `capture-pane` tmux + `sed` pour stripper l'ANSI + grep sur des patterns de prompt — fragile.
- **Success looks like:** `paneflow ls`/`search` émettent du JSON par défaut ; `paneflow wait` renvoie un code de sortie fiable ; les scripts ne parsent plus de texte tabulaire.

## Research Findings

Key findings que ce PRD applique (rapports d'agents complets dans le transcript de session) :

### Competitive Context
- **WezTerm** (`wezterm cli`) : la CLI de contrôle GUI la plus complète (`list --format json`, `send-text`, `get-text`, `spawn`, `split-pane`). Référence pour la sortie JSON-par-défaut. Pas de notion de hooks agents ni de workspace déclaratif. Paneflow ne vise PAS sa parité sur resize/zoom/activate-direction.
- **Zellij** : layouts KDL déclaratifs + `zellij run` (blocking pane, human-in-the-loop) + `zellij pipe`/plugins WASM. Le déclaratif existe mais n'est pas orienté agents.
- **Kitty** : seul à faire du matching par métadonnée (`--match cmdline:nvim`, `--match cwd:`). Inspire le sélecteur de cible de ce PRD ; Paneflow a déjà le champ `cmd` dans `surface.list`.
- **tmux** : `send-keys`/`capture-pane` ubiquitaires, devenus le substrat de facto de l'orchestration d'agents en 2025, mais sortie non-JSON, pollution ANSI, API peu ergonomique.
- **cmux** (concurrent direct, mac-only) : socket API JSON + `cmux hooks setup --agent` (le pattern d'adoption frictionless que EP-004 vise). Pas de bridge MCP read-only ; pas de build Linux/Windows (le moat de Paneflow).
- **Market gap:** aucun outil cross-platform n'offre un workspace d'agents déclaratif avec prompts pré-remplis non-soumis (human-in-loop). C'est le différenciant de EP-002.

### Best Practices Applied
- **Sortie JSON par défaut** sur les commandes d'introspection (la capacité CLI #1 plébiscitée). `read` reste du texte brut (c'est du scrollback), `ls`/`search` émettent du JSON, `--human` pour une table.
- **`Option<Commands>` clap derive** : un champ `#[command(subcommand)]` nu force automatiquement `subcommand_required(true)` + `arg_required_else_help(true)` ; il FAUT `Option<Commands>` pour que "aucune sous-commande = lance la GUI" (doc derive clap, confirmé par ctx7). Intercept positionnel `mcp` conservé avant clap plutôt que `external_subcommand` (qui masque les typos).
- **toml 0.8** (déjà `src-app/Cargo.toml:106`) pour la désérialisation serde ; ne pas migrer vers 0.9 (rebasé sur toml_edit, spec 1.1, aucun gain pour de la pure désérialisation). `#[serde(deny_unknown_fields)]` pour transformer un typo de clé en erreur explicite.
- **Prefill human-in-loop** : `send_text` (sans `\r`, `view.rs:626`) pose le texte dans la box d'input ; l'utilisateur presse Entrée. Référence d'implémentation : `diff/view.rs::send_to_review` (launch CLI → délai → `send_text`).
- **Moteur d'install réutilisable** : `write_if_changed` (idempotent + backup + atomic, `io.rs:64`), `read_json_or_default`/`merge_json_entry`/`remove_json_entry` (`merge.rs`), `upsert_toml_entry` pour Codex — tous dans `paneflow-mcp-install`.

*Full research sources available in session transcript.*

## Assumptions & Constraints

### Assumptions (to validate)
- Le poll côté client de `wait` (intervalle ≥ 500 ms, 1 connexion à la fois) ne sature pas le cap serveur de 16 connexions simultanées (`ipc.rs:143-150`). À valider en AC, pas à supposer.
- Une attente de disponibilité bornée (readiness-poll) avant le prefill élimine la perte de prompt observée avec le timer fixe 1800 ms (`diff/view.rs:100`). À valider sous charge (N panes).
- `clap` 4.5 (derive) ajoute un poids binaire négligeable et n'affecte pas le démarrage GUI (parse only en présence d'une sous-commande). À mesurer.
- Le matching `cmdline:` est fiable sur Linux (argv complet via `/proc`, `pty_session.rs:1169`) mais dégradé sur macOS/Windows (basename seul, `pty_session.rs:1196`). On assume que `cwd:` + `name` couvrent le cas macOS/Windows ; à documenter, pas à masquer.
- Les hooks persistants user-scope coexistent proprement avec les hooks éphémères du shim sans double-firing une fois la règle d'autorité tranchée (US-018).

### Hard Constraints
- **Rust + GPUI (fork Zed pinné `ArthurDEV44/zed@paneflow/markdown-append-fix`)** — ne jamais remplacer GPUI par une dép crates.io ; ne pas toucher le pin du fork dans ce PRD.
- **Cross-platform Linux + macOS + Windows** pour tout nouveau code. Les branches `cfg(windows)` de `paneflow-app` ne sont **pas compilables sur l'hôte Linux** (GPUI exige `windows.h`/`llvm-rc`) — vérifiées par la matrice CI, inspection-only sur le poste dev.
- **Human-in-loop strict** — aucune nouvelle méthode IPC ni commande CLI ne doit soumettre une commande/un prompt pour le compte de l'utilisateur (jamais de `\r` injecté). `surface.send_keystroke` rejette déjà `\r`/`\n` (`ipc_handler.rs:801-806`) ; cet invariant est étendu à `up`.
- **Authentification socket inchangée** — 0600 + peer-UID same-user (`ipc.rs:862-916`). Aucune commande ne doit l'affaiblir. La nouvelle méthode launch-agent reste same-UID.
- **Clippy lints** : `panic = "deny"`, `unwrap_used`/`expect_used` = `warn` ; nouveau `unwrap()`/`expect()` suit la convention (`?`, `ok_or`, `match`, ou `expect("invariant documenté")`).
- **`cargo fmt --check` est un gate CI sur les 4 legs** — lancer `cargo fmt` via Bash avant chaque commit/push touchant du Rust (le hook rustfmt du projet réordonne les imports différemment du `cargo fmt` canonique).
- **Pas de télémétrie desktop** — aucune métrique de succès ne peut reposer sur du phone-home d'usage (analytics web-only par décision produit ; `PANEFLOW_NO_TELEMETRY` existe). Les métriques sont perf/CI-mesurables ou proxies observables.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - gate de formatage (lancer `cargo fmt` d'abord s'il signale un diff ; CI le lance sur les 4 legs).
- `cargo clippy --workspace --all-targets -- -D warnings` - gate de lint (aucun nouveau warning `unwrap`/`expect` ; `panic`/`unimplemented`/`dbg` déniés).
- `cargo test --workspace` - tous les tests du workspace, dont les tests de régression ajoutés par chaque story.
- `cargo build --workspace` - le build debug compile.
- Pour les stories touchant Windows : CI verte sur les 4 legs de la matrice de release (Linux x86_64, Linux aarch64, macOS aarch64, Windows x86_64) — les chemins `cfg(windows)` ne compilent pas sur l'hôte Linux dev.

## Epics & User Stories

### EP-001: Socle CLI et surface de lecture (Phase 1)

Extraire le client IPC en crate partagée, introduire `clap` sans casser le démarrage GUI ni l'intercept `mcp`, et exposer les méthodes IPC existantes en sous-commandes thin avec sortie machine-readable et sélecteurs de cible.

**Definition of Done:** un développeur peut, sans `socat`, lister/lire/chercher dans les panes et créer/sélectionner/splitter des workspaces depuis le shell, contre une instance en cours, avec un échec propre quand aucune instance ne tourne ; `paneflow` sans argument lance la GUI exactement comme avant.

#### US-001: Extraire `paneflow-ipc-client` (crate partagée)
**Description:** As a maintainer, I want le transport IPC blocking dans une crate sans dépendance GPUI so that la CLI et le bridge MCP partagent un seul client testé.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given le transport extrait (`crates/paneflow-ipc-client/src/lib.rs` : `IpcClient`, `IpcTransport::call`, `resolve_socket_path`, framing JSON-RPC), when on extrait une crate `crates/paneflow-ipc-client`, then `paneflow-mcp` la consomme et compile sans changement de comportement.
- [x] La nouvelle crate ne dépend ni de GPUI ni d'aucune crate `src-app` (uniquement `interprocess` + `serde_json`), vérifié par `cargo tree -p paneflow-ipc-client`.
- [x] `resolve_socket_path` honore `PANEFLOW_SOCKET_PATH` absolu d'abord, puis le fallback XDG/TMPDIR (Unix) / named-pipe (Windows), identique à l'actuel (`ipc_client.rs:159-196`).
- [x] Given le socket absent, when `call` est invoqué, then l'erreur contient `unreachable` et mentionne "is Paneflow running?" (parité `ipc_client.rs:61-66`), couvert par un test.
- [x] `cargo clippy` ne montre aucun nouveau warning `unwrap`/`expect` issu de l'extraction.

#### US-002: Intégration `clap` 4.5 avec fallthrough GUI
**Description:** As a maintainer, I want un parseur d'arguments structuré qui coexiste avec l'intercept `mcp` et le démarrage GUI so that on peut ajouter 11+ sous-commandes sans parsing manuel fragile.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] `clap = { version = "4.5", features = ["derive"] }` ajouté à `src-app/Cargo.toml` ; le `Cli` racine utilise `command: Option<Commands>` (pas `Commands` nu).
- [x] Given `paneflow` sans argument, when le binaire démarre, then il tombe dans le bras `None` et lance la GUI exactement comme le fallthrough actuel (`main.rs:1332`) — aucun help/erreur affiché.
- [x] Given `paneflow mcp install` (et autres `mcp …`), when le binaire démarre, then l'intercept positionnel manuel existant (`main.rs:1206,1301-1313`) s'exécute AVANT clap et `mcp` n'est jamais routé par clap.
- [x] Given `--help`/`--version`/`--update-and-exit`, when invoqués, then ils conservent leur comportement et leurs codes de sortie actuels (`--update-and-exit` garde 0-5) — pas de double-parse ni de capture par un scan global.
- [x] Given une sous-commande inconnue ou un mauvais usage, when parsée, then `try_parse()` est utilisé et le code de sortie est géré explicitement (les sous-commandes métier retournent leur propre `i32`, pas le 2 réservé de clap).
- [x] Les sous-commandes CLI ne touchent jamais `ipc::start_server` ni le singleton guard (`ipc.rs:225`) — elles ouvrent un client et sortent avant init GPUI.

#### US-003: Sélecteur de cible transverse (`id` | `name` | `cmdline:` | `cwd:`)
**Description:** As a CLI user, I want cibler un pane par son process ou son répertoire plutôt que par un id numérique so that je peux viser "le pane où tourne claude" quand j'orchestre N agents.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** US-001

**Acceptance Criteria:**
- [x] Un parseur de sélecteur accepte : `<u64>` (surface_id), une chaîne nue (name exact puis préfixe désambiguïsé), `cmdline:<substr>` (match sur le champ `cmd` de `surface.list`), `cwd:<path>` (match sur le `cwd`).
- [x] La résolution interroge `surface.list` (`ipc_handler.rs:621-645`) puis filtre côté client ; le champ `cmd` provient de `foreground_command()` (`pty_session.rs:1169`).
- [x] Given un sélecteur qui matche plusieurs panes, when résolu sans flag explicite, then la commande échoue avec un message listant les candidats (id + name + cmd) et un code de sortie non-zéro dédié (pas de choix silencieux).
- [x] Given un sélecteur qui ne matche aucun pane, when résolu, then échec avec message clair et code non-zéro.
- [x] Tests unitaires : exact/préfixe/cmdline/cwd, ambiguïté, no-match.

#### US-004: Commandes de lecture `ls` / `read` / `search`
**Description:** As a script author, I want introspection machine-readable des panes so that mes scripts branchent sans parser de texte tabulaire.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** US-001, US-002, US-003

**Acceptance Criteria:**
- [x] `paneflow ls` appelle `surface.list` et émet du JSON par défaut (tous les champs : `surface_id`, `name`, `title`, `cwd`, `cmd`, `workspace`) ; `--human` produit une table alignée.
- [x] `paneflow read <target> [--lines N] [--offset N]` appelle `surface.read` (`ipc_handler.rs:646-688`) et émet le texte brut du scrollback par défaut ; `--json` enveloppe `{text, lines, total_lines, eof}`.
- [x] `paneflow search <target> <pattern> [--max N]` appelle `surface.search` et émet `{matches:[{line,text}], truncated}` en JSON par défaut.
- [x] Given un `--lines`/`--max` hors bornes, when envoyé, then la commande relaie le clamp serveur (1..4000 / 1..1000) sans erreur, et `--offset` > total renvoie l'erreur serveur `-32602` mappée en message clair + code non-zéro.
- [x] Given une instance non lancée, when n'importe laquelle est invoquée, then échec avec "is Paneflow running?" et code non-zéro (réutilise US-001).

#### US-005: Commandes de contrôle `new` / `select` / `split`
**Description:** As a CLI user, I want créer/sélectionner/splitter des workspaces depuis le shell so that je prépare mon espace sans la souris.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** US-001, US-002

**Acceptance Criteria:**
- [x] `paneflow new [--name N] [--cwd DIR]` appelle `workspace.create` (`ipc_handler.rs:505-583`) et imprime `{index, title, panes}` en JSON.
- [x] `paneflow select <index>` appelle `workspace.select` ; `paneflow split <h|v>` appelle `surface.split` (`value_enum` avec alias `h`/`horizontal`, `v`/`vertical`).
- [x] Given un `--cwd` inexistant, when `new` est appelé, then l'erreur serveur `-32602` (canonicalize/must-be-dir, `ipc_handler.rs:1296-1310`) est mappée en message clair + code non-zéro.
- [x] Given `split` quand le cap `MAX_PANES` est atteint, when appelé, then l'erreur serveur est relayée proprement (pas de panic, code non-zéro).

#### US-006: Commande `send` (gated) + échec propre hors instance
**Description:** As a power user with scripting enabled, I want injecter du texte dans un pane sans le soumettre so that je pré-remplis sans violer le human-in-loop.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** US-001, US-002, US-003

**Acceptance Criteria:**
- [x] `paneflow send <target> <text>` appelle `surface.send_text` (`ipc_handler.rs:742-781`) ; le texte est injecté sans `\r` (jamais auto-soumis).
- [x] Given `PANEFLOW_IPC_SCRIPTING` non égal à `"1"`, when `send` est appelé, then l'erreur serveur `-32601` est mappée en message explicite indiquant d'activer le gate, avec code non-zéro (le gate est lu côté serveur à chaque appel, `ipc_handler.rs:41-51`).
- [x] Given un texte > 64 KiB, when envoyé, then l'erreur serveur est relayée proprement.
- [x] La commande documente explicitement (help text) qu'elle ne soumet jamais — l'utilisateur/agent doit valider.

---

### EP-002: `paneflow up` — workspaces d'agents déclaratifs (Phase 1, phare)

Un fichier TOML décrit un workspace complet (layout + cwd/repo + agent + prompt par pane) ; `paneflow up` le matérialise en un appel, prompts pré-remplis non-soumis. Exige une nouvelle méthode IPC car aucune n'existe pour lancer un agent dans un pane et le chemin layout IPC actuel ignore les cwd/command par-surface.

**Definition of Done:** `paneflow up dev.toml` crée un workspace dont chaque pane démarre dans son repo déclaré, lance l'agent déclaré (bypass honoré) et affiche son prompt pré-rempli sans le soumettre ; un repo manquant ou un agent absent du PATH fait échouer atomiquement la commande en nommant le pane fautif.

#### US-007: Schéma + loader `paneflow.workspace.toml`
**Description:** As a CLI user, I want décrire mon workspace d'agents en TOML so that je versionne et rejoue mon cockpit.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Un module de schéma serde (dans `src-app` ou une crate dédiée, réutilisant `toml = "0.8"` + serde déjà présents, `src-app/Cargo.toml:106,120`) décrit : `name?`, `layout` (preset `even_h`/`even_v`/`main_vertical`/`tiled` OU split explicite direction+ratios+children), et une liste de panes `{cwd?, agent?, prompt?, command?, name?}`.
- [x] L'enum `agent` couvre l'ensemble des variantes `TerminalAgent` supportées (16, `agent_launcher.rs:49-66`) + `shell` ; un agent inconnu est une erreur de désérialisation.
- [x] `#[serde(deny_unknown_fields)]` est posé : une clé mal orthographiée (`agnt`, `prmpt`) produit une erreur explicite, pas un champ silencieusement ignoré.
- [x] Une étape de validation post-désérialisation borne le nombre de panes à `MAX_PANES`, vérifie que `agent` et `command` ne sont pas tous deux fournis sur un même pane, et que les ratios de split sont dans les bornes.
- [x] Given un TOML malformé ou hors-bornes, when chargé, then erreur avec ligne/champ fautif et code non-zéro ; test unitaire par cas (clé inconnue, agent inconnu, > MAX_PANES, ratio invalide).

#### US-008: Méthode IPC de matérialisation multi-pane honorant cwd/command par surface
**Description:** As a maintainer, I want une méthode IPC qui crée un workspace multi-pane en respectant le cwd et la commande de chaque surface so that `up` peut placer chaque agent dans son repo.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** US-001

**Acceptance Criteria:**
- [x] Une nouvelle méthode IPC (ex. `workspace.up` / `surface.launch_agent`) accepte un layout + des `SurfaceDefinition` par leaf (`cwd`/`command`/`env`/`name`, `schema.rs:988-1009`) et matérialise chaque pane via le chemin qui honore ces champs (`spawn_pane_from_surfaces`, `session.rs:386-442`), PAS `apply_layout_from_json` qui les ignore (`layout.rs:149-152`).
- [x] Given un layout 2x1 avec deux cwd distincts, when la méthode est appelée, then les deux panes démarrent chacun dans son cwd (et non le cwd par défaut), vérifié par un test d'intégration IPC.
- [x] La méthode reste same-UID (auth peer-cred inchangée) ; elle n'émet jamais de `\r` (le launch passe par le chemin commande, le prompt par `send_text`).
- [x] Given un échec de matérialisation d'un leaf (cwd invalide), when la méthode s'exécute, then le workspace est rollback (pas de pane orphelin, parité `ipc_handler.rs:560-579`) et une erreur structurée nomme le leaf.
- [x] Cap `MAX_WORKSPACES`/`MAX_PANES` appliqué ; un dépassement renvoie une erreur, pas un panic.

#### US-009: Résolution `agent: <kind>` → commande de lancement
**Description:** As a CLI user, I want déclarer `agent: claude` plutôt qu'une commande brute so that le bon CLI est lancé avec le bypass honoré.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** US-007, US-008

**Acceptance Criteria:**
- [x] Le champ `agent` de la spec est résolu vers `launch_command` via `TerminalAgent` (`agent_launcher.rs:284-332`), réutilisant la source de vérité unique (binaire + arguments + préfixe clear).
- [x] Given `agent: claude` avec bypass demandé dans la config, when résolu, then la commande inclut `--permission-mode bypassPermissions` (seul Claude Code honore le bypass, `agent_launcher.rs:287-292`).
- [x] Given un pane avec `command` brut au lieu d'`agent`, when résolu, then la commande brute est utilisée telle quelle.
- [x] Given un `agent` dont le binaire est absent du PATH, when `up` tente le lancement, then échec nommant le pane et l'agent + code non-zéro (réutilise `cli_on_path`, `support.rs:72`).

#### US-010: Prompt pré-rempli non-soumis + disponibilité bornée
**Description:** As a CLI user, I want que mon prompt apparaisse dans l'input de l'agent sans être envoyé so that je relis avant de valider (human-in-loop).

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** US-008

**Acceptance Criteria:**
- [x] Après le lancement de l'agent, le prompt déclaré est injecté via `send_text` (sans `\r`, sans wrap bracketed-paste, `view.rs:626`) — jamais soumis (invariant human-in-loop).
- [x] Le prefill attend une disponibilité bornée (readiness-poll, ex. via `surface.search` sur un marqueur, OU un délai par-agent borné en repli du timer fixe 1800 ms de `diff/view.rs:100`) avant d'écrire, pour éviter la perte de texte dans un buffer pas prêt.
- [x] Given N panes prefillés (N jusqu'à MAX_PANES), when `up` s'exécute, then aucun prompt n'est perdu (test : vérifier que chaque pane contient son prompt non-soumis) — la perte silencieuse documentée à `diff/view.rs:100` ne se reproduit pas.
- [x] Given un readiness-poll qui expire, when le marqueur n'apparaît pas, then le prompt est tout de même injecté (best-effort) et un avertissement est émis, plutôt qu'un échec dur.
- [x] Aucun chemin n'injecte `\r`/`\n` pour soumettre — vérifié par un test sur les octets écrits.

#### US-011: Sous-commande `up` + `--dry-run`
**Description:** As a CLI user, I want `paneflow up dev.toml` so that mon cockpit se reconstruit en une commande.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** US-007, US-008, US-009, US-010

**Acceptance Criteria:**
- [x] `paneflow up <file>` charge le TOML (US-007), construit le `LayoutNode` + les `SurfaceDefinition` (tag serde `type` snake_case `pane`/`split`, `schema.rs:696-721`) et appelle la méthode IPC (US-008), puis imprime ce qui a été créé (index workspace + panes + agents) en JSON.
- [x] `paneflow up <file> --dry-run` valide et imprime le plan (layout résolu + commandes par pane) SANS toucher l'instance.
- [x] Given un fichier absent ou illisible, when `up` est appelé, then erreur claire + code non-zéro.
- [x] Given une instance non lancée, when `up` est appelé (hors dry-run), then échec "is Paneflow running?" + code non-zéro.

#### US-012: Validation cwd/repo par surface + rollback + agent absent
**Description:** As a CLI user, I want un échec atomique et explicite quand un repo manque so that je ne me retrouve pas avec un workspace à moitié monté dans le mauvais dossier.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** US-008, US-011

**Acceptance Criteria:**
- [x] Chaque `cwd` de surface est validé (canonicalize + exists + is-dir) en réutilisant `canonicalize_workspace_cwd` (`ipc_handler.rs:1296-1310`) appliqué par-surface, pas seulement au workspace.
- [x] Given un `cwd` de pane inexistant, when `up` s'exécute, then aucun pane n'est créé (rollback complet) et l'erreur nomme le pane + le chemin fautif.
- [x] Given un agent absent du PATH sur un pane, when `up` s'exécute, then échec atomique nommant le pane (réutilise la détection US-009), pas un workspace partiel.
- [x] Le message d'erreur distingue "cwd invalide" de "agent introuvable" de "instance non lancée".

---

### EP-003: `wait` et primitives d'orchestration (Phase 2)

La primitive "Playwright-for-terminals" qui rend les pipelines multi-agents scriptables : bloquer jusqu'à ce qu'un regex apparaisse dans un pane, avec timeout et codes de sortie distincts.

**Definition of Done:** `paneflow wait --match <sel> --pattern <regex> --timeout <s>` renvoie 0 dès que le pattern matche dans le pane ciblé, un code distinct au timeout, sans saturer le serveur, avec un comportement cross-platform documenté.

#### US-013: Commande `wait` (poll borné, timeout, exit codes)
**Description:** As a script author, I want bloquer jusqu'à ce qu'un pane affiche un motif so that j'enchaîne mes étapes d'orchestration.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** US-001, US-003

**Acceptance Criteria:**
- [x] `paneflow wait --match <sel> --pattern <regex> [--timeout <s>]` résout la cible (US-003) puis poll `surface.search` (`ipc_handler.rs:689-724`) à intervalle ≥ 500 ms, 1 connexion à la fois.
- [x] Given le pattern apparaît, when détecté, then la commande sort avec code 0 et (optionnel `--json`) imprime la/les ligne(s) matchée(s).
- [x] Given le timeout est atteint sans match, when expiré, then la commande sort avec un code non-zéro DÉDIÉ au timeout (distinct de "instance non lancée" et de "no-match-target").
- [x] Given aucun `--timeout`, when fourni, then un défaut borné raisonnable s'applique (pas d'attente infinie par défaut) — valeur documentée dans le help.
- [x] Le poll respecte le cap serveur 16 connexions (`ipc.rs:143-150`) — chaque itération ouvre/ferme une connexion ; test vérifiant qu'un `wait` long ne laisse pas de connexion ouverte entre les polls.

#### US-014: Sémantique multi-match / no-match (`--any` / `--all`)
**Description:** As a script author, I want contrôler ce qui se passe quand plusieurs panes matchent le sélecteur so that mes pipelines sont déterministes.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** US-013

**Acceptance Criteria:**
- [x] Given un sélecteur matchant plusieurs panes, when `--any` est passé, then `wait` réussit dès qu'UN pane matche le pattern ; `--all` exige que tous matchent.
- [x] Given un sélecteur multi-pane sans `--any`/`--all`, when invoqué, then échec demandant de désambiguïser (cohérent avec US-003) + code non-zéro.
- [x] Given la cible disparaît pendant l'attente (pane fermé), when le poll suivant tourne, then comportement défini (échec avec code dédié) plutôt qu'attente infinie.

#### US-015: Matching cross-platform `cmdline:` documenté
**Description:** As a CLI user on macOS/Windows, I want savoir comment cibler un agent quand l'argv complet n'est pas disponible so that mes sélecteurs marchent partout.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** US-003, US-013

**Acceptance Criteria:**
- [x] Le help et la doc indiquent que `cmdline:` matche l'argv complet sur Linux (`pty_session.rs:1169`) mais seulement le basename de l'exécutable sur macOS/Windows (`pty_session.rs:1196`).
- [x] Given macOS/Windows où `cmd` est un basename, when un sélecteur `cmdline:claude` est utilisé, then il matche le basename `claude` (pas les arguments) — comportement testé/documenté, pas un faux négatif silencieux.
- [x] La doc recommande `cwd:` ou `name` comme sélecteurs portables quand l'argv n'est pas requis.

---

### EP-004: `hooks setup` et notification de fin de tour (Phase 2)

Installer des hooks de notification d'agents **persistants user-scope** en une commande, et (story P2, sous réserve de confirmation utilisateur) re-câbler la notification desktop de fin de tour retirée avec le chat ACP.

**Definition of Done:** `paneflow hooks setup <agent>` écrit des hooks persistants idempotents pointant le binaire `paneflow-ai-hook` à son path stable, sans double-firing avec les hooks éphémères du shim ; `hooks status`/`uninstall` existent par symétrie avec `mcp`.

#### US-016: Moteur d'installation de hooks persistants user-scope
**Description:** As a maintainer, I want un écrivain de hooks réutilisant le moteur idempotent de mcp-install so that l'install est sûre, atomique et sans clobber.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Un `HookConfigWriter` (calqué sur le trait `AgentConfigWriter`, `paneflow-mcp-install/src/agents/mod.rs`) réutilise `write_if_changed`/`backup`/`write_atomic` (`io.rs:23-76`) et `read_json_or_default`/`merge_json_entry`/`remove_json_entry` (`merge.rs:27-99`).
- [x] Les hooks référencent le binaire `paneflow-ai-hook` à un **path stable non-versionné** (style `runtime_paths::bridge_binary_path`), PAS le cache versionné `PANEFLOW_BIN_DIR` du shim (qui change à chaque update, `hooks.rs:359`).
- [x] Given une config agent déjà installée, when `hooks setup` est ré-exécuté, then le diff est de 0 octet (idempotence), vérifié par test.
- [x] Given une config agent présente mais JSON invalide, when on tente l'install, then erreur explicite (pas de clobber, parité `read_json_or_default`).
- [x] Un backup est créé avant toute écriture.

#### US-017: Shapes de hooks par agent (claude/codex/gemini/opencode), cross-platform
**Description:** As a CLI user, I want que `hooks setup` écrive le bon format au bon endroit pour chaque agent so that les notifications fonctionnent réellement.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** US-016

**Acceptance Criteria:**
- [x] Claude : hooks écrits dans `~/.claude/settings.json` sous `hooks.{UserPromptSubmit,Notification,Stop,PreToolUse,PostToolUse}` (PAS `~/.claude.json` qui est pour les `mcpServers` de `mcp install`, `support.rs:27`) — un nouveau resolver de path est introduit.
- [x] Codex/Gemini/opencode : hooks écrits dans leur emplacement attendu respectif, avec la shape correcte par agent (réutilise les events de `hooks.rs:28-34` et `hooks.rs:540-548`).
- [x] Chaque hook pointe `paneflow-ai-hook <Event>` avec timeout, format identique à celui que le shim produit (`hooks.rs:322-328`) mais au path stable.
- [x] Cross-platform : Windows (où Codex utilise un tee JSONL plutôt que des hooks fichier, `paneflow-shim/src/main.rs:82-84`) a un chemin défini ou un stub documenté ; aucun `cfg` Unix-only sans contrepartie.
- [x] Test par agent : la config écrite est relue et valide.

#### US-018: Règle d'autorité anti double-firing (persistant vs éphémère shim)
**Description:** As a CLI user, I want éviter que mes agents émettent deux fois chaque événement so that la sidebar n'est pas bruitée.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** US-016, US-017

**Acceptance Criteria:**
- [x] Une règle d'autorité tranche le conflit : les hooks persistants user-scope (US-016, `settings.json`) et les hooks éphémères du shim (`settings.local.json` projet, `hooks.rs:285-346`) pointent tous deux `paneflow-ai-hook` — la règle empêche le double-firing (ex. le shim ne réécrit plus quand un hook persistant managé est détecté, OU dédup par marqueur `_paneflow_managed`).
- [x] Given les deux jeux de hooks présents, when un agent émet un événement, then une seule frame `ai.*` atteint la socket (test d'intégration ou test de la logique de dédup).
- [x] La logique de fusion/détection (`merge_paneflow_hooks`/`is_paneflow_hook_command`, `hooks.rs:388,376`) est partagée (crate extraite) ou dupliquée de façon documentée entre shim et setup.
- [x] La règle est documentée dans `docs/` (quel jeu fait autorité, comment désinstaller proprement).

#### US-019: Sous-commandes `hooks setup` / `status` / `uninstall`
**Description:** As a CLI user, I want gérer les hooks comme je gère le MCP so that l'expérience est cohérente.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** US-002, US-016, US-017

**Acceptance Criteria:**
- [x] `paneflow hooks setup [agent…]` installe (tous les agents détectés si aucun argument, comme `mcp install`) ; sortie ligne par ligne `<agent>: <message>`.
- [x] `paneflow hooks status` rapporte par agent : NotDetected / Installed{path} / Stale{found,expected} / NotInstalled (parité `mcp status`, `cli.rs:171-207`).
- [x] `paneflow hooks uninstall [agent…]` retire uniquement les hooks managés (no-clobber des hooks voisins, réutilise `remove_json_entry`).
- [x] Codes de sortie alignés sur `mcp` (0 succès/aucun agent, 1 erreur agent, 2 usage) ; l'intercept se place avant clap ou comme sous-commande clap selon US-002.
- [x] Given un agent non détecté, when `hooks setup <agent>` est appelé, then message clair, pas un échec dur.

#### US-020: Re-câblage de la notification desktop de fin de tour
**Description:** As an agent orchestrator, I want une notif OS quand un agent finit/attend so that je sais quand revenir sans surveiller l'écran.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** US-017

**Acceptance Criteria:**
- [x] **Bloqué tant que l'Open Question Q1 n'est pas tranchée** (la notif turn-end a été retirée avec le chat ACP, `agents/notifications.rs:5-10` — confirmer qu'on la veut côté terminal avant de re-câbler).
- [x] Un firing path OS est introduit : sur `ai.stop`/`ai.notification` (`ipc_handler.rs:1009-1057`), une notification desktop est émise via un crate de toast cross-platform (à choisir), respectant l'état `WINDOW_ACTIVE`/`AGENTS_PANEL_VISIBLE` (`agents/notifications.rs:25-34`).
- [x] Given la fenêtre Paneflow active et au premier plan, when un agent finit, then pas de notification (anti-bruit) ; given la fenêtre en arrière-plan, then notification émise.
- [x] Cross-platform : Linux (notify-send/zbus), macOS, Windows — chacun a un chemin fonctionnel ou un stub documenté.
- [x] La notif ne se déclenche que sur transition d'état réelle (pas sur chaque `tool_use`).

## Functional Requirements

- FR-01: Une invocation `paneflow <subcommand>` reconnue DOIT parler à une instance en cours via la socket IPC existante et sortir avant l'init GPUI ; sans sous-commande, le binaire DOIT lancer la GUI comme aujourd'hui.
- FR-02: Les commandes d'introspection (`ls`/`search`) DOIVENT émettre du JSON par défaut ; `read` DOIT émettre le texte brut du scrollback par défaut ; un flag (`--human`/`--json`) bascule le format.
- FR-03: Le sélecteur de cible DOIT accepter `surface_id` numérique, `name` (exact puis préfixe désambiguïsé), `cmdline:<substr>`, `cwd:<path>`.
- FR-04: Les commandes mutantes (`new`/`select`/`split`) DOIVENT reporter l'état résultant en JSON et relayer proprement les erreurs/caps serveur.
- FR-05: `send` DOIT être refusé avec un message clair sauf si `PANEFLOW_IPC_SCRIPTING=1`, et NE DOIT JAMAIS soumettre (aucun `\r`).
- FR-06: `up` DOIT parser une spec TOML, créer un workspace au layout décrit, démarrer chaque pane dans son cwd déclaré en lançant l'agent déclaré, et pré-remplir (sans soumettre) le prompt déclaré.
- FR-07: `up` DOIT échouer atomiquement (rollback) si un cwd n'existe pas ou si un binaire d'agent est absent du PATH, en nommant le pane fautif.
- FR-08: `wait` DOIT bloquer jusqu'au match du regex dans le pane ciblé ou jusqu'au timeout, et retourner 0 au match, un code non-zéro dédié au timeout.
- FR-09: `hooks setup <agent>` DOIT écrire des hooks de notification persistants user-scope référençant le binaire `paneflow-ai-hook` à son path stable, idempotemment (re-run = diff 0 octet), avec backup.
- FR-10: `hooks setup` NE DOIT PAS provoquer l'émission d'événements `ai.*` en double quand les hooks éphémères du shim sont aussi actifs.
- FR-11: Le système NE DOIT introduire AUCUNE méthode IPC qui soumet une commande/un prompt pour le compte de l'utilisateur (jamais d'auto-Entrée) — invariant human-in-loop.
- FR-12: Aucune sous-commande ne DOIT affaiblir l'authentification socket 0600 + same-UID peer-cred.

## Non-Functional Requirements

- **Performance:** une commande read-only (`ls`/`read`/`search`) renvoie en P95 < 150 ms quand une instance tourne (1 aller-retour socket, plafond timeout 10 s du client `ipc_client.rs:24-31`). `paneflow up` matérialise un workspace de ≤ 8 panes en < 3 s hors temps de boot des agents.
- **Sécurité:** auth socket inchangée (0600 + same-UID peer-cred, `ipc.rs:862-916`) ; `send`/`send_keystroke` restent gatés `PANEFLOW_IPC_SCRIPTING=1` ; la nouvelle méthode launch-agent est same-UID et n'émet jamais `\r` ; aucune nouvelle dép ne pull `toml_edit` dans `src-app`.
- **Robustesse:** `wait` poll à intervalle ≥ 500 ms avec ≤ 1 connexion simultanée (cap serveur 16, `ipc.rs:143-150`) ; aucun panic sur entrée invalide (clippy `panic = "deny"`) ; `up` rollback complet sur échec partiel.
- **Idempotence:** `hooks setup` ré-exécuté produit un diff de 0 octet ; backup systématique avant écriture.
- **Cross-platform:** toutes les sous-commandes compilent et passent sur les 4 legs de la matrice CI ; les chemins `cfg(windows)` sont vérifiés en CI (inspection-only sur l'hôte Linux).
- **Compatibilité:** `paneflow` sans argument démarre la GUI avec un overhead nul ajouté par clap (parse uniquement en présence d'une sous-commande) ; `--help`/`--version`/`--update-and-exit`/`mcp …` conservent leur comportement et leurs codes de sortie exacts.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Instance non lancée | Toute sous-commande socket sans Paneflow ouvert | Échec immédiat, code non-zéro | "paneflow IPC unreachable at {path}; is Paneflow running?" |
| 2 | Mauvais socket (dev vs release) | `PANEFLOW_SOCKET_PATH` absent hors PTY | Cible le socket release par défaut ; documenter de poser `PANEFLOW_SOCKET_PATH` en dev | "no instance at default socket; set PANEFLOW_SOCKET_PATH" |
| 3 | Peer-UID divergent | CLI lancée sous un euid différent (sudo) | Rejet serveur `-32001`, code non-zéro | "permission denied: run as the same user as Paneflow" |
| 4 | TOML malformé / clé inconnue | `up` sur un fichier avec `agnt:` | Erreur de désérialisation avec ligne/champ | "unknown field `agnt` at line N" |
| 5 | Repo de pane inexistant | `up` avec un `cwd` absent | Rollback total, nomme le pane | "pane 'backend': cwd /x does not exist" |
| 6 | Agent absent du PATH | `up` avec `agent: claude` sans binaire | Échec atomique, nomme le pane + agent | "pane 'frontend': agent 'claude' not found on PATH" |
| 7 | Prefill avant readiness | Prompt injecté dans un buffer pas prêt | Readiness-poll borné avant écriture ; best-effort + warning au timeout | "warning: pane 'x' not ready, prompt injected best-effort" |
| 8 | Sélecteur ambigu | `read cmdline:claude` matche 3 panes | Échec listant les candidats | "ambiguous target; matches: 12(claude-a), 18(claude-b)" |
| 9 | Sélecteur sans match | `wait --match cwd:/nope` | Échec code dédié | "no pane matches selector" |
| 10 | Timeout `wait` | Pattern jamais affiché | Code non-zéro dédié au timeout | "timeout after Ns waiting for /pattern/" |
| 11 | `send` sans gate | `send` avec `PANEFLOW_IPC_SCRIPTING` non posé | Refus `-32601` mappé | "send disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable" |
| 12 | Hook config JSON invalide | `hooks setup` sur un `settings.json` cassé | Pas de clobber, erreur | "~/.claude/settings.json is not valid JSON; aborting" |
| 13 | Double-firing hooks | Hooks persistants + éphémères shim actifs | Une seule frame `ai.*` (règle d'autorité) | — |
| 14 | Cap panes/workspaces | `up`/`split` au-delà de MAX | Erreur serveur relayée, pas de panic | "cannot split: max panes reached" |
| 15 | macOS/Windows `cmdline:` | `cmdline:claude --resume` sur macOS | Matche le basename `claude` seul, documenté | "note: cmdline matches basename only on this OS" |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | La nouvelle méthode launch-agent IPC est RCE-class (lance des CLIs arbitraires) | Med | High | Same-UID peer-cred (pas plus puissant que le shell de l'utilisateur) ; pas de `\r` ; threat-model review comme EP-005/cli-hardening ; Open Question Q2 sur le gate. |
| 2 | Perte silencieuse de prompt au prefill (buffer pas prêt) à l'échelle N panes | High | Med | Readiness-poll borné au lieu du timer fixe 1800 ms (US-010) ; test multi-pane vérifiant la présence de chaque prompt ; best-effort + warning. |
| 3 | Double-firing des hooks (persistants user + éphémères shim) | High | Low | Règle d'autorité explicite + dédup par marqueur `_paneflow_managed` (US-018) ; doc. |
| 4 | Migration clap casse `--help`/`--version`/`mcp`/`--update-and-exit` | Med | High | `Option<Commands>` + intercept `mcp` manuel conservé avant clap + `try_parse` ; AC de non-régression sur chaque flag (US-002). |
| 5 | Matching `cmdline:` trompeur sur macOS/Windows (basename only) | Med | Med | Documenter l'asymétrie (US-015) ; recommander `cwd:`/`name` portables ; pas de faux négatif silencieux. |
| 6 | `paneflow up` non-idempotent crée des doublons sur retry client | Med | Med | Pas de retry client automatique sur les mutations ; `--dry-run` ; doc sur l'absence d'identité stable de surface (Open Question Q5). |
| 7 | Path du binaire hook pointant le cache versionné casse au prochain update | Med | High | Ancrer sur un path stable type `bridge_binary_path()` sous `data_dir()` (US-016). |
| 8 | Re-câbler une notif retirée volontairement (chat ACP) sans intention confirmée | Low | Med | US-020 bloquée derrière l'Open Question Q1 (décision utilisateur) ; P2. |

## Non-Goals

Frontières explicites — ce que cette version NE fait PAS :

- **Parité WezTerm sur le contrôle de terminal générique** (`resize`/`zoom`/`activate-pane-direction`/`move-pane-to-tab`) — terrain gagné par d'autres, hors positionnement cockpit d'agents. Volontairement exclu.
- **Orchestration d'agents headless** — aucune méthode ne soumet un prompt pour l'utilisateur ; pas de mode sans GUI. Les agents tournent dans de vrais panes visibles (contrainte human-in-loop). Exclu par design.
- **Streaming / `tail` / subscribe** d'un pane (push continu vs request/response) — utile mais différé ; `wait` couvre le besoin de synchronisation immédiat.
- **CLI standalone installable séparément de la GUI** (style `kitten` statique pour piloter via SSH/forward) — différé ; la CLI vit dans le binaire `paneflow`.
- **Système de plugins** (style WASM Zellij) — hors scope.
- **Identité de surface stable et persistante** entre redémarrages — non fourni par le serveur aujourd'hui ; `up` n'est donc pas idempotent par re-exécution (voir Open Question Q5).

## Files NOT to Modify

- `src-app/Cargo.toml` (section GPUI git deps) et le pin du fork Zed — ne jamais toucher dans ce PRD.
- `src-app/src/ipc.rs:862-916` (auth peer-UID) — étendre les méthodes, jamais affaiblir l'authentification.
- `src-app/src/app/ipc_handler.rs:41-51` (gate `PANEFLOW_IPC_SCRIPTING`) — ne pas retirer le gate de `send_text`/`send_keystroke`.
- `crates/paneflow-ai-hook/src/main.rs` — le binaire callback est correct, aucun changement requis côté handler (seul son chemin de référence change, côté installeur).
- `src-app/src/terminal/pty_session.rs` (PTY core, `foreground_command`) — lecture seule pour le sélecteur ; ne pas modifier le PTY.
- L'intégration alacritty / `ZedListener` / `FairMutex` — hors scope.

## Technical Considerations

Cadré comme questions pour l'engineering, pas comme mandats :

- **Architecture (crate client):** extraire `paneflow-ipc-client` (recommandé) vs faire dépendre `src-app` de `paneflow-mcp` comme lib. Recommandé : extraction (zéro dép GPUI, DRY, `paneflow-mcp` la consomme aussi). Engineering à confirmer.
- **Méthode IPC `up`:** une nouvelle méthode (`workspace.up`) routant via `spawn_pane_from_surfaces` vs faire en sorte qu'`apply_layout_from_json` honore les `SurfaceDefinition`. Recommandé : nouvelle méthode dédiée (le chemin restore est éprouvé ; éviter de changer la sémantique de `restore_layout`).
- **Gate de launch-agent:** la nouvelle méthode doit-elle exiger `PANEFLOW_IPC_SCRIPTING=1` (parité `send_text`) ou same-UID peer-cred suffit-il ? Recommandé : same-UID suffit pour le launch (pas plus puissant que le shell de l'utilisateur) ; garder `send_text` gaté car il injecte dans la session d'un AUTRE agent (mouvement latéral). À trancher avec une threat-model review (cf. Open Question Q2).
- **Readiness du prefill:** readiness-poll (sur un marqueur de prompt) vs délai par-agent borné. Recommandé : poll borné avec repli sur délai ; détection de marqueur par-agent best-effort (Open Question Q4).
- **Champ `agent` vs `command` dans `SurfaceDefinition`:** ajouter un champ `terminal_agent` à `SurfaceDefinition` (comme `ThreadDefinition` `schema.rs:942`) vs résoudre `agent→command` dans la couche spec avant de construire le layout. Recommandé : résolution couche spec (pas de churn sur le schéma de session partagé).
- **Crate de toast OS (US-020):** dépendance cross-platform à choisir (`notify-rust` Linux/macOS + winrt/wintoast Windows ?) — à évaluer si Q1 valide la feature.

## Success Metrics

Note : pas de télémétrie desktop (analytics web-only par décision produit). Métriques perf/CI-mesurables ou proxies observables.

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Sous-commandes CLI parlant à l'instance | 0 (mcp/update/help only) | ≥ 7 (P0), ≥ 11 (tout) | Phase-1 / Phase-2 | revue de code + `paneflow --help` |
| Latence P95 commande read-only | N/A (n'existe pas) | < 150 ms | Phase-1 | bench local scripté (1 aller-retour socket) |
| Spawn workspace d'agents | manuel, multi-clics GUI | 1 commande, ≤ 8 panes | Phase-1 | test d'intégration + démo `up` |
| Idempotence `hooks setup` (diff re-run) | N/A | 0 octet | Phase-2 | test d'idempotence |
| Couverture tests des nouveaux chemins | N/A | chaque story a ≥ 1 test unhappy-path | continu | `cargo test --workspace` |
| CI 4 legs verte | requise | requise | continu | matrice de release |
| Adoption (proxy observable) | 0 | ≥ 1 `paneflow.workspace.toml` d'exemple committé + section docs/ | Phase-2 | repo + docs |

## Open Questions

- **Q1 (US-020):** veut-on réellement re-câbler la notification desktop de fin de tour côté terminal ? Elle a été retirée volontairement avec le chat ACP (`agents/notifications.rs:5-10`), mais la mémoire de repositionnement la cite comme un axe. Décision utilisateur requise AVANT de re-câbler — bloque US-020. À trancher par Arthur.
- **Q2 (US-008, sécurité):** la méthode launch-agent IPC doit-elle être gatée derrière `PANEFLOW_IPC_SCRIPTING=1`, ou same-UID peer-cred suffit-il (recommandé) ? À trancher avec une threat-model review avant de shipper EP-002.
- **Q3 (US-018):** quel jeu de hooks fait autorité quand persistant (setup) et éphémère (shim) coexistent — le persistant remplace-t-il l'éphémère, ou inverse ? Détermine la logique de dédup.
- **Q4 (US-010):** stratégie de readiness retenue (poll sur marqueur par-agent vs délai borné par-agent) — dépend des marqueurs de prompt observables par agent ; à affiner pendant l'implémentation.
- **Q5 (`up` idempotence):** le serveur n'expose pas d'identité de surface stable entre redémarrages (`surface_id` = entity id GPUI non-persistant). Faut-il introduire une clé stable pour rendre `up` ré-applicable, ou accepter que `up` crée toujours un nouveau workspace (recommandé pour v1) ?
[/PRD]
