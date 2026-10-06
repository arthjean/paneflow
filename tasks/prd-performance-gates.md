[PRD]
# PRD: Gates de performance stricts, mesurés sur le travail et appliqués en CI

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-03 | Arthur Jean | PRD initial : instrumenter le travail des trois processus, corriger les sept régressions candidates relevées le 2026-10-03, puis verrouiller l'état corrigé par des gates déterministes en CI Linux, un A/B calibré, des baselines propres par plateforme et un protocole manuel sur matériel réel. 5 epics, 19 stories. |
| 1.1 | 2026-10-03 | Arthur Jean | Les références à pf portent le préfixe `pf/`, défini une fois dans Research Findings avec les deux clones locaux (`/home/arthur/dev/pf` sous Linux, `C:/dev/pf` sous Windows) et le commit `4049d49` qui fixe les numéros de ligne. |
| 1.2 | 2026-10-04 | Arthur Jean | US-006 : la latence p95 se mesure par l'aller-retour d'écho du scénario actif, car `terminal/perf_bench.rs` ne traverse pas le host et ne peut pas observer le thread de session. |
| 1.3 | 2026-10-06 | Arthur Jean | EP-004 après revue et premier run CI de l'A/B (37369016686) : l'A/A ne borne plus que les p50, et un p95 n'est jugé que si son propre A/A tient, car les p95 dérivent jusqu'à 55 % sur un code identique. Les compteurs d'instructions, déterministes, deviennent bloquants par leur propre variable après 10 runs CI ; le benchmark de mise en page sort du périmètre d'US-017 (crate binaire sans cible lib). La promotion du temps réel, qui exige 30 runs sur 3 semaines, passe dans US-020 (EP-006). 6 epics, 20 stories. |
| 1.4 | 2026-10-06 | Arthur Jean | EP-005 allégé après implémentation : un protocole complet à chaque version mineure, sur cinq versions et trois OS, ne tiendrait pas au rythme des versions correctives. Les baselines Windows deviennent opportunistes ; la première exécution se réduit à v0.17.5 contre `main` sous Fedora Wayland, dans deux états ; le runbook n'exige le protocole que pour un changement de rendu ; les sources GPU se limitent au matériel réel (nvidia-smi, amdgpu, GPU Engine). |
| 1.5 | 2026-10-06 | Arthur Jean | US-020 : la décision consultative peut être consignée dès que le critère ne peut plus être atteint sur les 30 premiers runs comptés, sans attendre le trentième. Les 11 premiers runs en contiennent déjà 2 dont la régression n'a pas été confirmée, et 4 non calibrés en tout, donc au mieux 26 calibrés sur 30 : 19 runs CI de plus ne changeraient pas l'issue. |
| 1.6 | 2026-10-06 | Arthur Jean | EP-007, deux causes trouvées par le premier run matériel (Risk 8) : les sondes diff-stat qui se relancent sur leurs propres lectures de `HEAD` et `index`, et le clignotement du curseur qui dessine ses propres frames dans une fenêtre focalisée. Le gate desktop mesure désormais la fenêtre focalisée. 7 epics, 22 stories. |
| 1.7 | 2026-10-06 | Arthur Jean | Revue d'EP-007 : le desktop du gate coupait le clignotement (US-013), si bien que la fenêtre focalisée ne pouvait pas clignoter et que `perf_gates` échouait sur `main`. Le desktop de `desktop_idle_and_thinking` tourne désormais clignotement activé, les autres desktops du harnais le gardent coupé. L'activer a révélé qu'un terminal se croyait focalisé dans une fenêtre inactive et y clignotait (56 rendus racine en 30 s) : un terminal n'est plus focalisé que si sa fenêtre est active. |

## Problem Statement

Les versions v0.16.0 à v0.17.5 ont dégradé les performances de Paneflow sans qu'aucun outil ne le signale. Le 2026-10-03, une exploration en lecture seule de Paneflow et de pf (clone local : `/home/arthur/dev/pf` sous Linux, `C:/dev/pf` sous Windows) a établi pourquoi.

1. **Aucune mesure ne peut échouer.** Les quatre suites de `bench/` affichent une comparaison mais ne renvoient jamais d'erreur sur un écart (`src-app/src/bench_harness.rs:410-525`). Le README le dit : « Neither suite runs in CI » (`bench/README.md:492`). Les seuls contrôles de performance en CI sont la taille des binaires d'aide (`.github/workflows/run_tests.yml:299-351`), le stress PTY sous Linux (`.github/workflows/libghostty-linux.yml:223-234`) et quelques latences larges en profil debug.
2. **Les baselines ne permettent aucune comparaison.**
   - Les quatre baselines viennent de Windows (`"cpu": "windows-x86_64"`), trois d'arbres sales.
   - La baseline éditeur date d'avant v0.12.0.
   - La baseline startup enregistre l'état d'avant optimisation (448 ms, puis 292 ms au run suivant du même commit), donc toute comparaison ressort comme un gain.
   - La baseline persistante est au schéma 2 alors que les runs écrivent le schéma 3, ce qui désactive la comparaison.
   - Aucun run n'existe après `7901ec37` (2026-09-26) : v0.17.1 à v0.17.5 n'ont jamais été mesurées.
3. **Les scénarios mesurés ne ressemblent pas à l'usage réel.** La suite persistante n'ouvre que des sessions `paneflow-session-fixture idle` : `NFR-08.throughput_ratio` vaut `null` dans tous les résultats. Or les régressions candidates grossissent avec le nombre d'agents qui produisent de la sortie.
4. **Sept régressions candidates, vérifiées dans le code mais jamais mesurées :**
   - **Badges d'onglet** (`65a9f385`, v0.17.1) : cinq `paint_path` par onglet, reconstruits à chaque frame (`src-app/src/pane.rs:225-247`, `src-app/src/ui_primitives/squircle.rs:47-70`). Ces frames sont entretenues par le spinner de la sidebar, une animation `repeat()` de 720 ms rendue à la cadence d'affichage tant qu'un agent réfléchit (`src-app/src/app/sidebar/lane.rs:212-243`).
   - **Listing complet des processus** (v0.17.0) : 500 ms après toute sortie, chaque session du host lit `/proc/<pid>/stat` pour tous les processus de la machine, sur son propre thread (`crates/paneflow-host/src/runtime.rs:26,1162-1164,1376-1381`, `crates/paneflow-host/src/process.rs:224-260`).
   - **Scan viewport** (`41f67ccc`, v0.17.0) : toutes les 500 ms, chaque session active formate l'écran et parcourt l'arbre de processus du premier plan (`crates/paneflow-host/src/viewport_scan.rs:357-410`, `crates/paneflow-host/src/runtime_observer.rs:78-92`).
   - **Snapshot du worker** (`c02e9ab8`, v0.17.0) : il diffuse un snapshot complet toutes les 2 s, même sans changement (`crates/paneflow-serve/src/worker.rs:272-294`). Le desktop répond par un `session.list` et deux `cx.notify()` racine (`src-app/src/app/host_agents.rs:533-552`, `src-app/src/app/hosted_sessions.rs:525-570`).
   - **Sondes git** (`bc180b42`, v0.17.5) : chaque sonde qui lit le worktree lance un `git config --get-regexp` de plus (`src-app/src/git_command.rs:134-178`), donc la sonde diff-stat passe de 3 à 5 processus.
   - **Démarrage Linux et macOS** : 140,3 ms fixes avant la première frame, sur le thread GPUI. Un socket périmé renvoie ECONNREFUSED, ce qui déclenche trois tentatives séparées par 70 ms (`src-app/src/ipc.rs:480-526`). Ce coût date de mai, pas des dernières versions, mais il représente environ 30 % des 461 ms mesurées sous Linux (`bench/results/startup-20260925T215458Z-3f75cd0b9328.json`).
   - **Diffusions par session** (`610e6fc6`, non publié) : chaque session qui imprime diffuse son entrée au worker toutes les 500 ms (`crates/paneflow-host/src/viewport_scan.rs:389`, `crates/paneflow-host/src/host.rs:866-871`).

**Why now:**
- **Fréquence de livraison** : cinq versions correctives sont sorties en six jours (v0.17.1 le 2026-09-26, v0.17.5 le 2026-10-01).
- **Coût en hausse** : `main` ajoute encore des diffusions (`610e6fc6`).
- **Charge à venir** : le PRD `prd-agents-browser` va ajouter une charge GPU.
- **Exécution par des agents** : les epics sont implémentées par des agents (`/implement-epic`), qui ont besoin d'un signal de vérification mécanique plutôt que d'un ressenti.

pf montre que des budgets stricts tenus en CI sont praticables. Ses limites montrent aussi ce qu'il ne faut pas copier :
- aucun check requis ;
- un budget de 2 ms tenu en temps réel sur un runner partagé alors que la mesure est déjà à 1,9-2,0 ms ;
- aucune mesure du temps CPU, des allocations ou des instructions.

## Overview

Le PRD procède en trois temps : mesurer, corriger, verrouiller.

**Mesurer.**
- Chaque processus (desktop, host, worker) compte, sur des compteurs monotones, le travail que les régressions ont multiplié :
  - frames dessinées ;
  - listings système des processus et parcours de l'arbre du premier plan ;
  - processus git lancés, par sorte ;
  - diffusions IPC par type ;
  - snapshots appliqués ;
  - appels `session.list`.
- Les compteurs sont compilés en release, sur les trois OS, et lus par les méthodes de statut existantes.
- La suite persistante gagne un scénario « agents actifs » qui utilise les fixtures `stream` et `flood` existantes.

**Corriger.** Chaque correctif joint sa mesure avant et après, prise avec ces compteurs.

**Verrouiller.** L'état corrigé devient un ensemble de budgets écrits dans le code (comme pf) et vérifiés par un job Linux bloquant :
- les compteurs du host et du worker, sans écran ;
- les compteurs du desktop sous un serveur X virtuel avec lavapipe ;
- les allocations des suites terminal et éditeur, que l'allocateur comptant rend déterministes.

Le temps réel n'est pas gaté en absolu. Un script A/B compare base et head construits dans le même job : rounds alternés, cohorte A/A de calibration, règle p50 et p95 ≤ +10 % reprise de pf. Il tourne d'abord en mode ombre. Il ne devient bloquant qu'après 30 runs sans échec parasite, la règle de promotion qui manquait aux projets dont les gates ont fini désactivés. Le comptage d'instructions par Gungraun reste optionnel (P2), après un spike.

Les baselines deviennent propres et propres à chaque plateforme. La comparaison entre OS est refusée, et un arbre sale ne peut plus devenir baseline. Le GPU et le temps de frame réels restent hors de la CI, faute de GPU fiable sur les runners partagés. Ils sont couverts par un protocole manuel court sur le matériel d'Arthur (Fedora Wayland, X11 et Windows 11 en dual boot documentés), exécuté quand une version touche le rendu et consigné dans `bench/results/`.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Régressions candidates mesurées puis corrigées, avec mesure avant/après jointe | 7/7 mesurées, ≥ 6 corrigées | 7/7 corrigées ou rejetées par la mesure |
| Budgets de travail vérifiés par un job CI bloquant sur `main` | ≥ 15 budgets (host, worker, desktop) | ≥ 30 budgets, couvrant chaque thread nommé du host |
| Échecs parasites des gates déterministes | 0 sur les 30 premiers runs | ≤ 1 pour 100 runs |
| Gate A/B en temps réel | En mode ombre, A/A vert sur ≥ 90 % des runs | Bloquant sur les PR qui touchent les chemins chauds |
| Baselines propres par plateforme | Linux à jour après EP-002 | Linux à jour à chaque version mineure, Windows à chaque passage en dual boot |

## Target Users

### Mainteneur (Arthur)
- **Role:** Développeur solo qui livre Paneflow et délègue l'implémentation des epics à des agents.
- **Behaviors:** Il découpe le travail en PRD, fait implémenter et certifier par `/implement-epic` et `/review-epic`, et publie des versions correctives en quelques jours.
- **Pain points:** Il a découvert les régressions au ressenti, après publication. Aucune mesure ne lui dit quelle version, quel commit ou quel thread coûte. Les baselines existantes ne se comparent pas entre elles.
- **Current workaround:** Des runs locaux ponctuels de `scripts/bench-*.sh`, sous Windows pour la plupart, comparés à l'œil.
- **Success looks like:** Une PR qui multiplie un travail récurrent échoue en CI avec le compteur, le budget, l'écart et la commande qui reproduit.

### Agent d'implémentation
- **Role:** Agent de code (Claude Code, Codex) qui implémente une story et doit prouver qu'elle ne régresse pas.
- **Behaviors:** Il lance les gates du PRD et lit leur sortie, sans accès à un écran ni à un GPU.
- **Pain points:** Les seuls critères de performance existants exigent une mesure manuelle ou un tableau à lire, sans verdict.
- **Current workaround:** Joindre une mesure avant/après faite à la main, quand l'environnement le permet.
- **Success looks like:** Un test `cargo test` ou un job CI qui rend un verdict binaire et explique l'écart.

### Utilisateur multi-agents
- **Role:** Développeur qui fait tourner 4 à 8 agents dans Paneflow, souvent sur portable.
- **Behaviors:** Plusieurs panes impriment en continu pendant que d'autres attendent une réponse.
- **Pain points:** Ventilateur et batterie sollicités par l'application elle-même, sidebar et onglets qui coûtent tant qu'un agent réfléchit.
- **Current workaround:** Fermer des panes, activer « Reduce motion ».
- **Success looks like:** Le coût de Paneflow croît avec la sortie réelle des agents, pas avec leur nombre ni avec le temps qui passe.

## Research Findings

Key findings that informed this PRD:

Les chemins préfixés `pf/` désignent le dépôt pf ([github.com/arthjean/pf](https://github.com/arthjean/pf)), cloné en local dans `/home/arthur/dev/pf` sous Linux et `C:/dev/pf` sous Windows. Leurs numéros de ligne correspondent au commit `4049d49` de pf.

### Competitive Context
- **rustc-perf** : compte les instructions sur une machine dédiée ; les cycles servent seulement de contrôle. Il ne bloque aucune fusion : un triage humain hebdomadaire classe les écarts ([triage](https://github.com/rust-lang/rustc-perf/blob/main/triage/2021-09-14.md)).
- **Firefox Perfherder et Chromium** : les alertes arrivent après la fusion, avec des sheriffs qui ont autorité pour annuler. Aucun gate en temps réel avant fusion ([Firefox](https://firefox-source-docs.mozilla.org/testing/perfdocs/perf-sheriffing.html), [Chromium](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/speed/bisects.md)).
- **Zed** : `bench_metrics`, comptage d'instructions via `perf_event_open`, fusionné le 2026-09-27 ([PR #64753](https://github.com/zed-industries/zed/pull/64753)), postérieur au pin `fecc3273` (2026-08-26). Pas de gate en CI.
- **pf** : quatre niveaux de rigueur.
  - budgets absolus sous Linux (`pf/benchmarks/check_budgets.py:10-28`) ;
  - ratio entre deux chemins dans un même process, médiane de 9 lots avec quorum (`pf/benchmarks/activity_progress.zig:170-202`) ;
  - A/B sur le même runner, p50 et p95 ≤ +10 % (`pf/scripts/pgso/qualify.py:42,365-392`) ;
  - bootstrap apparié avec cohorte A/A (`pf/benchmarks/pgso_artifacts.test.ts:316-342`).
- **Alacritty, Ghostty, WezTerm** : mesures manuelles (vtebench, typometer), aucun gate en CI.
- **Market gap:** aucun terminal ni éditeur GPUI ne gate le coût récurrent de son application en CI. Compter le travail plutôt que le temps rend ce gate possible sur des runners partagés.

### Best Practices Applied
- Compter le travail plutôt que le temps : seuls les compteurs déterministes bloquent une fusion. C'est la leçon commune de rustc-perf et de la Noise FAQ de Mozilla.
- Comparer base et head dans le même job, en rounds alternés, avec une cohorte A/A qui doit encadrer zéro (pf, CodSpeed). Le coefficient de variation des runners GitHub est d'environ 2,7 % ([CodSpeed](https://codspeed.io/blog/benchmarks-in-ci-without-noise)) : une bande de 10 % est le minimum.
- Mode ombre avant blocage, avec un critère de promotion explicite. Sans ce critère, les gates bruités finissent désactivés.
- Une sortie d'échec utile donne le compteur, le budget, l'écart et la commande de reproduction ; les rapports sont écrits avant les assertions (`pf/tests/e2e/tui-performance.test.ts:1007-1008`).
- Budgets dans le code, et un vérificateur lui-même testé contre un résultat synthétique en échec (`pf/benchmarks/check_budgets_test.py`, Paneflow `a_seeded_failure_fails_the_run_and_retains_its_artifact`).
- Déclencher les workflows de performance sur `pull_request`, jamais sur `pull_request_target` avec du code de fork, et isoler leurs caches. L'incident TanStack du 2026-05-11 est parti d'un workflow de taille en `pull_request_target`, avec empoisonnement du cache ([postmortem](https://tanstack.com/blog/npm-supply-chain-compromise-postmortem)).

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- **Les sept candidates expliquent l'essentiel du ralentissement perçu.** Base : lecture du code et des commits. À valider : US-002 mesure `main` dans le scénario actif, et US-019 compare v0.15.1, v0.17.0, v0.17.1 et v0.17.5.
- **Le desktop GPUI tourne dans un runner GitHub Linux** sous Xvfb ou sway headless avec lavapipe, et ses compteurs y sont stables à ± 10 %. Base : la démo HN tournée sous sway et lavapipe, et la CI de wgpu sur lavapipe. À valider : US-003.
- **Un compteur fenêtré dans le temps, comme les listings par seconde, reste stable sur un runner à 2 vCPU** si son budget est exprimé par événement (par rafale de sortie, par sonde). À valider : US-003 et les 30 premiers runs d'US-012.
- **Les compteurs d'allocation d'un même commit sont identiques** sur la machine Linux d'Arthur et sur un runner. Base : l'allocateur compte les appels Rust, pas ceux de la libc. À valider : US-014.
- **Valgrind exécute le code qui lie libghostty statiquement.** Base : le comportement standard de Valgrind. À valider : US-017.
- **Le pas du spinner, environ 11 pas par seconde, est visuellement identique à l'animation actuelle**, qui n'a que 8 positions discrètes (`lane.rs:216`). À valider : passe visuelle d'Arthur en US-004.

### Hard Constraints
- Linux, macOS et Windows restent des cibles de livraison. Les compteurs et les correctifs fonctionnent sur les trois. Les gates CI v1 tournent sous Linux x86_64 seulement.
- Le thread GPUI ne bloque jamais (AGENTS.md). Un compteur ne fait aucune I/O, il reste un atomique relâché.
- Les durcissements de sécurité ne reculent pas : neutralisation des filtres git, `core.fsmonitor=false`, `GIT_OPTIONAL_LOCKS=0` (`src-app/src/git_command.rs:23-35`), et durabilité des hooks acquittés (US-023 du PRD `prd-agent-integration-overhaul`).
- Les binaires d'aide gardent leurs plafonds : `paneflow-shim` 512 KiB, `paneflow-ai-hook` 384 000 octets, `paneflow-mcp` 512 KiB, total 1 835 008 octets (`run_tests.yml:320-323`).
- Aucun commentaire dans le code source (AGENTS.md) ; aucun item après un `mod tests`.
- Chaque invocation cargo reste `--locked`. Aucun profil cargo créé par run : les profils nécessaires sont déclarés une fois dans `Cargo.toml`.
- Les workflows de performance se déclenchent sur `pull_request`, `push` sur `main`, `schedule` ou `workflow_dispatch`, jamais sur `pull_request_target`.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration
- `cargo deny check advisories licenses sources` - uniquement quand une dépendance change

Gates additionnels :
- Stories qui touchent du code `#[cfg(windows)]` : le job « Windows x86_64 libghostty check » passe, et la PR dit si Windows a été vérifié par inspection ou sur le matériel réel.
- Stories UI (US-004, US-005) : passe visuelle d'Arthur sous Linux sur un build debug, capture ou enregistrement court joint à la PR. L'agent livre le changement sans lancer l'app pour la vérifier.
- Stories de correctif (EP-002) : mesure avant/après jointe à la PR, prise avec les compteurs d'US-001 et la commande qui la reproduit.
- Stories qui ajoutent ou modifient un workflow : `scripts/check-workflow-action-pins.sh` passe.
- À partir de la fusion d'US-011 : le job CI `perf-gates` passe.

## Epics & User Stories

### EP-001: Compter le travail et rejouer des agents actifs

Donner aux trois processus des compteurs de travail lisibles en release, et à la suite persistante un scénario qui ressemble à l'usage réel. C'est l'instrument de mesure de tout le reste du PRD.

**Definition of Done:** `host.status`, `worker.status` et `system.counters` renvoient les compteurs nommés d'US-001 sur Linux, macOS et Windows. Le scénario actif d'US-002 tourne sur `main` et sa mesure de référence, prise avant EP-002, est commitée dans `bench/results/`. Le spike US-003 a tranché la faisabilité du desktop sans écran en CI.

#### US-001: Exposer des compteurs de travail dans le host, le worker et le desktop
**Description:** As a mainteneur, I want que chaque processus compte le travail récurrent qu'il effectue et l'expose par IPC so that une régression devienne un nombre comparable plutôt qu'un ressenti.

Points de comptage, chacun sur un chemin déjà unique :
- listing système des processus (`unix_process_entries`, `crates/paneflow-host/src/process.rs:224` ; Toolhelp sous Windows, `:804`) ;
- observation du premier plan (`observe_foreground_runtime`, `runtime_observer.rs:78`) ;
- diffusions de l'`agent_bus` du host par type (`host.rs:866-871`) ;
- diffusions du worker par type (`worker.rs:290-294`) ;
- processus git lancés par profil et par sous-commande, dont la requête de filtres (`src-app/src/git_command.rs`) ;
- rendus de la vue racine du desktop ;
- snapshots d'agents appliqués (`host_agents.rs:533`) ;
- appels `session.list` (`hosted_sessions.rs:525`).

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un host démarré, when un client appelle `host.status`, then la réponse contient un objet `counters` de compteurs `u64` monotones nommés, dont `process_listings`, `foreground_observations`, `agent_bus_session_broadcasts` et `agent_bus_snapshot_broadcasts`, plus `process_identity` (pid et instant de démarrage).
- [ ] Given un worker démarré, when un client appelle `worker.status`, then la réponse contient `counters` avec `snapshot_broadcasts`, `projection_broadcasts`, `sweeps` et `process_identity`.
- [ ] Given un desktop démarré, when un client appelle `system.counters`, then la réponse contient `root_renders`, `host_agent_snapshots_applied`, `session_list_calls`, `git_spawns` ventilés par sous-commande, `process_spawns` et `process_identity`.
- [ ] `system.counters` passe par le même contrôle d'accès que `system.identify` (`src-app/src/app/ipc_handler/gates.rs:94-96`) et sert sans attendre le thread GPUI (lecture d'atomiques).
- [ ] Un test montre que chaque compteur augmente exactement de 1 par événement compté, sur un événement provoqué dans le test (par exemple un listing de processus forcé, une diffusion, une sonde git).
- [ ] Les compteurs sont des atomiques relâchés, sans allocation ni I/O. Un test borne leur coût à moins de 50 ns par incrément en release (`#[ignore]` documenté dans `bench/README.md`).
- [ ] Le nom, l'unité et le point de comptage de chaque compteur sont documentés dans `bench/README.md`, section « Compteurs de travail ».
- [ ] Les trois processus compilent et exposent les compteurs sous Linux, macOS et Windows ; `paneflow-shim`, `paneflow-ai-hook` et `paneflow-mcp` n'en dépendent pas et restent sous leurs plafonds.
- [ ] Échec : given un lecteur qui interroge un host d'une version sans `counters`, when la réponse ne contient pas l'objet, then le lecteur rapporte chaque compteur `pending` avec la raison, jamais `0` (test).
- [ ] Échec : given un processus redémarré entre deux lectures, when le lecteur calcule un delta, then il détecte le changement de `process_identity` et invalide l'échantillon au lieu de produire un delta négatif ou faux (test).

#### US-002: Ajouter un scénario « agents actifs » à la suite persistante
**Description:** As a mainteneur, I want mesurer Paneflow avec plusieurs sessions qui impriment en continu so that les coûts qui croissent avec le nombre d'agents actifs apparaissent, ce que les sessions `idle` ne montrent jamais.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `scripts/bench-persistent.sh --active` (et `.ps1 -Active`) ouvre 1, 4 et 8 sessions `paneflow-session-fixture stream 16384 60` plus une session `flood 8388608`, attend 4 s, puis échantillonne pendant 30 s.
- [ ] Chaque échantillon enregistre le CPU par thread nommé du host, du worker et, avec `--with-desktop`, du desktop. Il enregistre aussi le RSS de chaque processus et le delta de chaque compteur d'US-001 sur la fenêtre.
- [ ] Le document de résultat passe au schéma 4 ; la comparaison refuse une baseline d'un autre schéma avec un message explicite.
- [ ] La première exécution sur `main` propre, avant toute story d'EP-002, est commitée dans `bench/results/` et citée dans `bench/README.md` comme référence « avant EP-002 ».
- [ ] Le scénario actif prend moins de 6 min de bout en bout sur la machine Linux d'Arthur.
- [ ] Échec : given une session fixture qui meurt pendant la fenêtre, when l'échantillon se termine, then le scénario échoue avec le nom de la session et ne publie pas de moyenne calculée sur moins de sessions que prévu (test avec `delayed-exit`).
- [ ] Échec : given un compteur `pending`, when le rapport est écrit, then la valeur reste `pending` avec sa raison et le scénario ne la compte pas comme une amélioration.

#### US-003: Valider l'hypothèse : le desktop GPUI tourne et se mesure sans écran dans un runner GitHub Linux
**Description:** As a mainteneur, I want savoir si le vrai desktop peut tourner sous un serveur d'affichage virtuel avec un Vulkan logiciel dans GitHub Actions so that les compteurs du desktop deviennent un gate CI, ou que le PRD le sache avant d'écrire US-013.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Un workflow `workflow_dispatch` de spike installe Mesa lavapipe, lance le desktop release sur `ubuntu-24.04` sous Xvfb puis sous sway headless, avec un `PANEFLOW_HOME` isolé, `PANEFLOW_SOCKET_PATH`, `PANEFLOW_ALLOW_SOCKET_OVERRIDE=1` et le clignotement du curseur coupé. Il lit `system.counters` par IPC.
- [ ] Le spike mesure 5 runs de 30 s pour trois états : desktop au repos avec 4 panes, un agent simulé en réflexion (spinner de la sidebar actif), et 4 sessions `stream`. Il rapporte pour chaque compteur la moyenne et le coefficient de variation.
- [ ] Le résultat est consigné dans `bench/README.md`, section « Desktop sans écran ». Il précise le serveur retenu, le temps d'installation, le temps total et les compteurs dont le coefficient de variation dépasse 10 %. Ceux-là ne seront pas gatés en absolu.
- [ ] Critère de validation : l'hypothèse est validée si le desktop présente une première frame en moins de 90 s, si `root_renders` au repos est stable à ± 1 frame par 30 s, et si le job complet dure moins de 15 min.
- [ ] Échec : given un runner sans Vulkan logiciel fonctionnel, when le desktop démarre, then le spike rapporte l'erreur de l'adaptateur et le temps écoulé, et conclut « non validé » plutôt que de mesurer sur un rendu absent.
- [ ] Si l'hypothèse n'est pas validée, la conclusion propose une alternative (comptage dans `TestAppContext`, ou un runner tiers), et US-013 est marquée `BLOCKED` avec cette raison.

---

### EP-002: Corriger les régressions mesurées

Supprimer le travail récurrent que v0.16.0 à `main` ont ajouté, en mesurant chaque correctif avec les compteurs d'EP-001. Aucun correctif n'affaiblit une garantie de sécurité ou de durabilité.

**Definition of Done:** Les sept candidates sont corrigées ou rejetées par la mesure, chacune avec une mesure avant/après jointe à sa PR. Le scénario actif d'US-002 rejoué après EP-002 montre, à 8 sessions actives : au plus 2 listings de processus par seconde pour tout le host, 0 snapshot du worker diffusé sans changement, et au plus 12 rendus racine par seconde du desktop avec un agent en réflexion.

#### US-004: Faire avancer le spinner de la sidebar par pas plutôt qu'à chaque frame
**Description:** As a utilisateur multi-agents, I want que le spinner d'un agent en réflexion ne fasse plus redessiner la fenêtre à la cadence d'affichage so that le desktop dessine au plus 12 frames par seconde quand un agent réfléchit.

Le spinner a 8 positions sur un cycle de 720 ms (`src-app/src/app/sidebar/lane.rs:212-243`) et ne change donc d'image que toutes les 90 ms. Aujourd'hui, `Animation::repeat` redessine la fenêtre à chaque frame d'affichage.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un agent en réflexion et le reste de l'application au repos, when on mesure 30 s, then `root_renders` augmente d'au plus 12 par seconde. La mesure d'avant, à la cadence d'affichage, est jointe à la PR.
- [ ] Given 8 agents en réflexion, when on mesure 30 s, then `root_renders` augmente toujours d'au plus 12 par seconde : les spinners avancent ensemble sur la même horloge (`SYNC_EPOCH`, `lane.rs:214`).
- [ ] La séquence des positions et la durée d'un cycle sont identiques à celles d'aujourd'hui, à un pas près (test sur la fonction qui calcule la tête du spinner).
- [ ] Given « Reduce motion » activé, when un agent réfléchit, then le spinner est statique et `root_renders` n'augmente pas à cause de lui (test).
- [ ] Given plus aucun agent en réflexion, when la dernière réflexion se termine, then la minuterie du spinner s'arrête et aucun rendu périodique ne subsiste (test).
- [ ] Échec : given la vue de la sidebar fermée pendant qu'un agent réfléchit, when le pas suivant arrive, then la minuterie s'arrête sans erreur ni réveil supplémentaire (test).
- [ ] La PR liste les autres animations `repeat()` (`ui_primitives.rs:412` du bouton de mise à jour, `ui_primitives.rs:869` de l'état vide, `clone_repo.rs:437`) et confirme qu'aucune ne tourne dans l'état stable « agents au travail ».

#### US-005: Dessiner les badges d'onglet sans reconstruire de chemin à chaque frame
**Description:** As a utilisateur multi-agents, I want que les badges squircle des onglets ne soient plus tessellés à chaque frame so that le coût CPU et GPU d'une frame ne croisse plus avec le nombre d'onglets.

`tab_badge` peint 5 chemins par onglet (`src-app/src/pane.rs:225-247`) et `squircle_path` reconstruit un `PathBuilder` à chaque appel (`src-app/src/ui_primitives/squircle.rs:47-70`). Le chrome des panes n'est pas en cache (`pane.rs:1887` ne cache que le corps du terminal).

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un pane de 12 onglets terminal rendu deux fois sans changement de taille, d'échelle ni de thème, when la seconde frame est peinte, then aucun chemin squircle n'est reconstruit (test sur un compteur de construction dans `squircle.rs`).
- [ ] Given un changement de facteur d'échelle ou de thème, when la frame suivante est peinte, then les badges sont reconstruits une seule fois, puis réutilisés (test).
- [ ] Le rendu visuel du badge (anneau, fond, reflet, ombre basse, liseré) est identique à celui de `65a9f385` : captures avant/après jointes à la PR, à 100 % et 200 %, en thème clair et sombre.
- [ ] Avec un agent en réflexion et 12 onglets, le temps CPU du thread principal du desktop sur 30 s baisse par rapport à la mesure d'avant, jointe à la PR avec la commande qui la reproduit.
- [ ] Échec : given des bornes de badge de taille nulle (onglet en cours de fermeture), when le badge est peint, then rien n'est dessiné et aucune entrée de cache n'est créée (test).

#### US-006: Partager un seul listing système des processus par host et par intervalle
**Description:** As a utilisateur multi-agents, I want que le host ne lise plus tout `/proc` une fois par session et par demi-seconde so that le coût de suivi des processus ne croisse plus avec le nombre de sessions qui impriment.

Aujourd'hui, toute sortie arme un scan à 500 ms (`crates/paneflow-host/src/runtime.rs:1162-1164`), et `discover()` relit tous les processus de la machine sur le thread de la session (`process.rs:224-260,326-372`). Le listing complet reste nécessaire, car il suit les orphelins par groupe de processus. Sa fréquence ne l'est pas.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-002

**Acceptance Criteria:**
- [ ] Given 8 sessions `stream` actives pendant 30 s, when on lit `process_listings`, then il augmente d'au plus 2 par seconde pour tout le host. La mesure d'avant, environ 16 par seconde, est jointe.
- [ ] Given une seule session active, when un descendant naît puis meurt, then il est découvert dans les 1 000 ms qui suivent la sortie qui l'accompagne (test avec la fixture `descendants`).
- [ ] Les tests existants de suivi des orphelins (fixture `descendants-orphan`) passent sans modification de leurs assertions.
- [ ] Le même partage s'applique au snapshot Toolhelp sous Windows et au listing macOS, vérifié par inspection pour Windows dans la PR.
- [ ] Le listing ne s'exécute plus sur le thread qui alimente le parseur d'une session : la latence p95 de l'aller-retour d'écho par le host (entrée envoyée au PTY, écho publié au client), mesurée par le scénario actif d'US-002 sous 1, 4 et 8 sessions actives, n'augmente pas par rapport à `main` avant EP-002.
- [ ] Échec : given un listing qui échoue (`/proc` illisible), when une session demande le snapshot partagé, then elle marque `snapshot_failed` comme aujourd'hui et le listing suivant est retenté à l'intervalle suivant, pas en boucle (test).

#### US-007: Réutiliser l'observation du premier plan tant que son groupe ne change pas
**Description:** As a utilisateur multi-agents, I want que le scan du viewport ne reparcoure plus l'arbre de processus du premier plan à chaque passage so that une session active coûte au host l'analyse de son écran, pas une lecture de `/proc` toutes les 500 ms.

`scan_once` appelle `observe_foreground_runtime` à chaque scan dû (`crates/paneflow-host/src/viewport_scan.rs:400-404`).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-002

**Acceptance Criteria:**
- [ ] Given une session dont le groupe du premier plan et l'instant de démarrage de son leader ne changent pas, when 60 scans se succèdent, then `foreground_observations` augmente d'au plus 1 pour cette session.
- [ ] Given un changement de groupe du premier plan (l'agent lance un outil, puis revient), when le scan suivant arrive, then l'observation est refaite et l'état publié change comme aujourd'hui (test).
- [ ] Given le leader du premier plan remplacé par un autre processus de même pid, when le scan arrive, then l'instant de démarrage différent invalide l'observation en cache (test).
- [ ] La classification du corpus d'écrans (`bench/screen-corpus-baseline.json`) reste identique.
- [ ] Échec : given un leader devenu non observable, when le scan arrive, then l'observation passe à `Unobservable` sans réutiliser la valeur en cache (test).

#### US-008: Ne diffuser un snapshot du worker que s'il change, et ne rien redessiner sinon
**Description:** As a utilisateur multi-agents, I want que le worker et le desktop restent silencieux quand l'état des agents ne change pas so that l'application au repos ne se réveille plus toutes les 2 s pour redessiner toute la fenêtre.

Le worker diffuse un snapshot complet à chaque balayage de 2 s (`crates/paneflow-serve/src/worker.rs:272-294`). Le desktop l'applique, appelle `refresh_owned_sessions` puis `cx.notify()` racine, sans comparer (`src-app/src/app/host_agents.rs:533-552`). Le host diffuse aussi l'entrée d'une session à chaque changement de ses signaux terminal, donc toutes les 500 ms pour une session qui imprime (`viewport_scan.rs:389`).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given 8 sessions d'agents dont l'état ne change pas pendant 60 s, when on lit `snapshot_broadcasts` du worker, then il n'augmente pas. La mesure d'avant (30 par minute) est jointe.
- [ ] Given un snapshot reçu identique au dernier appliqué, when le desktop le traite, then ni `session_list_calls` ni `root_renders` n'augmentent à cause de lui (test).
- [ ] Le balayage de 2 s continue de produire les transitions qui dépendent du temps (expiration de bail, attention périmée) et diffuse chacune comme aujourd'hui (tests existants de `hook_state.rs`).
- [ ] Given 4 sessions qui impriment sans changer d'état réduit, when on mesure 60 s, then le worker ne diffuse aucune projection et le desktop ne fait aucun rendu racine dû aux agents. Le host peut continuer à pousser les signaux de sortie au worker.
- [ ] Given un desktop qui se reconnecte au worker, when la connexion reprend, then il reçoit un snapshot complet une fois, même identique au précédent (test).
- [ ] Échec : given un worker redémarré, when il reprend son état, then il diffuse un snapshot complet initial et le desktop l'applique (test).

#### US-009: Mettre en cache la requête des filtres git par dépôt, sans affaiblir la neutralisation
**Description:** As a utilisateur avec de grands dépôts et plusieurs workspaces, I want que chaque sonde git ne lance plus un `git config` supplémentaire so that une sonde diff-stat revienne de 5 à 3 processus, sans réouvrir l'exécution de filtres non fiables.

`neutralize_repository_filters` interroge `git config --get-regexp` avant chaque sonde qui lit le worktree (`src-app/src/git_command.rs:134-178`).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un dépôt dont aucun fichier de config n'a changé, when deux sondes diff-stat se suivent, then la seconde lance exactement 3 processus git (test sur `git_spawns`).
- [ ] La requête de filtres enregistre les fichiers d'origine de chaque clé, local, worktree et fichiers inclus (`--show-origin`). Le cache est indexé par l'identité de ces fichiers et de `.git/config` : chemin, taille, date de modification, inode sous Unix.
- [ ] Given un filtre ajouté dans un fichier inclus par `include.path`, when la sonde suivante s'exécute, then la requête est relancée et le nouveau filtre est neutralisé (test de sécurité).
- [ ] Given un `.git/config` remplacé par un fichier de même taille et de même date de modification mais d'inode différent, when la sonde suivante s'exécute, then le cache est invalidé (test, Unix).
- [ ] Given une requête de filtres qui échoue ou dépasse son délai, when la sonde s'exécute, then la sonde échoue comme aujourd'hui et aucune entrée de cache n'est écrite (test).
- [ ] `PROBE_CONFIG`, `PROBE_ENV` et la neutralisation des clés `clean`, `smudge`, `process` et `required` restent inchangés.
- [ ] Échec : given un dépôt sans fichier de config local, when la sonde s'exécute, then le cache est indexé sur l'absence du fichier, et la création du fichier l'invalide (test).

#### US-010: Démarrer sans attendre sur un socket IPC périmé
**Description:** As a utilisateur Linux ou macOS, I want que Paneflow ne perde plus 140 ms au démarrage à cause du socket laissé par la session précédente so that la première frame arrive plus tôt et le thread GPUI ne dorme plus.

`detect_existing_instance` (`src-app/src/ipc.rs:480-526`) traite ECONNREFUSED comme un échec transitoire et réessaie trois fois avec 70 ms d'attente. Cette relance servait une période de transition révolue (commentaire retiré par `04c468ef`). Le serveur ne retire son socket que si son thread sort de sa boucle (`ipc.rs:279`), ce que `cx.quit()` ne garantit pas.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un socket périmé, when Paneflow démarre sous Linux, then l'étape `ipc_server_started` de `scripts/bench-startup.sh` vaut au plus 5 ms p95. La mesure d'avant, 140,3 ms, est citée.
- [ ] Given une fermeture normale de l'application, when le processus se termine, then le socket IPC du desktop est retiré (test sous Unix).
- [ ] Given une première instance vivante, when une seconde démarre, then la seconde refuse de démarrer avec le message actuel (test existant ou ajouté).
- [ ] Given deux instances lancées à moins de 50 ms d'écart, when elles démarrent, then une seule continue (test de concurrence ; mécanisme au choix de l'implémentation, par exemple un verrou de fichier).
- [ ] Le comportement Windows (pipes nommés, `NotFound` immédiat) est inchangé, vérifié par inspection dans la PR.
- [ ] Échec : given un processus qui accepte la connexion mais ne répond jamais à `system.identify`, when Paneflow démarre, then il conclut en au plus 350 ms et démarre comme aujourd'hui quand la réponse n'est pas celle de Paneflow (test).

---

### EP-003: Verrouiller l'état corrigé par des gates déterministes en CI Linux

Transformer les compteurs et les allocations en budgets écrits dans le code, vérifiés par un job CI bloquant sur chaque PR et sur `main`. Le job ne mesure pas de temps, sauf des bornes larges qui protègent un contrat déjà écrit.

**Definition of Done:** Le job `perf-gates` tourne sur chaque `pull_request` et chaque `push` sur `main`. Il fait partie de `tests_pass` et vérifie au moins 15 budgets répartis entre host, worker et desktop. Il a passé 30 runs consécutifs sans échec parasite, et un échec volontaire (seed) prouve sa sortie d'erreur.

#### US-011: Créer le job CI `perf-gates` et son format d'échec
**Description:** As a agent d'implémentation, I want un job CI qui exécute les scénarios de compteurs et rend un verdict binaire expliqué so that une PR qui multiplie un travail récurrent échoue avant sa fusion.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-002

**Acceptance Criteria:**
- [ ] `.github/workflows/run_tests.yml` gagne un job `perf_gates` sur le même runner Linux que `run_tests_linux`, déclenché par les mêmes événements, avec `--locked`, libghostty récupéré par `.github/actions/fetch-libghostty` et un `timeout-minutes` de 30.
- [ ] Le job construit dans un profil déclaré une fois dans `Cargo.toml` et réutilisé, sans `--profile` propre au run.
- [ ] Les budgets sont des constantes Rust dans un seul module de test, chacune avec son nom, sa valeur, son unité et le scénario qui la mesure.
- [ ] Un échec affiche, pour chaque budget dépassé, une ligne `compteur | mesuré | budget | écart | scénario`. Il donne aussi la commande locale qui reproduit le scénario et écrit le tableau dans `$GITHUB_STEP_SUMMARY`.
- [ ] Le rapport JSON est écrit avant les assertions et téléversé comme artefact avec `if: always()`, 14 jours de rétention.
- [ ] `tests_pass` (`run_tests.yml:1327`) dépend de `perf_gates`.
- [ ] Le cache du job a une clé préfixée `perf-gates-`, disjointe de celles de `release.yml` ; le workflow n'utilise pas `pull_request_target` (vérifié par un test sur le fichier du workflow).
- [ ] Un test non ignoré, exécuté dans le job, alimente le vérificateur avec un résultat synthétique en dépassement et prouve qu'il échoue avec la ligne attendue.
- [ ] Échec : given un scénario qui ne produit pas de mesure (processus mort, compteur `pending`), when le vérificateur s'exécute, then le job échoue avec la raison. Une mesure absente n'est jamais acceptée comme dans le budget.

#### US-012: Poser les budgets du host et du worker
**Description:** As a mainteneur, I want que les compteurs du host et du worker soient bornés dans des scénarios au repos et actifs so that les régressions corrigées par US-006, US-007 et US-008 ne puissent pas revenir.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011, US-006, US-007, US-008

**Acceptance Criteria:**
- [ ] Repos, 8 sessions `idle`, 30 s :
  - `process_listings` : 0 ;
  - `snapshot_broadcasts` du worker : 0 ;
  - `agent_bus_session_broadcasts` : 0 ;
  - temps CPU cumulé du host et du worker : au plus 30 ms chacun (lecture de `/proc/<pid>/stat`).
- [ ] Actif, 8 sessions `stream 16384` pendant 30 s :
  - `process_listings` : au plus 2 par seconde ;
  - `foreground_observations` : au plus 1 par session et par changement de groupe ;
  - `agent_bus_session_broadcasts` : au plus 2 par seconde et par session.
- [ ] Mémoire, actif, 8 sessions : RSS du host au plus égal à la mesure d'après EP-002 majorée de 25 %, valeur inscrite comme budget.
- [ ] Hooks : le bench d'US-023 du PRD `prd-agent-integration-overhaul` (200 hooks en rafale, 100 ms de fsync injectée) tourne dans le job, sans `#[ignore]`, avec une réponse p95 inférieure à 350 ms et zéro événement perdu.
- [ ] Chaque budget est calé sur la mesure d'après les correctifs, avec la marge écrite à côté de la constante. Un budget sans marge justifiée est refusé en revue.
- [ ] Échec : given un worker qui diffuse un snapshot inchangé (régression réintroduite dans un test de mutation), when le job s'exécute, then le budget `snapshot_broadcasts` échoue (preuve jointe à la PR).

#### US-013: Poser les budgets du desktop sous un affichage virtuel
**Description:** As a mainteneur, I want que le desktop réel soit mesuré en CI Linux sous l'affichage virtuel validé par US-003 so that les régressions de rendu (spinner, badges, snapshots) et de démarrage échouent en CI.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003, US-011, US-004, US-005, US-008, US-009, US-010

**Acceptance Criteria:**
- [ ] Le job `perf_gates` lance le desktop release avec la configuration retenue par US-003, dans un home isolé, clignotement coupé, télémétrie désactivée.
- [ ] Repos, 4 panes, 30 s : `root_renders` au plus 3, `session_list_calls` 0, `host_agent_snapshots_applied` 0.
- [ ] Un agent en réflexion, 30 s : `root_renders` au plus 12 par seconde.
- [ ] Une sonde diff-stat sur un dépôt dont la config n'a pas changé lance exactement 3 processus git.
- [ ] Démarrage avec un socket périmé : étape `ipc_server_started` au plus 5 ms p95 sur 10 lancements. C'est la seule borne de temps de cette story ; elle protège un sommeil supprimé, pas une performance.
- [ ] Échec : given un affichage virtuel qui ne démarre pas, when le job s'exécute, then il échoue avec l'erreur de l'affichage ou de l'adaptateur Vulkan, sans sauter les budgets du desktop en silence.

#### US-014: Passer en gate les allocations des suites terminal et éditeur
**Description:** As a mainteneur, I want que les colonnes d'allocation des suites terminal et éditeur deviennent un gate so that une PR qui ajoute des allocations par frame ou par octet échoue, sans dépendre du bruit d'horloge.

`bench/README.md:504-506` décrit déjà ces colonnes comme déterministes.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] `perf_gates` exécute `terminal::perf_bench::terminal_pipeline_benchmark` et `app::diff_dock::code::perf_bench::editor_pipeline_benchmark`, puis compare `alloc_bytes_per_iter` et `allocs_per_iter` de chaque métrique à une baseline Linux commitée.
- [ ] Le gate échoue si une métrique dépasse sa baseline de plus de 1 %. Une baisse de plus de 1 % échoue aussi, avec le message « baseline à rafraîchir dans cette PR », pour qu'aucune amélioration ne reste hors baseline.
- [ ] Deux runs du même commit, l'un sur la machine Linux d'Arthur et l'autre sur le runner, donnent des colonnes d'allocation identiques. Sinon, la PR documente l'écart et la tolérance par métrique qui en découle.
- [ ] `gate_trickle_publishes` (`bench/README.md:218`), compteur déterministe de frames publiées, est gaté sur sa valeur exacte.
- [ ] Les colonnes de temps de ces suites restent informatives dans ce job.
- [ ] Échec : given une métrique marquée `available: false` (pas de shaper réel sur le runner), when le gate compare, then elle est rapportée comme non mesurée, ni acceptée ni comptée comme une régression. La PR liste ces métriques.

---

### EP-004: Comparer le temps réel par un A/B calibré dans le même job

Mesurer le temps réel, CPU compris, sans dépendre d'un runner stable : construire base et head dans le même job et alterner les rounds. Une cohorte A/A doit valider le run, et le gate ne devient bloquant qu'après une période d'ombre sans échec parasite.

**Definition of Done:** `scripts/perf-ab.sh` compare deux commits localement et en CI. Le workflow `perf-ab.yml` tourne en mode ombre pour le temps réel, avec son critère de promotion écrit dans `bench/README.md` ; la promotion elle-même relève d'US-020. Le spike Gungraun a conclu, et les compteurs d'instructions bloquent le workflow après 10 runs CI consécutifs sans faux positif, cités dans `bench/README.md`.

#### US-015: Écrire le comparateur A/B base contre head, calibré par une cohorte A/A
**Description:** As a mainteneur, I want comparer le temps réel et le temps CPU de deux commits sur la même machine dans le même run so that un écart de plus de 10 % sur un chemin chaud soit détecté malgré le bruit d'un runner partagé.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] `scripts/perf-ab.sh <base> <head>` (et `.ps1`) construit chaque commit dans un worktree détaché avec un `CARGO_TARGET_DIR` de scratch hors du dépôt, puis supprime ces répertoires à la fin, même en cas d'échec.
- [ ] Il exécute la suite terminal et le scénario actif d'US-002 (host seul) en rounds alternés A, B, B, A, avec au moins 10 rounds et au moins 50 échantillons par artefact et par métrique, plus une cohorte A/A.
- [ ] Une métrique régresse si le p50 de head dépasse celui de base de plus de 10 %, ou son p95 quand ce p95 est jugé (règle de pf, `pf/scripts/pgso/qualify.py:42,365-392`). Le temps CPU par thread nommé suit la même règle.
- [ ] Le run est rejeté comme « non calibré » si la cohorte A/A montre un écart supérieur à 5 % sur le p50 d'une métrique. Le p95 d'une métrique n'est jugé que si son propre écart A/A au p95 reste sous 5 % ; sinon la ligne l'indique « p95 non jugé ».
- [ ] La sortie JSON donne pour chaque métrique les deux distributions, l'écart et le verdict, plus l'identité des deux commits et de la machine. Un résumé Markdown en est tiré.
- [ ] Le comparateur refuse un arbre sale pour l'un des deux commits.
- [ ] Échec : given un commit de base qui ne compile pas, when le script s'exécute, then il rapporte « base indisponible » avec l'erreur de compilation et sort avec un code distinct de celui d'une régression (test du script).

#### US-016: Faire tourner l'A/B en mode ombre, puis le promouvoir en gate
**Description:** As a mainteneur, I want que l'A/B tourne d'abord sans bloquer, puis bloque quand son taux d'échec parasite est prouvé nul so that le gate de temps réel ne finisse pas désactivé comme les gates bruités des autres projets.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-015

**Acceptance Criteria:**
- [ ] `.github/workflows/perf-ab.yml` tourne :
  - sur `pull_request` quand la PR touche `src-app/src/terminal/**`, `src-app/src/app/**`, `crates/paneflow-host/**`, `crates/paneflow-serve/**` ou `Cargo.lock` ;
  - chaque nuit sur `main`, contre le dernier tag publié ;
  - sur `workflow_dispatch`.
- [ ] En mode ombre, le job publie son résumé dans `$GITHUB_STEP_SUMMARY` et reste vert quel que soit le verdict, sauf erreur d'exécution.
- [ ] Le critère de promotion est écrit dans `bench/README.md` : 30 runs consécutifs sur au moins 3 semaines, cohorte A/A calibrée sur au moins 90 % d'entre eux, et aucun verdict de régression non confirmé par une seconde exécution.
- [ ] Le workflow porte deux variables, `PERF_AB_BLOCKING` pour le temps réel et `PERF_AB_INSTRUCTIONS_BLOCKING` pour les instructions (US-017). Chacune se bascule par un seul commit qui cite ses runs ; celle du temps réel relève d'US-020.
- [ ] Le workflow n'utilise pas `pull_request_target` et son cache a une clé préfixée `perf-ab-`.
- [ ] Échec : given un run non calibré, when le job se termine, then il le signale comme tel, le relance une fois, et ne compte aucun des deux runs contre le critère de promotion.

#### US-017: Valider l'hypothèse : comptage d'instructions par Gungraun sur un runner Linux
**Description:** As a mainteneur, I want savoir si Gungraun mesure les chemins CPU purs de Paneflow en CI so that les régressions de quelques pour cent sur le parse, la conversion, la mise en page et les règles d'écran puissent être gatées par un nombre d'instructions.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un spike `workflow_dispatch` installe Valgrind 3.20 ou plus et un `gungraun-runner` de la même version que la bibliothèque, épinglée en 0.20.x. Il exécute deux benchmarks : parse et conversion d'un corpus de 1 MiB, 20 règles sur un écran 200x60. La mise en page 220x60 sort du périmètre : elle vit dans le crate binaire `paneflow-app`, sans cible lib qu'un benchmark puisse appeler, et y reviendra quand ce crate en aura une.
- [ ] Critère de validation : le spike passe si les deux benchmarks s'exécutent avec libghostty lié statiquement, si deux runs du même commit donnent un nombre d'instructions identique à 0,1 % près, et si le job complet dure moins de 15 min.
- [ ] Si le spike est validé, ces benchmarks rejoignent `perf-ab.yml` avec une limite douce de +2 % d'instructions contre la base construite dans le même job (`--save-baseline` puis `--baseline`). Ils ont leur propre variable, `PERF_AB_INSTRUCTIONS_BLOCKING`, basculée à `"true"` par un commit qui cite 10 runs CI consécutifs où les comptes ont été mesurés sans faux positif ; un verdict d'instructions est lu quel que soit le verdict temps réel.
- [ ] La dépendance Gungraun est une dev-dependency d'une cible `[[bench]]`, absente des binaires livrés ; `cargo deny check` passe.
- [ ] Échec : given Valgrind incapable d'exécuter le code de libghostty, when le spike tourne, then il conclut « non validé » avec l'erreur, sans ajouter de dépendance au dépôt.

---

### EP-005: Tenir des baselines propres par plateforme et mesurer le GPU sur matériel réel

Rendre les baselines comparables (une par plateforme, issues d'un arbre propre, au schéma courant) et couvrir ce que la CI ne peut pas mesurer : temps de frame, charge GPU et coût réel sous Wayland, X11, Windows et macOS.

**Definition of Done:** Les scripts refusent une baseline issue d'un arbre sale ou d'une autre plateforme. Les baselines Linux sont rafraîchies après EP-002. Le protocole manuel est documenté, a été exécuté une fois sur v0.17.5 et `main` après EP-002, et figure dans la checklist de version.

#### US-018: Rendre les baselines propres, par plateforme et vérifiées
**Description:** As a mainteneur, I want que chaque baseline soit propre à une plateforme, issue d'un arbre propre et au schéma courant so that une comparaison ne mélange plus Windows et Linux, ni un état sale et un état publié.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-005, US-006, US-007, US-008, US-009, US-010

**Acceptance Criteria:**
- [ ] Les baselines sont rangées par plateforme (`os-arch`) pour les suites terminal, éditeur, startup et persistante ; la comparaison cherche celle de la plateforme courante.
- [ ] Given aucune baseline pour la plateforme courante, when une suite compare, then elle affiche « pas de baseline pour <os-arch> » et ne compare à aucune autre.
- [ ] `--set-baseline` (et `-SetBaseline`) refuse un arbre dont `git_dirty` est vrai, avec un message qui donne la commande de vérification.
- [ ] `cpu_model()` rapporte le modèle réel sous macOS et Windows au lieu de `<os>-<arch>` (`src-app/src/bench_harness.rs:683-686`).
- [ ] La suite startup, qui mesure un sous-processus, écrit `cpu_share` comme non mesuré au lieu de `0.0` (`src-app/src/startup_bench.rs:267`), et `--set-baseline` y exige quand même un arbre propre.
- [ ] Les baselines Linux (machine d'Arthur) des quatre suites sont rafraîchies après EP-002, depuis un arbre propre. Les anciennes baselines Windows sont retirées ; les nouvelles sont enregistrées au prochain passage en dual boot, sans bloquer l'epic.
- [ ] Un test non ignoré vérifie chaque fichier de baseline commité : schéma courant, `git_dirty` faux, plateforme cohérente avec son emplacement.
- [ ] Échec : given une baseline au schéma ancien, when le test de cohérence s'exécute, then il échoue en nommant le fichier et le schéma attendu.

#### US-019: Documenter et exécuter le protocole manuel GPU et temps de frame sur matériel réel
**Description:** As a mainteneur, I want une procédure reproductible pour mesurer le temps de frame, la charge GPU et le coût réel sur mes machines so that ce que la CI ne peut pas mesurer soit quand même mesuré quand une version touche le rendu.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-002

**Acceptance Criteria:**
- [ ] `bench/README.md` gagne une section « Protocole matériel réel ». Elle décrit quatre états :
  - repos, 4 panes ;
  - 1 agent en réflexion ;
  - 4 sessions `stream` ;
  - 8 panes visibles.
- [ ] Pour chaque état, le protocole fixe les mesures sur 60 s :
  - CPU de chaque processus ;
  - RSS ;
  - `root_renders` ;
  - charge GPU par une source présente sur le matériel d'Arthur (`nvidia-smi` ou le fichier sysfs `gpu_busy_percent` d'amdgpu sous Linux ; le compteur « GPU Engine » sous Windows) ;
  - temps de frame p50 et p95, quand une source est disponible.
- [ ] Le protocole couvre Fedora sous Wayland et sous X11, et Windows 11. macOS est noté non mesuré tant qu'aucune machine ni source GPU n'est disponible.
- [ ] Une première exécution compare v0.17.5 (binaire publié) et `main` après EP-002 sous Fedora Wayland, dans les états « repos, 4 panes » et « 1 agent en réflexion », ceux où le spinner et les badges coûtent au GPU. Le résultat est commité dans `bench/results/` avec un résumé qui confirme ou infirme les régressions candidates que ces deux états décident.
- [ ] `docs/release/runbook.md` exige, avant de tagger une version mineure, que `perf-gates` soit vert sur le commit à tagger, et que le protocole ait été exécuté sous Linux quand la version contient un changement de rendu.
- [ ] Échec : given un outil GPU indisponible sur une machine, when le protocole s'exécute, then la mesure est consignée `non mesuré` avec la raison, jamais 0.

---

### EP-006: Promouvoir le temps réel après sa période d'ombre

Laisser l'A/B temps réel accumuler ses runs d'ombre, puis trancher sur preuve : le promouvoir en gate, ou le garder consultatif avec les taux observés.

**Definition of Done:** US-020 est tranchée : `PERF_AB_BLOCKING` est promu, ou la décision de garder le temps réel consultatif est consignée dans `bench/README.md` avec les taux de calibration et de régressions non confirmées.

#### US-020: Promouvoir l'A/B temps réel après sa période d'ombre
**Description:** As a mainteneur, I want que le verdict temps réel ne devienne bloquant qu'une fois son taux d'échec parasite prouvé nul so that le gate de temps réel ne finisse pas désactivé comme les gates bruités des autres projets.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] Le décompte suit `promotion.effect` de chaque `result.json` : 30 runs consécutifs sur au moins 3 semaines, p50 A/A calibrés sur au moins 90 % d'entre eux, aucune régression non confirmée par une seconde exécution.
- [ ] La promotion est une PR qui bascule `PERF_AB_BLOCKING` à `"true"`, met à jour le test du workflow et cite les 30 runs.
- [ ] Si le critère ne peut plus être atteint sur les 30 premiers runs comptés, ou n'est pas atteint au bout de 30 runs, la décision de garder le temps réel consultatif est consignée dans `bench/README.md`, avec les taux observés.
- [ ] Échec : given un run dont la base est indisponible ou exclu faute de mesure, when le décompte est fait, then il n'y entre pas.

---

### EP-007: Supprimer le travail que la fenêtre réelle révèle

Le protocole matériel du 2026-10-06 (`bench/results/hardware-summary-2026-10-06.md`) a trouvé deux coûts que les gates sous Xvfb ne voyaient pas.

**Definition of Done:** Un dépôt immobile ne lance plus qu'une sonde diff-stat par poll de 30 s, et une fenêtre focalisée ne dessine au repos que le clignotement du curseur, qui partage ses frames avec le spinner. Les deux sont bornés par `perf_gates`.

#### US-021: Ne plus relancer une sonde git sur ses propres lectures
**Description:** As a utilisateur multi-agents, I want que Paneflow ne sonde un dépôt que quand son état git change so that le desktop ne lance pas git en continu dans chaque dépôt ouvert.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le watcher git ignore les événements d'accès (`notify` 7 rapporte `IN_OPEN` sous Linux) et garde les créations, modifications et renommages de `HEAD` et `index`.
- [ ] Given un workspace sur un dépôt immobile, when on mesure 35 s, then `git_spawns.by_subcommand.diff` augmente d'au plus 2. La mesure d'avant, 44 sondes en 30 s en local et 15 en 35 s en CI, est citée.
- [ ] Un `git commit` et un `git switch` rafraîchissent toujours le dépôt sans attendre le poll.
- [ ] `desktop.diff_stat.probes` borne le nombre de sondes dans `perf_gates`.

#### US-022: Faire partager au clignotement les frames du spinner, et mesurer la fenêtre focalisée
**Description:** As a mainteneur, I want que les gates du desktop mesurent une fenêtre focalisée so that leurs bornes décrivent l'usage réel et pas une fenêtre que Xvfb ne rend jamais active.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le clignotement bascule toutes les 540 ms sur la grille de 90 ms du spinner, calée sur la même époque, de sorte qu'un agent en réflexion dans une fenêtre focalisée dessine environ 11,1 frames par seconde au lieu de 13,0.
- [ ] `perf_gates` donne le focus X à la fenêtre par `xdotool` et borne `desktop.focused_idle.root_renders_per_s` à 2 ; la mesure de réflexion se fait dans cette fenêtre focalisée.
- [ ] Échec : given une fenêtre qui ne prend pas le focus ou un terminal qui ne clignote pas, when le gate mesure, then le budget est manquant avec sa raison, jamais satisfait.

---

## Functional Requirements

- FR-01: Le host, le worker et le desktop doivent exposer des compteurs de travail monotones, nommés, accompagnés de l'identité du processus, par `host.status`, `worker.status` et `system.counters`.
- FR-02: Un lecteur de compteurs doit rapporter `pending` avec sa raison pour un compteur absent, et invalider un delta qui traverse un redémarrage de processus.
- FR-03: La suite persistante doit proposer un scénario de 1, 4 et 8 sessions qui impriment en continu, et enregistrer CPU par thread, RSS et deltas de compteurs.
- FR-04: Le desktop ne doit pas redessiner la fenêtre plus de 12 fois par seconde à cause du spinner de la sidebar, ni à cause d'un snapshot d'agents identique au précédent.
- FR-05: Le host ne doit pas lister tous les processus du système plus de 2 fois par seconde, quel que soit le nombre de sessions actives.
- FR-06: Le worker ne doit pas diffuser de snapshot dont le contenu n'a pas changé, sauf le snapshot initial d'une connexion ou d'un redémarrage.
- FR-07: Une sonde git ne doit pas relancer la requête des filtres tant que les fichiers de config d'origine de ce dépôt n'ont pas changé, et la neutralisation des filtres ne doit jamais être sautée.
- FR-08: Le démarrage du desktop ne doit pas dormir sur le thread GPUI pour détecter une instance existante.
- FR-09: Le job `perf_gates` doit échouer quand un budget est dépassé ou qu'une mesure manque, avec une ligne par budget et une commande de reproduction.
- FR-10: Le comparateur A/B doit rejeter un run dont un p50 de la cohorte A/A n'est pas calibré, ne juger un p95 que si son propre A/A tient, et ne jamais rendre un verdict de régression sans les deux distributions.
- FR-11: Une baseline ne doit jamais être écrite depuis un arbre sale ni comparée à une mesure d'une autre plateforme.
- FR-12: Aucun workflow de performance ne doit se déclencher sur `pull_request_target`, ni partager une clé de cache avec `release.yml`.

## Non-Functional Requirements

- **Performance:**
  - Un incrément de compteur coûte moins de 50 ns en release.
  - Une lecture de `system.counters`, `host.status` ou `worker.status` répond en moins de 5 ms p95 sur 100 lectures.
  - Desktop avec un agent en réflexion : au plus 12 rendus racine par seconde.
  - Desktop au repos : au plus 3 rendus racine par 30 s.
  - Host avec 8 sessions actives : au plus 2 listings système par seconde.
  - Démarrage avec un socket périmé : étape IPC au plus 5 ms p95.
- **Security:**
  - Les workflows de performance tournent avec `permissions: contents: read`, sans secret, sans `pull_request_target`, avec des caches préfixés.
  - Les compteurs ne contiennent ni chemin, ni nom de session, ni contenu de terminal : uniquement des entiers.
  - La neutralisation des filtres git et les options de sonde restent celles de `src-app/src/git_command.rs:23-35`.
- **Accessibility:** avec « Reduce motion », le spinner est statique et provoque 0 rendu racine périodique (US-004).
- **Scalability:** Les budgets du host tiennent à 8 sessions actives dans le job CI et à 50 sessions dans le scénario persistant existant, avec un coût de listing indépendant du nombre de sessions.
- **Reliability:**
  - Les gates déterministes : 0 échec parasite sur 30 runs consécutifs avant que le PRD soit clos.
  - Le job `perf_gates` dure au plus 30 min.
  - Le job `perf-ab` dure au plus 60 min.
  - Une mesure absente échoue le job, jamais l'inverse.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Aucune baseline pour la plateforme | Premier run sous macOS | La suite mesure sans comparer | « pas de baseline pour macos-aarch64 » |
| 2 | Host d'une ancienne version | `host.status` sans `counters` | Chaque compteur rapporté `pending` | « compteurs indisponibles : host <version> » |
| 3 | Processus redémarré pendant une fenêtre | Crash du worker en plein échantillon | Échantillon invalidé, pas de delta négatif | « <processus> redémarré pendant la mesure » |
| 4 | Affichage virtuel absent | Runner sans lavapipe | `perf_gates` échoue, budgets desktop non sautés | « adaptateur Vulkan introuvable : <erreur> » |
| 5 | Arbre sale à l'écriture d'une baseline | `--set-baseline` avec des fichiers modifiés | Refus, code non nul | « arbre sale : commitez avant de poser une baseline » |
| 6 | Base qui ne compile pas | `main` cassé pendant une PR | A/B rapporte « base indisponible », code distinct | « base <sha> indisponible : <erreur cargo> » |
| 7 | Cohorte A/A non calibrée | Voisin bruyant sur le runner | Run rejeté, relancé une fois, hors compte de promotion | « run non calibré : A/A <métrique> +7 % » |
| 8 | Spinner avec « Reduce motion » | Option activée | Spinner statique, aucun rendu périodique | Aucun |
| 9 | Fichier git inclus modifié | Filtre ajouté via `include.path` | Requête relancée, filtre neutralisé | Aucun |
| 10 | Socket qui accepte sans répondre | Processus étranger sur le chemin | Décision en au plus 350 ms, démarrage normal | Aucun |
| 11 | Zéro session | Scénario actif sans fixture | Le scénario échoue au lieu de publier des zéros | « aucune session active ouverte » |
| 12 | Métrique sans shaper réel | Runner sans police système | Métrique `available: false`, ni acceptée ni régression | « non mesuré : shaper indisponible » |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Les compteurs fenêtrés dans le temps (listings par seconde, rendus par seconde) varient sur un runner à 2 vCPU et rendent `perf_gates` instable | Med | High | Budgets exprimés par événement quand c'est possible ; coefficient de variation mesuré par US-003 ; marge écrite à côté de chaque budget ; 30 runs sans échec parasite exigés |
| 2 | Le desktop ne tourne pas sous Xvfb ou sway headless avec lavapipe dans un runner GitHub | Med | Med | Spike US-003 avant US-013 ; repli documenté (comptage dans `TestAppContext` ou runner tiers) ; budgets host et worker indépendants de l'affichage |
| 3 | Un correctif casse une garantie existante (orphelins suivis, filtres neutralisés, transitions du worker) | Med | High | Critères d'échec dédiés ; tests existants inchangés ; test de sécurité `include.path` en US-009 |
| 4 | Le gate A/B échoue à tort et finit désactivé, comme ailleurs | Med | Med | Cohorte A/A obligatoire, mode ombre, critère de promotion écrit, seconde exécution avant tout verdict |
| 5 | Les régressions Windows et macOS restent invisibles à des gates Linux | High | Med | Compteurs sur les trois OS ; protocole manuel sur Windows en dual boot pour les changements de rendu ; parité des correctifs vérifiée par inspection |
| 6 | Empoisonnement de cache ou exécution de code de fork par un workflow de performance | Low | High | `pull_request` uniquement, `permissions: contents: read`, caches préfixés, test sur les fichiers de workflow |
| 7 | Le coût CI (builds release supplémentaires) ralentit chaque PR | Med | Low | Un seul profil déclaré et mis en cache ; A/B limité aux PR des chemins chauds et à la nuit ; plafonds de 30 et 60 min |
| 8 | Les sept candidates n'expliquent pas tout le ralentissement perçu | Med | Med | Mesure de `main` avant EP-002 (US-002) et comparaison des versions publiées (US-019) ; une cause nouvelle devient une story ajoutée au PRD |

## Non-Goals

- **Réécrire le double parse host puis desktop, le transport base64 dans JSON ou le saut supplémentaire par frappe** introduits en v0.16.0. C'est un choix d'architecture qui mérite son propre PRD ; ce PRD le mesure seulement (RSS par session, CPU sous `flood`).
- **Changer la sémantique de durabilité des hooks.** L'acquittement après fsync vient d'US-023 du PRD `prd-agent-integration-overhaul` ; il est gaté (US-012), pas modifié.
- **Gater en CI le temps de frame absolu, la charge GPU ou la latence frappe-pixel.** Les runners partagés n'ont pas de GPU fiable ; ces mesures relèvent du protocole manuel (US-019).
- **Gates CI sous macOS et Windows en v1.** Les compteurs y fonctionnent ; les gates suivront si le protocole manuel révèle des régressions propres à une plateforme.
- **Runners auto-hébergés, bare metal ou services tiers** (CodSpeed, Bencher, Blacksmith). À reconsidérer seulement si le mode ombre de l'A/B échoue à se calibrer.
- **Envoyer les compteurs à la télémétrie.** Aucune métrique de performance du desktop ne part vers PostHog.
- **Corriger le dépôt pf.** La comparaison d'artefacts PGSO y vise encore `vercel-labs/fx` (`pf/benchmarks/pgso_artifacts.test.ts:283`) ; ce dépôt est hors périmètre.

## Files NOT to Modify

- `src-app/Cargo.toml` : les quatre valeurs `rev` de GPUI (`fecc3273...`) et la feature `font-kit` de `gpui_platform`. Un passage à `bench_metrics` de Zed exigerait une montée de pin, hors périmètre.
- `native/libghostty/manifest.toml`, les archives sous `native/libghostty/prebuilt/` et les plafonds de `src-app/build.rs`.
- `src-app/src/terminal/pty_session/session_backend.rs`, `src-app/src/terminal/types.rs` et son test de garde `alacritty_is_absent_from_the_app_crate`.
- `.github/workflows/release.yml` : aucun gate de performance n'est ajouté aux jambes de release, dont un échec force à recréer le tag. La vérification passe par le runbook (US-019).
- `crates/paneflow-shim`, `crates/paneflow-ai-hook`, `crates/paneflow-mcp` : ni compteurs ni nouvelle dépendance.
- `src-app/src/git_command.rs` : `PROBE_CONFIG`, `PROBE_ENV` et `NEUTRALIZED_FILTER_SETTINGS` restent tels quels. US-009 ajoute un cache autour de la requête, sans relâcher ce qu'elle neutralise.
- La chaîne de signature des versions (minisign, signature macOS et Windows).

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture des compteurs :** une petite crate sans dépendance, partagée par le host, le worker et le desktop, plutôt qu'un module dupliqué par crate ? Recommandé : la crate, pour que le lecteur CI et les trois processus partagent le même vocabulaire de noms. L'ingénierie confirme qu'elle n'entre dans aucun binaire d'aide.
- **Compter les frames :** compter les rendus de la vue racine (fonctionne en release, sans toucher GPUI), ou activer la feature `profiler` de GPUI dans un build de CI (`FrameTimingCollector`, déjà utilisé en dev-dependency par `src-app/src/layout/render.rs:290-331`) ? Recommandé : rendus racine pour les gates, `profiler` pour le temps de frame du protocole manuel.
- **Exposition :** étendre `host.status` et `worker.status`, et ajouter `system.counters` au desktop ? Ou une méthode `*.counters` homogène dans les trois processus ? Recommandé : étendre l'existant et n'ajouter que la méthode du desktop.
- **Profil de build :** un profil `profiling` déclaré une fois (hérite de `release`, `debug = "line-tables-only"`, `strip = false`) pour `perf_gates`, l'A/B et les flamegraphs ? Faut-il garder LTO et `codegen-units = 1` pour la fidélité, au prix du temps de build ? Les compteurs et les allocations n'en dépendent pas ; le temps réel de l'A/B oui, mais base et head partagent le profil.
- **Affichage virtuel :** Xvfb ou sway headless (le second a servi pour la démo HN) ? Décision par US-003.
- **Listing des processus :** cache partagé du host avec une durée de vie de 500 ms, ou recul exponentiel par session quand l'ensemble des descendants est stable ? Le cache partagé borne le coût indépendamment du nombre de sessions ; le recul réduit aussi le cas d'une seule session.
- **Badge squircle :** pré-rastériser le badge une fois par (taille, échelle, thème) en sprite, ou garder des `Path` en cache ? Un chemin en cache évite la tessellation CPU, mais GPUI le redessine à chaque frame. Le sprite supprime aussi ce coût GPU.
- **Détection d'instance :** verrou de fichier (`flock` sous Unix, `LockFileEx` sous Windows) ou une seule tentative de connexion ? Le verrou supprime la course entre deux lancements ; la tentative unique est le plus petit changement.
- **Dépendances :** Gungraun 0.20.x en dev-dependency d'une cible `[[bench]]`, seulement si US-017 valide. dhat-rs est écarté : l'allocateur comptant existant (`src-app/src/bench_harness.rs:7-45`) fournit déjà des allocations déterministes, et dhat exige de posséder l'allocateur global.
- **Migration :** le schéma du document persistant passe de 3 à 4 ; les anciens résultats restent lisibles comme archives, sans comparaison. Aucune donnée utilisateur n'est migrée.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Rendus racine par seconde du desktop, 1 agent en réflexion | Non mesuré, attendu égal à la cadence d'affichage | ≤ 12 | Month-1 | `system.counters`, scénario d'US-013 |
| Listings système des processus par seconde, 8 sessions actives | ≈ 16 (2 Hz par session, lecture du code) | ≤ 2 | Month-1 | `host.status`, scénario d'US-012 |
| Snapshots du worker diffusés par minute sans changement | 30 | 0 | Month-1 | `worker.status`, scénario d'US-012 |
| Processus git par sonde diff-stat, config inchangée | 5 | 3 | Month-1 | `system.counters`, test d'US-009 |
| Étape `ipc_server_started` au démarrage Linux | 140,3 ms | ≤ 5 ms p95 | Month-1 | `scripts/bench-startup.sh` |
| Contrôles de performance qui peuvent bloquer une fusion | 2 (taille des binaires d'aide, stress PTY) | ≥ 15 budgets | Month-1 | Job `perf_gates` |
| Échecs parasites des gates déterministes | N/A (new) | 0 sur 30 runs | Month-1 | Historique du job `perf_gates` |
| Baselines propres de la plateforme courante | 0 sur 4 (Windows, sales ou anciennes) | 4 sur 4 sous Linux et Windows | Month-6 | Test de cohérence d'US-018 |
| Gate A/B bloquant | N/A (new) | Instructions promues après 10 runs sans faux positif (EP-004) ; temps réel promu après 30 runs calibrés (US-020) | Month-6 | `bench/README.md`, historique de `perf-ab.yml` |

## Open Questions

- `tests_pass` est-il un check requis par la protection de `main` aujourd'hui ? Sinon, `perf_gates` n'empêche rien. Arthur, avant la fusion d'US-011 ; le réglage se fait hors du dépôt.
- Une machine macOS est-elle disponible pour le protocole manuel ? Arthur, avant US-019 ; sinon macOS reste « non mesuré ».
- La diffusion d'une entrée de session toutes les 500 ms pour chaque session qui imprime (`610e6fc6`) est-elle nécessaire au worker à cette cadence, ou une diffusion sur changement d'état réduit suffit-elle ? À trancher par l'implémentation d'US-008, avec la mesure.
- Tous les hooks doivent-ils être de classe `Critical`, avec un fsync dans le chemin de réponse, alors que le `PreToolUse` de Codex n'a pas de matcher et paie ce coût à chaque appel d'outil (`crates/paneflow-mcp-install/src/integrations.rs:18-28,949-951`) ? Hors périmètre de ce PRD ; à porter dans un suivi du PRD `prd-agent-integration-overhaul` si la mesure d'US-012 le justifie.
- Résolu le 2026-10-06 : l'échec de la suite persistante sous Linux (`NFR-04.runtime_release`, `NFR-12.host_shutdown`) venait d'un pid descendant recyclé que le propriétaire de l'arbre Unix retenait ; `762ac880` le corrige et la baseline persistante Linux est enregistrée sur ce commit.
- La première exécution du protocole matériel (`bench/results/hardware-summary-2026-10-06.md`) relève deux causes nouvelles hors des sept candidates : le desktop lance environ 1,6 sonde diff-stat par seconde dans un dépôt au repos (279 processus git par minute), et une fenêtre réelle au premier plan dépasse les bornes de rendus racine que les gates tiennent sous Xvfb (113 par minute au repos, 13 par seconde en réflexion), vraisemblablement par le clignotement du curseur. Deviennent-elles des stories de ce PRD (Risk 8) ? Arthur.
- Le spinner à pas de 90 ms convient-il visuellement dans toutes les tailles de la sidebar ? Arthur, à la passe visuelle d'US-004.
[/PRD]
