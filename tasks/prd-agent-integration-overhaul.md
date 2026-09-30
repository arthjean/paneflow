[PRD]
# PRD: Refonte de l'intégration des agents

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-30 | Arthur Jean | PRD initial : met en œuvre les recommandations de l'analyse concurrentielle du 2026-09-30 (herdr, Unpeel, cmux face à Paneflow `main` @ `a23713b9`). 6 epics, 35 stories, livraisons R1 à R4. |

## Problem Statement

Le 2026-09-30, une analyse du code de herdr (`331775c3`), Unpeel (`2504ed8`), cmux (`ecba57ac71`) et Paneflow (`a23713b9`) a été menée par huit sous-agents, et chacune de ses affirmations clés a été revérifiée dans le code. Elle montre que Paneflow a la meilleure base (host détaché, ingestion durable et clôturée, catalogue strict, hygiène du collage), mais que ce qu'il affiche sur ses agents n'est vrai que pour deux d'entre eux.

1. **Le catalogue affirme des intégrations qui n'existent pas.**
   - Dix `runtimes/*/runtime.toml` déclarent `authority = "complete"`. Seuls Claude Code et Codex ont un installateur de hooks (`crates/paneflow-mcp-install/src/integrations.rs:171-177`).
   - Huit résumés promettent un installateur absent.
   - `lifecycle.authority` n'est lu qu'à `integrations.rs:172`.
   - Les dix variantes non-Claude, non-Codex de `RuntimeHookAdapter` n'ont aucun consommateur.
2. **Le statut perd des tours et ment sur sa source.**
   - `observe_foreground_runtime` efface l'entrée quand l'identité passe de `None` à `Some` (`crates/paneflow-serve/src/hook_state.rs:500-513`). Un prompt arrivé avant la première observation (poll de 2 s) est donc effacé.
   - Le host répond `hooked: true`, `state: "unknown"` et `output_generation: 0` dès qu'un hook a été reçu, quelle que soit sa génération (`crates/paneflow-host/src/control.rs:150-170`).
   - Le BEL est ignoré (`src-app/src/terminal/ghostty_session/events.rs:316`). Un agent sans hook, comme fx qui émet un BEL pour demander l'attention, ne signale donc jamais rien.
   - `StopFailure` termine le tour comme un `Stop` normal (`crates/paneflow-ai-hook/src/event.rs:173`).
   - Un hook sans outil identifiable est attribué à Claude (`crates/paneflow-ipc-client/src/ai_hook.rs:14`).
3. **Le plan de contrôle n'est pas une frontière.**
   - `session.input` et `session.create` du host s'exécutent sans gate (`crates/paneflow-host/src/server.rs:752-785`), alors que `surface.send_text` (`control.rs:174-181`) et le desktop (`src-app/src/app/ipc_handler/gates.rs:3-65`) en ont un.
   - Un agent détourné peut écrire dans n'importe quel pane, de n'importe quel workspace.
4. **La livraison d'un prompt n'est pas fiable.**
   - `paneflow send --submit` prend son instantané avant le collage et confirme le démarrage dès que l'écho du collage fait bouger `output_generation` (`src-app/src/cli/send_cmd.rs:100-104,186-202`).
   - Par le host, la confirmation n'arrive jamais, puisque la génération y vaut toujours 0.
   - Rien n'empêche d'envoyer un prompt à un agent bloqué sur une permission, ou à un pane où l'agent a quitté.
5. **Un redémarrage perd toutes les conversations.**
   - Les ids de session des fournisseurs ne sont pas persistés dans `session.json`.
   - Après une perte du host, chaque pane rouvre un shell nu.
6. **La connaissance par agent est dispersée et les boucles coûtent.**
   - La connaissance par agent vit en sept endroits codés en dur (`agent_launcher.rs:15-29,117-139,175-187`, `agent_sessions.rs:2-38`, `auto_naming.rs:178-180`, `integrations.rs:13-35`, les listes WiX et release).
   - Deux moteurs d'installation appliquent des politiques de symlink différentes.
   - Le thread GPUI se réveille toutes les 50 ms pour drainer l'IPC (`src-app/src/app/bootstrap.rs:535`).
   - Le worker ne voit un changement qu'au poll suivant, jusqu'à 2 s plus tard (`crates/paneflow-serve/src/worker.rs:19`).
   - L'ingestion d'un hook garde le verrou du ledger pendant une attente de durabilité qui peut durer 5 s (`crates/paneflow-host/src/host.rs:843-900`, `persistence.rs:17`), alors que `paneflow-ai-hook` abandonne à 350 ms (`crates/paneflow-ai-hook/src/transport.rs:5`).

**Why now:** l'avantage multiplateforme se rétrécit.
- herdr est disponible sous Windows et met à jour à distance ses règles d'écran pour environ 22 agents.
- cmux-tui (Rust + libghostty-vt) porte déjà des chemins `cfg(windows)`.
- Les CLI d'agents évoluent chaque semaine : Claude Code 2.1.285 et Codex 0.159.2 ajoutent reprise, fork et équipes d'agents.

L'avantage durable de Paneflow tient désormais à trois choses : une GUI native sur trois OS, un host qui survit à l'application, et une orchestration sûre avec l'humain dans la boucle. Les deux derniers points supposent un statut véridique, une reprise après redémarrage et un plan de contrôle gardé. La restauration des sessions a été demandée le 2026-09-02, et l'intégration de fx attend un moteur de règles déclaratif.

## Overview

Le PRD rend d'abord vrai ce que Paneflow affiche (R1).
- Le build refuse une autorité que l'intégration ne fournit pas.
- La course du latch est reproduite puis corrigée.
- Le BEL devient un signal d'attention pour les agents sans hook.
- Les écritures du host passent par les mêmes gates que le desktop et restent dans le workspace appelant.
- `paneflow send` refuse un agent bloqué et ne confirme un démarrage que sur un changement d'état réduit, sur le modèle du gate d'activité de herdr.

R2 apporte la reprise après perte du host et consolide le code.
- **Reprise :** le catalogue déclare les gabarits de reprise et de fork de chaque runtime, et chaque surface mémorise l'id de session reçu par le hook `SessionStart`. Au redémarrage, Paneflow tape la commande de reprise dans le shell une fois la géométrie connue, avec un étalement, un dédoublonnage et une protection de la saisie. Si la reprise échoue, une bannière propose une nouvelle session.
- **Consolidation :** la connaissance par agent codée en dur passe dans le catalogue, et les deux moteurs d'installation fusionnent.
- **Boucles :** le poll GPUI de 50 ms, le poll de snapshot de 2 s et l'attente fsync sous verrou sont remplacés, mesures à l'appui.

R3 remplace les deux motifs d'écran figés par un moteur de règles déclaratif. Il couvre les régions, les priorités, les bloqueurs visibles, le titre OSC, les surcharges locales, le rechargement à chaud et `paneflow agent explain`. Un corpus d'écrans de référence capturés sur les vraies CLI le vérifie. Un catalogue distant, signé minisign et protégé contre le retour arrière, le met à jour. fx y entre comme runtime purement déclaratif.

R4 ouvre l'orchestration sûre.
- Un outil MCP d'écriture relaie la demande, et le host l'applique seulement après l'approbation d'un humain pour la paire d'occurrences d'agents.
- Le message porte une en-tête de provenance.
- Un shim tmux, porté par le binaire `paneflow`, ouvre les coéquipiers des équipes d'agents Claude dans des panes natifs. Un spike go/no-go le précède.

Décisions clés :
- **Honnêteté plutôt qu'installateurs :** le catalogue est rendu honnête au lieu d'écrire huit installateurs de hooks.
- **Pas de protocole herdr :** le protocole herdr n'est pas repris, et fx passe par l'écran et le BEL.
- **Pré-assignation Claude optionnelle :** la pré-assignation de l'id de session Claude reste optionnelle et désactivée par défaut.
- **Approbations éphémères :** les approbations vivent en mémoire, sont liées à l'occurrence, et aucune méthode IPC ne peut les accorder.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Runtimes dont l'autorité déclarée correspond à l'intégration réelle | 18/18 (8/18 aujourd'hui) | 19/19 avec fx |
| Panes Claude Code et Codex rouverts sur leur conversation après un redémarrage de la machine | ≥ 90 % (0 % aujourd'hui) | ≥ 95 %, plus fx par `--continue` |
| Délai p95 entre un changement dans le host et la projection du worker | < 100 ms (jusqu'à 2 000 ms aujourd'hui) | < 100 ms |
| Écritures PTY acceptées d'un client de contrôle sans gate ou hors de son workspace | 0 (illimitées aujourd'hui) | 0 |
| Écrans du corpus classés dans leur état attendu, par runtime | baseline mesurée par US-025 | ≥ 95 % pour chaque runtime du corpus |

## Target Users

### Développeur qui fait tourner plusieurs agents en parallèle
- **Role:** Développeur qui fait tourner de 2 à 8 agents CLI (Claude Code, Codex, OpenCode, Gemini, Pi, fx) dans plusieurs workspaces Paneflow, sous Linux, macOS ou Windows.
- **Behaviors:** Il laisse les agents travailler, surveille l'Attention Queue, relance les sessions après une mise à jour ou un redémarrage, et utilise `paneflow send` et `wait` dans des scripts.
- **Pain points:**
  - Un agent sans hook reste affiché « idle » quand il attend une réponse.
  - Un redémarrage lui fait perdre toutes ses conversations.
  - `send --submit` confirme un démarrage qui n'a pas eu lieu.
- **Current workaround:**
  - Il retape `claude --resume` à la main après avoir retrouvé l'id dans la sidebar « Resume Ended Sessions ».
  - Il passe d'un pane à l'autre pour voir qui attend.
- **Success looks like:** Après un redémarrage, chaque conversation revient dans son pane ; tout agent qui attend apparaît dans l'Attention Queue en moins de 2 s.

### Agent conducteur et auteur de scripts
- **Role:** Un agent qui orchestre d'autres agents avec le skill `paneflow-conductor`, ou un script CLI écrit par le développeur.
- **Behaviors:** Il appelle `paneflow send`, `wait --idle`, `read` et les outils MCP en lecture.
- **Pain points:**
  - Il ne peut pas savoir si un prompt a été pris en compte.
  - Il peut écrire dans le mauvais workspace sans le savoir.
  - Il ne dispose d'aucun canal d'écriture MCP approuvé.
- **Current workaround:** Il sonde l'écran avec `read` et devine l'état à partir du texte.
- **Success looks like:** Chaque envoi renvoie un résultat vérifiable (`started`, `state`, raison) ; une écriture vers un autre agent passe par une approbation humaine explicite.

### Contributeur qui ajoute un runtime
- **Role:** Un contributeur, ou Arthur, qui ajoute un agent au catalogue.
- **Behaviors:** Il crée `runtimes/<slug>/runtime.toml` et ses fixtures.
- **Pain points:** `runtimes/README.md` promet « no Rust edits », mais sept endroits du code doivent être modifiés ; les règles d'écran ne se vérifient que contre deux fixtures.
- **Current workaround:** Il copie un runtime existant et suit les échecs de compilation.
- **Success looks like:** Un nouveau répertoire, ses règles et ses captures suffisent, et `paneflow agent explain` montre pourquoi un écran est classé.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **herdr** (Rust TUI, Windows GA, `331775c3`)
  - **Ce qu'il fait bien :**
    - Manifests d'écran TOML (régions, priorité, `all`/`any`/`not`, drapeaux `visible_blocker` et `visible_idle`), avec un catalogue distant versionné par moteur, des surcharges locales, un rechargement à chaud et `agent explain`.
    - Le statut de Claude y passe uniquement par l'écran, par choix.
    - `agent prompt` refuse un agent bloqué ou absent du premier plan et applique un gate d'activité de 5 s.
    - La reprise est tapée dans le shell de login après la géométrie, avec étalement et dédoublonnage.
  - **Ses faiblesses :**
    - Catalogue non signé et sans corpus.
    - `valid_session_id` accepte un `-` initial (CWE-88).
    - `resume_argv` accepte n'importe quelle commande du PATH.
    - `ESC[201~` n'est pas filtré.
    - Les hooks exigent `python3`.
    - Le socket n'a aucune autorisation.
  - **Paneflow reprend** le moteur de règles, le gate et la reprise. Il ajoute la signature, le corpus et un gabarit borné aux alias du runtime.
- **Unpeel** (Mac et Linux, `2504ed8`)
  - **D'où vient le moteur de statut :** Paneflow a porté son moteur de statut au commit `443877b` (`THIRD_PARTY_NOTICES.md:18-30`).
  - **Ce qu'il apporte :**
    - Outils MCP d'écriture approuvés par un humain pour chaque paire, liés à `agent_ref` (session, runtime, PID, heure de démarrage, génération).
    - Refus de s'écrire à soi-même et en-tête de provenance.
    - Budget de schéma MCP (< 16 KiB au total, < 4 KiB par domaine).
    - Marqueurs d'échec de reprise, `has_been_written_to`, Ctrl-U et commande en une seule écriture.
  - **Point d'attention :** il a retiré l'injection au lancement dans `eb5500a`.
  - **Ce qui manque :** pas de Windows.
- **cmux** (app macOS, `ecba57ac71`)
  - Environ 51 000 lignes de code de hooks couvrent 18 agents.
  - Son wrapper Claude pré-assigne `--session-id` (`Resources/bin/cmux-claude-wrapper:2336-2344`).
  - Il sait forker une conversation vers un split, garde le propriétaire vivant, et fournit une compatibilité tmux (`cmux claude-teams`, environ 20 verbes).
  - Ses PTY meurent avec l'application, et il ne tourne que sous macOS.
- **fx** (vercel-labs, Zig, v0.0.5)
  - Pas de hooks. Il émet un BEL quand il demande l'attention, et son titre OSC 2 a la forme `fx · <titre> · <modèle>`.
  - Sa seule sortie de statut est le protocole herdr.
  - Pas de Windows.
  - Il se reprend avec `fx --resume <id>` et `fx --continue`.
- **Market gap:** aucun outil ne combine une GUI native sur les trois OS, un host qui garde les PTY au-delà de l'application, un statut vérifié par un corpus, et une orchestration entre agents approuvée par un humain.

### Best Practices Applied
- **Hooks d'abord.** Le statut vient des hooks quand ils existent : Claude Code expose environ 30 événements, dont `StopFailure`, `PermissionRequest` et `SessionStart` avec les sources `startup`/`resume`/`clear`/`compact`/`fork` ; Codex en expose 12, dont `Interrupt` et `SessionEnd`. L'écran et le BEL prennent le relais sinon. Sources : [Claude Code hooks](https://code.claude.com/docs/en/hooks), [Codex hooks](https://developers.openai.com/codex).
- **Flags vérifiés sur les CLI installées le 2026-09-30.**
  - Claude Code 2.1.285 : `--session-id <uuid>` (« must be a valid UUID »), `--resume <id>`, `--continue`, `--fork-session`.
  - Codex 0.159.2 : `codex resume [SESSION_ID]`, `codex fork [SESSION_ID]`, `--no-daemon`.
  - Le fork ne copie que la conversation, pas les fichiers ([Claude sessions](https://code.claude.com/docs/en/sessions)).
- **Humain dans la boucle.** Approbation humaine à la frontière du PTY, pas de confiance transitive entre agents, texte relayé traité comme non fiable, journal d'audit ([OWASP Top 10 for LLM Applications 2025](https://genai.owasp.org/llm-top-10/), LLM01 Prompt Injection et LLM06 Excessive Agency).
- **Annotations MCP.** Les annotations d'outil (`readOnlyHint`, `destructiveHint`) ne sont que des indications non fiables. La révision 2026-07-28 ajoute `input_required` pour l'elicitation, mais son support côté client n'est pas établi ([MCP specification 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28)).
- **Signature et anti-downgrade.** Signature minisign avec commentaire de confiance signé, qui porte la version et interdit le retour arrière ([minisign](https://jedisct1.github.io/minisign/)). Paneflow vérifie déjà ses mises à jour en fail-closed (`src-app/src/update/signature.rs:33-117`).

*Les rapports complets restent dans la session d'analyse du 2026-09-30 ; ils ne sont pas versionnés.*

## Assumptions & Constraints

### Assumptions (to validate)
- **Id de session des hooks.** Le hook `SessionStart` de Claude Code et de Codex porte le `session_id` de la conversation courante, y compris après `/clear`, `/resume` ou un fork, d'après la documentation des hooks Claude (champ `source`). US-012 le vérifie sur les deux CLI avant de s'y appuyer.
- **Reprise sans relance du travail.** `claude --resume <id>` et `codex resume <id>` rouvrent la conversation et attendent une saisie, sans relancer le travail en cours. US-013 le vérifie sur un vrai PTY.
- **BEL et OSC visibles par le host.** Le moteur libghostty-vt du host remonte le BEL, le titre OSC et la progression OSC 9;4. Le desktop les reçoit déjà (`events.rs:301,305,316`) ; US-004 et US-027 le confirment côté host.
- **Couverture par l'écran.** Des règles d'écran déclaratives atteignent au moins 95 % de bonne classification sur le corpus pour chaque runtime visé, comme herdr le fait pour environ 22 agents sans signature ni corpus. US-025 mesure la baseline.
- **Équipes d'agents Claude.** En `teammateMode` tmux, elles utilisent un petit ensemble de sous-commandes tmux sans mode de contrôle (`-C`) : cmux en émule une vingtaine. Les sous-agents de Codex n'ouvrent pas de terminal. Le spike US-034 valide les deux points.
- **Corpus sans secrets.** Un masquage automatique (chemins du home, motifs de clés connus) suffit pour publier des captures sans fuite de secret ; chaque capture est relue à la review de la PR.

### Hard Constraints
- **Trois plateformes :** Linux (Wayland et X11), macOS et Windows 10/11 pour chaque story. Seules exceptions : fx (Linux et macOS, fx n'existe pas sous Windows) et le shim tmux, si US-034 montre que Claude n'offre pas ce mode sous Windows ; ce cas est alors un stub documenté.
- **Thread de rendu :** il ne bloque jamais. Watchers, téléchargements, lectures de session et évaluation des règles tournent hors du thread GPUI.
- **Plafonds des helpers :** `paneflow-shim` ≤ 512 KiB, `paneflow-ai-hook` ≤ 384 000 B, `paneflow-mcp` ≤ 512 KiB, total ≤ 1 835 008 B (`src-app/build.rs:8`). Aucun nouveau helper embarqué.
- **Aucun commentaire dans le code source.** Clippy passe en `--all-targets` et `-D warnings`. Tout item déclaré avant `mod tests`, y compris `#[cfg(windows)]`.
- **Humain dans la boucle :**
  - Paneflow ne soumet jamais seul un prompt qu'il a composé.
  - Pas de nouveau chrome permanent agrégé : le triage passe par l'Attention Queue.
  - Pas de flux « Review with agent ».
- **Domicile Paneflow :** l'état propre à Paneflow vit sous le domicile de `crates/paneflow-home`, jamais sous `dirs::config_dir`.
- **Coordination avec d'autres PRD :**
  - `tasks/prd-fork-audit-fixes.md` : US-032, US-034 et US-038 sont en revue ; US-041 et US-044 sont à faire.
  - `tasks/prd-agents-browser.md` EP-006 : il partage le modèle d'accès agent par workspace.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, tests compris
- `cargo test --workspace --locked` - tests unitaires et d'intégration
- `cargo deny check advisories licenses sources` - uniquement quand une dépendance change

Gates additionnels :
- Stories qui touchent `paneflow-shim`, `paneflow-ai-hook` ou `paneflow-mcp` : le job CI « Release build + binary-size budget (Linux x86_64) » passe.
- Stories qui touchent du code `#[cfg(windows)]` : le job « Windows x86_64 libghostty check » passe, et la PR dit si Windows a été vérifié par inspection ou sur le matériel réel.
- Stories UI (bannières, Attention Queue, menu contextuel, lanceur) : passe manuelle sous Linux avec capture d'écran ou enregistrement court joint à la PR.
- Stories de performance : mesure avant/après jointe à la PR, avec la commande qui la reproduit.

## Epics & User Stories

### EP-001: Rendre le statut et l'attention véridiques

Livraison R1. Ce que Paneflow affiche sur un agent devient vrai : l'autorité déclarée correspond à l'intégration réelle, un tour n'est plus perdu, un agent sans hook signale l'attention, et `agent.status` ne ment plus.

**Definition of Done:** le build échoue sur une autorité non fournie ; la course du latch est reproduite puis corrigée, ou annulée avec la preuve ; un BEL d'un agent sans hook produit une ligne d'attention ; `agent.status` ne renvoie plus ni `unknown` ni génération constante.

#### US-001: Rendre le catalogue fidèle aux intégrations réelles
**Description:** As a développeur qui lance un agent depuis Paneflow, I want que la fiche de chaque runtime décrive l'intégration réellement installée so that je sache si son statut vient d'un hook, de l'écran ou de rien.

Contexte :
- Dix runtimes déclarent `authority = "complete"` : claude-code, codebuddy, codex, cursor-agent, gemini, muse-code, opencode, qoder, hermes, grok. Seuls Claude et Codex ont un installateur (`crates/paneflow-mcp-install/src/integrations.rs:171-177`).
- `RuntimeHookAdapter` compte 12 variantes nommées ; seules `Claude` et `Codex` sont consommées (`integrations.rs:152-454`).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un `runtime.toml` qui déclare `authority = "complete"` avec un `hook_adapter` sans installateur, when `paneflow-agent-config` compile, then le build échoue avec un message qui nomme le fichier, le champ et les adaptateurs acceptés.
- [ ] Les huit runtimes concernés déclarent l'autorité que leur intégration fournit : `screen` s'ils ont des règles d'écran, `none` sinon.
- [ ] Les résumés de ces huit runtimes ne mentionnent plus ni hook ni installateur.
- [ ] `RuntimeHookAdapter` ne garde que `None`, `Claude` et `Codex`.
- [ ] Un test parcourt le catalogue et vérifie, pour chaque runtime : `authority = "complete"` si et seulement si `has_installer` est vrai.
- [ ] Échec : given `hook_adapter = "gemini"` dans un runtime de test, when le build tourne, then il échoue en listant `none`, `claude` et `codex`.

#### US-002: Reproduire puis corriger la perte du latch à la première observation du runtime
**Description:** As a développeur, I want qu'un prompt envoyé juste après le lancement d'un agent reste visible so that la sidebar n'affiche pas « idle » pendant un tour réel.

`observe_foreground_runtime` (`crates/paneflow-serve/src/hook_state.rs:500-513`) traite un passage de `None` à `Some` comme un changement d'agent et réinitialise l'entrée. Comme l'observation arrive par le snapshot de 2 s, un `UserPromptSubmit` reçu avant elle est effacé. Cette story touche le même fichier que l'US-038 de `prd-fork-audit-fixes.md` (en revue) : elle se rebase dessus.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un test reproduit la séquence suivante :
  1. Observation `None`.
  2. Hook `UserPromptSubmit` pour la génération G.
  3. Observation `Some("claude")`.

  Le test attend l'état `working` après l'étape 3. S'il n'échoue pas sur `a23713b9`, la story est annulée avec la sortie du test comme preuve.
- [ ] Après correctif, un passage de `None` à `Some` ne réinitialise pas une entrée dont `last_hook_generation` égale la génération de lancement courante.
- [ ] Un passage de `Some(A)` à `Some(B)` réinitialise toujours l'entrée (test).
- [ ] Échec : given la séquence `Some("claude")`, puis `None`, puis `Some("codex")`, when Codex démarre, then aucun état hérité de Claude ne subsiste (test).

#### US-003: Faire remonter les échecs de tour et retirer l'attribution par défaut à Claude
**Description:** As a développeur, I want qu'un tour qui échoue (limite de débit, erreur d'API) apparaisse comme une attention avec sa raison, et qu'un hook non identifié ne crée pas de fausse ligne Claude, so that je voie les agents arrêtés sur une erreur.

`StopFailure` est traité comme `Stop` (`crates/paneflow-ai-hook/src/event.rs:173`), et `DEFAULT_TOOL = "claude"` sert de repli (`crates/paneflow-ipc-client/src/ai_hook.rs:14,90,100,324`). L'attribution sous le daemon Codex relève de l'US-032 de `prd-fork-audit-fixes.md` : cette story ne la duplique pas.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un hook `StopFailure` de la génération courante, when il est ingéré, then l'état réduit devient `attention`, avec pour raison la valeur du champ d'erreur du payload tronquée à 128 octets.
- [ ] `Interrupt` (Codex) ramène toujours l'état à `idle` (test de non-régression).
- [ ] Un hook dont l'outil ne se déduit ni d'argv[0], ni du payload, ni du runtime du pane est abandonné avec une `DropReason` journalisée ; il n'est jamais attribué à Claude.
- [ ] `paneflow-ai-hook` reste sous 384 000 B dans le profil `release-min`.
- [ ] Échec : given un payload sans outil dans un pane sans runtime reconnu, when le hook tourne, then aucune ligne « Claude » n'apparaît et le journal nomme la raison (test).

#### US-004: Transformer le BEL en signal d'attention pour les agents sans hook
**Description:** As a développeur qui utilise un agent sans hook (fx, Pi, Gemini), I want qu'un BEL émis par l'agent le fasse apparaître dans l'Attention Queue so that je sache qu'il m'attend sans surveiller son pane.

Aujourd'hui `BackendEvent::Bell => {}` (`src-app/src/terminal/ghostty_session/events.rs:316`). Le host observe le BEL dans le flux de la session : ainsi le signal atteint le reducer du worker même sans desktop attaché.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un pane dont le runtime au premier plan n'a pas `authority = "complete"`, when un BEL (0x07 hors séquence OSC, DCS ou APC) arrive, then l'état réduit devient `attention` avec la raison `bell`, et l'Attention Queue liste le pane.
- [ ] Au plus une transition due au BEL toutes les 2 s par session ; une rafale de 1 000 BEL produit une seule transition et ne déclare jamais le pane terminé (test).
- [ ] Pour un runtime avec `authority = "complete"`, le BEL ne change pas l'état réduit (test).
- [ ] L'attention due au BEL s'efface à la prochaine saisie de l'utilisateur dans le pane, ou à la sortie suivante si le runtime déclare `attention_clears_on_output`.
- [ ] Quand `agent_panel.notify_when_agent_waiting` n'est pas `Never`, une attention due au BEL déclenche la même notification qu'une attente signalée par hook.
- [ ] Échec : given un shell sans agent au premier plan, when `printf '\a'` s'exécute, then aucune ligne d'agent n'est créée (test).

#### US-005: Rendre `agent.status` véridique
**Description:** As a auteur de script, I want que `hooked`, `state` et `output_generation` décrivent l'occurrence courante de l'agent so that `send`, `wait` et le skill conductor décident sur des faits.

Le host renvoie `hooked: true`, `state: "unknown"` et `output_generation: 0` dès qu'un hook existe (`crates/paneflow-host/src/control.rs:150-170`).

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `hooked` vaut `true` seulement si le dernier hook porte la génération courante du manifest et si le runtime a `authority = "complete"`.
- [ ] `state` porte l'état réduit lu dans la projection du worker, comme la sidebar. Il est absent quand aucune projection n'existe, et la chaîne `unknown` n'est plus jamais renvoyée (test).
- [ ] La réponse porte `state_seq`, un compteur par session incrémenté à chaque transition d'état réduit.
- [ ] `output_generation` porte le compteur réel, ou il est absent ; jamais une constante.
- [ ] `docs/user/scripting.md` décrit ces champs.
- [ ] Échec : given un agent relancé dont le dernier hook porte la génération G-1, when `agent.status` est appelé, then `hooked` vaut `false` (test).

---

### EP-002: Sécuriser le plan de contrôle et fiabiliser la livraison des prompts

Livraison R1. Les écritures dans un PTY passent toutes par un gate, restent dans le workspace appelant, et un envoi rapporte ce qui s'est réellement passé.

**Definition of Done:**
- Un client de contrôle ne peut ni écrire ni lancer une commande sans la permission correspondante, ni écrire hors de son workspace.
- `paneflow send --submit` refuse un agent bloqué ou absent du premier plan, et ne confirme un démarrage que sur une transition d'état.
- `wait --idle` suit la fin de tour.

#### US-006: Soumettre les écritures du host aux gates du desktop
**Description:** As a développeur qui laisse des agents utiliser le CLI, I want que `session.input` et `session.create` refusent un client de contrôle non autorisé so that un agent détourné ne tape pas dans un autre pane ni ne lance de commande.

`crates/paneflow-host/src/server.rs:752-785` exécute ces méthodes sans gate. Les clients se déclarent dans `host.hello` (`server.rs:388,554-575`) : le desktop attache un moteur de terminal, un client de contrôle non. Ce gate protège contre un agent confus, pas contre un processus hostile du même utilisateur.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un client de contrôle sans permission `scripting`, when il appelle `session.input`, then le host répond par une erreur qui nomme `PANEFLOW_IPC_SCRIPTING`, et aucun octet n'est écrit.
- [ ] `session.create` depuis un client de contrôle exige `orchestration` quand la requête porte `command`, `env` ou `prompt` (mêmes déclencheurs que `src-app/src/app/ipc_handler/gates.rs:40-47`), et `scripting` sinon.
- [ ] Un client qui attache un moteur garde le comportement actuel ; `crates/paneflow-host/tests/conformance.rs` passe sans modification de ses attentes.
- [ ] `agent.event` reste accepté sans gate, puisque la vérification de génération (`crates/paneflow-host/src/host.rs:859-885`) le lie à une occurrence vivante. Un test prouve qu'une génération périmée est refusée.
- [ ] Chaque `session.input` accepté d'un client de contrôle est journalisé au niveau info : nom du client, session cible, nombre d'octets, sans le contenu.
- [ ] `docs/user/scripting.md` précise la portée de ce gate.
- [ ] Échec : given `scripting` actif mais `orchestration` inactif, when un client de contrôle appelle `session.create` avec `command`, then la requête est refusée et aucun processus n'est lancé (test).

#### US-007: Limiter les écritures d'un pane à son workspace
**Description:** As a développeur qui sépare ses projets en workspaces, I want qu'un agent lancé dans le workspace A ne puisse pas écrire dans un pane du workspace B sans demande explicite so that une injection de prompt reste confinée à son projet.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given un appelant qui s'exécute dans un pane Paneflow (contexte `PANEFLOW_SESSION_ID` transmis), when il appelle `surface.send_text`, `surface.send_keystroke` ou `session.input` vers une session d'un autre workspace, then l'appel est refusé avec un message qui nomme le workspace cible et l'option qui élargit la portée.
- [ ] Avec `scope: "all"` (`paneflow send --scope all`) et la permission `orchestration`, l'appel est accepté.
- [ ] Un appelant hors de tout pane (terminal externe) garde la portée de l'instance.
- [ ] La résolution du workspace appelant réutilise la fonction qui dérive côté serveur la portée MCP (US-036 de `prd-fork-audit-fixes.md`) ; il n'existe pas de seconde implémentation.
- [ ] Échec : given un `PANEFLOW_SESSION_ID` qui ne désigne aucune session connue, when l'appelant écrit, then l'appel est refusé avec « session appelante inconnue » (test).

#### US-008: Refuser la livraison à un agent bloqué ou absent du premier plan
**Description:** As a auteur de script, I want que `paneflow send` refuse d'écrire dans un agent qui attend une décision humaine, ou dans un pane où l'agent n'est plus au premier plan, so that mon texte ne réponde pas à un dialogue de permission ni n'atterrisse dans un éditeur.

Même règle que `agent prompt` de herdr.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un pane dont l'état réduit est `blocked` (permission, menu, question), when `paneflow send` le vise, then rien n'est écrit et la commande échoue dans la catégorie d'erreur `target`, avec le message « surface N attend une décision (raison) : réponds dans le pane ou relance avec --force ».
- [ ] Given un pane qui porte une ligne d'agent mais dont le processus au premier plan n'est plus ce runtime (shell, éditeur), when `paneflow send` le vise, then l'envoi est refusé de la même façon.
- [ ] `--force` contourne ces deux vérifications et l'usage est journalisé.
- [ ] Un pane sans ligne d'agent (shell simple) n'est pas concerné par ces vérifications.
- [ ] Échec : given un agent bloqué sur une permission, when `send --submit` est appelé sans `--force`, then aucun octet n'est écrit (test avec un statut simulé).

#### US-009: Confirmer le démarrage d'un tour par une transition d'état
**Description:** As a agent conducteur, I want que `paneflow send --submit` ne dise « démarré » que si l'agent a réellement commencé un tour so that je n'attende pas un résultat qui ne viendra pas.

Aujourd'hui l'écho du collage suffit à confirmer (`src-app/src/cli/send_cmd.rs:100-104,186-202`), et par le host la confirmation n'arrive jamais.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] `send --submit` lit `state_seq` avant d'écrire. Il confirme `started: true` seulement si, dans les 5 s, un statut montre un `state_seq` supérieur et un état `working`, `attention`, `blocked` ou `idle` postérieur à la transition.
- [ ] Le seul écho du collage ne confirme jamais : un test où la sortie grandit sans transition renvoie `started: false`.
- [ ] Sans transition au bout de 5 s, le JSON porte `delivered: true`, `started: false` et `reason: "no_state_transition"` ; le code de sortie reste celui d'un démarrage non confirmé aujourd'hui.
- [ ] Pour un runtime sans hook et sans règle d'écran, le JSON porte `started: null` et `reason: "no_signal"`.
- [ ] Le JSON porte toujours l'état réduit final observé.
- [ ] Échec : given un agent qui passe en `blocked` juste après la soumission, when la confirmation tombe, then le résultat vaut `started: true` avec `state: "blocked"` (test).

#### US-010: Faire suivre la fin de tour à `wait --idle` et garder la soumission différée côté host
**Description:** As a agent conducteur, I want que `wait --idle` rende la main à la fin du tour de l'agent plutôt qu'après un silence de sortie, et qu'une soumission par le host suive le même protocole que le desktop, so that une longue commande silencieuse ne passe pas pour une fin de tour et que le `\r` ne soit pas avalé.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un pane dont le runtime a `authority = "complete"` ou `screen`, when `wait --idle` tourne, then il rend la main sur une transition vers `idle` ou `attention` postérieure à son démarrage, et non sur le silence de sortie.
- [ ] Pour les autres panes, la quiescence de sortie actuelle reste le critère (test de non-régression).
- [ ] Given le desktop fermé et le host vivant, when `paneflow send --submit` passe par le host, then le collage entre crochets et le `\r` différé suivent les délais du desktop : plancher de 70 ms, poll d'écho de 15 ms, marge de 500 ms (`src-app/src/app/ipc_handler/surface_methods.rs`, `crates/paneflow-config/src/schema/config.rs:245`). Test sur le host.
- [ ] Échec : given un agent hooké qui exécute un outil silencieux pendant 30 s, when `wait --idle --for 5s` tourne, then il ne rend pas la main avant le hook `Stop` (test avec hooks simulés).

---

### EP-003: Reprendre les conversations d'agents après perte du host

Livraison R2. Un redémarrage de la machine ne coûte plus les conversations : chaque pane dont l'id de session est connu rouvre sa conversation dans son pane, et un fork ouvre une branche dans un split.

**Definition of Done:**
- Après un redémarrage, au moins 90 % des panes Claude Code et Codex dont l'id est connu rouvrent leur conversation, sans doublon ni saisie écrasée.
- Un échec affiche une bannière qui propose une nouvelle session.

#### US-011: Déclarer les commandes de reprise et de fork dans le catalogue
**Description:** As a contributeur qui ajoute un runtime, I want déclarer dans `runtime.toml` comment reprendre et forker une conversation so that la reprise ne dépende plus d'une table codée en dur.

Aujourd'hui `resume_command_spec` vit dans `src-app/src/app/sessions_sidebar.rs:958-1015`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `runtime.toml` accepte une section `[resume]` qui déclare :
  - `session_argv` : gabarit avec `{session_id}`, comme `session_argv` et `fork_argv` ;
  - `continue_argv` ;
  - `fork_argv` ;
  - `session_id_pattern` : une regex ;
  - `failure_markers` : une liste de textes.
- [ ] Le build refuse :
  - un gabarit dont le programme n'est pas un alias de commande du runtime ;
  - `{session_id}` en position de programme ;
  - une regex invalide.
- [ ] Les valeurs de chaque runtime reprennent exactement celles de `resume_command_spec`.
- [ ] `fork_argv` n'est déclaré que pour les CLI dont le flag a été vérifié :
  - Claude Code : `claude --resume {session_id} --fork-session` (2.1.285) ;
  - Codex : `codex fork {session_id}` (0.159.2).
- [ ] Un test compare, pour chaque agent de `SessionAgent`, l'argv produit avant et après : aucune différence.
- [ ] `is_valid_session_id` (`src-app/src/agent_sessions.rs:372-379`) s'applique en plus de `session_id_pattern`, et le test de régression CWE-88 reste en place.
- [ ] Échec : given l'id `--dangerously-skip-permissions`, when une commande de reprise est construite, then aucune commande n'est produite et un avertissement est journalisé (test).

#### US-012: Mémoriser par surface l'id de session de l'agent
**Description:** As a développeur, I want que Paneflow retienne quelle conversation tourne dans chaque pane so that elle puisse revenir après un redémarrage.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] `SurfaceDefinition` (`crates/paneflow-config/src/schema/layout.rs:326-347`) reçoit un champ optionnel `agent_session`, qui porte le runtime, l'id et le cwd, sérialisé seulement quand il est présent.
- [ ] Les trois sites de construction (`src-app/src/app/session.rs`, `src-app/src/layout/serde.rs:33,51`, `src-app/src/settings/tabs/workspaces/editor.rs:680`) et `schemas/paneflow.schema.json` sont mis à jour.
- [ ] Le champ est rempli à partir du `session_id` d'un hook `SessionStart` de la génération courante.
- [ ] Un nouveau `SessionStart` remplace le champ (`/clear`, `/resume`, fork), et un test le vérifie pour Claude Code et pour Codex avec des payloads capturés sur les CLI réelles.
- [ ] Le champ est effacé quand le runtime se termine alors que la session du pane continue (`SessionEnd`, retour au shell) : un agent fermé volontairement ne revient pas.
- [ ] Le champ est conservé quand la session entière disparaît avec le host (arrêt, crash, reboot). Un test couvre les deux cas.
- [ ] L'écriture passe par la coalescence de `save_session`, sans I/O synchrone sur le thread GPUI.
- [ ] Échec : given un `session.json` d'une version antérieure sans le champ, when il est chargé, then la restauration est identique à aujourd'hui (fixture).
- [ ] Échec : given un id qui ne respecte pas `session_id_pattern`, then il n'est pas stocké et un avertissement est journalisé.

#### US-013: Relancer l'agent dans son pane après perte du host
**Description:** As a développeur qui redémarre sa machine, I want que chaque pane d'agent rouvre sa conversation dans le même dossier so that je reprenne là où j'en étais sans rechercher les ids.

Modèles :
- herdr : saisie dans le shell de login après la géométrie, avec étalement et dédoublonnage.
- Unpeel : `has_been_written_to`, Ctrl-U et commande en une seule écriture.
- cmux : garde du propriétaire vivant.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] Given une surface restaurée avec `agent_session`, dont la session hébergée n'existe plus (`HostLinkState::Unavailable`, `src-app/src/terminal/view/host_attach.rs:207-218`, ou reboot), when le workspace se restaure, then Paneflow :
  1. lance le shell de login dans le cwd enregistré ;
  2. attend la première géométrie d'une vue attachée et 300 ms sans sortie, au plus 5 s ;
  3. écrit en une seule fois la séquence d'effacement de ligne du shell, la commande de reprise et `\r`.
- [ ] La séquence d'effacement vaut Ctrl-U pour bash, zsh et fish, et Escape pour PowerShell et cmd. Test par famille de shell.
- [ ] Les reprises sont espacées de 250 ms dans l'ordre des workspaces et des panes.
- [ ] Deux surfaces avec le même couple (runtime, id) : seule la première reprend. Les autres ouvrent un shell et affichent la bannière « Conversation déjà reprise dans le pane N ».
- [ ] Si une session vivante du host courant porte déjà ce couple, aucune reprise n'a lieu et la même bannière s'affiche.
- [ ] Si l'utilisateur a tapé dans le pane avant l'écriture, la commande n'est pas écrite ; une bannière propose « Reprendre la conversation ».
- [ ] Le réglage `agents.restore_conversations` (booléen, vrai par défaut) désactive la reprise automatique.
- [ ] Échec : given un cwd enregistré qui n'existe plus, when la restauration tourne, then le shell s'ouvre dans le home, aucune reprise n'a lieu, et une bannière affiche « Dossier introuvable : chemin ».

#### US-014: Détecter l'échec d'une reprise et proposer une nouvelle session
**Description:** As a développeur, I want voir immédiatement qu'une conversation n'a pas pu être reprise so that je puisse démarrer une nouvelle session sans lire la sortie d'erreur.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Dans les 15 s qui suivent la commande de reprise, une sortie qui contient un `failure_marker` du runtime, ou une fin du processus avec un statut non nul avant toute transition d'état, affiche la bannière « Reprise impossible : marqueur » avec les actions « Nouvelle session » et « Ignorer ».
- [ ] « Nouvelle session » lance la commande de lancement du runtime dans le même pane.
- [ ] `agent_session` est effacé après un échec, pour qu'un redémarrage suivant ne retente pas.
- [ ] Les marqueurs de Claude Code et de Codex sont capturés sur les CLI réelles et stockés dans `runtimes/<slug>/fixtures/resume-failure.txt`, avec un test qui vérifie qu'ils sont reconnus.
- [ ] Échec : given un marqueur qui apparaît après 15 s, dans une conversation normale, then aucune bannière ne s'affiche (test).

#### US-015: Forker une conversation dans un nouveau split
**Description:** As a développeur, I want ouvrir une branche de la conversation d'un agent dans un split so that j'explore une autre piste sans perdre la première.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012, US-014

**Acceptance Criteria:**
- [ ] L'action « Forker la conversation » apparaît dans le menu contextuel du pane et dans la palette pour un pane dont `agent_session` est connu et dont le runtime déclare `fork_argv`.
- [ ] Elle ouvre un split à droite, dans le même cwd, qui exécute `fork_argv`.
- [ ] L'action est absente, et non grisée, quand le runtime n'a pas de `fork_argv` ou que l'id est inconnu.
- [ ] Le nouveau pane reçoit son propre `agent_session` par son hook `SessionStart`, jamais l'id du parent (test).
- [ ] L'infobulle de l'action précise que les deux conversations partagent les mêmes fichiers.
- [ ] Échec : given un id que la CLI ne retrouve plus, when le fork démarre, then la bannière d'échec de US-014 s'affiche dans le nouveau pane.

#### US-016: Pré-assigner l'id de session de Claude Code au lancement
**Description:** As a développeur qui n'installe pas les hooks, I want que Paneflow connaisse l'id d'une conversation Claude dès son lancement so that elle puisse être reprise après un redémarrage.

Modèle : le wrapper cmux (`Resources/bin/cmux-claude-wrapper:2336-2344`). Unpeel a retiré l'injection au lancement dans `eb5500a` : l'option est donc désactivée par défaut.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] Quand `agents.claude_preassign_session_id` est vrai (faux par défaut), `paneflow-shim` ajoute `--session-id <uuid v4>` au lancement de `claude` si argv ne contient :
  - ni sous-commande ;
  - ni `-r`/`--resume` ;
  - ni `-c`/`--continue` ;
  - ni `--session-id` ;
  - ni `-p`/`--print` ;
  - ni `--fork-session`.
- [ ] L'id injecté est enregistré comme `agent_session` du pane avant l'arrivée du premier hook.
- [ ] `paneflow-shim` reste sous 512 KiB.
- [ ] Échec : given `claude --session-id X` ou `claude mcp list`, when le shim tourne, then argv n'est pas modifié (tests).

---

### EP-004: Unifier le catalogue, les installateurs et les boucles de rafraîchissement

Livraison R2. Le catalogue devient la seule source de connaissance par agent, un seul moteur installe les intégrations, et les trois boucles coûteuses sont remplacées.

**Definition of Done:**
- Ajouter un runtime de détection ne demande aucune modification Rust (test).
- Un seul moteur installe hooks, MCP et skill.
- Le poll GPUI de 50 ms, le poll de snapshot de 2 s et l'attente fsync sous verrou sont remplacés, mesures avant/après à l'appui.

#### US-017: Générer depuis le catalogue la connaissance par agent de l'application
**Description:** As a contributeur, I want qu'un nouveau répertoire `runtimes/<slug>/` suffise à faire apparaître l'agent dans le lanceur, les réglages de visibilité et la sidebar des sessions so that la promesse de `runtimes/README.md` devienne vraie.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Les éléments suivants sont générés depuis le catalogue ou remplacés par des champs du catalogue (`display.order`, `display.visibility_config_key`, `[sessions] reader`) :
  - les constantes `TerminalAgent` (`src-app/src/agent_launcher.rs:15-29`) ;
  - les 18 bras de `is_visible` (`:117-139`) ;
  - `session_agent` (`:175-187`) ;
  - l'énumération `SessionAgent` et son ordre (`src-app/src/agent_sessions.rs:2-38`) ;
  - `SUMMARIZER_ORDER` (`src-app/src/auto_naming.rs:178-180`).
- [ ] Un test de référence compare, avant et après, l'ordre du lanceur, les clés de visibilité et le lecteur de sessions de chaque runtime : aucune différence.
- [ ] Un test de compilation sur un catalogue de fixture prouve qu'un runtime ajouté sans modification Rust apparaît dans le lanceur et dans la table de visibilité.
- [ ] `ARCHITECTURE.md` ne cite plus un nombre fixe de CLI.
- [ ] `runtimes/README.md` décrit les champs ajoutés.
- [ ] Échec : given `[sessions] reader = "inconnu"`, when le build tourne, then il échoue en listant les lecteurs acceptés.

#### US-018: Vérifier les listes de shims du packaging contre le catalogue
**Description:** As a mainteneur, I want qu'un runtime ajouté sans son entrée dans l'installeur Windows ou la release fasse échouer la CI so that aucun shim ne manque à la livraison.

Aujourd'hui `packaging/wix/main.wxs:134-153` et `.github/workflows/release.yml:450-469` ne sont comparés qu'entre eux.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un test lit les deux listes et les compare à l'ensemble des alias de commande du catalogue qui visent chaque plateforme ; un écart fait échouer le test avec la différence.
- [ ] Le test tourne dans `cargo test --workspace --locked`, sans outil externe.
- [ ] Échec : given un runtime de fixture sans entrée WiX, when le test tourne, then il échoue en nommant l'alias manquant.

#### US-019: Fusionner les moteurs d'installation des intégrations
**Description:** As a développeur, I want qu'une seule commande installe, sans conflit, les hooks, l'entrée MCP et le skill de chaque agent so that deux installations Paneflow (release et debug) ne s'écrasent plus mutuellement.

Aujourd'hui, deux moteurs appliquent des politiques de symlink différentes :
- `integrations` : hooks Claude et Codex, plus MCP ;
- `mcp`, avec `AgentConfigWriter` : Claude, Codex, Gemini et OpenCode.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `paneflow integrations install` et `paneflow mcp install` appellent le même moteur, piloté par la section `[integration]` du catalogue (hooks, fichier et format de config MCP, dossier de skills).
- [ ] Un symlink de config est écrit à travers, par l'écrivain atomique partagé de `prd-fork-audit-fixes.md`, et jamais remplacé par un fichier (test).
- [ ] Un build debug, ou un `PANEFLOW_HOME` différent de celui qui possède une entrée existante, refuse d'écraser cette entrée sans `--force`, avec un message qui nomme les deux domiciles (test à deux domiciles).
- [ ] `refresh_integrations` (`crates/paneflow-serve/src/worker.rs:165`) utilise le domicile reçu et non `paneflow_home()` (`crates/paneflow-serve/src/integrations.rs:11`) ; test à deux domiciles.
- [ ] Échec : given un `settings.json` au JSON invalide, when l'installation tourne, then rien n'est écrit et l'erreur nomme le fichier.

#### US-020: Installer et versionner le skill conductor
**Description:** As a développeur, I want que Paneflow installe et mette à jour le skill `paneflow-conductor` so that l'agent conducteur utilise des commandes qui existent dans ma version.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-019

**Acceptance Criteria:**
- [ ] Le moteur installe `skills/paneflow-conductor/SKILL.md` dans le dossier de skills déclaré par le runtime : pour Claude Code, `skills/paneflow-conductor/` sous le dossier de config, en respectant `CLAUDE_CONFIG_DIR`. Un runtime sans dossier vérifié n'en reçoit pas.
- [ ] La copie installée porte la version de Paneflow dans son frontmatter.
- [ ] Une mise à jour remplace la copie seulement si son empreinte égale celle que Paneflow a écrite ; sinon, la copie modifiée est gardée et un message le signale.
- [ ] `paneflow integrations remove` retire le skill seulement s'il n'a pas été modifié.
- [ ] `docs/user` indique que Paneflow installe le skill.
- [ ] Échec : given un dossier de skills en lecture seule, when l'installation tourne, then seul le skill échoue, et les hooks et l'entrée MCP sont installés.

#### US-021: Remplacer le poll IPC de 50 ms du thread GPUI par un réveil poussé
**Description:** As a développeur sur portable, I want que l'application ne se réveille plus 20 fois par seconde au repos so that la batterie et la latence des commandes s'améliorent.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le `smol::Timer` de 50 ms (`src-app/src/app/bootstrap.rs:535`) est remplacé par une tâche GPUI qui attend un canal asynchrone alimenté par les threads du serveur IPC.
- [ ] Au repos, le thread GPUI ne se réveille plus pour l'IPC : 0 réveil par seconde contre 20 aujourd'hui. La mesure est jointe à la PR.
- [ ] Le délai p95 entre la réception d'une requête et le début de son traitement est inférieur ou égal à 5 ms sur 1 000 requêtes (test ou bench joint).
- [ ] Échec : given un canal fermé (thread IPC tombé), when la tâche le détecte, then une erreur est journalisée une fois, sans boucle active, et l'application continue (test).

#### US-022: Remplacer le poll de snapshot de 2 s du worker par les événements du host
**Description:** As a développeur, I want qu'un changement d'état d'agent atteigne la sidebar en moins de 100 ms so that l'attention s'affiche dès qu'elle existe.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le worker s'abonne aux changements de manifest du host (hook ingéré, runtime lié, premier plan observé) sous Linux, macOS et Windows.
- [ ] `HEALTH_REFRESH` (`crates/paneflow-serve/src/worker.rs:19`) passe à 30 s et ne sert plus qu'au contrôle de santé.
- [ ] Le délai p95 entre un changement de manifest et la mise à jour de la projection est inférieur à 100 ms (test horodaté).
- [ ] Après un redémarrage du host, le worker se réabonne en moins de 1 s et prend un snapshot complet (test).
- [ ] Échec : given un abonnement refusé par le host, when le worker démarre, then il journalise la raison et revient au poll de 2 s jusqu'au prochain essai, 30 s plus tard.

#### US-023: Sortir l'attente de durabilité du verrou d'ingestion des hooks
**Description:** As a développeur sur un disque lent, I want qu'un hook reçoive sa réponse avant le délai de 350 ms de `paneflow-ai-hook` so that aucun événement ne soit perdu et que les hooks d'une session ne s'attendent pas les uns les autres.

`ingest_agent_event` garde le mutex du ledger (`crates/paneflow-host/src/host.rs:843-900`) pendant `persist(WriteClass::Critical)`, qui peut attendre jusqu'à `CRITICAL_DEADLINE` = 5 s (`crates/paneflow-host/src/persistence.rs:17`).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] L'attente de durabilité se fait hors du mutex du ledger ; l'ordre des reçus et la déduplication sont conservés (test).
- [ ] Bench avec injection de 100 ms de latence fsync et 200 hooks en rafale sur une session :
  - réponse p95 < 350 ms ;
  - zéro événement perdu ;
  - baseline mesurée avant le correctif et jointe à la PR.
- [ ] Un événement acquitté `durable: true` survit à un `kill -9` du host (`crates/paneflow-host/tests/persistent_baseline.rs`).
- [ ] Échec : given un thread de persistance arrêté, when un hook arrive, then la réponse `durable: false` et l'erreur arrivent en moins de 350 ms (test).

#### US-024: Supprimer le code mort et les doublons de l'intégration agents
**Description:** As a mainteneur, I want une seule liste d'événements par agent et un seul détecteur de processus d'agent so that une correction ne soit pas faite deux fois.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-017, US-019

**Acceptance Criteria:**
- [ ] Une seule liste d'événements Claude subsiste. Aujourd'hui, `CLAUDE_EVENTS` (`crates/paneflow-mcp-install/src/integrations.rs:13`) double `claude_hooks::CLAUDE_HOOK_EVENTS` (`crates/paneflow-agent-config/src/lib.rs`). Idem pour Codex.
- [ ] La détection d'agents de `scan_panes` (`src-app/src/workspace/ports.rs:304,524,746`, appelée par `src-app/src/app/event_handlers/pane_scan.rs:283`) est retirée au profit de l'observation du host ; le scan de ports des serveurs de dev reste.
- [ ] Les énumérations devenues inutiles après US-017 sont supprimées.
- [ ] Échec : given un pane hébergé qui lance Codex, when la sidebar se met à jour, then la ligne d'agent vient de l'observation du host, et un test prouve qu'aucune seconde détection ne crée de doublon.

---

### EP-005: Détecter l'état des agents par des règles d'écran déclaratives

Livraison R3. Les deux motifs d'écran figés deviennent un moteur de règles vérifié par un corpus, surchargeable localement, mis à jour par un catalogue signé, et fx y entre sans code dédié.

**Definition of Done:**
- Le corpus couvre au moins sept runtimes dans les trois états.
- Chaque runtime du corpus atteint au moins 95 % de bonne classification.
- `paneflow agent explain` justifie chaque décision.
- Un catalogue signé plus récent est appliqué sans mise à jour de l'application.

#### US-025: Constituer le corpus d'écrans de référence et la commande de capture
**Description:** As a contributeur, I want capturer l'écran d'un agent dans un état connu so that chaque règle soit vérifiée contre de vrais écrans et non contre deux fixtures.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `paneflow agent capture <surface> --state working|idle|blocked` écrit l'écran courant, lu dans le viewport du host, en texte brut sans espaces finaux. L'en-tête du fichier porte les colonnes, les lignes, la version de la CLI, la version de Paneflow et la date.
- [ ] La destination par défaut est `~/.paneflow/cache/captures/` ; `--out` permet d'écrire dans le dépôt.
- [ ] La capture remplace le chemin du home par `~` et masque les motifs de secrets connus (`sk-`, `ghp_`, `xox`, `AKIA`, JWT). Test avec un écran piégé.
- [ ] Le corpus versionné `runtimes/<slug>/fixtures/screens/` contient au moins une capture `working`, `idle` et `blocked` pour claude-code, codex, opencode, gemini, pi, hermes et fx.
- [ ] Un test classe chaque capture avec les règles actuelles et écrit la précision par runtime dans `bench/screen-corpus-baseline.json`.
- [ ] Échec : given un pane sans runtime reconnu, when la capture est demandée, then elle est refusée avec « aucun agent reconnu dans la surface N ».

#### US-026: Définir les règles d'écran v2 et leur évaluateur dans le host
**Description:** As a contributeur, I want décrire les états d'un agent par des règles avec régions, priorités et conditions so that un nouvel agent se détecte sans code Rust.

Aujourd'hui `RuntimeScreen` n'a que `working` et `idle_prompt` (`crates/paneflow-agent-config/src/runtime_catalog.rs:76-79`), figés à la compilation (`:109`).

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [ ] `runtimes/<slug>/screen.toml` déclare `engine = 2` et une liste `[[rules]]`. Chaque règle porte :
  - `id` ;
  - `state` : `working`, `idle` ou `blocked` ;
  - `priority` ;
  - `region` : `all`, `last:N`, `first:N` ou `title` ;
  - les listes de regex `all`, `any` et `not` ;
  - le drapeau `visible_blocker`.
- [ ] La règle de plus haute priorité qui correspond l'emporte ; à égalité, l'ordre du fichier départage.
- [ ] Les règles intégrées sont validées au build, puis chargées au démarrage du host par le même analyseur que celui des sources distantes et locales.
- [ ] Les motifs `working` et `idle_prompt` existants sont convertis. La classification du corpus est identique avant et après la conversion (test).
- [ ] Les regex sont compilées une fois par chargement, avec une limite de taille de 1 MiB par regex.
- [ ] L'évaluation de 20 règles sur un viewport de 200×60 prend au plus 1 ms p95 (bench joint).
- [ ] Échec : given une source chargée à l'exécution avec une regex invalide ou un état inconnu, when elle est chargée, then elle est rejetée entière, les règles précédentes restent actives et l'erreur est journalisée (test).

#### US-027: Exposer titre et progression OSC aux règles et laisser un bloqueur visible l'emporter
**Description:** As a développeur, I want qu'un dialogue de permission visible passe l'agent en « bloqué » même si ses hooks le disent actif, et qu'un menu dans un pane sans agent ne crée pas de fausse attente, so that l'Attention Queue ne rate ni n'invente d'attente.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-026

**Acceptance Criteria:**
- [ ] Le host garde en mémoire, pour chaque session, le dernier titre OSC 0/2 et l'état de progression OSC 9;4. La région `title` et une condition `progress` des règles y accèdent.
- [ ] Une règle `visible_blocker` qui correspond met l'état réduit à `blocked`, y compris quand le dernier hook dit `working`. Le blocage cesse après deux évaluations consécutives sans correspondance.
- [ ] Les motifs de `crates/paneflow-host/src/menu_prompt.rs` deviennent des règles `visible_blocker` du runtime concerné.
- [ ] Les règles ne sont évaluées que si un runtime du catalogue est au premier plan.
- [ ] Échec : given un pane shell qui affiche le menu de `npm init`, when l'écran est évalué, then aucune ligne d'agent `blocked` n'apparaît (test).

#### US-028: Surcharger localement les règles, les recharger à chaud et expliquer une décision
**Description:** As a développeur dont l'agent a changé d'interface, I want corriger une règle chez moi et voir pourquoi un écran est classé so that je n'attende pas une release.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-026

**Acceptance Criteria:**
- [ ] `runtimes/<slug>/screen.toml` sous le domicile Paneflow, résolu par `crates/paneflow-home`, surcharge les règles par `id` :
  - même `id` : la règle remplace l'intégrée ;
  - `disabled = true` : la règle est retirée ;
  - nouvel `id` : la règle est ajoutée.
- [ ] Un watcher non récursif, au plus à deux niveaux, tourne dans le host hors de tout thread UI. Une modification valide est active en moins de 1 s.
- [ ] `paneflow agent explain <surface>` affiche :
  - le runtime et la source du signal (hook, écran, BEL) ;
  - chaque règle évaluée, avec son résultat et son origine (intégrée, distante vN, locale) ;
  - la règle gagnante et l'état final ;
  - le dernier hook et son âge.

  `--json` renvoie la même chose en JSON.
- [ ] Échec : given un fichier local invalide, when il est sauvegardé, then les règles précédentes restent actives, et `explain` et le journal affichent l'erreur avec la ligne fautive.

#### US-029: Distribuer un catalogue de règles signé et versionné
**Description:** As a développeur, I want que les règles d'écran se mettent à jour quand une CLI change son interface, sans attendre une release de Paneflow so that la détection reste juste entre deux versions.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-026, US-028

**Acceptance Criteria:**
- [ ] Un workflow publie les `runtimes/*/screen.toml` de `main` en asset d'une release GitHub `screen-catalog-v<N>` :
  - `screen-catalog.json` ;
  - sa signature minisign, dont le commentaire de confiance porte `engine=<E> version=<N>` ;
  - une clé de signature distincte de la clé de release.
- [ ] La vérification minisign est extraite de `src-app/src/update/signature.rs` dans une crate partagée par l'updater et le host. La clé de release et la sémantique fail-closed de l'updater ne changent pas (tests existants inchangés).
- [ ] Le host vérifie le catalogue au démarrage (après 60 s), puis toutes les 24 h, hors de tout thread UI. Téléchargement limité à 1 MiB, délai de 10 s.
- [ ] Un catalogue n'est accepté que si sa signature est valide, si `engine` égale la version du moteur et si `version` est strictement supérieure à la version en cache. Le cache vit sous `cache/screen-catalog/` du domicile.
- [ ] La priorité des sources est : locale, puis distante, puis intégrée.
- [ ] `agents.remote_screen_catalog = false` supprime tout appel réseau (test).
- [ ] Échec : given une signature invalide, un moteur différent, une version inférieure ou égale, ou un fichier trop gros, when le catalogue est reçu, then il est rejeté, les règles actives restent, et `explain` affiche « catalogue distant rejeté : raison » (un test par cas).

#### US-030: Intégrer fx comme runtime déclaratif
**Description:** As a développeur qui utilise fx, I want que Paneflow reconnaisse fx, signale ses attentes, l'intègre au MCP et reprenne sa dernière session so that fx ait le même traitement que les autres agents sans code dédié.

fx n'a pas de hook, émet un BEL pour demander l'attention, et son titre OSC 2 a la forme `fx · <titre> · <modèle>`. L'alias `fx` est aujourd'hui interdit, à cause de la collision avec le visualiseur JSON fx (`crates/paneflow-agent-config/src/runtime_catalog.rs:189`).

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-013, US-019, US-027

**Acceptance Criteria:**
- [ ] `runtimes/fx/runtime.toml` :
  - déclare les plateformes `linux` et `macos` et l'alias `fx` ;
  - exige la confirmation d'identité `detection.title_prefix = "fx · "` ;
  - déclare les règles d'écran tirées du corpus, le BEL comme signal d'attention, et les gabarits `fx --resume {session_id}` et `fx --continue`, avec l'id au format `^\d+-\d+-[0-9a-f]{16}$`.
- [ ] Le test de `runtime_catalog.rs:189` devient : un alias `fx` sans confirmation par titre est refusé.
- [ ] Le moteur d'installation écrit l'entrée `paneflow` dans `~/.fx/mcp.json` sans bloc `environment`, puisque fx remplace l'environnement de l'enfant ; test sur le JSON produit.
- [ ] Après une perte du host, un pane fx seul dans son cwd reprend par `continue_argv`. Si plusieurs panes fx partagent ce cwd, aucun ne reprend, et chacun affiche la bannière de US-013.
- [ ] Sous Windows, fx n'apparaît ni dans le lanceur ni dans les réglages (test du filtre de plateforme).
- [ ] Échec : given `fx data.json` (visualiseur JSON) dans un pane, when le titre ne commence pas par `fx · `, then aucune ligne d'agent n'est créée (test).

---

### EP-006: Orchestrer les agents entre eux sous contrôle humain

Livraison R4. Un agent peut écrire à un autre seulement avec l'accord d'un humain pour cette paire. Les équipes d'agents Claude ouvrent leurs coéquipiers dans des panes Paneflow.

**Definition of Done:**
- L'outil MCP d'écriture est opérationnel.
- Toute écriture passe par une approbation humaine liée aux deux occurrences d'agents, et porte une provenance visible.
- Le budget de schéma est testé.
- Le shim tmux est livré, ou annulé avec la preuve du spike.

#### US-031: Borner le schéma des outils MCP et annoncer leur nature
**Description:** As a développeur, I want que le bridge MCP reste léger dans le contexte des agents et annonce quels outils modifient un pane so that l'ajout d'outils ne gonfle pas chaque requête et que le client affiche la bonne confirmation.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un test vérifie que la réponse `tools/list` sérialisée fait au plus 16 KiB, et au plus 4 KiB par outil.
- [ ] `list_panes`, `read_pane` et `search_pane` portent `readOnlyHint: true`.
- [ ] Les outils d'écriture portent `readOnlyHint: false`, `destructiveHint: true`, `idempotentHint: false` et `openWorldHint: false`.
- [ ] La documentation précise que ces annotations sont indicatives et que le host applique les contrôles.
- [ ] `paneflow-mcp` reste sous 512 KiB.
- [ ] Échec : given un outil de fixture dont la description dépasse 4 KiB, when le test tourne, then il échoue en affichant les tailles.

#### US-032: Faire approuver par un humain chaque paire d'agents qui s'écrivent
**Description:** As a développeur, I want décider moi-même qu'un agent peut écrire dans le pane d'un autre, pour la durée de ces deux sessions d'agents, so that une injection de prompt ne se propage pas d'agent en agent.

Modèle : les approbations par paire d'Unpeel, liées à `agent_ref`.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006, US-007

**Acceptance Criteria:**
- [ ] Le host garde en mémoire les approbations, indexées par (occurrence source, occurrence cible). Une occurrence est le quadruplet : id de session du host, génération, PID, heure de démarrage du processus.
- [ ] L'identité de la source vient du contexte de pane capturé au démarrage du bridge MCP, jamais d'un argument de l'outil.
- [ ] Une première écriture pour une paire non approuvée crée une demande. Le desktop l'affiche en bannière dans le pane cible et en ligne de l'Attention Queue : « source veut écrire dans ce pane », avec les actions « Autoriser pour cette session d'agent » et « Refuser ». L'outil répond aussitôt `approval_pending`, avec la consigne de réessayer après la décision.
- [ ] Une approbation expire quand l'une des deux occurrences se termine (changement de génération, sortie) ou quand le host redémarre.
- [ ] Une demande sans réponse expire en refus après 120 s.
- [ ] Une écriture d'une occurrence vers elle-même est refusée sans demande.
- [ ] Aucune méthode JSON-RPC accessible à un client de contrôle n'accorde d'approbation (test) : seules les actions de la bannière et de l'Attention Queue le font, au clavier ou à la souris.
- [ ] Chaque demande et chaque décision sont journalisées au niveau info (source, cible, décision), sans le contenu.
- [ ] Aucun nouvel élément d'interface permanent n'est ajouté.
- [ ] Échec : given aucune fenêtre Paneflow ouverte, when une écriture est demandée, then l'outil répond « aucune fenêtre Paneflow pour approuver », et la demande expire après 120 s.

#### US-033: Ajouter l'outil MCP d'écriture avec provenance
**Description:** As a agent conducteur, I want envoyer un message à un autre agent par MCP so that je coordonne une tâche sans passer par le shell, sous le contrôle de l'humain.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008, US-009, US-031, US-032

**Acceptance Criteria:**
- [ ] L'outil `write_pane(target, text, submit)`, où `submit` vaut faux par défaut, vise une surface de la portée du bridge (`crates/paneflow-mcp/src/scope.rs`).
- [ ] Le host préfixe le texte d'une ligne de provenance « [Paneflow : message de nom source, surface N] » qu'aucun argument ne peut supprimer.
- [ ] Le texte est limité à 16 KiB. Les caractères de contrôle C0 et C1, sauf `\n` et `\t`, sont retirés avant le collage entre crochets, y compris `ESC[201~`.
- [ ] Avec `submit`, la vérification de US-008 et la confirmation de US-009 s'appliquent, et le résultat porte `started` et `state`.
- [ ] Le débit est limité à une écriture par seconde et par paire, avec une rafale de 3.
- [ ] Le binaire MCP ne fait que relayer : approbation, portée et nettoyage sont appliqués par le host.
- [ ] Échec : given une cible `blocked`, when l'outil est appelé, then rien n'est écrit et l'erreur dit que la cible attend une décision humaine.
- [ ] Échec : given un texte qui contient `\x1b[201~`, then la séquence est retirée (test).

#### US-034: Valider l'hypothèse : inventaire des commandes tmux des équipes d'agents Claude
**Description:** As a mainteneur, I want connaître exactement les commandes tmux qu'exécutent les équipes d'agents Claude so that le shim tmux n'émule que ce qui est utilisé, ou soit abandonné avec la preuve.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un faux `tmux` journalisant, placé en tête du PATH, enregistre argv, stdin, les variables lues (`TMUX`, `TMUX_PANE`) et les sorties attendues. Il est exécuté avec `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` et le mode d'équipe tmux, sous Linux, macOS et Windows.
- [ ] L'inventaire est versionné en fixture `src-app/tests/fixtures/claude-teams-tmux-<os>.log`, et le changelog du PRD passe en 1.1 avec la liste des verbes.
- [ ] Il est comparé aux verbes de la compatibilité tmux de cmux (`/home/arthur/dev/cmux`, `cmux claude-teams`).
- [ ] Décision go/no-go :
  - GO si l'inventaire compte au plus 25 formes de sous-commandes, sans mode de contrôle `-C` ;
  - sinon, US-035 est annulée avec la preuve.
- [ ] La valeur exacte de `teammateMode` et du flag `--teammate-mode` est relevée sur Claude Code 2.1.285 ; `claude --help` ne la montre pas.
- [ ] On vérifie sur Codex 0.159 si ses sous-agents ouvrent un terminal, et le résultat est consigné.
- [ ] Échec : given Claude qui refuse le faux binaire (contrôle de `tmux -V`), then la chaîne de version attendue est relevée et le faux binaire la renvoie.

#### US-035: Ouvrir les coéquipiers Claude dans des panes Paneflow via un shim tmux
**Description:** As a développeur qui utilise les équipes d'agents Claude, I want que chaque coéquipier s'ouvre dans un pane Paneflow natif so that je voie et pilote l'équipe comme mes autres agents, sous Linux, macOS et, si le spike le permet, Windows.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-034, US-006, US-007

**Acceptance Criteria:**
- [ ] Le binaire `paneflow` sert de shim tmux quand il est invoqué sous le nom `tmux` : aucun nouveau helper embarqué, les plafonds sont inchangés.
- [ ] Le préréglage « Claude Code (équipe) » du catalogue prépare, pour ce seul pane :
  - un PATH préfixé par `bin/tmux-compat/` du domicile, qui contient `tmux` (lien symbolique sous Unix, lien physique ou copie sous Windows) ;
  - des valeurs `TMUX` et `TMUX_PANE` ;
  - la variable d'équipe et le mode relevés par US-034.
- [ ] Les verbes de l'inventaire sont traduits :
  - `split-window` ouvre un split dans l'onglet du meneur avec la commande demandée ;
  - `send-keys` écrit par `session.input`, sous les gates de US-006 ;
  - les formats de `list-panes` et `display-message` sont ceux relevés.
- [ ] Le shim n'adresse que les panes créés pour la même équipe (test).
- [ ] Un verbe inconnu sort avec le code 1 et le message « paneflow tmux-compat : verbe non pris en charge : X », journalisé.
- [ ] Les autres panes gardent leur PATH. Un vrai `tmux` lancé dans un pane ordinaire n'est pas affecté (test).
- [ ] Si US-034 montre que le mode tmux n'existe pas sous Windows, le préréglage y est masqué par le filtre de plateforme, et `docs/user` le dit.
- [ ] Échec : given le pane du meneur fermé, when un coéquipier tourne encore, then son pane reste ouvert et se comporte comme un pane ordinaire.

---

## Functional Requirements

- FR-01: Le build doit refuser toute autorité de runtime que son intégration ne fournit pas (US-001).
- FR-02: Le reducer ne doit pas effacer un tour d'agent de la génération courante lors de la première observation du runtime (US-002).
- FR-03: Un BEL d'un runtime sans hook doit produire une attention limitée à une transition toutes les 2 s (US-004).
- FR-04: `agent.status` ne doit jamais renvoyer `state: "unknown"` ni une génération constante (US-005).
- FR-05: Le host ne doit accepter aucune écriture PTY d'un client de contrôle sans la permission correspondante, ni hors de son workspace sans `scope: "all"` et `orchestration` (US-006, US-007).
- FR-06: `paneflow send` ne doit pas écrire dans un agent `blocked` ni absent du premier plan sans `--force` (US-008).
- FR-07: `paneflow send --submit` ne doit confirmer un démarrage que sur une transition d'état réduit (US-009).
- FR-08: Chaque surface d'agent doit mémoriser son id de session fournisseur ; après perte du host, Paneflow doit reprendre la conversation une seule fois par id, sans écraser une saisie (US-012, US-013).
- FR-09: Une reprise qui échoue doit afficher une bannière avec « Nouvelle session » dans les 15 s (US-014).
- FR-10: Un nouveau runtime de détection ne doit demander aucune modification Rust (US-017).
- FR-11: Un seul moteur doit installer hooks, MCP et skill, sans écraser l'entrée d'un autre domicile sans `--force` (US-019, US-020).
- FR-12: Le host doit évaluer des règles d'écran versionnées, surchargeables localement et rechargées à chaud, et expliquer chaque décision (US-026, US-027, US-028).
- FR-13: Le host ne doit appliquer un catalogue distant que s'il est signé, compatible et strictement plus récent (US-029).
- FR-14: Le système ne doit pas écrire dans le pane d'un agent au nom d'un autre agent sans approbation humaine pour cette paire d'occurrences (US-032, US-033).
- FR-15: Le système ne doit jamais soumettre de lui-même un prompt qu'il a composé ; la reprise tape une commande de shell, pas un prompt.

## Non-Functional Requirements

- **Performance:**
  - Délai p95 du traitement IPC du desktop ≤ 5 ms ; zéro réveil par seconde du thread GPUI pour l'IPC au repos.
  - Délai p95 host vers projection du worker < 100 ms.
  - Réponse p95 à un hook < 350 ms avec 100 ms de latence fsync.
  - Évaluation de 20 règles sur 200×60 ≤ 1 ms p95.
  - Reprises espacées de 250 ms.
- **Security:**
  - Écritures PTY des clients de contrôle soumises aux gates et à la portée du workspace (0 exception hors `--force` et `scope: "all"`).
  - Approbations liées à l'occurrence, avec une expiration à 120 s pour une demande sans réponse.
  - Aucune méthode IPC ne peut approuver.
  - Catalogue distant : signature minisign avec une clé distincte de la clé de release, anti-downgrade, 1 MiB maximum, délai de 10 s.
  - Regex limitées à 1 MiB.
  - Texte relayé nettoyé des contrôles C0 et C1 et limité à 16 KiB.
  - Captures masquées (home, 5 motifs de secrets).
  - Référentiel : OWASP LLM01 et LLM06.
- **Size:** `paneflow-shim` ≤ 512 KiB, `paneflow-ai-hook` ≤ 384 000 B, `paneflow-mcp` ≤ 512 KiB, total ≤ 1 835 008 B ; zéro nouveau helper embarqué.
- **Accessibility:**
  - 100 % des actions des bannières (reprise, échec, approbation) atteignables au clavier depuis le pane, sans souris.
  - Aucun texte de bannière ne dépend de la couleur seule.
- **Reliability:**
  - Zéro reprise en double d'un même id.
  - Zéro événement de hook perdu sur le bench de US-023.
  - Réabonnement du worker en moins de 1 s après un redémarrage du host.
  - Détection d'échec de reprise en 15 s au plus.
- **Portability:**
  - Chaque story passe sur Linux, macOS et Windows, sauf fx (Linux et macOS) et le shim tmux (selon US-034).
  - Le code `cfg(windows)` est vérifié par le job Windows et, pour l'UI, sur matériel réel.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Aucun id de session connu | Agent sans hook, pré-assignation désactivée | Le pane rouvre un shell, sans reprise | Aucun |
| 2 | Attente de la géométrie | Restauration avant qu'une vue soit attachée | La commande attend la géométrie et 300 ms de calme, au plus 5 s | Aucun |
| 3 | Reprise refusée par la CLI | Conversation supprimée côté fournisseur | Bannière dans les 15 s, `agent_session` effacé | « Reprise impossible : marqueur. Nouvelle session ? » |
| 4 | Catalogue distant injoignable | Hors ligne, délai de 10 s dépassé | Règles en cache ou intégrées, nouvel essai 24 h plus tard | Aucun, visible dans `explain` |
| 5 | Id de session hostile | `--dangerously-skip-permissions` comme id | Aucune commande construite, avertissement journalisé | Aucun |
| 6 | Permission retirée en cours de session | `PANEFLOW_IPC_SCRIPTING` retiré du host | Les écritures suivantes sont refusées dès l'appel suivant | « session.input disabled; set PANEFLOW_IPC_SCRIPTING=1 » |
| 7 | Même conversation dans deux panes | Layout dupliqué, deux panes avec le même id | Seul le premier reprend | « Conversation déjà reprise dans le pane N » |
| 8 | Saisie pendant la restauration | L'utilisateur tape avant l'écriture de la commande | Commande non écrite | « Reprendre la conversation ? » |
| 9 | Rafale de BEL | `cat` d'un binaire dans un pane d'agent | Une transition au plus toutes les 2 s, pane jamais déclaré terminé | Aucun |
| 10 | Approbation jamais donnée | Aucun humain devant l'écran | Refus après 120 s | « Demande d'écriture expirée » |
| 11 | Agent qui s'écrit à lui-même | Le conducteur vise son propre pane | Refus sans demande | « Un agent ne peut pas écrire dans son propre pane » |
| 12 | Collision de nom fx | Visualiseur JSON `fx data.json` | Aucune ligne d'agent | Aucun |
| 13 | Catalogue rejoué | Ancien catalogue signé servi à nouveau | Rejeté (version ≤ cache) | « catalogue distant rejeté : version » dans `explain` |
| 14 | Deux domiciles Paneflow | Build debug et release sur la même machine | L'installation refuse d'écraser l'autre domicile sans `--force` | « Entrée gérée par <domicile A> ; --force pour la remplacer » |
| 15 | Fork partagé | Deux branches d'une conversation éditent les mêmes fichiers | Autorisé ; l'infobulle prévient | « Les deux conversations partagent les mêmes fichiers » |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Une CLI change ses flags ou son écran et casse la reprise ou la détection | High | Med | Gabarits et règles dans le catalogue, catalogue distant signé (US-029), marqueurs d'échec (US-014), corpus rejoué en CI (US-025) |
| 2 | Le mode tmux des équipes Claude est non documenté et change sans préavis | High | Med | Spike go/no-go (US-034), story placée en dernier (R4), verbe inconnu refusé proprement |
| 3 | La reprise automatique exécute une commande au démarrage que l'utilisateur ne veut pas | Med | Med | Commande tirée du catalogue et bornée aux alias, id validé (CWE-88), réglage `agents.restore_conversations`, protection de la saisie |
| 4 | La clé de signature du catalogue est compromise | Low | High | Clé distincte de la clé de release, anti-downgrade, règles limitées à la classification (aucune exécution), regex limitées |
| 5 | Le corpus publié contient un secret | Med | High | Masquage automatique, captures relues en review de PR, destination par défaut hors du dépôt |
| 6 | Collision avec des stories en cours d'autres PRD (fork-audit US-032/038, agents-browser EP-006) | Med | Med | Dépendances nommées dans les descriptions, rebase sur ces stories, fonction de portée partagée (US-007) |
| 7 | Le gate du host casse un client légitime qui n'attache pas de moteur | Med | High | Suite `conformance.rs` inchangée, message d'erreur qui nomme la variable, entrée de changelog utilisateur |
| 8 | L'approbation par paire lasse l'utilisateur, qui cesse d'orchestrer | Med | Low | Approbation valable pour toute l'occurrence, sans nouvelle demande pour la même paire |

## Non-Goals

- **Installateurs de hooks pour les agents autres que Claude Code et Codex** (Gemini, OpenCode, Cursor, Grok, etc.). La v1 s'appuie sur l'écran et le BEL. À réévaluer pour un runtime dont la précision sur le corpus reste sous 90 % après R3.
- **Protocole de statut de herdr** (`HERDR_SOCKET_PATH`, que fx utilise). Ce serait maintenir le protocole d'un concurrent ; fx passe par l'écran et le BEL.
- **Protocole public de statut ou SDK** pour les éditeurs d'agents.
- **Hibernation de processus, points de contrôle et réécriture de transcripts.**
- **Réintroduction du coût, de l'usage ou de l'attribution par agent**, retirés volontairement dans `b886208f`.
- **Renommage ou fusion des outils MCP en lecture existants.** Les renommer casserait les prompts et le skill installés.
- **Elicitation MCP `input_required`** de la révision 2026-07-28, en attendant que les clients la prennent en charge.
- **Panes dédiés aux sous-agents de Codex**, sauf si US-034 montre qu'ils ouvrent un terminal.
- **Changement de la valeur par défaut de `notify_when_agent_waiting`** (`Never`) : voir Open Questions.
- **Soumission automatique de prompts par Paneflow, chrome agrégé permanent, flux « Review with agent ».**

## Files NOT to Modify

- `src-app/Cargo.toml` (les quatre `rev` GPUI et `features = ["font-kit"]`) : pin de Zed amont.
- `native/libghostty/**`, dont `manifest.toml` et les bindings générés : moteur unique, archives épinglées.
- `src-app/src/terminal/pty_session/session_backend.rs` et `src-app/src/terminal/types.rs` : frontière `TerminalSessionBackend` et test de garde.
- `src-app/build.rs` : plafonds de taille des helpers, à ne relever qu'avec une mesure.
- `src-app/src/update/signature.rs` : clé de release embarquée et fail-closed. US-029 en extrait la vérification sans changer ni la clé ni la sémantique.
- `crates/paneflow-host/src/persistence.rs` : les classes de durabilité (`WriteClass`) et `CRITICAL_DEADLINE` ne changent pas ; US-023 ne change que la portée du verrou dans `host.rs`.
- `.github/workflows/release.yml` : seule la liste des shims est concernée, et seulement par le test de US-018.
- Fichiers touchés par des stories en revue dans `tasks/prd-fork-audit-fixes.md` : se rebaser dessus au lieu de les réécrire.
  - US-032 : attribution sous le daemon Codex ;
  - US-034 : boucle des shims ;
  - US-038 : `hook_state.rs`, `host_agents.rs`, `agent_frames.rs`.

## Technical Considerations

- **Architecture :** où observer le BEL, le titre et la progression OSC ?
  - Recommandé : dans le host, qui possède le moteur VT de chaque session, pour que le reducer du worker les reçoive sans desktop attaché.
  - À confirmer : que libghostty-vt côté host expose ces callbacks.
- **État réduit dans `agent.status` :** le host lit-il la projection du worker, ou la CLI interroge-t-elle directement le worker ?
  - Recommandé : la CLI lit la projection du worker, la source que la sidebar utilise déjà.
- **Modèle de données :** `agent_session` doit-il aller dans `SurfaceDefinition` ou dans le manifest du host ?
  - Recommandé : `SurfaceDefinition`, qui survit à un reboot, contrairement au manifest ; le manifest garde l'id vivant pour la garde du propriétaire.
- **Règles d'écran :** fichier `screen.toml` séparé ou section de `runtime.toml` ?
  - Recommandé : un fichier séparé, pour que le catalogue distant ne publie que les règles, jamais les commandes de lancement ou de reprise.
- **Événements pour le worker :** le mécanisme existant d'événements poussés couvre-t-il Windows ?
  - Une note de juin 2026 signalait `events.subscribe` absent sous Windows.
  - À vérifier avant US-022, qui ne doit pas livrer de stub.
- **Hébergement du catalogue :** GitHub Releases du dépôt OSS avec des tags `screen-catalog-v<N>`, ou `paneflow.dev` ?
  - Recommandé : GitHub Releases, qui réutilise l'infrastructure et `ureq` de l'updater.
- **Dépendances :** aucune nouvelle crate prévue.
  - Déjà dans `Cargo.lock` : `regex` 1.13.1, `minisign-verify` 0.2.5, `minisign` 0.9.1 (tests), `ureq` 3.4.0, `notify-rust` 4.18.0.
  - L'UUID v4 de US-016 doit tenir dans le budget du shim : faut-il utiliser l'aléa de l'OS ou une crate légère ?
- **Shim tmux sous Windows :** lien physique ou copie du binaire `paneflow` ? Une copie coûte la taille du binaire par domicile.
  - Recommandé : un lien physique sur le même volume, et une copie en repli.
- **Migration :** le nouveau `session.json` reste-t-il lisible par la version précédente en cas de retour arrière ?
  - `SurfaceDefinition` ne déclare pas `deny_unknown_fields` : le champ inconnu devrait donc être ignoré.
  - À confirmer par un test de fixture.
  - Pas de migration de données : un `session.json` ancien se charge sans le champ.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Runtimes dont l'autorité correspond à l'intégration | 8/18 | 18/18 | Month-1 | Test de catalogue de US-001 |
| Panes Claude Code et Codex repris après reboot, id connu | 0 % | ≥ 90 % | Month-1 après R2 | Journal de restauration (repris / échec / ignoré) sur 20 reboots manuels |
| Délai p95 host vers projection du worker | jusqu'à 2 000 ms | < 100 ms | Month-1 après R2 | Test horodaté de US-022 |
| Réveils par seconde du thread GPUI pour l'IPC au repos | 20 | 0 | Month-1 après R2 | Mesure de US-021 |
| Réponse p95 à un hook avec 100 ms de fsync | à mesurer (US-023) | < 350 ms | Month-1 après R2 | Bench de US-023 |
| Précision de classification du corpus par runtime | à mesurer (US-025) | ≥ 95 % | Month-6 | `bench/screen-corpus-baseline.json` |
| Écritures PTY acceptées sans gate ou hors workspace | illimitées | 0 | Month-1 | Tests de US-006 et US-007 |
| « Démarré » rapporté à tort par `send --submit` | toute soumission hors host | 0 dans la suite de tests | Month-1 | Tests de US-009 |

## Open Questions

- **Notifications de bureau :** faut-il passer `agent_panel.notify_when_agent_waiting` de `Never` à `WhenUnfocused` ? La raison du choix actuel n'est pas documentée. Arthur tranche avant R1 ; cela change la portée de US-004.
- **Pré-assignation Claude :** pourquoi Unpeel a-t-il retiré l'injection au lancement dans `eb5500a` ? Réponse à lire dans l'historique d'Unpeel avant US-016. Si la raison touche Claude Code, US-016 est annulée.
- **Codex et les mises à jour :** Codex 0.159 accepte-t-il `-c check_for_update_on_startup=false`, que cmux ajoute à la reprise ? À vérifier au moment de US-011 avant d'ajouter la clé au gabarit.
- **Forks d'OpenCode, de Pi et de Gemini :** quels sont leurs verbes de fork (`--session X --fork`, `--fork X`) ? Ils ne sont pas vérifiés et restent hors de `fork_argv` jusqu'à vérification sur les CLI installées.
- **Mode d'équipe Claude :** quelle est la valeur exacte du mode tmux des équipes d'agents Claude, et existe-t-il sous Windows ? US-034 répond, et conditionne US-035.
- **Modèle d'accès agent par workspace :** doit-il être partagé avec `prd-agents-browser.md` (désactivé, lecture, interaction) ? Le premier PRD livré le définit, l'autre le réutilise. Arthur tranche l'ordre avant R4.
[/PRD]
