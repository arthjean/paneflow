[PRD]
# PRD: Panes qui se relancent seules au redémarrage

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-30 | Arthur Jean | PRD initial : admission de lancement avec attente côté host, refus de capacité traités comme passagers côté desktop, attente d'une session encore en démarrage, Retry qui rejoue l'intention, relance automatique unique des panes perdues ou inconnues à la restauration, validation Windows sur matériel réel. 4 epics, 11 stories. |

## Problem Statement

**Convention de référence.** Les chemins désignent Paneflow au commit `58bf6819e`. Le diagnostic vient de l'audit vérifié du 2026-10-30 : lecture du code, puis boucle de reproduction déterministe contre un vrai `paneflow-host` sous Windows (test jetable `restart_capacity_probe.rs`, preuve locale non suivie par git, dans le scratchpad de la session).

Un utilisateur Windows signale qu'au redémarrage de Paneflow, des panes affichent souvent « The local host refused the request: host error -32000: the host is at capacity: … » avec un bouton Retry.

1. **Le host refuse les lancements au-delà de 8 au lieu de les mettre en file.**
   - Après « Stop everything and quit », et après **chaque** redémarrage de mise à jour, l'app enregistre `session.json` via `forget_terminal_sessions` (`src-app/src/app/session.rs:944`, appelé à `:147`). Cette fonction efface l'id de session de chaque terminal. Quand des sessions tournent, le dialogue de mise à jour ne propose que « Cancel » ou « Stop everything and restart » (`src-app/src/app/quit_dialog.rs:187-191`).
   - Au démarrage suivant, `restore_workspaces` construit toutes les panes de tous les onglets et workspaces d'un coup (`session.rs:543-569`, `:672-695`), et chaque pane sans id lance un `Create`.
   - Sous Windows, GPUI confie ces tâches bloquantes au pool de threads système (`TrySubmitThreadpoolCallback`, `gpui_windows/src/dispatcher.rs:68` de zed@37faef81), qui grossit : les N créations arrivent ensemble.
   - `admit_launch` (`crates/paneflow-host/src/host.rs:1251-1263`) refuse sans attendre dès que 8 lancements sont en attente (`MAX_PENDING_LAUNCHES`, `host.rs:39`).
   - Mesure : 12 créations simultanées donnent 4 refus, à chaque exécution, avec le message exact `8 launches are still unresolved`. Un lancement Windows dure 30 à 100 ms.
2. **Un refus de capacité laisse la pane morte.** Le desktop transforme l'erreur en `AttachOutcome::Unavailable` et ne réessaie jamais seul (`src-app/src/terminal/view/host_attach.rs:126-133`, `:171-176`). Le lien de sortie traite aussi un Busy comme définitif (`AttachRefused`, `src-app/src/terminal/ghostty_session/attached_runtime.rs:1005-1011`).
3. **Le bouton Retry ne répare pas la pane.**
   - Il relance toujours en `Reattach` (`host_attach.rs:227`). Or la création refusée n'a rien inscrit côté host (`host.rs:1186-1189`), donc la pane finit « missing » (`src-app/src/terminal/host_link.rs:543-545`) et il faut un second clic sur « New terminal ».
   - Le champ `session_intent` est écrit mais jamais lu (`src-app/src/terminal/view.rs:721`).
   - L'id refusé est quand même enregistré (`src-app/src/layout/serde.rs:46`), donc la pane revient « missing » à chaque démarrage suivant.
4. **D'autres états exigent un clic alors que rien n'est cassé.**
   - Une session encore en démarrage termine la pane en `Starting` avec Retry (`host_link.rs:535-536`, `:352-356`). Une création qui dépasse `STARTUP_DEADLINE` renvoie `LaunchPending` (`host.rs:1520-1525`).
   - Après un reboot de l'OS ou la mort du host, chaque pane hors agent revient `Lost` avec un bouton Restart (`host_link.rs:345-351`).
   - Ce comportement est un choix documenté : « The desktop never creates or restarts a process implicitly » (`ARCHITECTURE.md:644`).

Chaque clic sur Retry ou Restart pour une pane qui n'a rien de cassé donne une impression d'instabilité, sur un produit dont la promesse est de garder les agents en vie.

**Why now:** chaque mise à jour de Paneflow passe par « Stop everything and restart », donc tout utilisateur qui a plus de 8 terminaux rencontre l'erreur à chaque montée de version, et le rythme des releases est élevé. Le signalement vient d'un utilisateur réel, qui le décrit comme fréquent.

## Overview

Le correctif change un contrat plutôt qu'une constante. Une capacité dépassée fait **attendre**, elle ne fait plus mourir une pane.

- **EP-001, côté host.** `admit_launch` met la création en file, bornée par une deadline, au lieu de la refuser. Le plafond de 8 lancements en attente reste un garde-fou de ressources. Le seul Busy permanent (estimation de checkpoint au-dessus du budget, `crates/paneflow-host/src/host/staging.rs:39-43`) devient `CheckpointTooLarge`, si bien que `-32000` signifie toujours « réessaie ». Le déclencheur (« Stop everything » ou une mise à jour) relance toujours un host neuf, donc ce correctif est actif exactement quand il faut, sans changement de protocole.
- **EP-002, côté desktop.** Tout Busy est passager : nouvelles tentatives avec backoff exponentiel jitté, sous l'état d'attente existant (`HostLinkState::Attaching`, puis `Reconnecting`, qui s'affiche sans bouton). Une session en démarrage est attendue. Retry rejoue l'intention d'origine. Le desktop est la seule couche qui réessaie ; le host met en file sans réessayer.
- **EP-003, restauration.** À la restauration seulement, les panes `Lost` et `HostReplaced` sont relancées une fois, et les panes `Missing` reçoivent un shell neuf.
  - Ces relances ouvrent le shell enregistré, sans argument et sans rejouer de commande (`ARCHITECTURE.md:635`).
  - Les panes d'agent sont exclues, parce qu'elles se rétablissent déjà via la reprise de conversation (`src-app/src/terminal/view/conversation.rs:96-102`, `:415-420`).
  - `Exited` et `Failed` gardent leur bouton, ce qui exclut toute boucle de relance.
- **EP-004, en dernier comme pour chaque PRD.** Validation sur Windows réel.

**Décisions de conception :**
- **Pas de sémaphore côté desktop.** L'attente côté host protège tous les clients (desktop, CLI, MCP, Resume Ended Sessions), quelle que soit la version du desktop.
- **Pas de nouvel état d'interface.** Le jitter se calcule à partir d'un hash de l'id de session, sans nouvelle dépendance.
- **`Missing` est recréée automatiquement à la restauration.** Cela couvre aussi les ids refusés déjà enregistrés par les versions actuelles, sans toucher à la sérialisation du layout.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Aucune pane saine n'exige de clic après un redémarrage | 0 bouton Retry ou Restart affiché après un redémarrage de mise à jour avec 30 terminaux, sur Windows réel | 0 signalement d'erreur `-32000` au redémarrage |
| Aucun refus de capacité visible | 0 refus sur 12 et 40 créations simultanées dans le test d'intégration à host réel | Même résultat sur les 3 OS en CI à chaque release |
| Panes perdues rétablies sans action | 100 % des panes hors agent `Lost` relancées automatiquement après un reboot Windows | Idem sur Linux et macOS, vérifié en CI par test |
| Restauration rapide | Toutes les panes vivantes en 3 s au plus pour 30 terminaux après « Stop everything » | Idem pour 60 terminaux |

## Target Users

### Développeur multi-agents sous Windows
- **Role:** utilisateur de Paneflow qui garde plusieurs workspaces ouverts, avec des dizaines de terminaux et d'agents.
- **Behaviors:** met à jour Paneflow dès qu'une release sort, quitte parfois avec « Stop everything », redémarre son PC.
- **Pain points:** après une mise à jour, les panes au-delà de la 8e affichent une erreur rouge ; Retry les transforme en « missing », et il faut encore cliquer sur « New terminal ». Après un reboot, chaque terminal demande Restart.
- **Current workaround:** cliquer pane par pane, ou lancer « Resume Ended Sessions » workspace par workspace, ce qui peut lui-même dépasser les 8 lancements.
- **Success looks like:** l'app rouvre son espace de travail comme il l'a laissé, sans un seul clic.

### Utilisateur Linux ou macOS avec beaucoup de terminaux
- **Role:** même profil, sur une machine à 9 cœurs ou plus.
- **Behaviors:** le pool GPUI fixe (un thread par cœur, `gpui_linux/src/linux/dispatcher.rs:34`) étale moins la charge sur une machine à beaucoup de cœurs.
- **Pain points:** le même refus apparaît, moins souvent, dès que plus de 8 créations se chevauchent.
- **Current workaround:** aucun, sinon cliquer.
- **Success looks like:** même comportement sans clic que sous Windows.

### Outils qui pilotent le host (CLI `paneflow`, MCP, conducteur d'agents)
- **Role:** scripts et agents qui créent des sessions en rafale.
- **Behaviors:** appellent `session.create` ou `session.restart` en parallèle.
- **Pain points:** un refus `-32000` immédiat qu'il faut réessayer soi-même.
- **Current workaround:** boucles de retry ad hoc.
- **Success looks like:** une création patiente jusqu'à la deadline au lieu d'échouer.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **VS Code :** reconnecte le pty existant au rechargement de fenêtre, et relance le processus au redémarrage de l'app (`terminal.integrated.persistentSessionReviveProcess`, défaut `onExit`, [docs](https://code.visualstudio.com/docs/terminal/advanced)). Paneflow suit la même séparation : réattacher une session vivante, relancer une session perdue.
- **Windows Terminal :** `persistedWindowLayout` restaure le layout et le cwd avec un shell neuf par pane, sans contenu ([MS Learn](https://learn.microsoft.com/en-us/windows/terminal/customize-settings/startup)). Paneflow fait mieux pour les sessions encore vivantes, puisque le host les garde.
- **tmux-resurrect et Zellij :** ne relancent pas automatiquement les commandes. tmux-resurrect se limite à une liste blanche, et Zellij affiche « Press ENTER to run » pour éviter un `rm -rf` accidentel ([tmux-resurrect](https://github.com/tmux-plugins/tmux-resurrect/blob/master/docs/restoring_programs.md), [Zellij](https://zellij.dev/documentation/session-resurrection.html)). Paneflow ne relance que le shell, sans commande.
- **Market gap :** restaurer sans aucun clic un espace multi-agents après une mise à jour ou un reboot, en gardant les sessions vivantes quand le host survit.

### Best Practices Applied
- **Une seule couche de retry, avec jitter et backoff plafonné.** Sources : [Google SRE, Addressing Cascading Failures](https://sre.google/sre-book/addressing-cascading-failures/) et [AWS Builders' Library](https://builder.aws.com/content/3EumjoZascWd1oZiEgL8ORlv3qE/timeouts-retries-and-backoff-with-jitter). Le host met en file, et seul le desktop réessaie.
- **Files bornées avec deadline, plutôt qu'un refus immédiat ou une file infinie** (SRE). L'attente côté host est bornée et ne dépasse jamais la deadline du client.
- **Un code de surcharge distinct, et jamais de retry d'une erreur permanente** ([gRPC status codes](https://grpc.github.io/grpc/core/md_doc_statuscodes.html)). `-32000` devient strictement passager.
- **Une limite anti-boucle pour un processus qui plante au démarrage**, sur le modèle de Kubernetes CrashLoopBackOff (non vérifié sur la page source). Ici, au plus une relance automatique par pane et par lancement de l'app.

## Assumptions & Constraints

### Assumptions (to validate)
- **8 lancements en parallèle absorbent une restauration de 60 terminaux dans la deadline.** Base : 30 à 100 ms par lancement mesurés par la boucle de repro sur la machine d'Arthur. Non mesuré sur un Windows à froid avec antivirus actif (US-011).
- **Les relances automatiques ne dupliquent pas de shell.** `session.restart` est transactionnel par génération (`ARCHITECTURE.md:658-666`), et une relance perdante renvoie `GENERATION_MISMATCH`, que `resolve_at` réinspecte (`host_link.rs:560-569`).
- **La variante du message d'Olivier est bien celle du plafond de lancements.** Sa capture est tronquée après « the ». Le staging des checkpoints exigerait environ 250 réattaches simultanées en release pour saturer, mais ce n'est pas formellement exclu sur sa machine (Open Questions).
- **Polling plutôt qu'événement.** Aucun RPC n'annonce la résolution d'un lancement, et l'exposition de `AgentSubscription` sur le fil n'est pas vérifiée (`host.rs:577`).

### Hard Constraints
- **Multiplateforme.** Linux, macOS et Windows. Le code `cfg(windows)` n'est linté que par le job `Windows x86_64 libghostty check`.
- **Le thread de rendu ne bloque jamais.** Les attentes côté desktop passent par des timers asynchrones (`cx.background_executor().timer`, comme `hosted_sessions.rs:354-363`), jamais par `thread::sleep` sur le pool GPUI.
- **Aucun changement de protocole.** `HOST_PROTOCOL_VERSION` reste à 1 (`crates/paneflow-ipc-client/src/host_control.rs:12`). Un vieux desktop avec un nouveau host fonctionne, et l'inverse aussi, en mode dégradé.
- **Aucune nouvelle dépendance**, et aucun commentaire dans le code (`AGENTS.md`).
- **Ordre des items.** Tout item ajouté dans un fichier qui contient un `mod tests` est déclaré avant ce module.
- **Sécurité des relances.** Une relance automatique ne rejoue jamais de commande ni d'entrée, et ne persiste aucune variable d'environnement.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre legs
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, y compris les modules de test
- `cargo test --workspace --locked` - tests unitaires et d'intégration, dont les tests à host réel de `paneflow-host` et de `host_link.rs`

For the epic merge:
- Le workflow `run_tests.yml` est vert, y compris `Windows x86_64 libghostty check`, qui linte et teste le code `cfg(windows)`

For UI stories (US-004, US-005, US-008, US-009), additional gates:
- Revue visuelle par Arthur de l'état d'attente et de l'absence de bouton, sur un build debug lancé par lui

## Epics & User Stories

### EP-001: Le host met les lancements en file au lieu de les refuser

Une rafale de créations ou de relances ne produit plus de refus `-32000` tant que les lancements progressent. `-32000` devient strictement passager.

**Definition of Done:**
- 40 créations simultanées contre un vrai host aboutissent toutes, sans jamais dépasser 8 lancements en attente.
- Une réponse à `session.create` ou `session.restart` part toujours en moins de 9 s.
- Plus aucun chemin du host ne renvoie `-32000` pour une condition permanente.

#### US-001: Mettre l'admission des lancements en file avec une deadline
**Description:** As a client du host (desktop, CLI, MCP), I want qu'une création ou une relance au-delà de 8 lancements en attente patiente jusqu'à ce qu'une place se libère so that une rafale de restauration n'échoue plus.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given 8 lancements en attente, when un 9e `create` arrive, then il attend qu'un lancement soit résolu, puis démarre, sans renvoyer `HostError::Busy`
- [ ] Given des lancements en attente, when l'un d'eux est validé, échoue, est annulé, ou que son enregistrement est supprimé, then au moins un appel en attente est réveillé. Les points concernés à `58bf6819e` sont `host.rs:736`, `:1225`, `:1412`, `:1669`, `:1742` et `:1911`
- [ ] Given 12 créations simultanées avec un délai de lancement injecté par un hook `cfg(test)`, when elles se terminent, then les 12 sessions sont vivantes, et le nombre de lancements en attente n'a jamais dépassé 8
- [ ] Given 8 lancements qui ne se résolvent jamais, when un 9e `create` attend, then il renvoie `HostError::Busy` à la fin de sa deadline d'admission, et aucun enregistrement de session n'est créé pour lui
- [ ] Given un appel en attente, when le host commence à s'arrêter (`shutdown_until`), then l'appel se réveille et renvoie `HostError::ShuttingDown`
- [ ] Given un `create` admis après une attente, when on mesure le délai entre la requête et l'écriture de sa réponse (succès, Busy ou `LaunchPending`), then il est au plus de 9 s, sous les 10 s de `REQUEST_DEADLINE` (`protocol.rs:19`)
- [ ] Given `session.restart` avec 8 lancements en attente, when il arrive, then il suit la même file que `create`
- [ ] Given une session de même id créée pendant l'attente, when l'appel en attente est réveillé, then il renvoie `SessionExists` sans lancer de processus
- [ ] Given le test `admission_stops_at_eight_unresolved_launches` (`host/tests.rs:1599`), when la story est terminée, then il est remplacé par des tests qui décrivent le nouveau contrat, et aucun autre test du host ne régresse

#### US-002: Rendre le code -32000 strictement passager
**Description:** As a desktop qui réessaie automatiquement les refus de capacité, I want que le host ne renvoie jamais `-32000` pour une condition permanente so that je ne réessaie pas en vain pendant toute la durée de mon budget.

**Priority:** P0
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given une session dont l'estimation de snapshot dépasse `MAX_CHECKPOINT_BYTES`, when un client demande `session.attach`, then le host renvoie `ERR_CHECKPOINT_TOO_LARGE` avant toute admission au staging, et jamais `ERR_BUSY`
- [ ] Given une estimation comprise entre `MAX_CHECKPOINT_BYTES` et le budget de staging, when l'attache est demandée, then le comportement est le même que ci-dessus
- [ ] Given les chemins qui produisent `ERR_BUSY` (`server.rs:290`, `:306`, `host.rs:1260`, `staging.rs:61`), when on les relit après la story, then chacun correspond à une capacité qui se libère d'elle-même
- [ ] Given le test de staging `host/tests.rs:180-202`, when il s'exécute, then il vérifie l'erreur `CheckpointTooLarge` pour une estimation hors budget

#### US-003: Verrouiller la restauration en rafale par un test à host réel
**Description:** As a mainteneur de Paneflow, I want un test d'intégration qui rejoue une restauration en rafale contre un vrai `paneflow-host` so that une régression du plafond de lancements soit détectée en CI sur les 3 OS.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un `PANEFLOW_HOME` temporaire et un host lancé comme dans `crates/paneflow-host/tests/ownership_probes.rs`, when 40 clients font en même temps les appels de `resolve_at` en `Create` (connexion, `session.inspect`, `session.create`), then aucun ne reçoit `-32000`
- [ ] Given 16 sessions dont l'historique est rempli au plafond de scrollback, when 16 clients se réattachent en même temps, then aucun ne reçoit `-32000`
- [ ] Given le test, when il tourne sur le job Linux de la CI, then il dure moins de 30 s
- [ ] Given la fin du test, when on liste les processus, then aucun `paneflow-host` lancé par le test ne survit, même si une assertion a échoué
- [ ] Given un host dont l'admission refuserait sans attendre (comportement de `58bf6819e`), when le test s'exécute, then il échoue avec le nombre de refus observés

---

### EP-002: Le desktop traite un refus de capacité comme passager

Aucun refus de capacité ni démarrage en cours n'affiche de bouton. La pane attend sous l'état existant, puis s'attache. Retry et Restart ne servent qu'à une vraie panne.

**Definition of Done:**
- Un desktop face à un host qui refuse 4 créations sur 12 (comportement de `58bf6819e`) finit avec 12 panes vivantes sans action.
- Une session encore en démarrage est attendue.
- Retry rejoue l'intention d'origine.

#### US-004: Réessayer un Busy au premier attachement, avec backoff jitté
**Description:** As a utilisateur qui redémarre Paneflow, I want qu'une pane refusée pour capacité réessaie d'elle-même so that elle se lance sans que je clique.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `host_link::resolve` qui échoue avec `-32000`, when la tâche de `begin_hosted_attach` (`host_attach.rs:94-138`) reçoit l'erreur, then elle attend puis relance `resolve` avec la même intention et le même id, au lieu de produire `AttachOutcome::Unavailable`
- [ ] Given la tentative n (n à partir de 0), when on calcule le délai, then il est tiré dans [b/2, b], avec b = min(250 ms × 2^n, 4 s). Le tirage est déterministe à partir d'un hash de l'id de session et de n, sans nouvelle dépendance
- [ ] Given des refus répétés, when 60 s se sont écoulées depuis le premier refus, then la pane passe en `Unavailable` avec le message d'erreur, comme aujourd'hui
- [ ] Given une attente en cours, when on mesure le temps écoulé, then la pane reste en `HostLinkState::Attaching` pendant la première seconde, puis passe en `HostLinkState::Reconnecting`, qui s'affiche sans bouton (`host_attach.rs:340-351`)
- [ ] Given une attente en cours, when la vue est fermée ou qu'un nouvel attachement de la même vue démarre, then la boucle s'arrête sans autre appel au host
- [ ] Given une erreur autre que `-32000` (host injoignable, incompatible, `CheckpointTooLarge`), when elle arrive, then le comportement actuel est inchangé et aucune nouvelle tentative n'a lieu
- [ ] Given l'attente entre deux tentatives, when on inspecte le code, then elle passe par un timer asynchrone et aucun `thread::sleep` ne s'exécute sur le pool GPUI
- [ ] Given le test à host réel de `host_link.rs` (vers `:1330-1455`), when les deux slots de staging sont tenus par `host.checkpoint()` puis libérés après 500 ms, then le resolve concerné finit `Attached`

#### US-005: Attendre une session encore en démarrage
**Description:** As a utilisateur dont une pane retrouve une session en cours de lancement, I want que la pane attende la fin du lancement so that elle s'attache seule au lieu d'afficher Retry.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given une session dont `session.inspect` indique `pending_launch` (`host_link.rs:535-536`), when la pane la résout, then elle réinspecte avec le même backoff que US-004 jusqu'à ce que la session soit vivante ou terminée, au lieu de finir en `Starting`
- [ ] Given un `create` ou un `restart` qui renvoie `LaunchPending` (`host.rs:1520-1525`), when le desktop le reçoit, then il suit la même attente
- [ ] Given un lancement qui finit en échec pendant l'attente, when la pane le constate, then elle affiche l'état terminé correspondant (`Failed` ou `Exited`) avec son bouton habituel
- [ ] Given une attente de plus de 60 s, when le budget est épuisé, then la pane affiche `Starting` avec Retry, comme aujourd'hui
- [ ] Given l'attente, when on mesure l'interface, then elle suit la même règle `Attaching` puis `Reconnecting` que US-004

#### US-006: Reconnecter le lien de sortie sur un Busy
**Description:** As a utilisateur dont une pane perd son lien de sortie pendant une forte charge, I want que le lien se reconnecte sur un refus de capacité so that la pane ne finisse pas en `AttachRefused`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given la boucle du follower (`attached_runtime.rs:960-1011`), when un appel renvoie `-32000`, then elle passe par `lose_link` et `ReconnectBackoff` comme pour une perte de connexion, au lieu de `HostLinkEnd::attach_refused`
- [ ] Given des refus répétés, when le backoff est épuisé (`backoff.wait` renvoie `false`), then le lien se termine comme aujourd'hui
- [ ] Given `HostClientError`, when on inspecte `crates/paneflow-host/src/client.rs`, then un prédicat unique identifie un refus de capacité, et US-004, US-005 et cette story l'utilisent tous
- [ ] Given le test à host réel d'`attached_runtime.rs`, when les deux slots de staging sont tenus pendant qu'un checkpoint de reprise est demandé, then le lien passe par `Reconnecting` puis revient `Attached`

#### US-007: Faire rejouer à Retry l'intention d'origine
**Description:** As a utilisateur qui clique Retry après une vraie panne, I want que la pane rejoue ce qu'elle tentait (créer, réattacher ou relancer) so that un seul clic suffise.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given une pane dont le `Create` a fini en `Unavailable`, when l'utilisateur clique Retry, then la pane relance un `Create` avec le même id et s'attache, sans passer par « missing »
- [ ] Given une pane restaurée dont le `Reattach` a fini en `Unavailable`, when l'utilisateur clique Retry, then la pane relance un `Reattach`
- [ ] Given `TerminalView::open` (`view.rs:547-568`), when il démarre un attachement, then l'intention de départ est enregistrée et lue par `resume_hosted_session` (`host_attach.rs:216-231`)
- [ ] Given un Retry `Create` alors que la session existe déjà et vit (la première tentative avait abouti côté host), when il est résolu, then la pane s'attache à cette session sans en créer une seconde (`host_link.rs:534`)

---

### EP-003: La restauration rétablit seule les panes perdues ou inconnues

Après un reboot ou la mort du host, les terminaux hors agent reviennent avec un shell neuf dans leur cwd, sans clic. Les panes d'agent gardent leur reprise de conversation. Les panes terminées par l'utilisateur ou en échec gardent leur bouton.

**Definition of Done:**
- À la restauration, toute pane hors agent `Lost`, `HostReplaced` ou `Missing` est vivante sans action.
- Aucune pane n'est relancée deux fois.
- `ARCHITECTURE.md` décrit le nouveau contrat.

#### US-008: Relancer une fois les panes Lost et HostReplaced à la restauration
**Description:** As a utilisateur qui rouvre Paneflow après un reboot, I want que mes terminaux perdus repartent avec un shell neuf dans leur dossier so that je retrouve mon espace sans cliquer sur Restart.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given une pane restaurée par `restore_workspaces` dont la résolution finit `Ended` avec `Lost` ou `HostReplaced`, when l'issue arrive (`host_attach.rs:155-170`), then la pane lance `SessionIntent::Restart` avec la génération observée, sans action de l'utilisateur
- [ ] Given une pane d'agent pour laquelle `conversation_restore_pending()` (`conversation.rs:380`) est vrai, when elle finit `Lost`, then aucune relance automatique n'a lieu, et la reprise de conversation existante reste la seule voie
- [ ] Given une pane relancée automatiquement, when son nouveau shell finit `Exited` ou `Failed`, then elle affiche son bouton Restart et n'est plus relancée automatiquement pendant ce lancement de l'app
- [ ] Given une pane `Exited` ou `Failed` à la restauration, when elle est résolue, then elle garde son bouton Restart, comme aujourd'hui
- [ ] Given une pane qui finit `Lost` en cours de session (et non à la restauration), when l'issue arrive, then aucune relance automatique n'a lieu
- [ ] Given deux relances concurrentes de la même génération, when elles se résolvent, then une seule nouvelle génération est validée et la pane s'attache à elle
- [ ] Given une relance dont le cwd enregistré n'existe plus, when le host répond, then la pane affiche l'échec avec son bouton, sans nouvelle tentative automatique
- [ ] Given la relance, when on inspecte la requête envoyée au host, then elle ne contient aucune commande, aucune entrée à rejouer et aucune variable d'environnement persistée

#### US-009: Créer un shell neuf pour une pane Missing à la restauration
**Description:** As a utilisateur dont une pane restaurée désigne une session que le host ne connaît pas, I want qu'elle reçoive un shell neuf so that elle ne reste pas bloquée sur « New terminal » à chaque démarrage.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given une pane hors agent restaurée dont la résolution finit `Missing` (`host_link.rs:543-545`), when l'issue arrive, then la pane lance `SessionIntent::Create` avec une nouvelle identité, sans action de l'utilisateur
- [ ] Given un `session.json` produit par `58bf6819e`, contenant l'id d'une création refusée, when Paneflow démarre, then cette pane est vivante sans clic
- [ ] Given une pane d'agent `Missing`, when elle est résolue, then seule la reprise de conversation existante s'applique (`conversation.rs:96-102`)
- [ ] Given une pane `Missing` en cours de session (et non à la restauration), when l'issue arrive, then le bouton « New terminal » s'affiche, comme aujourd'hui
- [ ] Given une création automatique qui échoue, when le résultat arrive, then la pane affiche l'erreur avec son bouton, sans nouvelle tentative automatique au-delà du budget de US-004

#### US-010: Documenter le contrat de restauration
**Description:** As a contributeur ou agent qui lit l'architecture, I want que la documentation décrive quand le desktop attend, réessaie, relance ou crée une session so that personne ne réintroduise un refus immédiat ou un clic obligatoire.

**Priority:** P1
**Size:** XS (1 pt)
**Dependencies:** Blocked by US-001, US-004, US-008, US-009

**Acceptance Criteria:**
- [ ] Given `ARCHITECTURE.md`, when on lit la phrase « The desktop never creates or restarts a process implicitly » (`:644`), then elle est remplacée par la règle exacte : relance unique des panes hors agent `Lost` et `HostReplaced`, création pour `Missing`, à la restauration seulement, sans rejouer de commande
- [ ] Given la phrase « Admission is bounded at eight unresolved launches », when on la relit, then elle décrit l'attente bornée et le fait que `-32000` est toujours passager
- [ ] Given la table « Host lifetime » (`:626-636`), when on lit la ligne « Host process death or reboot » (`:634`), then elle indique que les panes hors agent reviennent avec un shell neuf
- [ ] Given la page utilisateur `blog/keep-coding-agents-running.md` (`docs/user/`, miroir généré), when elle est corrigée, then la modification est faite dans le dépôt source paneflow-web puis resynchronisée, jamais à la main dans `docs/user/`
- [ ] Given un dépôt paneflow-web indisponible pendant la story, when elle se termine, then le rapport liste la correction utilisateur comme suite à faire, et la story reste IN_REVIEW

---

### EP-004: Valider sur Windows réel

Le scénario du signalement est rejoué et mesuré sur le matériel d'Arthur, avec un build release, et avec un build debug pour la mise à jour factice.

**Definition of Done:**
- Les trois scénarios passent sans un clic.
- Les durées sont mesurées et consignées.
- Le rapport distingue ce qui a été vu sur matériel de ce qui a été vérifié par lecture.

#### US-011: Passe Windows sur la mise à jour, l'arrêt complet et le reboot
**Description:** As a mainteneur de Paneflow, I want rejouer le signalement sur Windows réel avec un build release so that le correctif soit prouvé là où il a été observé.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003, US-007, US-009

**Acceptance Criteria:**
- [ ] Given 30 terminaux répartis sur 3 workspaces, dont 4 panes d'agent, when Arthur fait « Stop everything and quit » puis relance, then toutes les panes sont vivantes sans clic, et le délai jusqu'à la dernière pane vivante est consigné, avec une cible de 3 s au plus
- [ ] Given la même configuration sur un build debug lancé avec `PANEFLOW_DEV_FORCE_UPDATE=9.9.9`, when Arthur choisit « Stop everything and restart » dans le dialogue de mise à jour, then aucune pane n'affiche Retry, Restart ni « New terminal » au redémarrage. Si la mise à jour factice ne redémarre pas l'app, le rapport le dit et s'appuie sur le premier scénario, qui passe par le même `forget_terminal_sessions`
- [ ] Given un reboot de Windows, when Arthur relance Paneflow, then les terminaux hors agent reviennent avec un shell neuf dans leur cwd, les panes d'agent proposent leur reprise de conversation, et une pane fermée volontairement avant le reboot garde son bouton
- [ ] Given 60 terminaux, when on rejoue le premier scénario, then aucune pane n'affiche d'erreur. Sinon, le nombre de panes en échec et le message sont consignés et la story reste IN_REVIEW
- [ ] Given le build desktop de cette branche face à un host de `58bf6819e` encore en cours d'exécution, when Arthur restaure 12 panes à créer, then toutes finissent vivantes grâce aux nouvelles tentatives de US-004
- [ ] Given le rapport, when on le lit, then il précise pour chaque scénario s'il a été vérifié sur matériel ou par lecture du code `cfg(windows)`

---

## Functional Requirements

- FR-01 : le host doit mettre en file toute création ou relance qui dépasse 8 lancements en attente, jusqu'à une deadline, au lieu de la refuser immédiatement.
- FR-02 : le host ne doit renvoyer `-32000` que pour une capacité qui se libère d'elle-même.
- FR-03 : le desktop doit réessayer automatiquement, avec backoff exponentiel jitté, tout `-32000` reçu au premier attachement ou sur le lien de sortie, pendant 60 s au plus.
- FR-04 : le desktop doit attendre une session en démarrage au lieu d'afficher un bouton.
- FR-05 : Retry doit rejouer l'intention d'origine de la pane.
- FR-06 : à la restauration, le desktop doit relancer une fois les panes hors agent `Lost` ou `HostReplaced`, et créer un shell neuf pour les panes hors agent `Missing`.
- FR-07 : le desktop ne doit PAS relancer automatiquement une pane `Exited` ou `Failed`, ni une pane qui se termine en cours de session.
- FR-08 : une relance automatique ne doit PAS rejouer de commande, d'entrée ni de variable d'environnement.
- FR-09 : aucune attente côté desktop ne doit bloquer le thread de rendu ni un thread du pool GPUI.

## Non-Functional Requirements

- **Performance :** 30 terminaux à créer sont tous vivants en 3 s au plus après le démarrage sur la machine Windows d'Arthur, en build release. Une réponse à `session.create` part en 9 s au plus dans tous les cas.
- **Reliability :**
  - 0 refus `-32000` visible pour 60 terminaux restaurés au plus.
  - Le budget de nouvelles tentatives est de 60 s par pane.
  - Une relance automatique au plus par pane et par lancement de l'app.
  - Au plus 8 lancements en attente en même temps côté host.
- **Scalability :** le test d'intégration couvre 40 créations simultanées. Au-delà d'environ 60 panes attachées, le plafond de 128 connexions du host (`server.rs:34-36`) reste la limite connue (Non-Goals).
- **Security :** une relance n'exécute que le shell enregistré sans argument (`ARCHITECTURE.md:635`), sans commande ni entrée rejouée et sans environnement persisté.
- **Compatibility :**
  - `HOST_PROTOCOL_VERSION` reste à 1.
  - Le nouveau desktop face à l'ancien host restaure 12 panes sans clic (US-011).
  - L'ancien desktop face au nouveau host ne reçoit plus de refus de lancement.
- **Accessibility :** l'état d'attente réutilise la barre `Reconnecting` existante : 0 nouvel élément interactif, 0 nouveau libellé à traduire.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Attente de capacité courte | Plus de 8 créations simultanées | La pane reste en `Attaching` puis s'attache | Aucun |
| 2 | Attente de capacité longue | Lancements lents (antivirus, disque froid) au-delà d'1 s | Barre `Reconnecting` sans bouton | « Reconnecting to the local host » |
| 3 | Budget épuisé | Host qui refuse pendant 60 s | `Unavailable` avec Retry, qui rejoue l'intention | « The local host refused the request: … » |
| 4 | Session en démarrage | `pending_launch` ou `LaunchPending` | Réinspection avec backoff jusqu'à vivante ou terminée | Aucun, puis la barre `Reconnecting` après 1 s |
| 5 | Shell qui plante après relance | Profil cassé, cwd supprimé | `Exited` ou `Failed` avec Restart, sans nouvelle relance | Message de fin habituel |
| 6 | Pane d'agent perdue | Reboot avec un agent enregistré | Reprise de conversation existante uniquement | Bannière de reprise existante |
| 7 | Fermeture pendant l'attente | L'utilisateur ferme la pane ou quitte l'app | La boucle s'arrête, sans appel au host | Aucun |
| 8 | Arrêt du host pendant l'attente | `shutdown_until` | Les appels en file renvoient `ShuttingDown` | Comportement actuel de fin |
| 9 | Valeur limite | 8, 9, 40 et 60 créations simultanées | 0 refus visible ; au plus 8 lancements en attente | Aucun |
| 10 | Relances concurrentes | Restauration et « Resume Ended Sessions » en même temps | Une seule génération validée, la pane s'attache à elle | Aucun |
| 11 | Ancien host encore actif | Desktop mis à jour, host de `58bf6819e` | Les nouvelles tentatives de US-004 absorbent les refus | Barre `Reconnecting` le cas échéant |
| 12 | Checkpoint hors budget | Historique au-delà de 64 MiB | `CheckpointTooLarge`, sans nouvelle tentative | Message actuel d'attache refusée |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Une notification oubliée laisse des appels en file jusqu'à leur deadline | Med | Med | Dans le pire cas, on retombe sur le comportement actuel (Busy à la deadline). Le test de US-001 couvre chaque point qui libère un lancement |
| 2 | Lancements Windows à froid assez lents pour dépasser la deadline d'admission | Low | Med | Les nouvelles tentatives côté desktop (US-004) prennent le relais ; mesure en US-011 |
| 3 | Relance automatique en course avec la reprise de conversation d'un agent | Med | High | Exclusion par `conversation_restore_pending()` (US-008), avec un test dédié |
| 4 | Boucle de relance d'un shell qui plante | Low | Med | Relance unique par pane et par lancement ; `Exited` et `Failed` ne sont jamais relancées automatiquement |
| 5 | La capture tronquée d'Olivier désignait le staging des checkpoints | Low | Med | US-002 et US-004 couvrent aussi ce Busy ; confirmation demandée (Open Questions) |
| 6 | Tempête de nouvelles tentatives en phase | Low | Low | Jitter par session et plafond à 4 s ; le host met en file au lieu de refuser |
| 7 | Plafond de 128 connexions dépassé par une très grande configuration | Low | High | Hors périmètre ; limite consignée en Non-Goals et à surveiller en US-011 avec 60 panes |

## Non-Goals

- **Garder les sessions vivantes pendant une mise à jour.** L'installeur MSI tue le host (`packaging/wix/main.wxs:233-244`), et les agents en cours verrouillent leurs shims. C'est une fonctionnalité produit séparée, et elle ne supprimerait pas « Stop everything ».
- **Attache « viewport-first ».** Le snapshot libghostty sépare déjà l'écran et l'historique (marqueur READY, `snapshot.h:27-30`). Le staging n'est pas saturé dans les mesures ; à rouvrir sur preuve.
- **Plafond de connexions du host.** Environ 60 panes attachées avec 2 connexions chacune. À traiter dans un PRD dédié si US-011 l'atteint.
- **Réglage pour désactiver la relance automatique** (équivalent de `persistentSessionReviveProcess: never`). Aucun besoin exprimé ; à reconsidérer sur demande.
- **Relancer des commandes ou des agents** au-delà de la reprise de conversation existante.
- **Sémaphore de restauration côté desktop**, ou hausse de `MAX_PENDING_LAUNCHES`. La file côté host rend le premier inutile, et la seconde ne ferait que déplacer le seuil.
- **Nouvel événement de télémétrie.** Le jeu d'événements de `paneflow-telemetry` reste inchangé.

## Files NOT to Modify

- `native/libghostty/**`, `src-app/src/terminal/pty_session/session_backend.rs`, `src-app/src/terminal/types.rs` : frontière du moteur de terminal, sans lien avec la restauration.
- `crates/paneflow-shim/**`, `crates/paneflow-ai-hook/**`, `crates/paneflow-mcp/**` : binaires auxiliaires à taille plafonnée.
- `crates/paneflow-host/src/protocol.rs` (constantes de deadline et `HOST_PROTOCOL_VERSION`), `crates/paneflow-ipc-client/src/host_control.rs` : aucun changement de protocole.
- La transaction de lancement du host (`commit_launch`, `fail_launch`, `own_late_launch`, contrôles de génération) : seule l'admission change.
- `src-app/src/layout/serde.rs` : la sérialisation des ids reste inchangée ; US-009 couvre les ids orphelins.
- `packaging/**`, `.github/workflows/**` : aucun changement d'installeur ni de job.
- `docs/user/**` : miroir généré, modifié uniquement par resynchronisation depuis paneflow-web.

## Technical Considerations

- **Deadline d'admission :** quelle borne pour l'attente dans `admit_launch` ? Recommandé : 5 s, comme `CHECKPOINT_ADMISSION_DEADLINE`. L'attente du démarrage utilise ensuite le reste du budget, pour que la réponse parte en 9 s au plus, sous les 10 s de `REQUEST_DEADLINE` du client. Alternative : décompter l'attente de `STARTUP_DEADLINE`. L'ingénierie confirme que `LaunchPending` couvre bien le dépassement.
- **Primitive d'attente :** une `Condvar` sur le mutex `sessions` (`host.rs:206`), ou une `Condvar` dédiée aux lancements ? Recommandé : une `Condvar` dédiée associée au mutex `sessions`, sur le modèle de `settled` (`host.rs:64`). Ordre de réveil : `notify_all` suffit, puisque chaque appel réveillé recompte les lancements en attente. Une file FIFO n'est utile que si des famines apparaissent dans le test de US-003.
- **Jitter :** hash de `SessionId` et du numéro de tentative avec `std::hash`, ou `RandomState` ? Recommandé : un hash déterministe, qui rend les tests reproductibles. L'ingénierie confirme qu'aucun aléa cryptographique n'est requis.
- **Polling de `Starting` :** `session.inspect` en boucle, ou exposition d'un événement de cycle de vie sur le fil ? Recommandé : polling avec le backoff de US-004, sans changement de protocole. L'ingénierie vérifie si `AgentSubscription` (`host.rs:577`) transporte déjà les changements de cycle de vie.
- **Où relancer à la restauration :** dans le bras `AttachOutcome::Ended` (`host_attach.rs:155-170`), à côté de `conversation_host_ended` ? Ou par un passage groupé de type `resume_ended_panes` (`hosted_sessions.rs:870-891`) après la restauration ? Recommandé : le bras `Ended`, avec un drapeau « restauration » porté par la vue. Cela évite un second parcours et une course avec la reprise de conversation.
- **Prédicat de capacité :** où classer `-32000` ? Recommandé : une méthode sur `HostClientError` dans `crates/paneflow-host/src/client.rs`, à côté de `is_connection_loss` et `is_session_gone`.
- **Migration et rollback :** aucun changement de schéma. Un `git revert` des commits de l'epic restaure le comportement de `58bf6819e`. Un host et un desktop de versions différentes restent compatibles dans les deux sens.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Refus `-32000` sur 12 créations simultanées | 4 sur 12, à chaque exécution | 0 | Avant la fusion | Test à host réel de US-003 |
| Panes qui exigent un clic après un redémarrage de mise à jour, 30 terminaux | 22 au moins (toutes au-delà de la 8e) | 0 | Month-1 | US-011 sur Windows réel |
| Panes hors agent qui exigent Restart après un reboot | Toutes | 0 | Month-1 | US-011 sur Windows réel |
| Délai jusqu'à la dernière pane vivante, 30 terminaux | N/A (nouveau) | 3 s au plus | Month-1 | Chronométrage en US-011 |
| Signalements d'erreur `-32000` au redémarrage | Au moins 1 utilisateur, en récurrence | 0 | Month-6 | Issues GitHub et retours de la communauté |

## Open Questions

- **Variante du message d'Olivier :** le texte après « at capacity: » est-il « 8 launches are still unresolved » ? À demander par Arthur à Olivier avant US-011, avec la ligne `hosted session resolution failed:` du log desktop, le nombre de terminaux et le contexte (mise à jour ou « Stop everything »). Une variante de staging ne change pas le plan, mais elle justifierait de rouvrir l'attache « viewport-first ».
- **Libellé de l'état d'attente :** « Reconnecting to the local host » convient-il à une pane qui attend sa création ? À trancher par Arthur avant US-004 ; changer le libellé est un choix de design, pas une nouvelle story.
- **Matériel macOS :** la relance après la mort du host n'est vérifiée sur macOS que par les tests de CI. Une passe sur matériel est-elle souhaitée ? À décider par Arthur avant la release.
[/PRD]
