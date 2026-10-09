[PRD]
# PRD: Remédiation du re-pin libghostty-vt b699ea79

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-09 | Arthur Jean | PRD initial issu de l'audit du 2026-10-09 du PRD `tasks/prd-libghostty-b699ea79.md` (17 stories DONE, CI verte sur `958c574e`). Corrige les défauts confirmés (durée de vie OSC 7501, reset, rafales, défilement au pixel, capture du mode 2026, config, `host.status`), étend OSC 7501 aux programmes non-agent, rend falsifiables les tests qui ne l'étaient pas, réaligne la documentation, clôt le PRD source et prépare la release 0.17.7. 6 epics, 20 stories. |
| 1.1 | 2026-10-09 | Arthur Jean | Ajoute EP-007 (US-021) : démo publique d'OSC 7501 par un programme sans agent et réponse à l'annonce de la spec par Mitchell Hashimoto, après la publication de 0.17.7. 7 epics, 21 stories. |

## Problem Statement

Le re-pin libghostty-vt sur `b699ea79` est livré sur `main` et toutes ses stories sont marquées DONE. Six audits parallèles, recoupés ligne à ligne, montrent pourtant que plusieurs comportements livrés contredisent la spécification ou le PRD source, et que certaines preuves ne prouvent rien.

**Convention de référence.** Les chemins sans préfixe désignent Paneflow au commit `958c574e`. `ghostty/` désigne le clone de référence `/home/arthur/dev/ghostty` au commit `b699ea79`. « Spec » désigne la page https://www.superlogical.com/rex/docs/build/program-status telle que lue le 2026-10-09.

1. **Les enregistrements OSC 7501 ne suivent pas la spécification.**
   - Le début d'une invite efface tous les enregistrements (`crates/paneflow-host/src/runtime.rs:1285-1288` appelle `clear()`). La spec et `ghostty/include/ghostty/vt/terminal.h:1312-1317` ne retirent que `working` et `blocked` : dans un shell, un `done` ou un `error` vit quelques millisecondes. Le test `runtime.rs:2569` fige l'écart.
   - Un reset demandé par l'utilisateur (`Command::Reset`, `runtime.rs:1396-1399`) appelle `ghostty_terminal_reset`, qui ne déclenche aucun callback (`ghostty/src/terminal/c/terminal.zig:1693-1701`). Les enregistrements et la progression OSC 9;4 survivent au reset. Après un RIS, le titre n'est pas rafraîchi, alors que l'en-tête upstream le demande.
   - Au-delà de 64 événements de statut en attente, les plus récents sont jetés (`crates/paneflow-terminal-ghostty/src/callbacks.rs:237-254`). Un `CLEAR`, un état final ou un début d'invite peut se perdre, ce qui laisse un `working` ou un `blocked` périmé.
2. **Un programme non-agent qui émet OSC 7501 n'atteint jamais l'interface.** Sans runtime agent reconnu, la classification est abandonnée (`crates/paneflow-host/src/viewport_scan.rs:229`), puis la session est écartée par serve (`crates/paneflow-serve/src/state.rs:331-342`) et par l'app (`src-app/src/app/host_agents.rs:312`). Le Definition of Done d'EP-003 du PRD source et son objectif « 100 % des sessions sans hooks » ne sont donc pas tenus pour `cargo`, `terraform` ou `brew`, l'usage même que vise la spec. Par ailleurs, `error` est mappé vers `idle` (`viewport_scan.rs:126-135`) alors qu'`Errored` existe (`state.rs:56,74`), et la notification nomme le workspace, pas le pane (`src-app/src/agents/notifications.rs:86-102`).
3. **Deux défauts de rendu sont visibles.**
   - Le drapeau `smooth_scroll` n'est jamais remis à faux (seule l'initialisation `src-app/src/terminal/view.rs:655` l'écrit à faux). Après un geste trackpad, une application qui lit la souris voit la grille décalée d'une fraction de ligne, et activer `reduce_motion` n'enlève pas le décalage déjà présent (`src-app/src/terminal/input.rs:1272-1322,1383-1389`).
   - La capture du mode 2026 et la publication en direct partagent un seul render state (`callbacks.rs:54`, `crates/paneflow-terminal-ghostty/src/snapshot.rs:77-92`). Un défilement ou une sélection pendant une hold écrase la frame capturée, et la publication tenue suivante montre le contenu vivant, parfois à moitié dessiné.
4. **Des preuves ne peuvent pas échouer.**
   - La boucle de 1 000 redimensionnements (`crates/paneflow-terminal-ghostty/tests/upstream_fixes.rs:142-153`) efface le caractère large avant chaque réduction et n'asserte rien.
   - Les tests de `2b0ceff7d` passent aussi contre la bibliothèque d'avant le correctif.
   - Le refus d'une notice ou d'un SBOM modifié n'existe nulle part dans `crates/paneflow-libghostty-sys`.
   - Les discriminants sont comparés à des littéraux, pas à `ghostty_type_json()`.
   - Les budgets de capture (2 ms) et d'overscan (5 %) ne figurent pas dans `bench/baselines/linux-x86_64/terminal.json`.
5. **Robustesse et mémoire.**
   - Une `xt_checksum_extension` hors plage au démarrage remet toute la config aux défauts (`crates/paneflow-config/src/loader.rs:111-116`), alors que toutes les autres clés retombent individuellement.
   - `host.status` omet les sessions sans runtime (`crates/paneflow-host/src/host.rs:511-514`) et interroge les sessions l'une après l'autre avec 500 ms chacune (`runtime.rs:50`).
   - L'historique restauré compressé n'est jamais recompressé après une recherche, car Paneflow n'appelle pas l'API de compression pilotée par l'appelant (`ghostty/include/ghostty/vt/terminal.h:45-54`).
6. **La documentation et le suivi sont en retard sur le code.**
   - `ARCHITECTURE.md:197-214` décrit encore le sondage du mode 2026.
   - La note d'Upgrade (`CHANGELOG.md:10`) annonce `Incompatible` après toute mise à jour, alors que la mise à jour depuis l'app arrête déjà le host.
   - Les clés `xt_checksum_*` sont dans le miroir généré `docs/user`, mais absentes du site `paneflow-web` : la prochaine resynchronisation les effacera.
   - Le PRD source a quatre questions ouvertes sans réponse et un statut `IN_PROGRESS`.
   - La version reste 0.17.6, comme la dernière release.

**Why now:** rien de ce travail n'est encore publié : la version 0.17.6 ne contient pas le re-pin. Corriger avant la release 0.17.7 évite de livrer un statut qui ment (`done` effacé, `working` coincé) dans la fonctionnalité qui doit positionner Paneflow sur OSC 7501, alors que la spec date du 2026-10-06 et qu'aucun autre cockpit n'en lit encore les enregistrements pour des programmes ordinaires. La documentation publique du site, elle, sera écrasée à la prochaine synchronisation si rien ne bouge.

## Overview

Le PRD corrige d'abord ce qui est faux, puis étend ce qui manque, puis prouve, documente et livre.

**EP-001 et EP-002 (P0)** corrigent les défauts confirmés.
- Les enregistrements OSC 7501 suivent la spec :
  - au début d'une invite et à la sortie, `working`, `blocked` et `idle` disparaissent ;
  - `done` et `error` restent jusqu'à la première frappe réelle de l'utilisateur, que l'host détecte déjà (`input_at_ms`, `runtime.rs:1370-1374`) ;
  - un reset manuel vaut un RIS ;
  - aucune rafale ne fait perdre une transition.
- Le défilement au pixel ne s'applique que lorsque Paneflow fait défiler son propre scrollback.
- La frame capturée d'une hold survit aux publications en direct, grâce à un render state dédié.

**EP-003 (P1)** porte l'état déclaré de chaque session, agent ou non, de l'host jusqu'à l'app, par un champ optionnel du manifeste et de l'entrée de snapshot, sans nouvelle chaîne wire. L'état déclaré s'affiche dans la puce de progression existante de l'en-tête du pane. Un programme non-agent `blocked` ou `error` entre dans l'Attention Queue, et `blocked` notifie en nommant le pane. Pour un agent, `error` devient `Errored`.

**EP-004 (P1)** remplace les tests qui ne peuvent pas échouer, compare les discriminants au JSON de types upstream, vérifie les hash de notice et de SBOM, limite la config invalide à sa clé, rend `host.status` complet et borné, et recompresse au repos l'historique restauré.

**EP-005 (P1)** réaligne la documentation interne, publique (`paneflow-web`, puis resynchronisation) et le PRD source, qu'il clôt.

**EP-006 (P1)** regroupe la passe visuelle Linux, la vérification sur le matériel macOS et Windows, puis la préparation de la release 0.17.7.

**EP-007 (P2, epic final)** prépare, une fois 0.17.7 publiée, une courte démo d'un script sans agent qui déclare son état, et la réponse à l'annonce de la spec par Mitchell Hashimoto. La publication reste l'action d'Arthur.

Décisions structurantes et leur preuve :
- **Les hooks gardent la précédence** (`AgentStateSource`, `crates/paneflow-ipc-client/src/agent.rs:53-79`). Aucune variante wire d'`AgentState` ni chaîne `SCREEN_*` ne change : l'erreur passe par la variante `Errored` existante.
- **Aucun nouvel élément d'interface permanent.** La puce de pane et l'Attention Queue existent déjà, et l'Attention Queue est la surface de triage retenue.
- **Le desktop n'installe toujours pas le callback OSC 7501**, sinon il répondrait au PTY une seconde fois (`src-app/src/terminal/ghostty_session/events.rs:298-304,318`). L'état arrive de l'host, qui survit au redémarrage du desktop.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Règles de durée de vie de la spec OSC 7501 couvertes par un test qui échoue si la règle casse | 7/7 (remplacement, sous-arbre, vidage, prompt, sortie, vu, RIS et reset) | 7/7 à chaque re-pin |
| Transitions de statut perdues sur un bloc lu de 32 Kio rempli de rapports minimaux | 0 | 0 |
| Sessions non-agent dont l'état OSC 7501 déclaré est visible dans le pane | 100 % des sessions qui émettent OSC 7501 | 100 % |
| Frames avec un décalage au pixel non nul pendant que l'application lit la souris, en défilement alterné ou avec `reduce_motion` | 0 | 0 |
| Tests de correctifs upstream qui échouent quand le correctif est retiré | 100 % de ceux d'US-012 | 100 % |
| Sections de documentation signalées par l'audit et réalignées | 100 % (liste d'US-016 et US-017) | 0 dérive au re-pin suivant |

## Target Users

### Développeur qui fait tourner des agents et des outils longs dans Paneflow
- **Role:** utilisateur sous Linux, macOS ou Windows, avec des agents CLI (Claude Code, Codex, OpenCode) et des commandes longues (`cargo`, `terraform`, `brew`, déploiements) dans des panes côte à côte.
- **Behaviors:** lance un build ou un agent puis va ailleurs ; revient quand la sidebar, l'Attention Queue ou une notification l'appelle ; défile au trackpad ; utilise des TUI qui lisent la souris (htop, lazygit, Neovim).
- **Pain points :**
  - un `done` déclaré disparaît avant qu'il ne revienne ;
  - un `working` reste affiché après un `reset` ;
  - un `terraform` bloqué sur une confirmation n'apparaît nulle part ;
  - une TUI qui lit la souris se décale d'une demi-ligne après un geste trackpad ;
  - un TUI en mode 2026 laisse voir une frame à moitié dessinée après un défilement.
- **Current workaround:** regarder chaque pane un par un, relancer le pane, éviter le trackpad dans les TUI.
- **Success looks like:** le pane dit ce que le programme a déclaré, jusqu'à ce que l'utilisateur l'ait vu ; ce qui l'attend est dans l'Attention Queue ; le défilement ne déplace jamais la grille d'une application qui gère la souris.

### Mainteneur de Paneflow
- **Role:** Arthur, seul contributeur, et les agents qui implémentent les stories.
- **Behaviors:** re-pin libghostty par le workflow de bump, s'appuie sur les tests du wrapper pour détecter une régression upstream, publie depuis un runbook, vérifie Windows sur son dual boot.
- **Pain points :**
  - des tests verts qui ne prouvent rien ;
  - une documentation publique qui sera écrasée ;
  - un PRD source marqué terminé alors que son objectif n'est pas atteint ;
  - pas de version prête pour publier le re-pin.
- **Current workaround:** relire le code à la main pour savoir ce qui est vraiment tenu.
- **Success looks like:** chaque correctif upstream traversé a un test qui échoue sans lui, la documentation dit ce que fait le code, et la release 0.17.7 est prête à tagger après les vérifications réelles.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Rex (Superlogical) :** implémentation de référence d'OSC 7501.
  - Il marque d'un indicateur les sessions qui ont des tâches terminées ou bloquées dans son sélecteur.
  - Il affiche un spinner ou un symbole dans l'en-tête des onglets non focalisés.
  - Il expose un événement Lua `terminal.program_status_changed` (spec, section « In Rex »).
  - Il n'est pas encore public, donc on n'a aucun retour d'utilisateur.
  - Paneflow fait la même chose sans nouveau chrome, par la puce de pane et l'Attention Queue, et en multiplateforme.
- **iTerm2, kitty, WezTerm, tmux :** alerte sur une marque, `notify_on_cmd_finish`, OSC 9;4, `monitor-activity`. Tous binaires, sans distinction entre `done` et `error` et sans enregistrement gardé jusqu'à ce qu'il soit vu.
- **Market gap:** lire l'état déclaré de n'importe quel programme, pas seulement des agents, et le garder jusqu'à ce que l'utilisateur l'ait vu. Aucun émetteur grand public d'OSC 7501 n'est confirmé à ce jour, donc l'avance porte sur le consommateur.

### Best Practices Applied
- **Durée de vie selon la spec :**
  - le début d'une invite (OSC 133 A) et la sortie du programme MUST retirer `working` et `blocked`, et MAY retirer `idle` ;
  - `done` et `error` survivent jusqu'à ce que le terminal juge qu'ils ont été vus, l'exemple de la spec étant le retour de l'utilisateur et une frappe ;
  - un RIS retire tout, un DECSTR rien.
- **Héritage d'`app` :** un enregistrement sans `app` le prend à son ancêtre le plus proche (MUST de la spec).
- **Texte non fiable :** neutraliser les surcharges de direction et les caractères invisibles, nommer le terminal d'origine, limiter le débit de ce que l'enregistrement déclenche hors du terminal (spec, section Security).
- **Défilement :** quand l'application lit la souris, la molette devient des événements de bouton par lignes entières. Le fractionnaire s'accumule et ne s'émet qu'en lignes entières, l'approche de Zed et d'Alacritty ([Zed](https://git.secluded.site/zed/commit/6c712d88e4bd46d5aabf852fcbcd67a6ceffaf78), [jvns](https://jvns.ca/til/two-ways-the-mouse-wheel-works-in-the-terminal/)).
- **Config :** une clé invalide retombe seule à sa valeur par défaut et le reste s'applique. C'est le comportement de Ghostty, WezTerm et Alacritty, et déjà celui de toutes les autres clés de Paneflow (`crates/paneflow-config/src/schema/terminal.rs:2-4,166-198`).
- **ConPTY :** il transmet les OSC inconnus, mais peut les couper autour d'autres sorties ([Warp](https://www.warp.dev/blog/building-warp-on-windows)) ; à vérifier sur matériel.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- **`is_user_input` distingue une frappe réelle d'un envoi programmatique** (`crates/paneflow-host/src/runtime.rs:1370-1374`), ce qui permet de l'utiliser comme signal « vu ». La raison : serve s'en sert déjà pour la même distinction (`state.rs:237`). US-001 le vérifie par un test.
- **Un bloc lu de 32 Kio** (`READ_CHUNK_BYTES`, `runtime.rs:20`) **rempli de rapports minimaux tient dans un budget de 256 Kio d'octets en attente.** Raison : environ 1 700 rapports de 19 octets, avec un surcoût par événement compté de moins de 100 octets. US-003 le prouve.
- **Un second render state par terminal du desktop coûte moins de 10 % de la mémoire résidente du terminal sur un écran de 200 × 60.** Raison : un render state contient une copie du viewport, pas du scrollback. US-006 mesure.
- **GPUI livre des deltas `ScrollDelta::Pixels` pour le trackpad sous macOS et pour le pavé tactile de précision sous Windows**, et `ScrollDelta::Lines` pour une molette. US-019 le vérifie.
- **ConPTY transmet OSC 7501 sans le perdre sous Windows 10 et 11.** Raison : le billet de Warp ; aucun test public de la réponse à `OSC 7501 ; ?`. US-019 le vérifie.
- **L'API de compression pilotée par l'appelant** (`ghostty_terminal_compression_activity`, `ghostty_terminal_compress`, `ghostty/include/ghostty/vt/terminal.h:3142,3173`) **recompresse un historique restauré compressé puis décompressé par une recherche.** US-015 le vérifie.
- **Un statut déclaré tient dans le plafond de 64 Kio du manifeste** (`crates/paneflow-host/src/manifest.rs:13-15`). Raison : un `msg` fait au plus 2 048 octets décodés, un `title` 192 et un `app` 32.

### Hard Constraints
- Aucun commentaire dans le code source (AGENTS.md). L'intention passe par les noms, les types et les tests.
- Dans un fichier qui contient un `mod tests`, tout nouvel item se déclare avant ce module, quel que soit son `cfg`.
- Le thread de rendu GPUI ne bloque jamais. La recompression et les lectures mémoire s'exécutent sur les threads de session, jamais sur le thread principal.
- Les types moteur restent dans `paneflow-terminal-ghostty`. L'app ne voit que les miroirs neutres de `src-app/src/terminal/types.rs`.
- Toute invocation cargo reste `--locked`. Aucune nouvelle crate, et aucune dépendance ajoutée à `paneflow-shim`, `paneflow-ai-hook` ou `paneflow-mcp`.
- Les variantes wire d'`AgentState` et d'`AgentStateSource`, et les chaînes `SCREEN_*` (`crates/paneflow-serve/src/state.rs:24-26`), ne changent pas. Un nouveau champ du manifeste ou d'`AgentSnapshotEntry` est optionnel, avec `skip_serializing_if`.
- Humain dans la boucle : un état OSC 7501 s'affiche et peut notifier, mais ne déclenche jamais d'envoi, de soumission ou de relance sur un agent.
- Le desktop n'installe pas le callback OSC 7501 : seul le terminal de l'host répond au PTY. Le test de garde `src-app/src/terminal/ghostty_session/mod.rs:1464-1480` reste vert.
- Les actions externes demandent l'accord explicite d'Arthur : un push vers `paneflow-web`, un tag de release, une publication. Comme le travail peut arriver sur `main` sans PR, les preuves demandées par une story (chiffres, listes, commandes) vont dans le corps du commit de la story.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration
- `cargo test -p paneflow-libghostty-sys --locked` - intégrité du manifeste, des bindings et de l'ABI

Gates additionnels :
- Stories du chemin de rendu (US-005, US-006, US-007) : `scripts/bench-terminal.sh` comparé à `bench/baselines/linux-x86_64/terminal.json`, et `scripts/perf-gates.sh` vert. Le résultat est cité dans le corps du commit.
- Stories qui touchent un chemin `#[cfg(windows)]` ou ConPTY : le job « Windows x86_64 libghostty check » passe après le push. Le commit dit que Windows a été vérifié par inspection ; la vérification matérielle est en US-019.
- Stories UI (US-005, US-009, US-010) : l'agent livre sans lancer l'app. La passe visuelle d'Arthur est regroupée en US-018.
- Story de démo (US-021) : aucun code Rust ne change et les gates cargo ne s'appliquent pas. Le script de démo passe `bash -n`, et `shellcheck` s'il est installé.
- Story du site (US-017) : les vérifications propres à `paneflow-web` (lint, format, build, définis dans son `package.json`) passent.
- `cargo deny check advisories licenses sources` : seulement si une dépendance change ; aucune story n'en prévoit.

## Epics & User Stories

### EP-001: Enregistrements OSC 7501 conformes à la spécification

Faire que l'état déclaré par un programme vive exactement aussi longtemps que la spec le prévoit, à travers les invites, les sorties, les resets et les rafales.

**Definition of Done:** chaque règle de durée de vie de la spec a un test au niveau du host qui échoue si la règle casse : remplacement, effacement de sous-arbre, vidage complet, invite, sortie, « vu », RIS et reset manuel. Un bloc lu de 32 Kio rempli de rapports ne fait perdre aucune transition. Un état purgé ne reste affiché nulle part en aval.

#### US-001: Appliquer la durée de vie de la spec à l'invite, à la sortie et à la frappe
**Description:** As a développeur qui lance un build puis revient plus tard, I want qu'un `done` ou un `error` déclaré reste visible jusqu'à ce que je tape dans le pane so that je trouve le résultat en revenant, au lieu d'un état effacé par l'invite suivante.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given des enregistrements `working`, `blocked`, `idle`, `done` et `error`, when le host reçoit `SemanticPrompt` de type `PromptStart` (`runtime.rs:1285-1288`), then `working`, `blocked` et `idle` sont retirés, et `done` et `error` restent.
- [ ] Given les mêmes enregistrements, when le programme de la session sort, then la même purge s'applique (`runtime.rs:1305-1306`).
- [ ] Given un enregistrement `done` ou `error`, when une entrée marquée `is_user_input` arrive dans `Command::Input` (`runtime.rs:1362-1376`), then tous les enregistrements `done` et `error` de la session sont retirés.
- [ ] Given un enregistrement `done`, when une entrée programmatique arrive (envoi IPC, `paneflow send`, `write_pane`), then l'enregistrement reste (test qui couvre les deux origines).
- [ ] Le test `runtime.rs:2569` est réécrit pour la nouvelle règle, pas supprimé.
- [ ] `ProgramStatusRecords` (`crates/paneflow-host/src/program_status.rs`) expose une opération nommée par règle : invite ou sortie, et frappe vue. `clear()` reste réservé à `CLEAR` sans id, au RIS et au reset.
- [ ] Échec : given un `done` déclaré puis un `CLEAR` d'id vide, when le host les traite, then aucun enregistrement ne reste, même sans frappe.

#### US-002: Faire d'un reset manuel l'équivalent d'un RIS
**Description:** As a développeur qui lance `reset` ou l'action « Reset terminal », I want que l'état déclaré, la progression et le titre repartent de zéro so that un `working` ou un titre de TUI périmé ne survive pas au reset.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given des enregistrements et une progression OSC 9;4, when le host traite `Command::Reset` (`runtime.rs:1396-1399`), then les enregistrements et `self.progress` sont vides (test, à côté de `a_program_reset_empties_records_and_progress`, `runtime.rs:2579-2589`).
- [ ] Given un titre posé par OSC 2, when un RIS du programme ou un reset manuel a lieu, then le host remet son titre à vide, notifie `RuntimeNotice::Title` une seule fois, et le pane revient à son nom par défaut (test côté host).
- [ ] Le desktop applique la même remise à zéro du titre sur `BackendEvent::Reset` (`src-app/src/terminal/ghostty_session/events.rs`, autour de `:315-323`).
- [ ] Le cwd connu est conservé après un reset. La décision et sa raison sont dans le commit : le processus n'a pas changé de répertoire, et le prochain OSC 7 le rafraîchit.
- [ ] Échec : given un reset manuel pendant une hold du mode 2026, when il est traité, then la hold se termine et le contenu vivant est publié (comportement existant, test gardé vert).

#### US-003: Ne perdre aucune transition de statut sous rafale
**Description:** As a mainteneur, I want que la file des événements de statut ne jette jamais un `CLEAR`, un état final ou un début d'invite so that une rafale ne laisse jamais un `working` ou un `blocked` périmé.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Le plafond en nombre `MAX_PENDING_PROGRAM_STATUS_EVENTS = 64` (`callbacks.rs:17`) est remplacé par un budget en octets `MAX_PENDING_PROGRAM_STATUS_BYTES` de 256 Kio. Il compte les chaînes copiées plus un surcoût fixe par événement, sur le modèle de `MAX_PENDING_WRITE_PTY_BYTES` (`callbacks.rs:12`).
- [ ] Given un seul `feed` de 32 Kio rempli de rapports OSC 7501 minimaux, avec des ids distincts, des `CLEAR` et des `OSC 133 A` intercalés, when le host le traite, then l'état final des enregistrements est identique à celui d'un traitement rapport par rapport (test).
- [ ] Given un dépassement du budget, when le host reçoit l'`EffectsOverflow` correspondant, then il vide tous les enregistrements et la progression, et journalise un avertissement au niveau `warn` avec le nombre d'événements perdus. Aucun état partiel ne reste.
- [ ] L'`EffectsOverflow` permet au host de reconnaître un débordement de statut, sans confusion avec les débordements d'autres effets (test).
- [ ] Échec : given 10 000 rapports aux ids distincts, when le host les traite, then la mémoire des enregistrements reste bornée à 256 entrées (test existant gardé vert).

#### US-004: Valider puis corriger la propagation d'un état purgé
**Description:** As a développeur, I want qu'un état déclaré retiré par la sortie ou l'invite disparaisse aussi de la sidebar et de l'Attention Queue so that un « working » ou un « blocked » ne reste pas affiché après la fin du programme.

Deux défauts sont déduits du code, sans être prouvés :
- `classify` garde l'état déclaré comme état stable (`viewport_scan.rs:181-188`). Après la purge, un écran qu'aucune règle textuelle ne reconnaît resterait « working ».
- La boucle de scan ne visite que les sessions vivantes (`crates/paneflow-host/src/host.rs:565`), et l'avis de sortie ne remet pas `screen_activity` ni `declared_blocker` à zéro (`host.rs:2751-2756`).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Un test du tracker reproduit la séquence suivante : un `working` déclaré, puis la purge, puis un écran qu'aucune règle ne reconnaît. Si le test échoue sur le code actuel, then la correction fait revenir l'état stable à celui d'avant la déclaration. Sinon, le commit consigne que le défaut est réfuté et le test reste comme garde.
- [ ] Un test du host et de serve reproduit une session dont le programme sort pendant un `blocked` déclaré. Given la sortie, when serve dérive le statut, then il n'est pas `Attention` pour cause de blocage déclaré. Correction si le test échoue, réfutation consignée sinon.
- [ ] Échec : given une session sortie, when le desktop se rattache plus tard, then aucun `blocked` déclaré ne réapparaît depuis le manifeste persistant (test).

---

### EP-002: Défilement au pixel et sortie synchronisée exacts

Supprimer les deux défauts de rendu visibles et mesurer ce que coûtent l'overscan et le défilement au pixel.

**Definition of Done:** aucun décalage au pixel quand l'application lit la souris, en défilement alterné ou avec `reduce_motion`. La frame capturée d'une hold survit à toute publication en direct. Les budgets de capture et d'overscan figurent dans les baselines, et une nouvelle mesure couvre la mise en page avec décalage.

#### US-005: Limiter le décalage au pixel au défilement du scrollback local
**Description:** As a développeur qui utilise une TUI qui lit la souris après avoir défilé au trackpad, I want que la grille ne soit jamais décalée d'une fraction de ligne so that la TUI reste alignée et ses clics tombent sur la bonne cellule.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] L'accumulateur du défilement au pixel est séparé de celui des branches qui lisent la souris et du défilement alterné (`input.rs:1272-1322`). Ces branches n'écrivent plus la valeur que lit `smooth_scroll_offset()`.
- [ ] `smooth_scroll_offset()` (`input.rs:1383-1389`) renvoie 0 px quand l'application lit la souris, quand l'écran alternatif est en défilement alterné ou quand `reduce_motion` est vrai, quel que soit l'état du drapeau (test par cas).
- [ ] Given un décalage non nul et `reduce_motion` qui passe à vrai, when la frame suivante est construite, then le décalage vaut 0 px sans attendre un nouvel événement de défilement (test sur la lecture de `reduce_motion`, `src-app/src/ui_primitives.rs:83-91`).
- [ ] Given un décalage non nul en bas du scrollback, when l'utilisateur tape une touche, then le décalage revient à 0 et la ligne d'invite n'est plus coupée (test, sur le modèle des remises à zéro clavier `input.rs:413,439`).
- [ ] Échec : given un geste trackpad suivi d'un programme qui active la lecture de la souris, when la souris défile dans ce programme, then des événements de molette par lignes entières lui sont envoyés et le décalage reste à 0 (test).

#### US-006: Garder la frame capturée d'une hold à travers les publications en direct
**Description:** As a développeur qui défile ou sélectionne pendant qu'un TUI redessine en mode 2026, I want que le pane revienne à la frame finie quand j'arrête so that je ne vois jamais une frame à moitié dessinée après une interaction.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `snapshot_live` (`crates/paneflow-terminal-ghostty/src/snapshot.rs:46-48`) met à jour un render state distinct de celui que capture `begin_render_hold` (`callbacks.rs:111-126`). La frame capturée et sa barre de défilement ne sont jamais écrasées par une publication en direct.
- [ ] Given la frame A, une hold, la frame B complète, la libération, une nouvelle hold et une frame C partielle, when un `snapshot_live` puis un `snapshot` tenu ont lieu, then le premier renvoie le contenu vivant et le second la frame B (test qui prolonge `tests/display_terminal.rs:135-160`).
- [ ] La mémoire résidente ajoutée par le second render state sur un écran de 200 × 60 est mesurée par `memory_usage()` ou le harnais de bench, et citée dans le commit. Au-delà de 10 % du terminal, le commit justifie le choix ou applique une création paresseuse.
- [ ] Given une restauration de snapshot avec le mode 2026 déjà actif, when la session est attachée, then le délai de 150 ms (`SYNC_OUTPUT_MAX_HOLD`, `src-app/src/terminal/ghostty_session/mod.rs:79`) s'applique, ou le mode est remis à zéro à l'attache. Un test fixe le comportement retenu.
- [ ] Le test des 1 000 itérations (`src-app/src/terminal/ghostty_session/publish.rs:542-559`) ne dépend plus d'une horloge réelle.
- [ ] Échec : given un render state secondaire impossible à créer, when le terminal est construit, then la construction échoue avec une erreur du wrapper, sans panique.

#### US-007: Mesurer et garder les budgets de rendu de l'overscan et du défilement au pixel
**Description:** As a mainteneur, I want que les budgets annoncés par le PRD source soient dans les baselines et que la mise en page avec décalage ait sa mesure so that une régression de coût de rendu échoue en local et en CI au lieu de passer inaperçue.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005, US-006

**Acceptance Criteria:**
- [ ] La ligne d'overscan au-dessus du viewport (`src-app/src/terminal/element/mod.rs:829-848`) n'est mise en page que lorsque le décalage est non nul. Une frame à décalage nul n'en construit aucune (test).
- [ ] `src-app/src/terminal/perf_bench.rs` gagne un scénario de mise en page avec décalage non nul et overscan, nommé sur le modèle de `layout_echo_uncached`. Sa valeur p95 vaut au plus 1,10 fois celle de la même frame sans décalage, chiffre cité dans le commit.
- [ ] `bench/baselines/linux-x86_64/terminal.json` contient `render_hold_capture_200x60`, `publish_scroll_overscan_220x60` et le nouveau scénario, enregistrés par `scripts/bench-terminal.sh --set-baseline` depuis un arbre propre, dans un commit séparé.
- [ ] `bench/baselines/linux-x86_64/terminal-alloc.json` contient le nouveau scénario, enregistré par `scripts/perf-gates.sh --refresh-alloc-baselines` depuis un arbre propre.
- [ ] La capture d'une hold reste sous 2 ms au p95 sur 200 × 60 en release, et la publication avec overscan `{1, 1}` sous 5 % de surcoût (`scripts/bench-terminal.sh`, chiffres cités).
- [ ] `#[allow(clippy::too_many_arguments)]` de `element/mod.rs:889` porte une `reason`.
- [ ] Échec : given un dépassement d'un de ces budgets, when la mesure est prise, then la story reste `IN_PROGRESS` avec le chiffre, sans baseline réenregistrée pour masquer l'écart.

---

### EP-003: Statut OSC 7501 pour tout programme

Faire voir l'état déclaré de n'importe quel programme, agent ou non, avec les surfaces existantes.

**Definition of Done:** un `cargo`, un `terraform` ou un script qui émet OSC 7501 dans un pane sans agent montre son état dans l'en-tête du pane. Un programme non-agent `blocked` ou `error` est dans l'Attention Queue, et `blocked` notifie en nommant le pane. L'`error` déclaré d'un agent devient `Errored`. Les hooks gardent la précédence.

#### US-008: Porter l'état déclaré de chaque session jusqu'à l'app
**Description:** As a développeur qui lance des outils longs dans des panes sans agent, I want que l'état déclaré de chaque session arrive dans l'app, agent ou non so that le pane et l'Attention Queue puissent l'afficher.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001, US-003

**Acceptance Criteria:**
- [ ] Le scan du host publie pour chaque session vivante, avec ou sans runtime agent, un champ optionnel `declared_status` dans le manifeste (`crates/paneflow-host/src/manifest.rs:135-141`) et dans `AgentSnapshotEntry` (`crates/paneflow-host/src/agent.rs:153-190`). Le champ contient `state`, `kind`, `progress`, `app`, `title` et `message`, et il est écrit seulement quand il change.
- [ ] L'enregistrement publié est la racine (id vide), ou à défaut le plus récemment mis à jour (`program_status.rs:61-67`). Son `app` est hérité de l'ancêtre le plus proche qui en a un, comme l'exige la spec (test : `build/test` sans `app` prend celui de `build`).
- [ ] Serve conserve `declared_status` pour les sessions sans runtime (`crates/paneflow-serve/src/state.rs:331-342`) et le sérialise dans les lignes envoyées à l'app (`state.rs:294-326`).
- [ ] L'app reçoit `declared_status` pour chaque surface, y compris celles qui ne deviennent pas des `AgentSession` (`src-app/src/app/host_agents.rs:312`).
- [ ] La classification des agents est inchangée : le champ s'ajoute au chemin existant sans changer la précédence `Hook > Terminal` (test existant `state.rs:2512` gardé vert).
- [ ] Le manifeste reste sous son plafond de 64 Kio avec un `message` de 2 048 octets (test).
- [ ] Échec : given un desktop qui redémarre pendant qu'une session porte un `blocked` déclaré, when il se rattache, then l'état arrive depuis l'host sans que le programme le réémette (test au niveau de serve ou du host).

#### US-009: Afficher l'état déclaré dans la puce de l'en-tête du pane
**Description:** As a développeur, I want que l'en-tête du pane dise ce que le programme a déclaré : en cours, bloqué et pourquoi, terminé, en échec so that je sache en un coup d'œil quel pane m'attend.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] La puce de progression existante (`src-app/src/pane.rs:1210-1211`, `progress_chip_label` `:2152`) affiche l'état déclaré quand il existe :
  - `working`, avec le pourcentage s'il est donné ; sans `progress`, l'état est indéterminé, comme le prévoit la spec ;
  - `blocked`, avec le `kind` (permission, question, auth) ;
  - `done` ;
  - `error`.
- [ ] Quand un état déclaré existe, il l'emporte sur la progression OSC 9;4 locale du desktop pour ce pane, conformément à la règle de la spec qui fait passer OSC 7501 avant OSC 9;4.
- [ ] Le `message` et le `title` affichés sont débarrassés des caractères U+202A à U+202E, U+2066 à U+2069, U+200B à U+200F, U+2028, U+2029 et U+206A à U+206F, puis tronqués à une ligne (test par plage).
- [ ] Une session sans état déclaré garde exactement l'affichage actuel (test).
- [ ] La correspondance entre état et libellé est une table testée valeur par valeur.
- [ ] Échec : given un `state` absent ou inconnu dans `declared_status`, when la puce est construite, then elle retombe sur l'affichage OSC 9;4, sans panique (test).

#### US-010: Mettre les programmes non-agent bloqués ou en échec dans l'Attention Queue
**Description:** As a développeur, I want qu'un `terraform` qui attend ma confirmation ou un build déclaré en échec apparaisse dans l'Attention Queue, et qu'un blocage me notifie so that je n'aie plus à inspecter chaque pane.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] `QueueRow` (`src-app/src/app/attention_queue.rs:15-22`) accepte une surface sans `AgentSession`. Son libellé est l'`app` déclaré, sinon le titre du pane, et son message est le `message` déclaré nettoyé.
- [ ] Une surface non-agent dont l'état déclaré est `blocked` ou `error` est une entrée de l'Attention Queue. `working`, `idle` et `done` n'en créent pas (test par état).
- [ ] L'entrée disparaît dès que l'état déclaré change ou que l'enregistrement est retiré, y compris après la frappe d'US-001 (test).
- [ ] Un `blocked` déclaré par un programme non-agent produit une notification de bureau par `program_notification` (`src-app/src/agents/notifications.rs:117-136`), dont le résumé nomme le pane et dont le corps est le `message` nettoyé.
- [ ] Au plus une notification par pane toutes les 10 s, quel que soit le nombre de rapports (test).
- [ ] Aucun état déclaré ne déclenche d'envoi, de soumission ou de relance sur un agent ou un programme (test qui vérifie que le chemin n'écrit jamais dans le PTY).
- [ ] Échec : given un programme qui alterne `blocked` et `working` 100 fois en une seconde, when l'app les reçoit, then une seule notification part et l'Attention Queue montre l'état final (test).

#### US-011: Faire de l'erreur déclarée d'un agent un état Errored et nommer le pane
**Description:** As a développeur qui suit ses agents dans la sidebar, I want qu'un agent qui déclare `error` apparaisse en échec, et qu'une notification de blocage nomme le pane so that je distingue un agent en échec d'un agent au repos et je sache où aller.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Serve dérive `Status::Errored` pour un agent dont l'état déclaré est `error`, en réutilisant la variante existante (`state.rs:56,74,852-856`), sans nouvelle chaîne wire (test).
- [ ] Given des hooks actifs qui disent autre chose, when l'agent déclare `error`, then l'état des hooks l'emporte (test).
- [ ] La notification de blocage d'un agent (`needs_input_for`, `notifications.rs:86-102`) nomme le pane d'origine en plus du workspace (test).
- [ ] Échec : given un `error` déclaré puis une frappe de l'utilisateur, when l'enregistrement est retiré (US-001), then l'agent quitte `Errored` à la dérivation suivante, sauf si un hook ou la sortie maintient l'erreur (test).

---

### EP-004: Preuves, config et mémoire

Remplacer les preuves qui ne prouvent rien, limiter la config invalide à sa clé, et rendre la mémoire observable et contenue.

**Definition of Done:** chaque test de correctif upstream d'US-012 échoue quand on retire le correctif ou qu'on simule son absence. Les discriminants lus par Paneflow sont comparés à `ghostty_type_json()`, et les hash de notice et de SBOM sont vérifiés. Une clé de config invalide n'affecte qu'elle-même. `host.status` liste toutes les sessions dans un délai borné. L'historique restauré est recompressé au repos.

#### US-012: Rendre falsifiables les tests de correctifs upstream
**Description:** As a mainteneur, I want que chaque test nommé d'après un correctif upstream échoue si ce correctif régresse so that un futur re-pin qui le casse soit arrêté par notre CI.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] La boucle de 1 000 redimensionnements (`crates/paneflow-terminal-ghostty/tests/upstream_fixes.rs:142-153`) place, avant chaque réduction, la tête d'un caractère large dans la dernière colonne, puis asserte après la réduction que cette cellule est vide et étroite.
- [ ] Un test de `2b0ceff7d` (`crates/paneflow-terminal-ghostty/src/terminal_ops.rs`) utilise un lecteur qui ignore l'écriture refusée, écrit de nouveau et renvoie vrai. Il asserte que le collage échoue et que rien après le refus n'atteint le PTY.
- [ ] Le test d'OSC 22 couvre la priorité de la barre de défilement : `Arrow` sur la barre, même avec une forme `pointer` demandée (`src-app/src/terminal/view.rs:2190`, priorités `:908-916`).
- [ ] Pour chaque test modifié, le commit explique pourquoi il échouerait sans le correctif, en citant la ligne upstream qui le prouve.
- [ ] Échec : given un test qui ne peut pas être rendu falsifiable par l'API du wrapper, when la story se termine, then le commit le dit et le test est renommé pour décrire ce qu'il prouve réellement.

#### US-013: Vérifier les discriminants par le JSON de types et les hash de notice et de SBOM
**Description:** As a mainteneur, I want que la validation ABI compare les discriminants au JSON de types de la bibliothèque, et qu'un test vérifie les hash de la notice et du SBOM so that une dérive d'enum ou un inventaire de licences modifié sans hash échoue dans `cargo test`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `validate_discriminants` (`crates/paneflow-terminal-ghostty/src/abi.rs:79-205`) compare chaque discriminant lu par Paneflow aux valeurs d'enum de `ghostty_type_json()`, pas seulement à des littéraux. Sont ajoutés `GhosttyOscTerminator`, `GhosttyProgramStatusKind`, `GhosttySemanticPromptPromptKind` et `GHOSTTY_RENDER_STATE_DATA_OVERSCAN_REQUEST`.
- [ ] `abi_layout.rs` valide aussi `GhosttyTerminalModeConfig`, que `release_render_hold` écrit, et le membre APC de l'union des séquences inconnues.
- [ ] Échec : given un JSON de types altéré qui renumérote une valeur d'enum lue par Paneflow, when la validation tourne, then elle renvoie `AbiMismatch` en nommant l'enum et la valeur (test).
- [ ] Un test de `crates/paneflow-libghostty-sys` calcule le SHA-256 de `native/libghostty/THIRD_PARTY_NOTICES.md` et de `native/libghostty/sbom.cdx.json`, et le compare à `notice_sha256` et `sbom_sha256` du manifeste.
- [ ] Échec : given une notice modifiée sans hash à jour, when le test tourne, then il échoue en nommant le fichier (test sur une copie altérée).

#### US-014: Limiter une xt_checksum_extension invalide à sa propre clé
**Description:** As a développeur qui édite `paneflow.json`, I want qu'une valeur `xt_checksum_extension` hors plage ne retire que cette clé so that mon thème, mon shell et mes raccourcis restent appliqués.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `xt_checksum_extension` est lue par un désérialiseur tolérant, sur le modèle des helpers `lenient_*` (`crates/paneflow-config/src/schema/terminal.rs:2-4,166-198`). `validate_xt_checksum_extension` ne fait plus échouer tout le fichier (`crates/paneflow-config/src/loader.rs:128,142-160`).
- [ ] Given `{"theme": "…", "terminal": {"xt_checksum_extension": 32}}` au démarrage, when la config se charge, then le thème s'applique, `resolved_xt_checksum_extension()` vaut 0 et un avertissement nomme la clé et la plage de 0 à 31 (test).
- [ ] La même règle s'applique au rechargement par le watcher. Le test `crates/paneflow-config/src/watcher_tests.rs:453` est mis à jour pour la nouvelle règle, et l'amendement du PRD source le note (US-016).
- [ ] Les valeurs `-1`, `1.5` et `"4"` suivent la même règle (test par valeur).
- [ ] Échec : given une valeur valide de 31, when la config se charge, then elle s'applique sans avertissement (test).

#### US-015: Rendre la mémoire observable pour toutes les sessions et recompresser l'historique restauré
**Description:** As a mainteneur, I want que `host.status` liste chaque session dans un délai borné et que l'historique restauré se recompresse au repos so that un budget mémoire se mesure sur toutes les sessions et que le gain de la compression ne disparaisse pas à la première recherche.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `resource_report` (`crates/paneflow-host/src/host.rs:504-530`) liste aussi les sessions sans runtime, avec `memory: null` et une raison parmi `not_started`, `exited` et `timeout`, conformément à `CHANGELOG.md:18`.
- [ ] Les lectures mémoire partent vers toutes les sessions avant d'attendre les réponses, avec une échéance commune de 500 ms (`MEMORY_USAGE_BUDGET`, `runtime.rs:50`). Given 64 sessions dont aucune ne répond, when `host.status` est demandé, then la réponse arrive en moins de 600 ms (test).
- [ ] Le terminal du desktop qui a restauré un historique compressé (`src-app/src/terminal/ghostty_session/attached_runtime.rs:209`) suit `ghostty_terminal_compression_activity`. Après 2 s sans activité, il appelle `ghostty_terminal_compress` par étapes bornées sur son thread de session jusqu'à ce qu'il ne reste plus de travail. Le wrapper expose ces deux appels.
- [ ] Given un historique restauré de 50 000 lignes puis une recherche qui le parcourt en entier, when 5 s d'inactivité s'écoulent, then `primary_resident_bytes` revient sous 1,2 fois sa valeur juste après la restauration (test ou mesure citée dans le commit, avec la commande qui la reproduit).
- [ ] Aucune étape de compression ne s'exécute sur le thread GPUI (test ou inspection consignée).
- [ ] Échec : given une plateforme où la compression n'est pas supportée, when l'étape de repos se déclenche, then elle s'arrête sans erreur et sans changement de comportement (test qui force le cas par le wrapper, ou inspection consignée si ce n'est pas forçable).

---

### EP-005: Documentation alignée et clôture du PRD source

Faire dire à la documentation interne, publique et au PRD source ce que fait réellement le code.

**Definition of Done:** chaque dérive listée par l'audit est corrigée. `paneflow-web` documente les nouvelles clés et le statut OSC 7501, puis le miroir `docs/user` est resynchronisé par son script. Le PRD source porte ses réponses, ses preuves manquantes et le statut `DONE`.

#### US-016: Réaligner la documentation interne et clore le PRD source
**Description:** As a mainteneur ou agent qui lit la documentation du dépôt, I want que ARCHITECTURE, DESIGN, le README des benchs et le PRD source décrivent le code livré so that le prochain travail parte de faits exacts.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-007, US-011, US-015

**Acceptance Criteria:**
- [ ] `ARCHITECTURE.md` est mis à jour aux endroits suivants :
  - `:197-214` décrit le gate de publication par l'effet render hold, avec le plafond de 150 ms et la frame capturée gardée à travers les publications en direct ;
  - `:305` ajoute OSC 7501 aux signaux de programme ;
  - `:498-516` décrit l'état déclaré, les cinq champs écrits par le scan, l'état déclaré pour toute session et la précédence des hooks ;
  - `:744-748` ajoute la mémoire par session à `host.status`.
- [ ] `DESIGN.md:416-419` compte le défilement au pixel parmi les usages de `reduce_motion`, avec la règle d'US-005.
- [ ] `bench/README.md` documente `render_hold_capture_200x60`, `publish_scroll_overscan_220x60` et le scénario d'US-007, avec leur unité et leur budget.
- [ ] `docs/release/libghostty-linux.md:77-78` cite aussi le workflow macOS, qui porte `GHOSTTY_SHA`.
- [ ] Le PRD source `tasks/prd-libghostty-b699ea79.md` gagne une entrée de Changelog 1.2 qui renvoie vers ce PRD, et consigne :
  - les réponses à ses quatre Open Questions : la mise à jour depuis l'app arrête le host, 150 ms est gardé, seul `blocked` notifie, le défilement fluide est livré et `reduce_motion` le désactive ;
  - la caducité de l'exigence `stub.rs`, supprimé en `4752a664` ;
  - la persistance d'OSC 22 à travers un RIS, comme upstream ;
  - la règle de config d'US-014 ;
  - la liste des terminaux alimentés par ConPTY (US-014 source) ;
  - la commande qui reproduit la mesure d'US-016 source ;
  - l'inspection du cas « compression non supportée ».
- [ ] `tasks/prd-libghostty-b699ea79-status.json` passe `"status": "DONE"` au niveau du PRD, sans toucher aux statuts des epics et des stories.
- [ ] Échec : given une affirmation de la documentation qu'aucun fichier ni test ne confirme, when la story se termine, then l'affirmation est retirée plutôt que laissée approximative.

#### US-017: Mettre à jour la documentation publique et le CHANGELOG
**Description:** As a utilisateur qui lit paneflow.dev ou le CHANGELOG, I want trouver les nouvelles clés, le statut OSC 7501 et une note d'Upgrade exacte so that je configure et mets à jour Paneflow sans surprise.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011, US-014

**Acceptance Criteria:**
- [ ] Dans `~/dev/paneflow-web`, les six `content/docs/configuration/schema*.mdx` documentent `terminal.xt_checksum_report` et `terminal.xt_checksum_extension`, avec la phrase de risque upstream et la règle d'US-014.
- [ ] Toujours dans `paneflow-web` : la description de `reduce_motion` mentionne le défilement au pixel, et la page des fonctionnalités décrit l'état OSC 7501 de tout programme, avec un exemple de séquence émise par un script.
- [ ] Le miroir `docs/user` de ce dépôt est régénéré par `scripts/sync-public-docs.ts` de `paneflow-web` (`docs/user/README.md:3-6`), jamais édité à la main.
- [ ] Les vérifications de `paneflow-web` passent, y compris la règle des tokens interdits en français. Le push du site attend l'accord d'Arthur.
- [ ] `CHANGELOG.md`, section `[Unreleased]` :
  - la note d'Upgrade (`:10`) limite `Incompatible` aux mises à jour hors de l'app (paquet, Homebrew, MSI, ou sortie avec « Keep sessions running ») ;
  - l'exemple OSC 7501 est remplacé par un script qui fonctionne tel quel, écrit en `printf` POSIX avec `\033` ;
  - les corrections et extensions de ce PRD sont annoncées, une phrase chacune.
- [ ] Échec : given un push vers `paneflow-web` refusé ou non autorisé, when la story se termine, then elle reste `IN_PROGRESS` avec le diff prêt, et le miroir n'est pas régénéré à partir d'un état non publié.

---

### EP-006: Vérification réelle et préparation de la release

Regrouper en fin de PRD la passe visuelle, la vérification sur le matériel macOS et Windows, et la préparation de la version 0.17.7.

**Definition of Done:** la passe visuelle Linux et la vérification matérielle sont faites et consignées, et chaque défaut trouvé est corrigé avec un test s'il est reproductible sous Linux. Le commit de version 0.17.7 est prêt, avec les contrôles du runbook verts. Le tag reste l'action d'Arthur.

#### US-018: Passe visuelle Linux des changements d'interface
**Description:** As a Arthur, I want voir chaque changement d'interface du PRD source et de celui-ci dans un build debug so that aucun défaut visuel ne parte en release.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005, US-006, US-009, US-010

**Acceptance Criteria:**
- [ ] Sur un build debug lancé par `scripts/dev.sh`, Arthur vérifie et capture :
  - un TUI en mode 2026 qui redessine pendant un défilement (US-006 et US-005 source) ;
  - une forme de pointeur demandée par OSC 22 (US-006 source) ;
  - le défilement au trackpad, puis htop ou lazygit qui lisent la souris (US-005 et US-008 source) ;
  - un script qui émet `working`, `blocked`, `done` et `error` dans un pane sans agent (US-009, US-010) ;
  - un agent bloqué (US-011 source).
- [ ] Le décalage d'inset de 6 px en haut (`src-app/src/terminal/constants.rs:16`) ne laisse ni bande vide ni ligne dupliquée quand le décalage passe par zéro.
- [ ] Les captures ou enregistrements sont dans `tasks/` comme preuve locale, et le commit de clôture les nomme.
- [ ] Échec : given un défaut constaté, when il est corrigé, then le correctif reste dans cette story avec un test s'il est reproductible sans GUI.

#### US-019: Vérifier sur le matériel macOS et Windows et corriger les écarts
**Description:** As a développeur sous macOS ou Windows, I want que le défilement au pixel, les formes de curseur, le placement des glyphes et OSC 7501 se comportent comme sous Linux so that la release 0.17.7 n'embarque pas de régression vue seulement après publication.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-018

**Acceptance Criteria:**
- [ ] Le job « Windows x86_64 libghostty check » et les lanes libghostty sont verts sur le dernier commit de la branche.
- [ ] Sur le dual boot Windows d'Arthur, avec un build debug lancé par `scripts/dev.ps1`, sont vérifiés :
  - le pavé tactile de précision produit un défilement au pixel, et une molette défile par lignes ;
  - les formes de curseur OSC 22 s'affichent ;
  - `dir /s` après un agrandissement puis une réduction ne décale plus la sortie (revérifié après `56b646ec`) ;
  - un script PowerShell qui émet OSC 7501 montre son état dans la puce et l'Attention Queue.
- [ ] Validation de l'hypothèse ConPTY : given un programme qui envoie `OSC 7501 ; ?` puis `CSI c` dans un pane Windows, when les réponses arrivent, then la réponse 7501 précède celle de DA1. Le résultat est consigné, qu'il confirme ou infirme l'hypothèse.
- [ ] Sur macOS, sur le matériel dont dispose Arthur, le trackpad produit un défilement au pixel et les formes de curseur s'affichent. Sans matériel macOS disponible, la story le dit et la vérification macOS est inscrite comme non faite dans le commit, jamais présumée.
- [ ] Avant tout commit depuis Windows, `git config user.email` du clone Windows renvoie l'adresse noreply du dépôt. Les six commits déjà signés `arthur.jean@strivex.fr` ne sont pas réécrits.
- [ ] Échec : given un défaut constaté sur l'une des plateformes, when il est corrigé, then le correctif reste dans cet epic, avec un test s'il est reproductible sous Linux, sinon avec la vérification manuelle refaite et décrite.

#### US-020: Préparer la release 0.17.7
**Description:** As a mainteneur, I want un commit de version 0.17.7 conforme au runbook so that la release qui embarque le re-pin et sa remédiation se tague en une commande après les vérifications.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-016, US-017, US-019

**Acceptance Criteria:**
- [ ] Le commit de version suit `docs/release/runbook.md:44-85` :
  - version de `Cargo.toml` à `0.17.7` ;
  - strophe en tête de `debian/changelog` ;
  - section `[Unreleased]` du CHANGELOG renommée ;
  - entrée `<release version="0.17.7">` en tête de `assets/io.github.arthurdev44.paneflow.metainfo.xml`.
- [ ] Le job CI `perf_gates` est vert sur ce commit. Les deux contrôles de performance que `runbook.md:94` réserve aux versions mineures ne s'appliquent pas à 0.17.7, et le commit le dit.
- [ ] Le test temporel `crates/paneflow-mcp-install/src/agents/opencode.rs:115-133` n'asserte plus un délai réel de moins de 100 ms. Il vérifie le comportement sans dépendre de l'horloge (test).
- [ ] Le home de développement `~/.paneflow-dev-repin` est mis à la corbeille par `gio trash` après `PANEFLOW_HOME=~/.paneflow-dev-repin target/debug/paneflow serve stop` et `host stop`.
- [ ] Aucun tag n'est créé ni poussé : le commit de clôture dit que le tag attend Arthur.
- [ ] Échec : given un contrôle du runbook rouge, when la story se termine, then elle reste `IN_PROGRESS`, et aucun contrôle n'est contourné pour verdir le commit.

---

### EP-007: Démonstration publique d'OSC 7501 hors agents

Montrer, une fois 0.17.7 publiée, qu'un programme ordinaire déclare son état dans Paneflow, et en faire une réponse à l'annonce de la spec par Mitchell Hashimoto (https://mitchellh.com/writing/program-status-osc7501), qui demande des retours d'implémentation.

**Definition of Done:** un script de démo, un enregistrement court fait sur la release 0.17.7 publiée et un brouillon de réponse sont prêts. La publication reste l'action d'Arthur.

#### US-021: Préparer la démo OSC 7501 et la réponse à l'annonce de la spec
**Description:** As a Arthur, I want une vidéo courte où un script sans agent passe par `working`, `blocked` puis `done` dans Paneflow so that ma réponse à l'annonce de Mitchell Hashimoto montre une implémentation concrète de la spec hors des agents, le cas qu'il défend.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-020

**Acceptance Criteria:**
- [ ] Un script bash de démo, sans dépendance externe, est dans `tasks/osc7501-demo/` (preuve locale, non suivie par git). Au format de `ghostty/include/ghostty/vt/terminal.h:1227`, il émet :
  - `working` avec une progression ;
  - `blocked` de type `permission` avec un message ;
  - `done`, puis il attend une frappe.
- [ ] Avant d'écrire le script, la spec (https://www.superlogical.com/rex/docs/build/program-status) est relue. Si elle a changé depuis le 2026-10-06, le script suit la version en ligne et l'écart est consigné.
- [ ] Arthur enregistre, sur la release 0.17.7 publiée et non sur un build debug, une vidéo de 30 s au plus qui montre :
  - l'état dans la puce de l'en-tête du pane ;
  - pendant `blocked`, l'entrée du pane dans l'Attention Queue et la notification qui le nomme ;
  - `done` qui reste affiché jusqu'à la première frappe.
- [ ] Un brouillon de réponse en anglais, trois phrases au plus et sans tiret dans la prose, est dans `tasks/osc7501-demo/`. Il dit ce que montre la vidéo et renvoie au repo.
- [ ] Aucun agent ne publie : la réponse à l'annonce sur X est l'action d'Arthur.
- [ ] Échec : given un état qui ne s'affiche pas comme prévu pendant l'enregistrement, when il est constaté, then la vidéo n'est pas publiée, la démo attend le correctif, et le défaut est consigné dans le statut de la story.

---

## Functional Requirements

- FR-01: Au début d'une invite (OSC 133 A) et à la sortie du programme, le host doit retirer les enregistrements `working`, `blocked` et `idle`, et garder `done` et `error`.
- FR-02: Le host doit retirer les enregistrements `done` et `error` d'une session à la première entrée marquée `is_user_input`, et seulement à ce moment ou sur `CLEAR`, RIS ou reset.
- FR-03: Un reset manuel doit vider les enregistrements, la progression et le titre de la session, comme un RIS.
- FR-04: Le système ne doit PAS perdre une transition de statut tant que les événements en attente restent sous 256 Kio. Au-delà, il doit vider tous les enregistrements plutôt que garder un état partiel.
- FR-05: Chaque session vivante, agent ou non, doit exposer son état déclaré à l'app.
- FR-06: Un programme non-agent `blocked` ou `error` doit apparaître dans l'Attention Queue. `blocked` doit produire au plus une notification par pane toutes les 10 s.
- FR-07: L'`error` déclaré d'un agent doit donner `Errored`, sauf si les hooks disent autre chose.
- FR-08: Le système ne doit PAS déclencher d'action sur un agent ou un programme à partir d'un état déclaré.
- FR-09: Le décalage au pixel doit valoir 0 tant que l'application lit la souris, que l'écran alternatif est en défilement alterné ou que `reduce_motion` est vrai.
- FR-10: Pendant une hold du mode 2026, une publication tenue doit montrer la frame capturée au début de la hold, même après une publication en direct.
- FR-11: Une `xt_checksum_extension` invalide ne doit retirer que cette clé.
- FR-12: `host.status` doit lister toutes les sessions, avec `memory: null` et une raison quand la mémoire n'est pas lisible.

## Non-Functional Requirements

- **Performance :**
  - capture d'une hold sous 2 ms au p95 sur 200 × 60 en release ;
  - publication avec overscan `{1, 1}` sous 5 % de surcoût ;
  - mise en page avec décalage au plus 1,10 fois celle sans décalage ;
  - 0 ligne d'overscan mise en page quand le décalage est nul ;
  - réponse de `host.status` en moins de 600 ms pour 64 sessions qui ne répondent pas.
- **Security :**
  - le texte déclaré affiché hors de la grille est débarrassé de U+202A à U+202E, U+2066 à U+2069, U+200B à U+200F, U+2028, U+2029 et U+206A à U+206F ;
  - toute surface affiche le pane d'origine ;
  - au plus 1 notification par pane toutes les 10 s ;
  - 0 action déclenchée par un état déclaré ;
  - 0 réponse OSC 7501 émise par le desktop (test de garde).
- **Reliability :**
  - 0 transition perdue sur un bloc de 32 Kio de rapports minimaux ;
  - 0 enregistrement `working` ou `blocked` après une invite, une sortie, un RIS ou un reset ;
  - un débordement ne laisse aucun enregistrement partiel.
- **Scalability :**
  - 256 enregistrements par session au plus ;
  - 256 Kio d'événements de statut en attente par terminal ;
  - un manifeste sous 64 Kio avec un message de 2 048 octets.
- **Memory :**
  - le second render state ajoute au plus 10 % de la mémoire résidente d'un terminal de 200 × 60, ou le dépassement est justifié dans le commit ;
  - après une recherche complète d'un historique restauré de 50 000 lignes, la mémoire résidente revient sous 1,2 fois sa valeur après restauration en 5 s d'inactivité.
- **Accessibility :** `reduce_motion: true` annule 100 % du décalage au pixel dès la frame suivante, sans attendre un événement de défilement.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Aucun état déclaré | Session ordinaire sans OSC 7501 | Puce, file et sidebar inchangées | Aucun |
| 2 | `done` puis invite | `cargo build` qui émet `done` puis rend la main au shell | `done` reste jusqu'à la première frappe | Puce « Done » |
| 3 | Frappe programmatique | Le conductor envoie du texte au pane | `done` et `error` restent | Aucun |
| 4 | Reset manuel avec un `working` | L'utilisateur lance `reset` pendant un build | Enregistrements, progression et titre vidés | Aucun |
| 5 | Rafale de rapports | Un outil émet 1 700 rapports dans un seul bloc lu | État final exact ; au-delà de 256 Kio, tout est vidé | Avertissement dans le log |
| 6 | Sortie pendant `blocked` | Le programme est tué pendant une confirmation | `blocked` retiré, l'entrée de la file disparaît | Aucun |
| 7 | Desktop redémarré | Un `blocked` déclaré, puis le desktop redémarre | L'état revient depuis l'host | Puce et file inchangées |
| 8 | Texte piégé | `msg` contenant U+202E ou U+2028 | Caractères retirés, une seule ligne | Notification nettoyée qui nomme le pane |
| 9 | Alternance de 100 changements par seconde | `blocked` et `working` 100 fois par seconde | Une notification, état final affiché | Une notification |
| 10 | TUI qui lit la souris après un geste trackpad | htop lancé après un défilement au pixel | Décalage 0, molette envoyée par lignes | Aucun |
| 11 | `reduce_motion` activé pendant un décalage | L'utilisateur change le réglage | Décalage 0 à la frame suivante | Aucun |
| 12 | Interaction pendant une hold | Défilement pendant un redessin en mode 2026 | Contenu vivant pendant l'interaction, puis la frame capturée | Aucun |
| 13 | Restauration avec le mode 2026 actif | Rattachement à une session au milieu d'une hold | Échéance de 150 ms appliquée, ou mode remis à zéro | Aucun |
| 14 | Clé de config hors plage | `xt_checksum_extension: 32` | La clé retombe à 0, le reste s'applique | Avertissement nommant la clé et la plage 0-31 |
| 15 | Session sans runtime dans `host.status` | Session sortie ou pas encore lancée | `memory: null` avec la raison | Aucun |
| 16 | Compression non supportée | Plateforme sans compression | L'étape de repos s'arrête sans erreur | Aucun |
| 17 | OSC 7501 coupé par ConPTY | Sortie entrelacée sous Windows | À constater en US-019 ; correctif dans l'epic final | Aucun |
| 18 | Push du site non autorisé | US-017 sans accord d'Arthur | Diff prêt, miroir non régénéré | Aucun |

Catégories écartées :
- dégradation réseau : rien ne passe par le réseau à l'exécution ;
- changement de permissions : aucune notion d'accès n'est touchée ;
- modifications concurrentes entre utilisateurs : Paneflow est mono-utilisateur. La concurrence interne est couverte par les cas 5, 12 et 13 (rafale, hold contre publication en direct, restauration) ;
- états de chargement : aucune opération asynchrone visible n'est ajoutée.

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | La spec OSC 7501 évolue encore, elle a quelques jours | High | Med | Comportement aligné sur la page lue le 2026-10-09, date citée dans ARCHITECTURE (US-016) ; les écarts futurs passent par un amendement |
| 2 | Afficher les programmes non-agent ajoute du bruit dans l'Attention Queue | Med | Med | Seuls `blocked` et `error` entrent ; une notification par pane toutes les 10 s ; passe visuelle en US-018 |
| 3 | Le second render state coûte trop de mémoire | Med | Med | Mesure exigée et seuil de 10 % ; création paresseuse en repli (US-006) |
| 4 | `is_user_input` compte comme frappe une entrée programmatique | Low | Med | Test des deux origines en US-001 |
| 5 | ConPTY coupe ou retient OSC 7501 sous Windows | Med | Med | Spike de validation en US-019 ; les hooks restent la source principale sous Windows si l'hypothèse tombe |
| 6 | La recompression au repos gêne la recherche ou le défilement | Low | Med | Étapes bornées, uniquement après 2 s d'inactivité, sur le thread de session (US-015) |
| 7 | Le champ ajouté au manifeste casse un host ou un serve d'une autre version | Low | Med | Champ optionnel avec `skip_serializing_if` ; un host d'un autre pin est déjà `Incompatible` |
| 8 | Une baseline réenregistrée masque une régression | Med | Med | Baselines seulement depuis un arbre propre, en commit séparé ; un budget dépassé bloque la story (US-007) |
| 9 | Le site `paneflow-web` et le miroir divergent encore | Med | Low | Le miroir n'est régénéré que par le script, après le push du site (US-017) |
| 10 | Pas de matériel macOS disponible pour la vérification | Med | Med | US-019 consigne la vérification macOS comme non faite plutôt que présumée |

## Non-Goals

- **Pas d'entrée terminfo `Pst`.** Paneflow exporte `TERM=xterm-256color` (`crates/paneflow-host/src/host/spawn_env.rs:108`) et ne livre pas d'entrée terminfo propre. La spec dit qu'une absence de `Pst` ne permet pas de conclure, et la réponse à `OSC 7501 ; ?` fait foi.
- **Pas d'émission d'OSC 7501 par les hooks de Paneflow**, ni de remplacement des hooks par OSC 7501 : les hooks restent la source principale et gardent la précédence.
- **Pas d'affichage des diagnostics de config dans l'interface.** Toutes les clés invalides restent signalées dans le log, comme aujourd'hui. Un panneau ou une commande de validation relèvent d'un PRD séparé.
- **Pas de compression du scrollback côté host.** La recompression au repos ne touche que l'historique restauré du desktop. La compression du host demande sa propre mesure de mémoire et de CPU, dans un PRD séparé.
- **Pas de nouvel élément d'interface permanent pour les statuts**, ni de barre agrégée, ni de marque par commande ou par invite : la puce de pane et l'Attention Queue suffisent.
- **Pas de réécriture de l'historique de `main`** pour les six commits signés `arthur.jean@strivex.fr` : réécrire une branche publique est destructif.
- **Pas de notification pour `idle` ou `done`**, ni pour l'`error` d'un programme non-agent : seul `blocked` notifie.
- **Pas de tag ni de publication de release** dans ce PRD : le tag reste l'action d'Arthur, comme la réponse publique d'US-021.

## Files NOT to Modify

- `native/libghostty/bindings.rs` et `native/libghostty/prebuilt/*/bindings.rs` : générés par `scripts/generate-libghostty-bindings.sh`.
- Les hash de `native/libghostty/manifest.toml` : réécrits seulement par `scripts/repin-libghostty-manifest.sh`. US-013 les lit sans les modifier.
- `native/libghostty/prebuilt/*/lib/` : assets de release, hors git.
- `rust-toolchain.toml` et les quatre `rev` GPUI de `src-app/Cargo.toml`.
- `crates/paneflow-ipc-client/src/agent.rs` : les variantes wire d'`AgentState` et d'`AgentStateSource` ne changent pas.
- Les chaînes `SCREEN_*` et `Status::wire_str` de `crates/paneflow-serve/src/state.rs` : vocabulaire wire partagé par l'host, serve et l'app.
- `bench/baselines/**` : seulement par `--set-baseline` ou `--refresh-alloc-baselines`, depuis un arbre propre.
- `docs/user/**` : miroir généré, régénéré seulement par `scripts/sync-public-docs.ts` de `paneflow-web`.
- `src-app/src/terminal/types.rs::alacritty_is_absent_from_the_app_crate` et `src-app/src/terminal/ghostty_session/mod.rs:1464-1480` : gardes existantes, à laisser vertes.
- Les statuts des epics et des stories de `tasks/prd-libghostty-b699ea79-status.json` : seul le statut du PRD change (US-016).
- `/home/arthur/dev/ghostty` : clone de référence en lecture seule.

## Technical Considerations

- **Transport de l'état déclaré :**
  - recommandé : un champ optionnel du manifeste, écrit seulement quand il change comme `screen_activity`, et repris par `AgentSnapshotEntry` ;
  - alternative : un atomique vivant comme `bell_at_ms`, plus réactif mais perdu au redémarrage du host ;
  - le manifeste survit au redémarrage du desktop, ce qu'exige le cas 7. Engineering confirme le coût d'écriture.
- **Signal de débordement :** `EffectsOverflow` porte-t-il déjà de quoi reconnaître un débordement de statut ? Recommandé : une variante ou un champ dédié plutôt que deviner par les octets. À confirmer en lisant `callbacks.rs:237-290`.
- **Second render state :**
  - recommandé : un `OwnedHandle` créé à la première publication en direct pendant une hold ;
  - alternative : un render state créé et libéré à chaque appel, sans état à garder mais avec une allocation par publication ;
  - à trancher avec la mesure d'US-006.
- **Lecture de `reduce_motion` à la construction de la frame :** l'atomique global (`ui_primitives.rs:83-91`) suffit-il, ou faut-il notifier la vue quand la config change ? Recommandé : lire l'atomique dans `smooth_scroll_offset()`.
- **Correspondance entre une surface et une session pour la puce** : quel index l'app tient-elle déjà entre `surface_id` et session de l'host ? Recommandé : réutiliser celui d'`attached_runtime`.
- **Ordonnanceur de la recompression :** une minuterie de repos par session sur le thread de session du desktop, ou un passage groupé dans le cycle de publication ? Recommandé : la minuterie, réarmée par le jeton `compression_activity`.
- **Notifications des programmes non-agent :** quel réglage les active ? Recommandé : celui qui gouverne déjà les notifications de programme OSC 9 et 777, puisque ce sont des programmes et non des agents. Engineering vérifie le nom du réglage.
- **Migration :** aucun format persistant ne change de manière incompatible ; le nouveau champ est optionnel. Le retour arrière revert les commits.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Durée de vie d'un `done` après l'invite suivante | Quelques ms (purge au prompt) | Jusqu'à la première frappe | Month-1 | Test d'US-001 |
| Transitions perdues sur un bloc de 32 Kio de rapports | Au-delà de 64 événements, les plus récents sont perdus | 0 | Month-1 | Test d'US-003 |
| Sessions non-agent qui émettent OSC 7501 avec un état visible | 0 % | 100 % | Month-1 | Tests d'US-008 et US-009, passe d'US-018 |
| Frames décalées pendant que l'application lit la souris | Non nul après un geste trackpad | 0 | Month-1 | Test d'US-005 |
| Tests de correctifs upstream qui échouent sans leur correctif | 2 tests connus pour ne pas pouvoir échouer | 0 | Month-1 | US-012, raisonnement cité par commit |
| Budgets de rendu présents dans les baselines | 0 sur 3 dans `terminal.json` | 3 sur 3 | Month-1 | `bench/baselines/linux-x86_64/terminal.json` |
| Délai de `host.status` avec 64 sessions bloquées | Jusqu'à 32 s (64 × 500 ms) | Moins de 600 ms | Month-1 | Test d'US-015 |
| Sections de documentation en dérive | 12 signalées par l'audit | 0 | Month-1 | Liste d'US-016 et US-017 |
| Régressions signalées après la release 0.17.7 | N/A (new) | 0 plantage, 0 état de statut faux reproductible | Month-6 | Issues GitHub |

## Open Questions

- **Notifications des programmes non-agent :** seul `blocked` notifie. Arthur veut-il aussi `error` ? À trancher à la passe d'US-018 ; seule la condition d'US-010 en dépend.
- **Matériel macOS :** Arthur dispose-t-il d'un Mac pour US-019, ou faut-il louer un runner ? Sans réponse avant US-019, la vérification macOS est consignée comme non faite.
- **Durée du signal « vu » :** la première frappe suffit-elle, ou faut-il aussi le focus du pane ? L'host ne connaît pas le focus aujourd'hui ; le PRD retient la frappe, exemple de la spec. À revoir à la passe d'US-018.
[/PRD]
