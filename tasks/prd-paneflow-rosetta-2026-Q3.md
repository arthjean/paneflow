[PRD]
# PRD: Paneflow Rosetta - card interactive d'etat agents (2026-Q3)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-27 | Arthur Jean | Draft initial - 4 epics / 14 stories. Definit Rosetta comme card in-app top-center qui decode l'etat des agents, priorise l'attention humaine et route vers la bonne pane/thread sans dependre d'une surface OS externe. |

## Problem Statement

Paneflow sait deja lancer, suivre et orchestrer plusieurs agents CLI, mais l'utilisateur n'a pas encore une surface centrale, persistante et actionnable qui lui dit quand son attention compte.

1. **Le statut existe, mais il reste disperse.** Les etats agents vivent dans `Workspace::agent_sessions`, les dots de sidebar, l'Attention Queue, les notifications OS et les lignes Agents. Aucun endroit ne transforme toute cette verite en un signal top-level: "qui attend, pourquoi, depuis combien de temps, et ou cliquer".
2. **Les notifications OS ne sont pas une base produit fiable.** Elles dependent des reglages systeme, du focus, des politiques Windows/macOS/Linux et ne peuvent pas porter une interaction riche cross-platform. Elles sont utiles comme fallback, pas comme experience principale.
3. **Les agents paralleles changent le rythme du travail.** Quand 3-8 agents tournent, le probleme n'est plus "un agent a fini" mais "quelle intervention humaine est la plus importante maintenant". Sans priorisation, l'utilisateur retourne scanner les panes, perd le fil et rate des agents en attente.
4. **Les alternatives du marche sont soit macOS-first, soit externes au terminal owner.** Vibe Island, Open Island, CodeIsland, Notchi et AgentNotch prouvent l'usage d'une surface top-center/notch pour agents, mais la plupart observent les terminaux depuis l'exterieur. Paneflow possede deja les panes, sessions, workspaces et cibles de focus.

**Why now:** les workflows AI coding passent du chat mono-agent au travail multi-agents local. Les produits concurrents s'installent vite autour de la metaphore "agent island", mais la fenetre est encore ouverte pour une version Paneflow-native: cross-platform, liee aux panes reelles, sobre, actionnable et securisee. Rosetta doit devenir le signal top-level qui montre que Paneflow n'est pas seulement un multiplexer, mais un cockpit d'agents.

## Overview

Paneflow Rosetta est une card interactive top-center, directement integree a la fenetre Paneflow. Elle decode les signaux des agents en etats humains: running, needs input, stalled, done, failed. Elle reste compacte par defaut, s'etend quand une intervention est requise, et ramene l'utilisateur vers la pane ou le thread exact sans voler le focus.

Rosetta reste volontairement in-app. Le coeur du produit doit etre fiable sur Windows, macOS, Linux X11 et Linux Wayland sans dependre d'une fenetre native separee. La promesse produit est une surface Paneflow integree, stable et actionnable, pas une surcouche OS.

La decision structurante: Rosetta ne maintient pas une seconde verite agent. Elle derive ses rows depuis les sources existantes (`agent_sessions`, `ThreadStatus`, surface ids, waiting timestamps, last_result) et ne garde localement que l'etat UI: collapsed/expanded, selection, snooze/dismiss, recent history volatile.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Reduire les agents en attente rates | 100% des sessions `WaitingForInput` hookees visibles dans Rosetta en < 1 frame apres `cx.notify()` | >= 90% des sessions en attente sont rejointes via Rosetta ou Attention Queue plutot que scan manuel |
| Rendre le statut multi-agent lisible | Card compacte affiche le bon etat dominant pour jusqu'a 32 sessions actives | Rosetta devient la surface par defaut de supervision dans les demos Paneflow multi-agents |
| Fiabiliser le retour a la bonne pane | Clic sur une row resolue focus workspace + pane/thread exact dans 100% des cas ou `surface_id`/thread target existe | Fallback documente pour 100% des sessions non resolues, sans navigation vers une mauvaise pane |
| Eviter le bruit | Les etats passifs (`Thinking`, `Finished`) ne provoquent pas d'expansion automatique hors seuils definis | Taux de dismiss/snooze manuel < 20% sur sessions dogfood de 1h |

## Target Users

### Dev orchestrateur Paneflow
- **Role:** solo dev, indie maker ou power-user qui lance plusieurs agents CLI dans Paneflow.
- **Behaviors:** travaille dans CLI/Agents mode, dispatch plusieurs prompts, laisse des agents tourner en arriere-plan, revient quand une permission ou une question bloque.
- **Pain points:** doit regarder la sidebar, scanner les panes ou compter sur des notifications OS inconstantes pour savoir qui attend.
- **Current workaround:** status sidebar, Attention Queue CLI, notifications OS, memoire personnelle de ce qui tourne.
- **Success looks like:** savoir en un coup d'oeil quel agent attend, cliquer, arriver dans la bonne pane, repondre, repartir.

### Utilisateur Agents mode
- **Role:** utilisateur de la vue Agents qui gere des threads/projets Codex/Claude/OpenCode dans Paneflow.
- **Behaviors:** navigue entre projets/chats, lit les diffs, surveille les agents sans rester dans chaque terminal.
- **Pain points:** les dots compactes de la sidebar ne portent pas assez de contexte pour decider quoi ouvrir.
- **Current workaround:** ouvrir chaque thread avec un indicateur non-idle, inspecter le terminal, revenir.
- **Success looks like:** Rosetta explique "Codex needs input - paneflow-web" et ouvre directement le thread.

### Utilisateur prudent / security-minded
- **Role:** dev qui accepte l'autonomie agent mais ne veut pas qu'une UI d'agent puisse tromper ses actions.
- **Behaviors:** lit les permissions, refuse les actions risquee, veut distinguer texte agent et controles Paneflow.
- **Pain points:** les prompts compacts peuvent cacher le contexte d'une commande ou donner trop de pouvoir au texte genere par l'agent.
- **Current workaround:** ouvrir la pane, relire le terminal complet, approuver manuellement.
- **Success looks like:** Rosetta montre uniquement des controles Paneflow typés, garde le texte agent inert, et force l'ouverture de la pane quand le contexte est insuffisant.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Vibe Island:** benchmark payant macOS, card/notch native pour agents avec approvals, questions, plan review, sons et jump terminal. Rosetta differencie par integration Paneflow-native et cross-platform fiable.
- **Open Island / open-vibe-island:** reference OSS qui reduit des hooks JSON en etat session puis overlay. Confirme le pattern event -> reducer -> status card.
- **AgentsRoom Dynamic Island:** concurrent cross-platform avec fenetre flottante always-on-top. Confirme la demande, mais expose le risque de promettre une surface OS globale sur Wayland.
- **CodeIsland / Notchi / AgentNotch:** prouvent que les utilisateurs attendent statut, tool calls, permissions, historique, usage/quota et signaux locaux. Leur faiblesse commune est la dependance a des hooks/adapters externes.
- **Market gap:** une surface agent card qui possede les panes, le routage et la source de verite au lieu de surveiller depuis l'exterieur.

### Best Practices Applied
- Distinguer action-required et passive updates: `WaitingForInput`, `Errored`, `Stalled` ont priorite; `Thinking` et `Finished` n'ouvrent pas automatiquement le panel.
- Eviter l'alert fatigue: Rosetta groupe, priorise, snooze et n'affiche pas chaque tool call dans la card compacte.
- Ne pas voler le focus: la card signale et route; la saisie/reponse se fait dans une surface Paneflow focusable.
- Texte agent non fiable: rendu inert, borne, jamais interprete comme un bouton ou une commande.
- Placement stable: top-center dans la zone principale Paneflow, hors sidebar et hors titlebar OS.

### Research Sources
- [Vibe Island](https://vibeisland.app/)
- [Open Island / open-vibe-island](https://github.com/Octane0411/open-vibe-island)
- [AgentsRoom Dynamic Island](https://agentsroom.dev/features/dynamic-island)
- [CodeIsland](https://github.com/wxtsky/CodeIsland)
- [Notchi](https://github.com/sk-ruban/notchi)
- [AgentNotch](https://github.com/AppGram/agentnotch)
- [Microsoft notification UX guidance](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/app-notifications-ux-guidance)
- [NN/g notifications guidance](https://www.nngroup.com/articles/indicators-validations-notifications/)
- [MDN ARIA live regions](https://developer.mozilla.org/en-US/docs/Web/Accessibility/ARIA/Guides/Live_regions)
- [Canonical Mir: Wayland window positioning](https://canonical.com/mir/docs/stable/explanation/window-positions-under-wayland/)
- [OWASP AI Agent Security Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/AI_Agent_Security_Cheat_Sheet.html)
- [Claude Code Agent View](https://code.claude.com/docs/en/agent-view)

## Assumptions & Constraints

### Assumptions (to validate)
- Les utilisateurs preferent une card in-app fiable a une surface OS externe imparfaite sur Linux Wayland.
- Une projection live derivee depuis `agent_sessions` et `ThreadStatus` suffit pour v1 sans store persistant dedie.
- Le recent history volatile (session app courante) resout le probleme des `Finished` courts sans ajouter de migration disque.
- Les actions directes `Approve/Deny` ne doivent apparaitre que lorsqu'un event type contient assez de metadata; sinon Rosetta doit router vers la pane.

### Hard Constraints
- Cross-platform obligatoire: Windows 10/11, macOS Intel/Apple Silicon, Linux X11/Wayland.
- V1 in-app obligatoire; aucune dependance a un overlay OS global.
- Aucun I/O, subprocess ou scan long dans le render path GPUI.
- Pas de seconde source de verite agent: Rosetta derive depuis l'etat existant.
- Texte agent affiche comme donnees non fiables: borne, inert, pas de Markdown actif, pas de controles generes par l'agent.
- Ne pas casser l'Attention Queue, les notifications OS existantes, la sidebar ou Agents mode.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage Rust canonique, obligatoire avant commit/push.
- `cargo clippy --workspace -- -D warnings` - zero warning sur le workspace.
- `cargo test --workspace` - tests unitaires et integration du workspace.

For UI stories, additional gates:
- Verification visuelle manuelle dans Paneflow a 1280x720, 1920x1080 et largeur <= 900px.
- Verification manuelle dans CLI mode et Agents mode.
- Si un OS cible ne peut pas etre verifie localement, noter explicitement l'OS non verifie dans le statut de story.

## Epics & User Stories

### EP-001: Projection Rosetta de l'etat agent

Construire une projection unique et peu couteuse qui transforme les sources existantes en rows Rosetta priorisees.

**Definition of Done:** Rosetta peut lister les sessions hookees, threads Agents et evenements recents sans store parallele ni I/O render-thread.

#### US-001: Projection des sessions workspace
**Description:** As a Paneflow user, I want Rosetta to derive live rows from workspace agent sessions so that the card reflects the same truth as panes and sidebar.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given multiple workspaces with `agent_sessions`, when Rosetta builds its projection, then each visible session includes `{tool, state, workspace_title, surface_id, message, waiting_secs, last_activity_secs, active_tool_name, last_result}` when available.
- [x] Given no active sessions, when projection runs, then it returns an empty list without rendering a stale card.
- [x] Given multiple states, when rows are ranked, then priority order is `Errored > WaitingForInput > Stalled > Thinking > Finished`.
- [x] Given a session with `surface_id = None`, when rendered later, then it is visible but not navigable.
- [x] Given projection runs during render, when measured with 32 sessions, then it performs no filesystem, process, network or IPC work.

#### US-002: Projection des threads Agents mode
**Description:** As an Agents mode user, I want Rosetta to include active agent threads so that the card covers both CLI panes and Agents threads.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given an Agents thread with `ThreadStatus::WaitingForInput`, when Rosetta builds rows, then the row includes thread title, project/chat context and a focus target.
- [x] Given an Agents thread with `ThreadStatus::Failed`, when Rosetta builds rows, then it ranks above waiting/thinking rows.
- [x] Given `ThreadStatus::Idle`, when no recent completion event exists, then Rosetta omits the row from the active card.
- [x] Given a thread lacks a terminal view target, when clicked later, then Rosetta shows an unavailable target state instead of focusing a wrong pane.
- [x] Given `Stalled` is collapsed to `Thinking` in `ThreadStatus`, when Rosetta needs stalled precision, then it uses session-level data where available and never fabricates stalled state from idle threads.

#### US-003: Recent event history volatile
**Description:** As a user, I want recent done/error/wait transitions retained briefly so that short-lived agent changes are not missed.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given a session transitions to `Finished`, when the active row auto-clears, then Rosetta retains a recent event row for 5 minutes or until a cap of 25 events is reached.
- [x] Given more than 25 recent events, when a new event is added, then the oldest event is dropped.
- [x] Given the app restarts, when Rosetta initializes, then recent history starts empty and no stale disk-backed event is shown.
- [x] Given an event contains agent text, when stored in history, then the text is capped at 512 chars and rendered inert.

---

### EP-002: Surface in-app top-center

Livrer la card Rosetta visible, compacte et responsive dans la zone principale de Paneflow.

**Definition of Done:** une card top-center s'affiche dans CLI et Agents mode, se collapse/expand correctement, respecte la titlebar/sidebar et reste lisible sur petites largeurs.

#### US-004: Card compacte top-center
**Description:** As a user, I want a compact top-center Rosetta card so that I can see the dominant agent status without scanning the sidebar.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given at least one active or recent Rosetta row, when Paneflow renders, then the compact card appears top-center in the main content area, not inside the sidebar or OS titlebar.
- [ ] Given no active or recent rows, when Paneflow renders, then Rosetta is hidden by default.
- [ ] Given the app width is <= 900px, when rendered, then card width is <= viewport minus 32px and text ellipsizes without overlap.
- [ ] Given the card is collapsed, when passive rows are only `Thinking`, then it shows a single-line summary without expanding automatically.
- [ ] Given the render path runs repeatedly, when profiling, then Rosetta adds no animation layout shift to the terminal surface.

#### US-005: Expanded prioritized panel
**Description:** As a user, I want to expand Rosetta into a prioritized list so that I can decide which agent needs attention first.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given the compact card is clicked, when rows exist, then an expanded panel lists sections for `Needs input`, `Failed`, `Stalled`, `Running`, and `Recent`.
- [ ] Given multiple waiting agents, when expanded, then waiting rows are sorted by longest `waiting_since` first.
- [ ] Given a row has a long message, when expanded, then the message is one or two inert ellipsized lines and never overlaps controls.
- [ ] Given a row is not navigable, when expanded, then its action area shows `No pane` or `Unavailable` instead of an active focus button.
- [ ] Given there are more than 8 rows, when expanded, then the panel scrolls inside a bounded height instead of growing over the full workspace.

#### US-006: Input discipline and keyboard behavior
**Description:** As a keyboard-heavy user, I want Rosetta to be interactive without stealing focus so that terminal input stays routed to the active terminal unless I explicitly activate Rosetta.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given Rosetta is collapsed, when it appears due to a passive event, then it does not take keyboard focus.
- [ ] Given Rosetta is expanded, when `Esc` is pressed, then it collapses and focus returns to the previous terminal/thread if it still exists.
- [ ] Given Rosetta is expanded, when arrow keys move selection and `Enter` is pressed, then the selected navigable row focuses the target.
- [ ] Given pointer events hit Rosetta, when clicking inside the card, then events are occluded and do not leak into the PTY behind it.
- [ ] Given the target pane closes while Rosetta is open, when the user presses `Enter`, then no panic occurs and the row becomes unavailable on next render.

#### US-007: Visual polish, status language and motion
**Description:** As a user, I want Rosetta to match Paneflow's visual system so that it reads as product chrome rather than temporary toast chrome.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given dark and light themes, when Rosetta renders, then it uses existing `UiColors` tokens and remains readable at contrast ratio >= 4.5:1 for text.
- [ ] Given status changes, when the compact card updates, then motion duration is between 120ms and 180ms and respects reduced-motion settings if available.
- [ ] Given the card is visible, when compared with sidebar cards, then it uses background alpha between 0.08 and 0.18, 8px max radius and font weight <= `MEDIUM` except for one section title.
- [ ] Given status copy renders, when labels appear, then they use product language: `needs input`, `failed`, `stalled`, `running`, `done`, not raw wire strings.
- [ ] Given narrow width, when the longest workspace title appears, then layout remains stable and truncates before buttons.

---

### EP-003: Navigation et handoff humain

Transformer Rosetta en surface actionnable: elle signale, route vers le bon contexte, et ne prend des actions directes que quand les evenements sont typés et complets.

**Definition of Done:** chaque row navigable peut ouvrir le bon workspace/pane/thread; les prompts humains sont traites comme handoff vers Paneflow; aucun bouton n'est genere depuis du texte agent.

#### US-008: Focus exact workspace/pane/thread
**Description:** As a user, I want clicking a Rosetta row to focus the exact agent target so that I do not scan panes manually.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-005

**Acceptance Criteria:**
- [ ] Given a CLI row with live `surface_id`, when clicked, then Paneflow switches workspace, selects the pane tab and focuses the terminal.
- [ ] Given an Agents row with a thread target, when clicked, then Paneflow switches to Agents mode and selects that project/chat thread.
- [ ] Given the target no longer exists, when clicked, then Rosetta marks the row unavailable and does not change to an unrelated workspace.
- [ ] Given the user clicks a passive `Finished` recent event, when no live target exists, then Rosetta opens the closest available workspace/project context or shows unavailable state.
- [ ] Given a jump succeeds, when the card collapses, then the next keyboard navigation starts from the focused target.

#### US-009: WaitingForInput handoff
**Description:** As a user, I want Rosetta to handle agent questions by sending me to the right input surface so that I can respond without losing context.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given a `WaitingForInput` row, when the primary action is clicked, then Paneflow focuses the target pane/thread and leaves text entry to the normal terminal/composer.
- [ ] Given the waiting message is present, when displayed in Rosetta, then it is visibly separated from Paneflow action buttons.
- [ ] Given the row is unresolved, when the user tries to respond, then Rosetta explains that the pane is unavailable and does not create a synthetic input target.
- [ ] Given multiple waiting rows, when one is resolved by the user, then Rosetta updates priority and selects the next waiting row if expanded.
- [ ] Given the agent clears waiting state before the user clicks, when clicked, then Rosetta refreshes and does not send stale input.

#### US-010: Typed action affordance framework
**Description:** As a cautious user, I want direct actions only for typed internal events so that agent text cannot manufacture approvals.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given a row only has freeform `message`, when rendered, then Rosetta shows `Open`/`Reply` handoff actions, not `Approve`/`Deny`.
- [ ] Given a future typed approval event includes command, cwd, tool, risk level and action id, when rendered, then Rosetta may show direct `Approve`/`Deny` controls generated from typed fields only.
- [ ] Given an approval command exceeds 160 chars, when rendered compactly, then Rosetta truncates display but requires opening the pane or expanded detail before approval.
- [ ] Given a high-risk typed action is marked destructive/network/credential/publish, when rendered, then direct approval is disabled in compact mode.
- [ ] Given agent text includes button-like wording, when rendered, then it remains inert text and cannot bind actions.

#### US-011: Dismiss, snooze and noise rules
**Description:** As a user, I want to suppress low-value Rosetta noise without hiding urgent agent needs so that the card keeps a low interruption rate.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given a `Finished` recent row, when dismissed, then it disappears from recent history and does not remove the real session state.
- [ ] Given a `WaitingForInput` row, when snoozed, then the compact card reduces urgency for 10 minutes but the row remains visible in expanded view.
- [ ] Given an `Errored` row, when dismissed, then it remains visible in expanded history until the underlying session is cleared or a new run replaces it.
- [ ] Given passive `Thinking` rows only, when user disables passive display, then Rosetta hides until waiting/error/stall/done history exists.
- [ ] Given snooze expires, when the row still waits, then Rosetta restores urgent compact state.

---

### EP-004: Securite, settings et readiness cross-platform

Durcir Rosetta comme surface de confiance integree a Paneflow: texte non fiable, settings explicites et QA cross-platform.

**Definition of Done:** Rosetta est securisee contre le texte agent trompeur, configurable, testee, et documentee comme surface in-app cross-platform.

#### US-012: Texte agent non fiable et trust boundary
**Description:** As a security-minded user, I want Rosetta to treat agent text as untrusted so that the card cannot become a phishing or command-injection surface.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given agent text contains Markdown, ANSI, links or control-looking text, when rendered in Rosetta, then it is inert plain text.
- [ ] Given agent text exceeds 512 chars, when stored or displayed, then it is capped and ellipsized.
- [ ] Given bidi/control characters are present, when displayed, then existing sanitization behavior is preserved or matched.
- [ ] Given Paneflow controls render next to agent text, when visually inspected, then controls are separated by layout, color and copy so text cannot impersonate a button.
- [ ] Given a future direct action exists, when its metadata is incomplete, then Rosetta refuses direct action and falls back to `Open`.

#### US-013: Settings and mode gating
**Description:** As a user, I want Rosetta display behavior to be configurable so that I can reduce interruptions during focused work.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given no config key exists, when Paneflow starts after Rosetta ships, then Rosetta is enabled for urgent states and passive rows are collapsed by default.
- [ ] Given the user disables Rosetta, when Paneflow renders, then no Rosetta card appears but sidebar/Attention Queue/OS notification behavior remains unchanged.
- [ ] Given the user changes passive display or snooze defaults, when config is saved, then settings persist across restart.
- [ ] Given config is invalid, when loaded, then Paneflow falls back to urgent-only defaults and does not panic.
- [ ] Given the app is in Review mode, when Rosetta is enabled, then the PRD implementation either explicitly supports it or documents why it is mode-gated off.

#### US-014: Regression coverage and visual QA runbook
**Description:** As an implementer, I want automated and manual verification for Rosetta so that the card does not regress panes, focus or layout.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-008, US-012

**Acceptance Criteria:**
- [ ] Given projection helpers, when unit tests run, then empty, waiting, errored, stalled, multi-agent and unresolved-surface cases are covered.
- [ ] Given keyboard routing, when tests or manual verification run, then `Esc`, arrow navigation and `Enter` behavior are checked.
- [ ] Given the UI runbook, when followed, then it covers CLI mode, Agents mode, narrow width, multiple workspaces and closed-target fallback.
- [ ] Given Linux Wayland is not locally available, when validating, then the runbook records the unverified platform instead of claiming full manual coverage.
- [ ] Given unrelated dirty files exist, when implementing Rosetta stories, then only Rosetta-scoped files are staged and status JSON is updated per story.

## Functional Requirements

- FR-01: The system must render Rosetta in-app, top-center, inside the Paneflow window.
- FR-02: The system must derive Rosetta rows from existing Paneflow agent/session/thread state, not from a parallel lifecycle store.
- FR-03: The system must rank rows by user salience: error, waiting, stalled, running, done/recent.
- FR-04: The system must support collapsed and expanded states.
- FR-05: The system must focus the exact workspace/pane/thread when a navigable row is activated.
- FR-06: The system must display unresolved rows as visible but non-navigable.
- FR-07: The system must keep agent-provided text inert and bounded.
- FR-08: The system must not show direct approval actions from freeform agent text.
- FR-09: The system must remain integrated inside the Paneflow window and must not rely on OS notifications or a separate OS-level window for core behavior.
- FR-10: The system must preserve existing Attention Queue, sidebar and OS notification behavior.

## Non-Functional Requirements

- **Performance:** Rosetta projection must add < 2 ms P95 render-path CPU cost with 32 active sessions on a dev build machine; no filesystem/process/network/IPC work in render.
- **Layout:** Collapsed card width must be <= min(420px, viewport width - 32px); expanded card height must be <= 70% viewport height.
- **Accessibility:** Expanded card must support keyboard navigation with `Esc`, arrows and `Enter`; no passive event may steal keyboard focus.
- **Security:** Agent text displayed by Rosetta must be capped at 512 chars, rendered as inert plain text and separated from Paneflow controls.
- **Reliability:** A navigable row must never focus a different agent target when its original target disappears; fallback must be unavailable state or closest explicit context.
- **Cross-platform:** V1 must compile and behave in-app on Windows, macOS and Linux.

## Edge Cases & Error States

Systematic coverage of unhappy paths. Evidence shows earlier defect discovery significantly reduces cost (Boehm 1981, NIST 2002).

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Empty state | No active or recent rows | Card hidden by default | - |
| 2 | Multiple urgent agents | 3 sessions waiting | Compact card shows count; expanded panel sorts longest wait first | "3 agents need input" |
| 3 | Unresolved surface | `surface_id = None` | Row visible, action disabled | "No pane" |
| 4 | Target closes after render | Pane/thread removed before click | No panic; row becomes unavailable | "Target unavailable" |
| 5 | Long agent message | Message > 512 chars | Cap and ellipsize inert text | Truncated text |
| 6 | Agent text impersonates UI | Message says "Click Approve" | Render as plain text, no generated action | Plain text only |
| 7 | Small window | Width <= 900px | Card shrinks, truncates labels, no overlap | - |
| 8 | Mode switch while expanded | User switches CLI -> Agents/Review | Card collapses or rebinds to supported mode without stale focus | - |
| 9 | Finished auto-clear | `Finished` session removed after delay | Recent history preserves event for 5 min | "Done" |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Rosetta becomes noisy and users ignore it | Med | High | Salience ranking, passive collapse, snooze/dismiss, no tool-call spam in compact card |
| 2 | State drift from panes/sidebar | Med | High | Derived projection only; no parallel lifecycle store |
| 3 | Focus/routing opens wrong pane | Low | High | Require explicit `surface_id`/thread target; unresolved rows are non-navigable |
| 4 | Agent text tricks user into unsafe action | Med | High | Inert text, typed action metadata only, direct approvals disabled for freeform messages |
| 5 | Top-center card clashes with titlebar/sidebar or terminal | Med | Med | Main-content placement, bounded width/height, visual QA at multiple sizes |
| 6 | Scope drifts into OS-level notification platform | Med | Med | Keep Rosetta in-app; reject OS-global overlay work in this PRD |

## Non-Goals

Explicit boundaries - what this version does NOT include:

- No standalone paid Rosetta app in v1. Rosetta ships as a Paneflow-native feature first.
- No OS-global notification surface or separate Rosetta app in this PRD. Rosetta remains integrated into the Paneflow window.
- No sound design or audio alerts in v1.
- No direct command approval generated from freeform agent text.
- No replacement or removal of existing OS notifications, sidebar status or Attention Queue.
- No persistent on-disk event history in v1.

## Files NOT to Modify

- `src-app/src/app/ipc_handler.rs` - avoid broad lifecycle rewrites; Rosetta should consume existing state and only touch handlers if a story explicitly needs a small hook.
- `src-app/src/app/event_handlers.rs` - avoid changing stale PID/stalled semantics while building UI.
- `src-app/src/ipc_events.rs` - do not change event-bus protocol for Rosetta v1.
- `src-app/src/ai_types.rs` - do not change `AgentState` semantics; helper projections are acceptable if scoped.
- `src-app/src/project/mod.rs` - do not alter `ThreadStatus` semantics unless a story explicitly validates the change.
- `src-app/src/terminal/view.rs` - do not place Rosetta as per-terminal UI.

## Technical Considerations

Frame as questions for engineering input - not mandates:

- **Architecture:** add a focused `src-app/src/app/rosetta.rs` module and a small render hook in `PaneFlowApp::render`. Engineering to confirm whether state fields belong directly in `PaneFlowApp` or a nested `RosettaState`.
- **Projection:** derive rows from `Workspace::agent_sessions`, Agents thread status and recent volatile events. Engineering to confirm if existing `workspace_agent_status` can be reused or if Rosetta needs a richer projection type.
- **Rendering:** use GPUI `deferred` absolute positioning, top-center within main content, and mouse occlusion. Rosetta should render inside the existing Paneflow window.
- **Focus:** reuse existing surface routing (`find_pane_by_surface_id`, workspace focus helpers, Agents target selection). Engineering to confirm exact APIs per mode.
- **Settings:** extend config only after US-013; default should keep urgent Rosetta enabled and passive rows collapsed.
- **Testing:** pure projection/ranking helpers should be unit-tested; GPUI visual behavior needs manual verification.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Waiting sessions visible centrally | 0% central card coverage | 100% hook-backed `WaitingForInput` rows visible in Rosetta | Month-1 | Manual dogfood + projection tests |
| Exact target jump success | Attention Queue covers CLI waiting only; no Rosetta | 100% success when `surface_id` or Agents target exists | Month-1 | Manual QA runbook |
| Passive noise | N/A (new feature) | Passive-only Rosetta does not auto-expand | Month-1 | Manual QA + tests for priority policy |
| Render overhead | N/A (new feature) | < 2 ms P95 projection/render overhead at 32 sessions | Month-1 | Local instrumentation or debug timing |
| Cross-platform v1 reliability | OS notifications inconsistent on Windows/dev setups | In-app Rosetta verified or explicitly recorded on Windows/macOS/Linux | Month-6 | QA matrix in story status |

## Open Questions

- Should Rosetta be visible in Review mode, or only CLI + Agents for v1? Owner: product/engineering before US-013.
- What exact typed event schema should unlock direct `Approve/Deny` later? Owner: engineering before US-010 implementation.
- Should recent history become persistent after v1? Owner: product after dogfood metrics.
[/PRD]
