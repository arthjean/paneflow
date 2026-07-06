[PRD]
# PRD: Optimisation memoire multi-agents - 2026-Q3

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-23 | Arthur Jean | Initial draft. PRD pour reduire la pression RAM de Paneflow quand 5-8 agents tournent plusieurs heures. Scope: caches diff/review, Agents diff, scrollback live, terminaux agents retenus, sessions sidebar, IPC queues, validation locale minimale. |

## Problem Statement

Paneflow est construit pour rester plus lean qu'un IDE complet pendant des workflows multi-agents. Le feedback "RAM usage" de l'issue #11 signale pourtant que, avec 5+ agents, la RAM semble proche de Zed, alors que Paneflow devrait avoir moins d'overhead qu'un IDE avec LSPs et surfaces additionnelles.

1. **Le risque vient surtout d'etats vivants conserves pour l'UX.** Le code a deja beaucoup de limites d'entree et de persistence, mais plusieurs surfaces gardent des donnees chaudes: scrollback live de terminaux, `TerminalView` caches, rows diff unified/split, review terminals et listes de sessions.
2. **Les bornes actuelles sont parfois larges et multipliees par agent/pane/workspace.** Le terminal garde 10 000 lignes par defaut et peut monter a 100 000; plusieurs caches sont limites par comportement utilisateur plus que par budget explicite.
3. **L'utilisateur cible a besoin de marge RAM pour des workloads bursty.** Le feedback cite audio/video gen, ASR et outils de visualisation: Paneflow peut etre foreground/background pendant que d'autres process consomment fortement.
4. **La reponse doit eviter le feature bloat.** L'objectif n'est pas d'ajouter un dashboard ou un systeme d'observabilite lourd, mais de rendre l'architecture memoire previsible et sobre.

**Why now:** l'issue #11 valide que Paneflow atteint deja la barre UX/polish, mais le passage de Zed a Paneflow devient plus convaincant si Paneflow reste visiblement plus leger quand 6-8 agents tournent. Le code est encore assez petit pour installer des budgets memoire propres avant que les surfaces Agents/Review/Diff grossissent.

## Overview

Cette PRD livre une passe memoire structurelle, pas un benchmark. Le design choisi est conservateur: Paneflow libere les donnees froides et cachees, reduit les budgets de scrollback des surfaces agents/review, borne les queues et listes, mais ne tue jamais silencieusement un agent actif.

Le travail se decompose en cinq epics. EP-001 libere les gros caches de diff/review et Agents diff quand ils ne sont plus visibles. EP-002 ajoute des profils de scrollback et une politique LRU/TTL prudente pour les terminaux agents caches. EP-003 borne les listes de sessions, attribution et undo scrollback. EP-004 durcit l'IPC cote backpressure. EP-005 ajoute une validation locale minimale pour prouver que les budgets fonctionnent sans transformer la feature en projet de benchmarking.

Decisions structurantes:
- D'abord liberer ce qui n'a aucun effet UX visible: diff rows cachees, review terminals caches, Agents diff cache ferme, sessions lists trop longues.
- Garder les processus actifs vivants. Les agents en cours ne sont jamais evinces ou termines par une politique memoire.
- Faire des limites structurelles mesurables: nombre de rows, lignes de scrollback par profil, pending IPC cap, nombre de sessions conservees.
- Rester 100% cross-platform: pas de chemin POSIX, pas d'API OS-specific pour la memoire, pas de comportement different Linux/macOS/Windows sauf si le backend terminal l'exige explicitement.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Liberer les donnees diff/review cachees | Une colonne Review cachee ne retient plus de rows diff, raw files, attribution ou review terminal | 0 regression connue sur gros diffs, meme avec 8 agents |
| Reduire la RAM terminal par agent | Les surfaces agent/review utilisent un profil scrollback plus bas que le terminal normal | Les terminaux caches sont trims apres idle sans tuer les agents actifs |
| Borner les caches Agents | Au plus 8 terminaux agents caches en mode chaud complet; les anciens inactifs sont trims ou droppes si exit | Usage stable apres 2h avec 6-8 agents et navigation repetee |
| Borner les listes et queues | Sessions sidebar/attribution limitees a 100 items par source; IPC GPUI queue borne a 256 pending requests | Aucun growth non borne observe dans les queues/listes sous charge locale |
| Garder l'UX warm-resume | 0 agent actif tue silencieusement par la politique memoire | Warm-resume garde les threads recents et actifs, libere seulement le froid |

## Target Users

### Developpeur multi-agents intensif
- **Role:** developpeur qui garde 5-8 agents CLI ouverts dans Paneflow, avec un mix Claude Code, Codex, opencode, GLM ou outils custom.
- **Behaviors:** travaille par worktrees, fait tourner un agent principal, des reviewers et des outils d'audit, puis laisse Paneflow ouvert pendant des workloads gourmands.
- **Pain points:** la RAM de Paneflow devient moins differenciante si elle se rapproche de Zed quand plusieurs agents tournent.
- **Current workaround:** fermer manuellement des panels, limiter les agents, ou revenir a Zed malgre l'overhead IDE.
- **Success looks like:** Paneflow reste fluide et sobre pendant une session longue, sans devoir fermer a la main les surfaces caches.

### Developpeur sur machine contrainte
- **Role:** utilisateur sur laptop 8-16 GB, Linux/macOS/Windows, qui veut utiliser Paneflow comme cockpit leger.
- **Behaviors:** garde quelques agents, bascule souvent entre Agents/Review/CLI, ouvre des diffs, puis revient au terminal.
- **Pain points:** les caches invisibles et le scrollback long peuvent consommer une part disproportionnee de la RAM disponible.
- **Current workaround:** reduire le scrollback global ou redemarrer l'app.
- **Success looks like:** les defaults sont sobres sans perdre la capacite de scrollback normale dans les terminaux manuels.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Issue #11 / Zed baseline:** l'utilisateur compare Paneflow a Zed avec 5+ agents et attend que Paneflow ait un overhead inferieur a 6-8 sessions agents comparables. Source: https://github.com/ArthurDEV44/paneflow/issues/11
- **VS Code integrated terminal:** le scrollback par defaut est 1 000 lignes, et le scrollback restaure pour sessions persistantes est configure separement. Cela valide l'idee de separer terminal live, restore et profils d'usage. Source: https://code.visualstudio.com/docs/terminal/basics et https://code.visualstudio.com/docs/terminal/advanced
- **Alacritty:** le scrollback `history` est limite a 100 000 et defaut a 10 000, ce qui correspond au comportement Paneflow actuel via Alacritty. Source: https://alacritty.org/config-alacritty.html
- **Windows Terminal:** `historySize` a un defaut 9 001 et un maximum 32 767, plus conservateur que le plafond Paneflow actuel de 100 000. Source: https://learn.microsoft.com/en-us/windows/terminal/customize-settings/profile-advanced
- **Rust `std::sync::mpsc`:** `channel()` est conceptuellement "infinite buffer"; les queues IPC critiques doivent donc utiliser une borne explicite ou une strategie de rejet. Source: https://doc.rust-lang.org/std/sync/mpsc/index.html

### Best Practices Applied
- Separer les budgets par usage: terminal manuel, agent, review, cache froid et restore n'ont pas besoin du meme scrollback.
- Favoriser des caps structurels plutot qu'une chasse RSS ponctuelle: rows, lignes, entries, pending requests, TTL.
- Ne pas interrompre les processus actifs pour gagner de la RAM; preferer trim de scrollback, drop de caches UI et evictions d'entites exit/inactives.
- Construire les representations diff a la demande: unified et split n'ont pas besoin d'etre tous deux chauds pour toutes les colonnes cachees.
- Transformer les unbounded channels en queues explicites avec erreur d'overload actionnable.

### Existing Codebase Findings
- `DEFAULT_SCROLLBACK_LINES` vaut 10 000 dans `src-app/src/terminal/pty_session.rs`; la config clamp jusqu'a 100 000 dans `crates/paneflow-config/src/schema.rs`.
- Le scrollback persisté est deja limite a 4 000 lignes / 400k chars dans `extract_scrollback`, donc le risque principal est le `Term` live, pas `session.json`.
- `agents_terminal_view_cache` et `bottom_terminals` gardent des terminaux chauds pour UX warm-resume.
- `diff::view::hide_column` marque une colonne `Loading`, mais les `Rc` de display rows, offsets, attribution et review terminals peuvent rester accroches par les champs de display.
- `app::agents_diff` cache volontairement les rows Agents diff quand le panneau est ferme pour reouverture rapide.
- `ipc.rs` utilise `std::sync::mpsc::channel()` pour la queue de requetes vers GPUI; les requetes individuelles sont cappees, mais la queue elle-meme ne l'est pas.

## Assumptions & Constraints

### Assumptions (to validate)
- Le trim de scrollback live est possible sans detruire le PTY actif, ou au minimum applicable aux terminaux caches/inactifs via une API Alacritty/Term existante. Si ce n'est pas possible, US-005 doit documenter le fallback.
- Liberer `disp_unified`/`disp_split` et `review_terminals` au hide ne degrade pas l'UX: la reouverture peut reconstruire les rows a partir de git/diff.
- Les utilisateurs acceptent un scrollback agent plus bas que le terminal manuel si le terminal normal conserve le default historique.
- Une limite de 100 sessions par source/cwd est suffisante pour l'UI, avec compteur d'omission pour les historiques longs.
- Une queue IPC de 256 pending requests couvre les usages legitimes et rejette proprement les clients agressifs.

### Hard Constraints
- Cross-platform Linux, macOS et Windows. Pas de chemin hardcode, pas de shell OS-specific, pas de comportement qui depend de `/proc`, `$HOME` ou d'un separateur POSIX dans ce PRD.
- Ne jamais remplacer les dependances GPUI/Alacritty locales par crates.io.
- Ne jamais tuer silencieusement un agent actif, un PTY actif ou un process utilisateur pour respecter un budget memoire.
- Ne pas ajouter de dashboard memoire, telemetry remote ou collecte intrusive.
- Les changements Rust doivent passer `cargo fmt --check` avant chaque commit/push, puis `cargo clippy --workspace -- -D warnings` et `cargo test --workspace`.
- Garder les commits atomiques et Conventional Commit, sans attribution IA.

## Quality Gates

These commands must pass for every user story that touches Rust:
- `cargo fmt --check` - formatage canonique requis avant commit et push.
- `cargo clippy --workspace -- -D warnings` - aucun warning de lint.
- `cargo test --workspace` - tests unitaires et integration workspace.

Additional gates:
- `git diff --check` - pas de whitespace error dans PRD/docs/code.
- Manual cross-platform reasoning note per story: confirmer que le changement est OS-neutral ou documenter la branche `cfg`.
- UI/manual verification for stories touching live terminals, Review or Agents diff: verify no active agent is killed and hidden panels reopen correctly.

## Epics & User Stories

### EP-001: Liberer les caches Diff, Review et Agents Diff caches

Les plus gros quick wins sont des etats UI caches qui ne devraient pas survivre a un hide/close. Cet epic baisse la RAM sans changer les processus agents.

**Definition of Done:** fermer ou cacher une surface diff/review libere ses rows, raw files, offsets, attribution et terminaux review; fermer Agents diff libere son modele ou l'expire par TTL; reouverture reconstruit proprement.

#### US-001: Clear complet des colonnes Review cachees
**Description:** As a mainteneur, I want que `hide_column` libere vraiment les donnees d'une colonne Review so that les gros diffs ne restent pas accroches apres fermeture visuelle.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given une colonne Review chargee, when l'utilisateur la cache, then `state`, `disp_unified`, `disp_split`, offsets, spans, hunk tops, `h_offsets`, attribution et raw file references sont clears.
- [x] Given la colonne avait des `review_terminals`, when elle est cachee, then ils sont droppes proprement ou explicitement fermes selon le comportement existant de close terminal.
- [x] Given une colonne cachee puis reouverte, when le loader reconstruit, then le diff affiche les memes fichiers sans stale rows ni panic.
- [x] Given une colonne cachee pendant qu'un load async obsolete revient, when le generation guard detecte l'obsolescence, then aucune donnee n'est reinjectee.
- [x] Tests: unit/helper test ou integration ciblée prouvant qu'apres hide, les vectors/Options de display sont vides.

#### US-002: Agents diff libere son cache au close ou apres TTL
**Description:** As an agent user, I want que fermer le dock Agents diff libere la memoire du modele diff so that ouvrir un gros diff ne penalise pas une session agents longue.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given le dock Agents diff charge, when l'utilisateur le ferme, then `agents_diff` devient `None` ou passe par un cache TTL explicitement borne.
- [x] Given un reopen apres close, when le cwd/git state n'a pas change, then la vue se recharge correctement meme si elle n'est plus immediate.
- [x] Given un load async arrive apres close, when la cible n'est plus active, then le resultat est ignore sans recreer le cache.
- [x] Given un gros diff ferme, when l'utilisateur continue a travailler dans Agents, then les rows unified/split ne restent pas retenues par le state global.
- [x] Tests: close/reopen et late async result.

#### US-003: Diff rows construites a la demande par mode actif
**Description:** As a user, I want que Paneflow ne garde pas unified et split complets pour chaque colonne si je n'utilise qu'un mode so that Review reste sobre sur gros diffs.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** US-001

**Acceptance Criteria:**
- [x] Given une colonne Review ouverte en unified, when elle charge, then le split n'est construit qu'au premier toggle split ou est droppe quand unified redevient le seul mode actif.
- [x] Given un toggle unified/split, when la representation manquante est reconstruite, then les scroll offsets restent coherents ou sont remis a un debut de hunk explicite.
- [x] Given une erreur de reconstruction, when le toggle est demande, then l'UI affiche l'etat d'erreur existant plutot que de paniquer.
- [x] Given syntax highlighting active, when seule une representation est visible, then les syntax runs non necessaires ne sont pas dupliques.
- [x] Tests: loader construit seulement le mode attendu; toggle reconstruit sans stale rows.

### EP-002: Budgets terminaux, scrollback et cache agents

Le multiplicateur RAM principal reste le terminal live. Cet epic introduit des profils et une politique cache conservatrice.

**Definition of Done:** les terminaux agents/review/bottom peuvent utiliser un scrollback plus sobre que le terminal manuel; les terminaux caches froids sont trims ou evinces sans interrompre un agent actif.

#### US-004: Profils de scrollback par type de surface
**Description:** As a Paneflow user, I want des defaults de scrollback adaptes aux agents et reviews so that plusieurs agents n'allouent pas le meme budget qu'un terminal manuel.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given un terminal manuel normal, when il est cree, then le default historique 10 000 lignes reste conserve sauf config utilisateur.
- [x] Given un terminal Agent ou Review, when il est cree, then il utilise un profil sobre documente (target initial: 4 000 lignes agent, 2 000 lignes review) sauf override explicite.
- [x] Given une valeur config > profil cap, when appliquee a un profil agent/review, then elle est clampée a une limite documentee ou exige un opt-in explicite.
- [x] Given un OS Linux/macOS/Windows, when le profil est applique, then aucun chemin ou shell OS-specific n'est introduit.
- [x] Tests: resolution de scrollback par profil, clamp, compat default normal.

#### US-005: Trim de scrollback des terminaux caches/inactifs
**Description:** As a laptop user, I want que les terminaux caches reduisent leur scrollback apres idle so that Paneflow rend de la RAM sans perdre les agents actifs.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** US-004

**Acceptance Criteria:**
- [x] Given un terminal agent cache et actif, when il devient cache pendant plus de 10 minutes, then son scrollback live est trimme vers le profil cache cible si l'API terminal le permet.
- [x] Given le backend ne permet pas un trim non destructif, when la story est implementee, then le fallback est documente et la story limite son action aux terminaux exit/inactifs.
- [x] Given un agent produit encore de la sortie, when le trim timer arrive, then aucun process/PTY n'est tue et aucune sortie active n'est perdue hors scrollback ancien.
- [x] Given un terminal redevient visible, when l'utilisateur scroll, then l'UI indique clairement si l'ancien scrollback a ete trimme ou montre seulement la fenetre restante.
- [x] Tests/manual: un terminal actif reste vivant; un terminal cache sortant reste lisible apres trim.

#### US-006: LRU/TTL pour `agents_terminal_view_cache` et bottom terminals
**Description:** As a mainteneur, I want une politique explicite pour les terminaux agents caches so that une longue session n'accumule pas tous les threads ouverts depuis le lancement.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** US-004

**Acceptance Criteria:**
- [x] Given plus de 8 terminaux agents caches chauds, when un nouveau terminal est ajoute, then les plus anciens terminaux inactifs/exited sont droppes selon LRU.
- [x] Given un terminal cache est actif/running, when il depasse la limite LRU, then il n'est pas tue; il peut seulement passer en profil cache/trim.
- [x] Given un bottom terminal cache, when le panel est ferme, then la retention suit la meme politique que le cache Agents ou une limite plus stricte documentee.
- [x] Given un terminal evince puis reouvert, when son thread existe encore, then Paneflow recree une surface propre ou affiche un etat "terminal was released" actionnable.
- [x] Tests: LRU evince uniquement exited/inactive; active terminals proteges; bottom terminals respectent la limite.

#### US-007: Review terminals sous profil sobre et cleanup au hide
**Description:** As a reviewer, I want que les terminaux de review ne restent pas en memoire quand la colonne disparait so that lancer des reviewers ne cree pas de retention cachee.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** US-001, US-004

**Acceptance Criteria:**
- [x] Given un review terminal cree, when il demarre, then il utilise le profil Review.
- [x] Given une colonne Review cachee, when elle contient des review terminals, then ils sont droppes/fermes selon la semantique de close existante.
- [x] Given un review terminal encore actif, when l'utilisateur cache la colonne, then l'action ne tue pas silencieusement sans comportement documente; si fermeture active est requise, elle passe par le meme chemin explicite que close terminal.
- [x] Given une nouvelle review lancee dans la meme colonne, when elle remplace l'ancienne, then les anciens PTYs sont droppes comme aujourd'hui et ne restent pas references.
- [x] Tests/manual: cacher une colonne review libere les terminaux et la colonne se recharge proprement.

### EP-003: Borner sessions, attribution et undo scrollback

Les historiques de sessions et les copies de scrollback sont moins critiques que les terminaux live, mais doivent rester predictibles.

**Definition of Done:** les listes UI de sessions/attribution et les copies de scrollback d'undo ont des caps explicites, avec indication d'omission quand des donnees anciennes sont laissees de cote.

#### US-008: Cap des sessions sidebar et attribution diff
**Description:** As an agent user, I want que la sidebar sessions reste rapide et bornee meme avec beaucoup d'historiques so that un vieux dossier agent ne gonfle pas l'UI.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given plus de 100 sessions Claude/Codex/OpenCode pour un cwd, when la sidebar charge, then seules les 100 plus recentes par source sont retenues en memoire UI.
- [x] Given des sessions omises, when l'UI rend, then un compteur discret indique combien d'anciennes sessions ne sont pas affichees.
- [x] Given l'attribution diff cherche des sessions, when plus de 50 matchs par colonne existent, then les resultats sont bornes et tries par recence/pertinence.
- [x] Given une erreur de scan d'une source, when deux autres sources repondent, then les resultats partiels restent affiches et la source en erreur ne laisse pas de Vec stale.
- [x] Tests: cap par source, compteur omitted, attribution cap.

#### US-009: Budget memoire pour undo de panes fermees
**Description:** As a terminal user, I want pouvoir restaurer une pane fermee sans que l'undo stack garde trop de scrollback so that les fermetures repetees ne consomment pas une RAM disproportionnee.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given plusieurs panes fermees, when `closed_panes` atteint sa limite, then les plus anciennes restent evincees comme aujourd'hui.
- [x] Given un scrollback capture depasse 400k chars, when stocke dans `ClosedPaneRecord`, then il est tronque par le cap existant ou un cap plus strict documente.
- [x] Given la memoire cumulee de `closed_panes` depasse un budget documente (target initial: 2 MiB text), when une nouvelle pane est fermee, then les plus anciennes captures sont droppees avant d'ajouter la nouvelle.
- [x] Given l'utilisateur undo une pane dont le scrollback a ete droppe, when restauree, then la pane revient sans scrollback ancien plutot que d'echouer.
- [x] Tests: budget cumule et undo avec scrollback absent.

### EP-004: Backpressure IPC et queues d'evenements

Les requetes sont cappees en taille, mais la queue vers GPUI doit aussi etre bornee pour eviter les pics artificiels.

**Definition of Done:** aucun chemin IPC/MCP courant ne peut accumuler une queue illimitee en memoire; les clients recoivent une erreur claire si Paneflow est surcharge.

#### US-010: Remplacer la queue IPC GPUI non bornee par une queue bornee
**Description:** As a mainteneur, I want que les requetes IPC vers GPUI aient une capacite maximale so that un client agressif ne pousse pas une croissance memoire non bornee.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `std::sync::mpsc::channel()` dans `ipc.rs`, when remplace, then la queue a une capacite documentee (target initial: 256 pending requests).
- [x] Given la queue est pleine, when une nouvelle requete arrive, then Paneflow retourne une erreur overload actionnable au client sans bloquer indefiniment.
- [x] Given 16 connexions concurrentes, when elles envoient vite des requetes cappees, then la memoire de pending requests reste bornee par la capacite choisie.
- [x] Given un receiver GPUI ferme, when une requete arrive, then le serveur retourne l'erreur existante/propre sans panic.
- [x] Tests: queue pleine, response overload, receiver dropped.

#### US-011: Drain IPC par tick avec budget
**Description:** As a user, I want que Paneflow reste responsive meme sous rafale IPC so that une queue importante ne monopolise pas le frame loop.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** US-010

**Acceptance Criteria:**
- [x] Given plusieurs requetes sont pending, when `process_ipc_requests` tourne, then il traite au plus un nombre documente par tick (target initial: 64) ou jusqu'a un budget temps court.
- [x] Given des requetes restent apres le budget, when le tick finit, then elles restent pending pour le prochain tick sans perte.
- [x] Given des requetes annulees, when elles sont drainees, then elles ne consomment pas de budget inutile au-dela d'un seuil raisonnable.
- [x] Given une rafale de `surface.read`, when la queue est sous charge, then l'UI ne se bloque pas sur un drain complet non borne.
- [x] Tests: drain cap, requests remaining, cancelled skipped.

### EP-005: Validation locale minimale et preparation PR

Cet epic prouve que la passe fonctionne localement sans transformer le travail en benchmark produit.

**Definition of Done:** une procedure de smoke locale reproduit le cas 6-8 agents, verifie les caps structurels et documente le lien avec l'issue #11 pour la PR.

#### US-012: Scenario smoke memoire 6-8 agents et note PR
**Description:** As a mainteneur, I want une validation locale minimale du cas issue #11 so that la PR peut expliquer concretement ce qui a change sans vendre un benchmark fragile.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** US-001, US-002, US-004, US-006, US-010

**Acceptance Criteria:**
- [x] Given une build locale, when 6-8 agents/shells generent de la sortie pendant 30 minutes, then Paneflow reste responsive et les caps structurels sont observables via logs/debug assertions ou inspection.
- [x] Given Agents diff et Review sont ouverts puis fermes, when la smoke continue, then les caches correspondants sont liberes dans les 30 secondes ou au prochain tick documente.
- [x] Given un agent actif, when le scenario depasse les limites cache, then aucun agent actif n'est tue silencieusement.
- [x] Given le scenario ne peut pas etre execute sur un OS dans l'environnement local, when la story est livree, then la PR note explicitement "not verified on {OS}" au lieu de supposer.
- [x] PR note: resume l'impact utilisateur, les limites structurelles ajoutees, les checks locaux, et lie l'issue #11.

## Functional Requirements

- FR-01: The system must release hidden Review column data when a column is hidden or closed.
- FR-02: The system must release or expire Agents diff cached row models after close.
- FR-03: The system must support terminal memory profiles for normal, agent, review and cached terminal surfaces.
- FR-04: The system must never silently terminate an active agent or PTY for memory pressure.
- FR-05: The system must cap agent session UI lists and diff attribution result vectors.
- FR-06: The system must cap closed-pane undo scrollback by entry count and cumulative text budget.
- FR-07: The system must use a bounded IPC request queue or equivalent backpressure.
- FR-08: The system must process IPC requests with a per-tick budget.
- FR-09: The system must remain cross-platform across Linux, macOS and Windows.
- FR-10: The system must document any intentionally retained warm cache and its bound.

## Non-Functional Requirements

- **Performance:** UI frame loop must not drain more than 64 IPC requests per tick unless an explicit time budget proves safe.
- **Memory:** Agent terminal profile target is <= 4 000 scrollback lines by default; Review profile target is <= 2 000; cached/hidden terminal trim target is <= 1 000 when supported.
- **Memory:** Sessions sidebar retains <= 100 sessions per source/cwd; diff attribution retains <= 50 matches per column.
- **Memory:** IPC GPUI pending request queue capacity target is 256.
- **Reliability:** 0 active agents are silently terminated by cache eviction across the smoke scenario.
- **Compatibility:** Existing user config without new memory fields loads with the same normal terminal default of 10 000 lines.
- **Cross-platform:** Every story must compile and behave on Linux, macOS and Windows or document an explicit stub/fallback.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Active agent hidden | Agent terminal becomes cached while still producing output | Process stays alive; only safe trim/cache downgrade occurs | None unless scrollback was trimmed |
| 2 | Late async diff load | User closes Agents diff or hides Review while loader is running | Generation guard ignores stale result and does not recreate cache | None |
| 3 | Queue overload | IPC queue reaches capacity | Request rejected with overload error; server stays responsive | "Paneflow is busy; retry shortly" |
| 4 | Scrollback trim unsupported | Terminal backend has no safe trim API | Fallback applies only to exited/inactive terminals; active terminals are protected | Optional debug log |
| 5 | Session history too large | Thousands of old sessions exist for one cwd | UI loads recent capped set and reports omitted count | "{N} older sessions hidden" |
| 6 | Undo scrollback evicted | User restores a closed pane whose scrollback exceeded budget | Pane restores without old scrollback; no panic | "Scrollback was released to save memory" |
| 7 | OS not locally verified | Developer cannot run macOS/Windows smoke | PR states verification gap explicitly | PR checklist note |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Scrollback trim is not supported safely by the terminal backend | Med | High | Gate US-005 behind API validation; fallback to profiles at creation + evict only exited/inactive terminals |
| 2 | Users miss old scrollback after hidden-terminal trim | Med | Med | Keep normal terminal default unchanged; apply lower defaults first to agent/review; surface trim only when relevant |
| 3 | Rebuilding diff rows after close feels slower | Med | Low | Prefer clear-on-close for hidden data; keep active visible rows hot; measure with manual UX |
| 4 | Bounded IPC queue rejects legitimate automation bursts | Low | Med | Capacity 256, clear overload error, tune after local smoke |
| 5 | LRU eviction accidentally drops a running agent | Low | High | Explicit active/running guard, tests, manual smoke with live output |
| 6 | Scope becomes too broad | Med | Med | Implement by priority order; stop after P0/P1 if memory wins are sufficient |

## Non-Goals

Explicit boundaries - what this version does NOT include:

- No full memory profiler UI, no charts, no long-running benchmark suite as the product deliverable.
- No remote telemetry or collection of user terminal contents.
- No process killing or automatic agent termination for memory pressure.
- No rewrite of terminal backend, GPUI, Alacritty, IPC protocol architecture or session format beyond additive fields/caps.
- No OS-specific memory hacks for Linux/macOS/Windows.
- No user-facing feature expansion unrelated to memory pressure.

## Files NOT to Modify

- `Cargo.toml` GPUI/Alacritty dependency pins - do not replace local/path/fork dependencies for this work.
- `packaging/`, `debian/`, `assets/` - packaging assets are out of scope unless a story explicitly needs release notes.
- `crates/paneflow-process/` - process scanning is not the target of this PRD unless a cache policy directly needs process state already exposed by the app.
- `keys/` - signing and key material are unrelated and must not be touched.

## Technical Considerations

Frame as questions for engineering input - not mandates:

- **Terminal scrollback trim:** Does the current Alacritty `Term` expose a safe way to shrink history for an existing terminal? If not, should Paneflow limit profile-at-creation and LRU only exited/inactive terminals for v1?
- **Cache policy:** Should `agents_terminal_view_cache` be split into hot/warm/cold states, or is a simple LRU + active guard enough?
- **Diff state clearing:** Should `hide_column` fully clear fields directly, or should `Column` expose a `drop_loaded_data()` helper to avoid future partial clears?
- **IPC backpressure:** Is `sync_channel` acceptable in the current thread model, or should this move to a small custom bounded queue with non-blocking `try_send`?
- **User messaging:** Should scrollback trim be invisible, logged, or indicated in-terminal with a small system line?
- **Config shape:** Should memory profiles be user-configurable in `paneflow.json` now, or hardcoded conservative defaults first with later config if requested?

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Hidden Review retained data | Hidden columns may retain display rows/review terminals | 0 display rows/review terminals retained after hide | Month-1 | Unit/helper test + code inspection |
| Agents diff close retention | Closed dock keeps cached model for fast reopen | Cache cleared on close or TTL bounded | Month-1 | Unit/manual close-reopen |
| Agent scrollback default | 10 000 lines like normal terminal | <= 4 000 lines for agent profile | Month-1 | Unit test profile resolver |
| Review scrollback default | 10 000 lines like normal terminal | <= 2 000 lines for review profile | Month-1 | Unit test profile resolver |
| Sessions sidebar retention | Full result Vecs while open | <= 100 sessions per source/cwd | Month-1 | Unit test |
| IPC request queue | Unbounded `mpsc::channel()` | <= 256 pending requests | Month-1 | Unit test overload |
| Active agent eviction | No explicit cache policy | 0 active agents killed in smoke | Month-1 | Manual smoke |
| Long-session stability | Not measured for this issue | No unbounded growth from listed structures in 30-min smoke | Month-1 | Local smoke note, not benchmark |

## Open Questions

- Engineering: can live Alacritty scrollback be trimmed safely without reconstructing the terminal?
- Product: should users be able to configure agent/review scrollback profiles immediately, or should v1 ship sane defaults only?
- Engineering: should overload errors be exposed through the existing IPC error envelope or a new code?
- QA: which OSes can be verified locally before PR, and which should be called out as not verified?
[/PRD]
