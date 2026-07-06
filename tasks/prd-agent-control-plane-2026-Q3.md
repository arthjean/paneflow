[PRD]
# PRD: Agent Control Plane - lecture, push d'événements et accès libre IA (2026-Q3)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-18 | Arthur Jean | Draft initial - 5 epics / 18 stories. Expose l'état de la flotte (fleet.list/surface.status), construit la voie efférente (events.subscribe + paneflow watch), ajoute le mode "accès libre IA" débrayable, durcit fiabilité/adressage/contexte, livre le skill conducteur. Construit le plan de contrôle SOUS l'orchestration-v2 (EP-003 flow engine). |

## Problem Statement

Paneflow a déjà construit, éparpillés, les capteurs et le cerveau d'un plan de contrôle pour agents CLI, mais aucune fenêtre de lecture ni voie de sortie dessus. L'audit du 2026-06-18 (4 passes croisées) établit:

1. **L'état de la flotte existe mais n'est lisible par personne.** `Workspace::agent_sessions` (`workspace/mod.rs:92`) porte l'état live de chaque agent (Thinking/WaitingForInput/Finished/Errored), avec la question réelle de l'agent (`ai.notification`). Mais aucune méthode IPC ne le rend: `workspace.list` ne retourne que title/cwd/pane_count. Un conducteur (humain ou agent) ne peut pas énumérer la flotte ni connaître l'état d'une pane sans scraper le scrollback.

2. **Il n'y a aucune voie de sortie temps réel.** Le bus entrant est solide (shim 16 binaires -> paneflow-ai-hook -> IPC -> 50ms poll GPUI). Mais l'IPC est strictement requête/réponse, connexions one-shot (structurel sur Windows, `ipc.rs:835-836`). Zéro subscribe, zéro push. Un conducteur ne peut être prévenu qu'un agent a fini ou attend: il doit poller.

3. **Le flow engine prouve le manque.** Seul client multi-agents existant (`paneflow flow run`), il poll à 500ms faute de push, réimplémente un settling heuristique (1800ms + 2 lectures stables) parce que `output_generation` existe en interne (`ipc_handler.rs:896`) mais n'est pas exposé, et scrape le scrollback au regex faute de `surface.status`. Ses 3 béquilles mappent 1:1 les primitives absentes.

4. **Les garde-fous sont binaires et non configurables.** Le gate `PANEFLOW_IPC_SCRIPTING` est tout-ou-rien, process-wide, et le prefill-non-soumis est codé en dur. Un power-user IA qui veut laisser un agent piloter ses pairs sans friction n'a aucun réglage: c'est tout verrouillé ou rien. Inversement, le seul garde-fou qui protège l'IA d'un détournement (le fence untrusted) n'existe que sur le chemin MCP, pas sur `surface.read` CLI/IPC, laissant un vecteur d'injection inter-agents ouvert.

**Why now:** la recherche concurrentielle (2026-06) montre un marché à deux pôles et un centre vide. Pôle 1: les pane-splitters worktree (Claude Squad, dmux, Conductor, vibe-kanban) isolent N CLIs côte à côte mais ne lisent pas l'état des agents et n'auto-dispatchent pas. Pôle 2: les protocoles agent-natifs (A2A, swarm-protocol) coordonnent des agents headless invisibles. Personne ne combine hétérogène + interactif + visible + plan de contrôle read-state/push-events où le conducteur peut être humain OU agent. C'est le wedge de Paneflow (agent cockpit cross-platform, seul moat vs cmux mac-only), mais il se referme vite (vibe-kanban, AGX, clideck migrent vers des dashboards state-aware). Le terme "control plane" est devenu vocabulaire mainstream ([Medium - Rise of the AI Control Plane](https://medium.com/@adnanmasood/agent-harness-engineering-the-rise-of-the-ai-control-plane-938ead884b1d)); il faut le revendiquer sur le wedge unique maintenant.

## Overview

Cinq chantiers qui transforment des primitives éparpillées en UN plan de contrôle nommé et exposé, plus le réglage d'accès libre exigé par le profil power-user. Le skill conducteur (un SKILL.md) est le manuel optimisé par-dessus ce plan, pas le produit: le produit est le substrat Rust.

**EP-001 - Lecture de la flotte.** Exposer l'état déjà collecté: `fleet.list` (snapshot de tous les agents: pid, tool, state, surface, waiting_since, last_activity), `surface.status` (état d'une pane), et `output_generation` dans la réponse de `surface.read` pour tuer le settling hack. CLI miroir `paneflow ps` / `paneflow status`.

**EP-002 - Bus d'événements sortant (la voie efférente).** Le chantier central. Un modèle de connexion persistante abonnée, une méthode `events.subscribe`, un pont GPUI -> abonnés qui rediffuse les `ai.*` et les changements d'`output_generation`, et un verbe CLI `paneflow watch` qui stream en JSONL. Construit une fois en IPC, consommé et par un orchestrateur externe (IPC direct) et par un conducteur in-pane (via `paneflow watch`).

**EP-003 - Mode "accès libre IA" (unrestricted).** L'exigence power-user, first-class. Un toggle Settings -> AI Agent qui débraye les garde-fous BRIDANTS (prefill-non-soumis devient auto-submit autorisé; le gate write s'ouvre; capability d'écriture accordée aux conducteurs). Défaut SUR (garde-fous ON), opt-in explicite. Distinction encodée: le fence anti-injection (qui PROTÈGE l'IA d'un détournement, ne la bride pas) est un sous-toggle séparé, défaut ON même en mode unrestricted.

**EP-004 - Fiabilité, adressage durable, contexte structuré.** Label atomique au spawn (adressage stable sans race), watchdog resserré sur le Thinking-bloqué, dette de tests sur `surface.read` et la transition `ai.notification`, et un canal de contexte structuré (champ last_result + bascule fichier au-delà de 64 KiB).

**EP-005 - Conducteur, hardening cross-platform, dogfood.** Le SKILL.md conducteur harness-agnostic, le hardening macOS (orphan guard) + Windows (SIGINT -> ai.stop), et un vrai `flow.toml` de démo commité (Goal 2 de la v2, jamais atteint).

Décisions structurantes: le push est lecture seule (pas de gate); le mode unrestricted brille sur worktrees isolés (caveat "throwaway branches" de la pratique YOLO 2026); le fence reste ON par défaut car le désactiver ne donne aucun pouvoir supplémentaire à l'IA, il expose juste le conducteur à être détourné par un repo malveillant, et la reprise humaine ne rattrape pas une injection rapide et silencieuse; tout I/O hors render-thread GPUI.

## Goals

| Goal | Phase-1 Target | Phase-6 Target |
|------|---------------|----------------|
| Un conducteur lit l'état complet de la flotte en 1 appel | `fleet.list` retourne N agents avec state + surface en 1 appel (vs 3 sources à coudre aujourd'hui) | le flow engine consomme `surface.status` au lieu de scraper le scrollback |
| Être prévenu d'un changement d'état sans poller | `paneflow watch` émet l'event d'un changement en < 100 ms P95 (vs poll 500ms / jamais) | le flow engine remplace son settling poll par l'abonnement push |
| Accès libre IA débrayable, défaut sûr | toggle Settings fonctionnel: OFF = comportement actuel inchangé, ON = auto-submit autorisé; fence sous-toggle séparé défaut ON | un conducteur agent pilote 3 panes en mode unrestricted sans friction, fence actif |
| Orchestrer des harness hétérogènes sans script shell | le SKILL.md pilote Claude + Codex + OpenCode via la CLI publique | >= 1 flow.toml réel commité + consommé par le skill |

## Target Users

### Le dev orchestrateur power-user (Arthur et profils similaires)
- **Role:** solo dev / indie maker, AI maximaliste, qui pilote 3-8 agents CLI hétérogènes (Claude Code, Codex, OpenCode, Gemini) en parallèle dans Paneflow.
- **Behaviors:** lance les agents via `paneflow up`, supervise la grille, veut qu'un agent conducteur dispatch et lise les autres à sa place, et reprend la main sur n'importe quelle pane à tout moment.
- **Pain points:** ne peut pas connaître l'état d'un agent par programme; doit poller; les garde-fous sont tout-ou-rien et bloquent le pilotage agent-of-agents qu'il veut assumer.
- **Current workaround:** scripts shell `up && wait && send`, scraping de scrollback, pas de pilotage inter-agents du tout.
- **Success looks like:** activer un mode accès libre, lancer un conducteur qui orchestre la flotte en lisant l'état réel et en soumettant les prompts, et n'intervenir que quand il le décide.

### Le conducteur (agent CLI ou orchestrateur externe)
- **Role:** Claude Code / Codex tournant dans une pane (via le skill + la CLI), ou un process externe parlant à l'IPC.
- **Behaviors:** énumère la flotte (`fleet.list`), s'abonne aux events (`watch`), dispatch (`send`), attend un état (push), rend la main à l'humain selon une discipline documentée.
- **Pain points:** aucune lecture d'état structurée, aucun push, doit scraper et poller; l'output d'un pair est untrusted et non fence sur le chemin CLI.
- **Current workaround:** aucun (capacités absentes).
- **Success looks like:** un conducteur exécute un workflow multi-agents complet via la CLI publique, réagit aux events poussés, sans poll ni scraping fragile.

## Research Findings

### Competitive Context
- **Claude Squad** (smtg-ai, OSS): tmux + worktrees, supporte Claude/Codex/OpenCode/Aider/Gemini. Concurrent le plus direct, mais "split + isolate": ne lit pas l'état des agents, n'auto-dispatch pas. Paneflow lit l'état + pousse les events.
- **vibe-kanban** (Bloop/YC): dashboard kanban sur plusieurs agents, suivi de statut + diffs. Coordination par board, pas un terminal live interactif.
- **Conductor / dmux / emdash / amux**: variations split-panes + worktrees, TUI ou mac-only, coordination superficielle.
- **Warp** (Rust, GPU): Agent Mode lit la sortie shell, mais philosophie "shell as the UI, no panels", mono-agent-in-shell, anti-multiplexer.
- **LangGraph / AutoGen / CrewAI / OpenAI Agents SDK**: orchestrent des appels API in-process, jamais des CLIs réels hétérogènes. Catégorie différente (ils possèdent la boucle modèle; Paneflow pilote des process opaques par leur I/O).
- **A2A** (Linux Foundation, 50+ partenaires): couche agent-to-agent horizontale, mais pour agents headless. **MCP** a gagné la couche agent-to-tool.
- **Market gap:** personne ne combine hétérogène + interactif + visible + read-state/push-events + conducteur humain-ou-agent. Aucun standard agent-state/event-bus n'existe ("no protocol mandates structured logging/observability - every team builds it independently"). Paneflow scraping PTY + emitting events est une implémentation locale légitime d'une couche non standardisée.

### Best Practices Applied
- Autonomie étagée gatée sur la réversibilité ("interrupt where reversibility ends"): le mode unrestricted assume le risque côté utilisateur, mais brille sur worktrees isolés (caveat "throwaway branches", pratique YOLO 2026, Boris Cherny juin 2026).
- OWASP LLM Top 10: "Excessive Agency" + prompt injection #1. Le fence anti-injection est une frontière de sécurité, pas un confort UX -> garde-le ON par défaut même en unrestricted.
- Accès PTY-level > screen-scraping: framer comme avantage (line-buffering contrôlé, pas de fragilité écran).
- Push lecture seule, pas de gate (parité avec read/list/search).

*Sources tracées dans la conversation de recherche (awesome-agent-orchestrators, Claude Squad, vibe-kanban, A2A/MCP protocol map, OWASP GenAI Q1-2026, Cogent failure playbook, Christian Schneider prompt-injection amplification).*

## Assumptions & Constraints

### Assumptions (to validate)
- Le pont GPUI -> connexions abonnées peut rediffuser un event en < 100 ms P95 sans bloquer le render-thread (le poll IPC est déjà à 50ms; le push s'y branche). US-006 inclut la mesure.
- Une connexion persistante abonnée est tenable cross-platform via interprocess; sur Windows, la contrainte one-request-per-connection (`ipc.rs:835-836`) impose un design séparé (un pipe-instance par abonné ou un endpoint dédié). US-004 valide.
- Le fence serveur peut réutiliser `neutralize_sentinel` (`tools.rs`) sans coût perceptible sur `surface.read`. US-011 valide.

### Hard Constraints
- **Cross-platform Linux/macOS/Windows obligatoire** (CLAUDE.md): chaque story a un chemin par OS ou un stub documenté cohérent avec l'existant.
- **Défaut sûr:** garde-fous ON par défaut; le mode unrestricted est un opt-in explicite. Le fence anti-injection reste ON par défaut même en unrestricted.
- **Push = lecture seule:** `events.subscribe` n'a pas de gate scripting (parité read). Les writes restent gatés sauf mode unrestricted actif.
- **Jamais d'I/O bloquant sur le render-thread GPUI** (audit 2026-06-04).
- `MAX_PANES = 32`, `MAX_WORKSPACES = 20`, exit codes CLI (0 OK, 1 runtime, 3 target, 4 timeout) réutilisés.
- **Couverture agents:** turn-tracking sur les 13 agents hookés; cécité documentée pour les 6 sans hooks (copilot, kiro-cli, droid/factory, agy/antigravity, openclaw, amp).
- **Free + OSS:** cette feature est un levier de distribution, jamais derrière un plan payant.
- Convention commits: `feat(module): US-NNN - description`, atomiques par story. GPL-3.0. Aucune attribution Claude.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique (gate CI release, 4 legs)
- `cargo clippy --workspace -- -D warnings` - zéro warning
- `cargo test --workspace` - tous les tests workspace

Pour les stories UI (US-009):
- Vérification visuelle manuelle dans l'app (GPUI non testable headless) - noter "UI non vérifiée GUI" dans le commit si la passe visuelle n'a pas eu lieu.

## Epics & User Stories

### EP-001: Lecture de la flotte

Exposer l'état déjà collecté par les hooks pour qu'un conducteur énumère la flotte et lise l'état d'une pane sans scraper le scrollback. Tue aussi le settling hack du flow engine.

**Definition of Done:** `fleet.list` et `surface.status` retournent l'état live contre une instance; `surface.read` expose `output_generation`; CLI `paneflow ps`/`status` fonctionnent; méthodes documentées dans `docs/`.

#### US-001: `fleet.list` IPC + `paneflow ps`
**Description:** As a conducteur, I want énumérer tous les agents en cours avec leur état so that je connais la flotte en un appel au lieu de coudre 3 sources.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given 3 agents en cours, when `fleet.list`, then la réponse liste pour chacun `{pid, tool, state, surface_id, surface_name, workspace, waiting_since, last_activity, active_tool_name}` lus depuis `agent_sessions`
- [x] Given un agent dont la surface n'est pas encore résolue (`surface_id = None`), when `fleet.list`, then il est listé avec `surface_id: null`, pas omis
- [x] Given aucun agent en cours, when `fleet.list`, then `{agents: []}` et exit 0 (pas une erreur)
- [x] Given un agent détecté sans hooks (parmi les 6), when `fleet.list`, then il apparaît avec `state: "unknown_running"` et un flag `hooked: false`
- [x] `paneflow ps` rend la table en humain et `paneflow ps --json` en JSON; pas de gate scripting (lecture)

#### US-002: `surface.status` IPC + `paneflow status <target>`
**Description:** As a conducteur, I want lire l'état d'une pane ciblée so that je sais si l'agent réfléchit, attend une réponse (et laquelle) ou a fini.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given une pane avec agent `WaitingForInput`, when `surface.status backend`, then la réponse contient `{state: "waiting_for_input", message: "<question>", active_tool_name, output_generation}`
- [x] Given une pane sans agent (shell nu), when `surface.status`, then `{state: "idle"}` (pas une erreur)
- [x] Given un selector ambigu, when `surface.status ba`, then exit 3 + liste des candidats (parité `resolve_target`)
- [x] Given un selector sans match, when `surface.status zzz`, then exit 3
- [x] La question retournée est déjà sanitizée (bidi-strip + cap 512, parité `sanitize_notification_message`)

#### US-003: Exposer `output_generation` dans `surface.read`
**Description:** As a conducteur, I want connaître le compteur de génération de sortie d'une pane so that je détecte la stabilité sans heuristique à base de timers.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given un `surface.read`, when la réponse est formée, then elle inclut `output_generation: u64` (le compteur interne `ipc_handler.rs:896`)
- [x] Given deux `surface.read` consécutifs sans nouvelle sortie, when comparés, then `output_generation` est identique (signal de stabilité)
- [x] Given une pane fermée entre deux reads, when `surface.read`, then l'erreur sentinelle existante est retournée (comportement inchangé, pas de panic)
- [x] Le champ est purement additif: les clients existants qui ignorent `output_generation` ne cassent pas

### EP-002: Bus d'événements sortant

Construire la voie efférente: une fois l'état lisible, le pousser en temps réel à un client abonné. Le chantier central du plan de contrôle.

**Definition of Done:** un client peut tenir une connexion ouverte, recevoir les `ai.*` et changements d'`output_generation` poussés en < 100 ms P95; `paneflow watch` stream en JSONL; un client lent ou mort n'affecte ni le render-thread ni les autres abonnés; cross-platform.

#### US-004: Modèle de connexion persistante abonnée
**Description:** As a conducteur, I want maintenir une connexion ouverte qui reçoit des messages initiés par le serveur so that je ne re-connecte pas à chaque event.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given un client abonné, when le serveur a un event, then il l'écrit sur la connexion ouverte (newline-delimited JSON) sans attendre de requête
- [x] Given un client lent qui ne draine pas, when la file d'envoi dépasse un cap borné (ex. 1024 events ou 8 MiB), then les events les plus anciens sont droppés avec un marqueur `{"dropped": N}`, le render-thread n'est JAMAIS bloqué
- [x] Given un client qui se déconnecte, when le serveur tente d'écrire, then l'abonnement est évincé proprement (pas de panic, pas de fuite de compteur de connexions)
- [x] Given Windows (contrainte one-request-per-connection `ipc.rs:835-836`), when un client s'abonne, then un design par-OS documenté est utilisé (pipe-instance dédié ou endpoint séparé) et un stub cohérent existe si non implémentable sur le host
- [x] Le peer-UID + 0600 s'appliquent à la connexion d'abonnement comme aux autres

#### US-005: Méthode `events.subscribe` + wire format
**Description:** As a conducteur, I want déclarer à quels events je m'abonne so that je ne reçois que ce qui m'intéresse.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [x] Given `events.subscribe {surfaces?: [...], types?: ["ai.stop", "ai.notification", ...]}`, when un event matche, then il est poussé avec `{type, surface_id, workspace_id, tool, pid, ts, ...payload}`
- [x] Given `types` omis, when subscribe, then tous les types d'events sont poussés
- [x] Given un filtre `types` invalide (type inconnu), when subscribe, then erreur JSON-RPC immédiate, connexion fermée
- [x] Given un event `surface_changed` (output_generation incrémenté), when un abonné filtre cette surface, then il le reçoit (permet de remplacer le settling poll)
- [x] Pas de gate scripting (lecture seule); le wire format est documenté dans `docs/`

#### US-006: Pont GPUI -> abonnés (rediffusion)
**Description:** As a conducteur, I want que les changements d'état internes soient rediffusés aux abonnés so that je réagis à `ai.stop`/`ai.notification` sans poll.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Given un `upsert_session_state` (transition d'état), when il s'applique, then l'event correspondant est diffusé à tous les abonnés matchants
- [x] Given un changement d'`output_generation` sur une pane, when il survient, then un `surface_changed` est diffusé (debounce <= 50ms pour éviter le flood)
- [x] Given 0 abonné, when un event survient, then no-op (pas de coût)
- [x] Given un event diffusé, when mesure de la source (transition GPUI) à l'écriture socket, then latence < 100 ms P95
- [x] Given un abonné dont l'écriture échoue, when diffusion, then il est évincé sans interrompre la diffusion aux autres

#### US-007: `paneflow watch [--surface <sel>] [--type <t>]`
**Description:** As a conducteur, I want streamer les events en JSONL depuis la CLI so that un agent in-pane consomme le même push qu'un orchestrateur externe.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Given une instance live, when `paneflow watch`, then chaque event est écrit sur stdout en une ligne JSON, en flux continu
- [x] Given `--surface backend --type ai.stop`, when watch, then seuls les `ai.stop` de la pane `backend` sont émis
- [x] Given aucune instance Paneflow, when watch, then exit code 3 + message actionnable (pas de hang)
- [x] Given un SIGINT (Ctrl-C), when watch tourne, then sortie propre exit 0, abonnement libéré côté serveur
- [x] Given aucun event pendant N secondes, when watch, then un heartbeat `{"type":"heartbeat"}` est émis toutes les 30s (détecte une connexion morte)

### EP-003: Mode "accès libre IA" (unrestricted)

Débrayer les garde-fous bridants pour le power-user IA, défaut sûr, opt-in explicite, en séparant le garde-fou qui protège l'IA d'un détournement (le fence) des garde-fous qui la brident.

**Definition of Done:** un toggle Settings -> AI Agent active le mode; OFF = comportement actuel strictement inchangé; ON = auto-submit autorisé + capability d'écriture accordée aux conducteurs; le fence est un sous-toggle séparé défaut ON; le raisonnement est documenté dans le PRD et `docs/`.

#### US-008: Setting `ai_unrestricted` (config + lifecycle)
**Description:** As a power-user, I want un réglage persistant qui ouvre l'accès IA so that je choisis explicitement d'assumer le mode libre.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `paneflow.json` sans la clé, when lu, then `ai_unrestricted` vaut `false` (défaut sûr)
- [x] Given `ai_unrestricted: true`, when l'app démarre, then un warn log unique est émis (parité warn boot du gate scripting `ipc.rs:206-210`)
- [x] Given une valeur invalide (non-booléenne), when lu, then fallback `false` + warn, jamais un état ouvert par accident
- [x] Given le toggle change à chaud, when appliqué, then l'effet est immédiat sans restart (le gate effectif est ré-évalué par appel)
- [x] Le sous-champ `ai_injection_fence` existe, défaut `true`

#### US-009: Settings -> AI Agent: toggles unrestricted + fence
**Description:** As a power-user, I want activer le mode libre et voir l'état du fence dans les settings so that le compromis est explicite et visible.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Given Settings -> AI Agent, when ouvert, then un toggle "Accès libre IA (unrestricted)" reflète `ai_unrestricted`, OFF par défaut
- [x] Given le toggle unrestricted ON, when affiché, then un sous-toggle "Fence anti-injection" apparaît, ON par défaut, avec un texte expliquant qu'il protège le conducteur sans le brider
- [x] Given l'utilisateur désactive le fence, when il clique, then un avertissement explicite s'affiche (le risque: une pane malveillante peut détourner le conducteur, non rattrapé par la reprise humaine) avant d'appliquer
- [x] Given un toggle change, when appliqué, then `paneflow.json` est écrit en read-modify-write atomique (parité `config_writer`)
- [x] UI: vérification visuelle manuelle (GPUI headless)

#### US-010: Auto-submit + capability d'écriture en mode unrestricted
**Description:** As a conducteur, I want soumettre des prompts à d'autres panes quand le mode libre est actif so that je pilote la flotte sans friction de gate.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Given `ai_unrestricted: false`, when un conducteur tente `surface.send_text submit:true`, then comportement actuel: refus sauf `PANEFLOW_IPC_SCRIPTING=1` (strictement inchangé)
- [x] Given `ai_unrestricted: true`, when `surface.send_text submit:true`, then la soumission est autorisée sans exiger la var d'env, et un log structuré `{method, surface_id, caller_pid, length, submit}` est émis
- [x] Given mode unrestricted, when un conducteur cible une pane, then la capability d'écriture lui est accordée par pane (pas un open global silencieux); l'octroi est tracé
- [x] Given le mode repasse OFF à chaud, when un envoi suit, then il est de nouveau gaté (pas de capability résiduelle)
- [x] Given un envoi en mode unrestricted, when la pane n'existe plus, then exit 3, aucun envoi partiel

#### US-011: Fence anti-injection sur le chemin read -> forward
**Description:** As a power-user, I want que l'output d'une pane reste fence même en mode libre so that un repo malveillant ne détourne pas mon conducteur.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Given `ai_injection_fence: true`, when `surface.read` (CLI/IPC) retourne du texte, then il est wrappé `<untrusted_terminal_output id="...">` avec sentinelle neutralisée (parité `neutralize_sentinel`, `tools.rs`)
- [x] Given `ai_injection_fence: true`, when le body contient un faux tag de fermeture, then il est défangé (zero-width space inséré)
- [x] Given `ai_injection_fence: false`, when `surface.read`, then texte brut (comportement historique), et ce choix est documenté comme risque assumé
- [x] Given le fence actif, when mesure de l'overhead sur un read de 64 KiB, then surcoût < 5 ms (négligeable)
- [x] Le PRD et `docs/` documentent: désactiver le fence ne donne aucun pouvoir supplémentaire à l'IA, il ouvre seulement un vecteur de détournement

### EP-004: Fiabilité, adressage durable, contexte structuré

Combler les dettes qui rendent l'orchestration fragile: adressage qui dérive, état qui colle, primitives non testées, contexte volatile.

**Definition of Done:** un conducteur peut cibler une pane par un nom stable posé à la création; un Thinking bloqué est détecté en temps borné configurable; `surface.read` et la transition `ai.notification` sont testés; le contexte >64 KiB transite par fichier.

#### US-012: Label atomique au spawn (`workspace.up` / `surface.split`)
**Description:** As a conducteur, I want nommer une pane dès sa création so that je la cible ensuite par un nom qui ne dérive pas quand le foreground change.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `surface.split {label: "reviewer", ...}`, when la pane est créée, then `custom_name = "reviewer"` est posé atomiquement (pas de fenêtre de race avec l'auto-name)
- [x] Given `workspace.up` avec un `label` par pane spec, when créé, then chaque `surface_id` retourné porte déjà son label stable
- [x] Given deux labels identiques dans un même up, when créé, then le second est désambiguïsé (suffixe) et un warn est émis
- [x] Given `label` omis, when créé, then l'auto-name actuel s'applique (rétrocompatible)

#### US-013: Watchdog resserré sur le Thinking-bloqué
**Description:** As a conducteur, I want qu'un agent dont le `ai.stop` est perdu soit détecté vite so that je ne crois pas qu'il réfléchit pendant 5 minutes.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given un `ai.stop` perdu (shim tué, shell vivant), when le watchdog tourne, then la session passe `Stalled` en temps borné configurable (défaut <= 60s, vs ~330s aujourd'hui)
- [x] Given un agent qui réfléchit réellement et émet des `ai.tool_use`, when le watchdog tourne, then il NE passe PAS `Stalled` (l'activité hook resette le timer)
- [x] Given le seuil configurable dans `paneflow.json`, when absent, then défaut documenté appliqué
- [x] Given une session passée `Stalled` puis un nouveau hook arrive, when reçu, then elle revient à l'état live (non collante)

#### US-014: Tests de la fondation (surface.read, settling, ai.notification)
**Description:** As a mainteneur, I want couvrir les chemins critiques non testés so that la fondation de l'orchestration ne casse pas silencieusement.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Given un handler `surface.read`, when testé, then offset hors borne (clamp), pagination, pane fermée (sentinelle), et `output_generation` sont couverts
- [x] Given le chemin settling/bailout du flow engine, when testé, then le cas "output jamais stable -> bailout 8s" est exercé (aujourd'hui non testé)
- [x] Given une frame `ai.notification`, when injectée dans un harness de test, then la transition vers `WaitingForInput` + stockage du message est vérifiée end-to-end
- [x] Given un `ai.stop` perdu simulé, when le watchdog tourne, then la transition `Stalled` est testée (lié à US-013)

#### US-015: Contexte structuré inter-agents (last_result + bascule fichier)
**Description:** As a conducteur, I want récupérer le dernier résultat d'un agent de façon structurée so that je ne scrape pas le scrollback et ne suis pas plafonné à 64 KiB.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given un agent qui termine un tour, when `fleet.list`/`surface.status`, then un champ `last_result` optionnel porte le résumé du dernier tour (si disponible via hook/transcript)
- [x] Given un contexte à passer > 64 KiB, when un canal de contexte est utilisé, then il est écrit dans un fichier temp et le chemin est passé en variable (pas de troncature silencieuse)
- [x] Given aucun résultat disponible, when lu, then `last_result: null` (pas une erreur)
- [x] Given le fichier temp, when le tour est fini, then il est nettoyé (pas de fuite disque)

### EP-005: Conducteur, hardening cross-platform, dogfood

Livrer le manuel (le skill), durcir les trous OS qui minent la fiabilité, et commiter un flow réel.

**Definition of Done:** un SKILL.md conducteur permet à un harness de piloter la flotte via la CLI; macOS ne laisse plus d'agents orphelins; Windows émet `ai.stop` sur Ctrl+C; un `flow.toml` de démo est commité et consommé.

#### US-016: SKILL.md conducteur (le manuel du plan de contrôle)
**Description:** As a power-user, I want un skill qui apprend à Claude Code (ou un autre harness) à orchestrer la flotte so that je n'écris pas l'orchestration à la main.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-007

**Acceptance Criteria:**
- [x] Given le SKILL.md, when lu par un harness, then il documente: découvrir (`paneflow ps`), lire l'état (`status`/`watch`), dispatcher (`send`), attendre un event (`watch`), et la discipline de reprise humaine
- [x] Given une action destructrice ou ambiguë, when le skill guide, then il prescrit de rendre la main à l'humain (sauf mode unrestricted explicitement assumé par l'utilisateur)
- [x] Given aucune instance Paneflow détectée, when le skill s'exécute, then il l'indique et s'arrête (pas de boucle)
- [x] Given le skill, when testé manuellement sur Claude + Codex + OpenCode, then il pilote les 3 sans modification (harness-agnostic)

#### US-017: Hardening cross-platform (macOS orphan guard, Windows ai.stop)
**Description:** As a mainteneur, I want fermer les trous OS du shim so that les agents ne survivent pas à un crash et l'état ne reste pas bloqué.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given macOS, when Paneflow est tué, then le shim/agent ne survit pas (kqueue NOTE_EXIT ou équivalent), comblant le stub `exec.rs:115`
- [x] Given Windows, when l'utilisateur fait Ctrl+C sur un agent, then `ai.stop` est émis (watcher SIGINT/console handler), évitant le spinner collé
- [x] Given un host Linux ou un cfg non compilable (Windows paneflow-app), when la story est livrée, then les branches sont inspection-only et documentées comme telles
- [x] Given le hardening, when testé sur l'OS cible, then un agent orphelin n'apparaît pas après `kill -9` de Paneflow (smoke documenté)

#### US-018: `flow.toml` de démo commité + consommé
**Description:** As a dev orchestrateur, I want un flow réel dans le repo so that il sert de référence et de dogfood (Goal 2 de la v2, jamais atteint).

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [x] Given le repo, when on cherche un flow, then un `flow.toml` de démo (review-pipeline: impl -> review) existe, commité
- [x] Given `paneflow flow run --dry-run <demo>`, when exécuté, then le plan est validé sans muter (parité dry-run)
- [x] Given le gate scripting OFF et un step `submit=true`, when run, then refus clair (parité comportement existant)
- [x] Given le SKILL.md, when il référence un exemple, then il pointe ce flow.toml

## Functional Requirements

- FR-01: Le système doit exposer une méthode `fleet.list` retournant l'état de tous les agents en cours, et `surface.status` pour une pane.
- FR-02: `surface.read` doit inclure `output_generation` dans sa réponse.
- FR-03: Le système doit permettre à un client de tenir une connexion abonnée et recevoir les events poussés (`events.subscribe`), sans gate scripting.
- FR-04: Un client lent ou mort ne doit jamais bloquer le render-thread ni les autres abonnés.
- FR-05: Le système doit fournir un verbe CLI `paneflow watch` qui stream les events en JSONL.
- FR-06: Le système doit exposer un réglage `ai_unrestricted` (défaut false) qui, actif, autorise l'auto-submit et accorde la capability d'écriture par pane aux conducteurs.
- FR-07: Le système doit exposer un réglage `ai_injection_fence` (défaut true) indépendant du mode unrestricted, fençant `surface.read`.
- FR-08: Le système NE doit PAS, en mode unrestricted OFF, modifier le comportement actuel (prefill-non-soumis, gate scripting).
- FR-09: Le système doit permettre un label stable posé atomiquement à la création d'une pane.
- FR-10: Le système doit détecter un agent Thinking bloqué en temps borné configurable.

## Non-Functional Requirements

- **Performance (push):** latence transition-interne -> écriture socket < 100 ms P95; debounce `surface_changed` <= 50 ms.
- **Performance (fence):** surcoût < 5 ms sur un `surface.read` de 64 KiB.
- **Reliability (watchdog):** un Thinking bloqué passe Stalled en <= 60 s par défaut (vs ~330 s aujourd'hui).
- **Reliability (watch):** heartbeat toutes les 30 s; backpressure bornée à 1024 events ou 8 MiB par abonné avant drop marqué.
- **Reliability (render-thread):** 0 I/O bloquant sur le thread GPUI (mesurable: aucun appel FS/socket synchrone dans un handler de render).
- **Security:** push lecture seule (pas de gate); writes gatés sauf unrestricted; fence neutralise 100 % des sentinelles de fermeture; peer-UID + 0600 conservés sur la connexion d'abonnement.
- **Compatibility:** Linux (Wayland+X11), macOS (Intel+Apple Silicon), Windows 10/11; chaque organe a un chemin par OS ou un stub documenté.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Flotte vide | Aucun agent en cours | `fleet.list` retourne `{agents: []}`, exit 0 | - |
| 2 | Agent sans hooks | Un des 6 agents non couverts | Liste avec `state: unknown_running, hooked: false` | - |
| 3 | Client abonné lent | Ne draine pas la connexion | Drop des events anciens, marqueur `{dropped: N}`, render-thread jamais bloqué | - |
| 4 | Abonné mort | Déconnexion brutale | Éviction propre, pas de fuite de compteur | - |
| 5 | Watch sans instance | Aucune instance Paneflow | Exit 3, message actionnable | "No running Paneflow instance" |
| 6 | ai.stop perdu | Shim tué, shell vivant | Stalled en <= 60 s (watchdog) | - |
| 7 | Toggle mid-session | Unrestricted OFF -> ON ou inverse | Gate effectif ré-évalué par appel, pas de capability résiduelle | - |
| 8 | Fence désactivé + repo malveillant | Sentinelle dans l'output | Risque assumé documenté; aucun fencing appliqué | avertissement à l'activation |
| 9 | Contexte > 64 KiB | Résultat volumineux | Bascule fichier temp, chemin en variable, pas de troncature silencieuse | - |
| 10 | Restart app | Paneflow redémarre | État de flotte perdu (non persisté), reconstruit au prochain hook (limite documentée) | - |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Prompt-injection amplification inter-agents (output d'un pair détourne le conducteur) | Med | High | Fence défaut ON même en unrestricted (US-011); documenté que human-takeover ne rattrape pas une injection rapide |
| 2 | Mode unrestricted: blast radius élevé sur erreur d'agent | Med | Med | Défaut OFF (US-008/009); recommandé sur worktrees isolés; reprise humaine sur toute pane; log structuré des writes (US-010) |
| 3 | Cost blowup multi-agents (+58% à +285% de tokens) | Med | Med | Cost déjà tracké; le skill recommande la parcimonie (ne pas fan-out gratuitement); pas de fan-out automatique imposé |
| 4 | Contrainte connexion persistante sur Windows | Med | Med | Design par-OS (US-004), pipe-instance dédié ou endpoint séparé; stub documenté si non implémentable sur le host |
| 5 | Le gap concurrentiel se referme (vibe-kanban, AGX migrent state-aware) | Med | High | Shipper le wedge (cross-vendor + interactif + visible + push) vite; revendiquer le framing control plane |
| 6 | Fragilité du pilotage par scrollback | Low | Med | Accès PTY-level + events structurés (US-005/006) > scraping; `output_generation` (US-003) remplace l'heuristique |

## Non-Goals

- **Orchestration cloud/remote.** Local-first uniquement; pas d'agents distants ni de control plane hébergé.
- **Réimplémenter les harness.** Paneflow pilote des process opaques par leur I/O; il ne réimplémente ni l'auth ni la boucle modèle d'un agent.
- **Turn-tracking des 6 agents sans hooks** (copilot, kiro-cli, droid, agy, openclaw, amp). Cécité documentée, pas comblée ce cycle.
- **Branchement runtime dans `flow.toml`.** Le DAG reste statique; l'adaptatif est le rôle du conducteur agentique, pas du format déclaratif.
- **Implémenter un standard A2A.** On fait une couche locale d'events, pas un protocole inter-agents standardisé.
- **Persistance de l'état de flotte au restart.** Hors scope ce cycle; limite documentée (edge case 10).
- **Monétisation.** Feature free + OSS, jamais derrière un plan payant.

## Files NOT to Modify

- `alacritty_terminal` (upstream crates.io 0.26) - VT emulation, ne pas forker.
- Le pin du fork GPUI (`ArthurDEV44/zed@paneflow/markdown-append-fix`) - ne pas remplacer par crates.io.
- La couche auth IPC (peer-UID/SO_PEERCRED, `ipc.rs:660-692`) - étendre, jamais affaiblir; le push réutilise le même check.
- Le fence MCP (`tools.rs`, `neutralize_sentinel`/`fence_id`) - réutiliser tel quel pour US-011, ne pas diverger.

## Technical Considerations

Frame comme questions pour l'engineering, pas mandats:

- **Architecture (connexion persistante):** recommandé d'étendre la boucle d'accept Unix pour garder la connexion d'abonnement ouverte; sur Windows, un pipe-instance par abonné vs un endpoint dédié - quel coût/complexité? US-004 à trancher.
- **Pont de diffusion:** brancher la rediffusion sur `upsert_session_state` + le poll `output_generation` existant (50ms). Un `mpsc` sortant des abonnés vers le thread socket, ou une diffusion directe? Éviter tout I/O socket sur le render-thread.
- **Modèle de capability:** un token de session minté à `workspace.up` et requis sur les writes, vs un flag per-pane accordé au caller? Le token scope mieux mais alourdit la CLI. Recommandé: per-pane capability tracée, token différé.
- **Fence serveur:** réutiliser `neutralize_sentinel` (`tools.rs`) sur `surface.read`; le wrapper doit-il être opt-in par appel (param `fenced: true`) en plus du réglage global? Recommandé: réglage global + override par appel.
- **Watchdog:** le seuil resserré (60s) réutilise le sweep 30s existant ou un timer par session? Recommandé: timer par session pour la granularité, avec garde anti-accumulation.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Appels pour lire l'état complet de la flotte | 3 sources non réconciliées (aucune API) | 1 (`fleet.list`) | Phase-1 | code + test d'intégration |
| Latence "agent change d'état -> conducteur notifié" | poll 500ms (flow) / jamais (pas de push) | < 100 ms P95 | Phase-1 | bench US-006 |
| Détection d'un Thinking bloqué | ~330 s | <= 60 s | Phase-1 | test watchdog US-013 |
| Harness hétérogènes orchestrés par le skill sans script shell | 0 | 3 (Claude+Codex+OpenCode) | Phase-6 | vérification manuelle US-016 |
| flow.toml réels commités dans le repo | 0 | >= 1 | Phase-6 | repo |
| Couverture de tests de `surface.read` | 0 test | handler couvert (offset/pagination/closed/generation) | Phase-1 | `cargo test` |

## Open Questions

- Le conducteur in-pane partage son propre contexte avec sa tâche de chef d'orchestre: a-t-on besoin d'un mode "conducteur dédié" (un agent dont le seul rôle est d'orchestrer) ou le skill suffit-il? À trancher avant US-016.
- La capability d'écriture en mode unrestricted doit-elle expirer (TTL) ou tenir tant que le mode est ON? Sécurité vs ergonomie. À trancher en US-010.
- Faut-il un indicateur visuel permanent (chrome) quand le mode unrestricted est actif, ou le warn log + le toggle suffisent? Éviter le chrome agrégat permanent (leçon Fleet Bar). À trancher en US-009.
[/PRD]
