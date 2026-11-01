[PRD]
# PRD: Usage des abonnements d'agents

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-31 | Arthur Jean | PRD initial : capsule d'usage dans le pied de sidebar (variante A) et page Settings › Usage (variante P2). Les limites viennent du status line de Claude Code et des rollouts Codex, sans lecture de credential ; le worker tient un journal quotidien des tours, du temps actif et des tokens. 5 epics, 19 stories. |
| 1.1 | 2026-10-31 | Arthur Jean | Les validations macOS et Linux de US-001 passent dans deux epics finaux, EP-006 (Linux, US-020) puis EP-007 (macOS, US-021, sur une machine Scaleway). Le critère Team, Enterprise ou clé API de US-001 est retiré faute de compte : le comportement sans `rate_limits` repose sur le code et ses tests. 7 epics, 21 stories. |
| 1.2 | 2026-10-31 | Arthur Jean | La preuve en pane réel sous Windows du relais vers le host passe de la Definition of Done de EP-001 à US-007 : avant EP-002, le worker ignore le frame `plan_limits` et aucun outil ne l'observe. EP-001 est certifiée sur les tests du relais (processus réel vers un host simulé), du host (IPC réel) et sur le rejet par un host ancien. |
| 1.3 | 2026-11-01 | Arthur Jean | La preuve en pane réel sous Windows du trajet relais, host, worker passe de US-007 à US-011 : la passe visuelle de la capsule l'observe dans l'app réelle. EP-002 est certifiée sur le test bout en bout du worker (host réel, PTY réel, worker réel, frame `status_line` envoyé par IPC comme le relais). Les bornes de US-008 acceptent une remise à zéro future jusqu'à `max(8 jours, durée de la fenêtre + 1 jour)`, faute de quoi la fenêtre mensuelle du free tier Codex serait toujours rejetée ; Claude garde 8 jours. |
| 1.4 | 2026-11-01 | Arthur Jean | La passe visuelle macOS de la capsule et de la bulle passe de la Definition of Done de EP-003 à US-021 (EP-007), sur la machine Scaleway ; Arthur a fait la passe Windows le 2026-11-01. La dernière ligne de la bulle de US-012 devient « Open agent settings » et ouvre Settings › Agents, parce que Settings › Usage n'existe qu'à partir de US-014, bloquée par US-013 ; US-014 la renomme « Open usage settings » et la fait pointer vers Usage. |
| 1.5 | 2026-11-01 | Arthur Jean | Les limites Claude viennent aussi d'une sonde du binaire Claude Code non modifié (US-022, ajoutée à EP-003) : la requête de contrôle `get_usage` du protocole stream-json, comme T3 Code au commit `37d7615a91` : options de la sonde dans [`ClaudeProvider.ts:198-228`](https://github.com/pingdotgg/t3code/blob/37d7615a91d880c3776d575e1152b9c955104de2/apps/server/src/provider/ClaudeProvider.ts#L198-L228), appel `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET` dans [`ClaudeProvider.ts:337-363`](https://github.com/pingdotgg/t3code/blob/37d7615a91d880c3776d575e1152b9c955104de2/apps/server/src/provider/ClaudeProvider.ts#L337-L363). Un spike du 2026-11-01 sur Claude Code 2.1.316 sous Windows l'a validée : réponse en 0,96 s, `total_cost_usd` à 0, fenêtre de 7 jours à 67 %, identique à Zeron. Paneflow ne lit toujours aucun credential et n'émet aucune requête HTTP ; seul le binaire contacte Anthropic avec son propre login, ce que la page légale de Claude Code autorise (« signing in to the unmodified Claude Code binary with their own Claude subscription »). L'approche de Zeron (lecture du token et appel à `api/oauth/usage`) reste écartée. US-017 (chaînage) est annulée : la sonde lit les limites quel que soit le status line. Codex suit la même approche avec US-023 : la méthode `account/rateLimits/read` de `codex app-server`, comme T3 Code ([`CodexProvider.ts:580`](https://github.com/pingdotgg/t3code/blob/37d7615a91d880c3776d575e1152b9c955104de2/apps/server/src/provider/CodexProvider.ts#L580)) ; les rollouts de US-008 restent la source hors sonde. Grok et Gemini restent hors périmètre : leur CLI n'expose aucune lecture du quota hors du mode interactif (`grok usage` ne rend que les tokens d'une session locale), et Zeron comme T3 Code ne les obtiennent qu'en lisant le token pour appeler un endpoint privé (`cli-chat-proxy.grok.com/v1/billing`, [`cloudcode-pa.googleapis.com`](https://github.com/pingdotgg/t3code/blob/37d7615a91d880c3776d575e1152b9c955104de2/apps/server/src/provider/antigravityUsageLimits.ts)), ce que FR-04 exclut. FR-02, FR-03, FR-04, US-011, US-013, EP-005 et les Non-Goals sont ajustés. |
| 1.6 | 2026-11-01 | Arthur Jean | Les textes des états `ForeignStatusLine` et `HooksDisabled` de US-013 (et des cas limites 3 et 4) deviennent « Claude Code has not reported plan limits for {dossier} yet. Check that it is installed and signed in for this folder, then reopen this panel. ». Depuis l'amendement 1.5, la sonde de US-022 lit les limites quel que soit le status line et coupe elle-même les hooks : sans instantané, ces deux états ne signifient plus que la configuration empêche la lecture, mais que la sonde n'a pas encore répondu ou a échoué (binaire absent, login manquant, délai, erreur). L'ancien texte accusait la configuration de l'utilisateur et ne nommait aucune correction ; `/better-interface` l'a classé HIGH (texte trompeur, erreur sans issue). |
| 1.7 | 2026-11-01 | Arthur Jean | La passe visuelle macOS de la page Settings › Usage passe de la Definition of Done de EP-004 à US-021 (EP-007), sur la machine Scaleway, comme l'amendement 1.4 pour la capsule ; Arthur a fait la passe Windows le 2026-11-01. Le glyphe de l'entrée Usage de US-014 passe de 17 à 15 px : toutes les entrées de la navigation des Settings partagent `SETTINGS_NAV_ICON_SIZE` (15 px), et une seule icône plus grande casserait l'alignement de la colonne. |

## Problem Statement

**Convention de référence.** Les chemins désignent Paneflow au commit `96c525144`. L'exploration validée est l'artifact https://claude.ai/artifact/MxxLTrgSS2ynpErz8LpBHn (privé) : Arthur y a retenu la variante A pour la sidebar et la variante P2 pour Settings le 2026-10-31. L'implémentation de référence est Zeron (`C:\dev\zeron`), notamment `crates/proto/src/plan_usage.rs` et `crates/ui/src/account_usage.rs`.

1. **Les limites d'abonnement sont invisibles là où les agents tournent.** Un abonné Max ou ChatGPT Pro découvre qu'il a atteint sa fenêtre de 5 h ou sa semaine quand l'agent s'arrête sur une erreur `rate_limit` (Zeron les traduit dans `crates/harness/src/claude/normalize.rs:18`). Pour voir la jauge, il faut taper `/usage` dans Claude Code ou `/status` dans Codex, pane par pane et compte par compte.
2. **Deux comptes, aucune vue d'ensemble.** Paneflow sépare déjà un compte perso et un compte d'entreprise par espace (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `docs/user/settings.md:131-137`) et par profil d'agent (`crates/paneflow-config/src/schema.rs:144-163`). Il n'affiche jamais ce que chacun consomme.
3. **Aucune mémoire de l'usage.** Claude Code purge ses transcripts après 30 jours par défaut ([data usage](https://code.claude.com/docs/en/data-usage)), et Zeron exclut l'historique (`C:\dev\zeron\ARCHITECTURE.md`). L'utilisateur ne sait ni à quel rythme il consomme, ni quels jours.

**Why now:** Depuis février 2026, la page légale de Claude Code interdit aux tiers de « collect, store, or intermediate Claude.ai credentials or session tokens » ([legal](https://code.claude.com/docs/en/legal-and-compliance)). L'approche de Zeron et de CodexBar, qui appellent l'endpoint d'usage avec le token du CLI, devient un risque pour les comptes des utilisateurs. Or Claude Code transmet lui-même `rate_limits.five_hour` et `rate_limits.seven_day` à toute commande de status line ([statusline](https://code.claude.com/docs/en/statusline)). Paneflow installe déjà ses hooks dans chaque dossier de config (`docs/user/settings.md:161-167`), et le worker reçoit déjà chaque changement d'état d'agent (`ARCHITECTURE.md:354-374`). Une version conforme coûte donc peu.

## Overview

Une capsule de 36 px s'insère dans le pied de la sidebar, entre le filtre et l'engrenage. Elle contient un anneau de 16 px et le pourcentage de la fenêtre la plus consommée des comptes de l'espace actif. Un clic ouvre une bulle construite sur `menu_surface`, avec un bloc par compte : des jauges Session et Week (Month pour le free tier Codex), l'heure de remise à zéro, une phrase de projection au rythme actuel, puis la ligne « Open usage settings ». La page Settings › Usage, dans le groupe Integrations après Agents, présente les limites en tuiles à anneaux concentriques (semaine dehors, session dedans) et un registre de 53 semaines par compte, sur une échelle commune. Un sélecteur Turns, Tokens ou Active time choisit la métrique, Turns par défaut.

Paneflow ne lit, ne copie, n'écrit et ne rafraîchit aucun credential, et n'émet aucune requête réseau pour cette fonctionnalité. Côté Claude, un relais `paneflow-ai-hook StatusLine` est installé comme `statusLine` dans chaque dossier de config qui n'en a pas : il transmet au worker l'objet `rate_limits` que Claude Code lui passe. Un utilisateur qui a déjà son propre status line peut le chaîner derrière le relais, sur demande. Côté Codex, le worker lit la fin du rollout le plus récent sous `<CODEX_HOME>/sessions/` et prend le dernier événement `token_count`. Un compte est un couple (agent, dossier de config canonique) : le dossier par défaut, celui de chaque espace et celui de chaque profil. Les comptes perso et entreprise tournent ainsi en parallèle, ce que le basculement de Zeron ne permet pas.

Le worker tient deux fichiers sous `~/.paneflow/usage/` : `plans.json` pour les derniers instantanés de limites, `ledger.json` pour le journal quotidien. Le journal compte un tour à chaque entrée dans l'état working d'OSC 7501 ou à chaque `UserPromptSubmit`, mesure le temps actif, et additionne les tokens lus dans les transcripts. Il ne stocke aucun contenu, et les jours de plus de 400 jours sont élagués. L'app ne fait qu'afficher ce que le worker publie. Les agents sans limites lisibles (Gemini, OpenCode, Pi) n'apparaissent que dans les registres, et seulement s'ils émettent OSC 7501 ou des hooks Paneflow.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Voir ses limites sans quitter Paneflow | La capsule affiche un pourcentage moins de 1 s après la première réponse de Claude Code dans un pane, sur macOS, Linux et Windows | Idem pour Codex, une fois US-002 validée |
| Zéro credential touché | 0 lecture de `.credentials.json`, du Keychain ou de `auth.json`, vérifié par un test de garde | Inchangé |
| Mémoire de l'usage | 100 % des tours d'une session scriptée de 20 tours comptés dans le journal | 365 jours de registre pour un utilisateur installé depuis un an |
| Coût pour le rendu | 0 I/O synchrone sur le thread de rendu ; page Usage rendue en moins de 16 ms avec 4 registres (build release) | Inchangé |
| Budget des helpers | `paneflow-ai-hook` reste sous 384 000 octets (`release-min`, Linux x86_64) | Inchangé |

## Target Users

### Développeur multi-agents sur abonnement
- **Role:** Arthur et les utilisateurs de Paneflow abonnés à Claude Pro, Max ou Ultra et à ChatGPT Plus ou Pro, qui lancent 3 à 10 agents en parallèle.
- **Behaviors:** répartit les tâches entre Claude Code et Codex, relance un agent quand l'autre est limité, consulte `/usage` au besoin.
- **Pain points:** découvre la limite au moment du blocage, sans prévenir ; ne sait pas s'il tiendra la semaine au rythme du jour.
- **Current workaround:** `/usage` dans Claude Code, `/status` dans Codex, ou une app de barre de menus comme CodexBar (macOS seulement) qui utilise le token du CLI.
- **Success looks like:** un chiffre toujours visible dans le pied de la sidebar, et une phrase qui dit s'il atteindra la limite avant la remise à zéro.

### Utilisateur à deux comptes, perso et entreprise
- **Role:** développeur qui a un abonnement personnel et un siège fourni par son entreprise, séparés par un espace Paneflow (`CLAUDE_CONFIG_DIR=~/.claude-work`) ou par un profil d'agent.
- **Behaviors:** travaille dans l'espace « Work » le jour, dans l'espace perso le soir, parfois les deux en même temps.
- **Pain points:** ne sait pas quel compte s'épuise ; un outil qui bascule un seul login global, comme Zeron, casse l'usage simultané.
- **Current workaround:** `/usage` dans chaque pane.
- **Success looks like:** chaque compte a sa ligne, étiquetée par son email ou par son espace ; la capsule suit l'espace actif.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Zeron** : lit les endpoints d'usage avec le token OAuth du CLI, gère plusieurs comptes en écrasant le fichier de credentials du CLI (`C:\dev\zeron\crates\engine\src\agent_accounts.rs:40-62`), et n'a aucun historique. Paneflow reprend son modèle de fenêtres, ses seuils et sa règle de libellé, sans toucher aux tokens et sans basculement.
- **CodexBar** ([README](https://github.com/steipete/CodexBar)) : fenêtres session, semaine et mois avec compte à rebours, courbe de consommation comparée à un rythme régulier, aucune demande d'accès au Keychain depuis l'arrière-plan, interrupteur pour couper le Keychain. Paneflow en garde le compte à rebours et le rythme, sous forme d'une phrase plutôt que d'un graphique.
- **Claude Code** : `/usage` à la demande, et `rate_limits` dans le JSON du status line pour Pro, Max et Ultra, après la première réponse de la session ([statusline](https://code.claude.com/docs/en/statusline)).
- **ccusage et cship** : rapports de tokens tirés des JSONL locaux, ou appels à l'endpoint OAuth ([cship](https://docs.rs/crate/cship/latest/source/src/usage_limits.rs)).
- **Market gap:** aucun multiplexeur n'affiche les limites de plusieurs comptes à côté des panes d'agents, avec un historique d'un an, sans manipuler de credential.

### Best Practices Applied
- Le pourcentage est toujours écrit, et la couleur ne fait que le doubler (WCAG 1.4.1).
- Compte à rebours relatif sous 24 h, jour et heure au-delà (CodexBar).
- Projection au rythme actuel, comparée à l'heure de remise à zéro (l'« even-use guide » de CodexBar).
- Libellé de fenêtre dérivé de sa durée, pas du plan : le free tier Codex a une fenêtre de 30 jours (`C:\dev\zeron\crates\proto\src\plan_usage.rs:84-98`).
- Parsing tolérant : les deux sources ne sont pas des API stables, et le schéma de l'endpoint Anthropic a déjà dérivé ([ccusage 0.1.10](https://pypi.org/project/ccusage/0.1.10/)).

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- Claude Code passe `rate_limits.five_hour` et `seven_day` (`used_percentage` de 0 à 100, `resets_at` en secondes Unix) à la commande de status line d'un compte Pro, Max ou Ultra, après la première réponse ([statusline](https://code.claude.com/docs/en/statusline)). La commande hérite de l'environnement du pane (`PANEFLOW_SESSION_ID`, `PANEFLOW_HOST_ENDPOINT`). Validé par US-001.
- Claude Code tolère une clé inconnue dans l'objet `statusLine`, comme il tolère `_paneflow_managed` dans les groupes de hooks (`crates/paneflow-agent-config/src/claude_hooks.rs:20`). Validé par US-001 ; sinon le marqueur passe dans la commande.
- Les rollouts Codex contiennent des événements `token_count` avec `rate_limits.primary` et `secondary` et des totaux de tokens cumulatifs ([analyse](https://codex.danielvaughan.com/2026/03/30/codex-cli-session-analytics-jsonl-rollout), source communautaire). Les deux rollouts présents localement le 2026-10-31 n'en contiennent aucun. Validé par US-002.
- `.claude.json` porte l'identité du compte dans `oauthAccount` (Zeron la lit à cet endroit, `agent_accounts.rs:24-27`) : `~/.claude.json` pour le dossier par défaut, `$CLAUDE_CONFIG_DIR/.claude.json` sinon (`CHANGELOG.md:260`).
- Le worker reçoit chaque frame de hook et chaque état OSC 7501 (`ARCHITECTURE.md:354-374`), et peut connaître l'environnement de lancement de chaque session (`src-app/src/terminal/pty_session/spawn_env.rs`, non vérifié en détail).
- Les entrées assistant des transcripts Claude portent `message.id` et `message.usage.input_tokens` / `output_tokens` (source communautaire ccusage, non vérifiée).
- Les sièges Team et Enterprise ne reçoivent probablement pas `rate_limits` : la documentation ne cite que Pro, Max et Ultra. À confirmer par US-001 si un compte est disponible.

### Hard Constraints
- Aucune lecture, copie, écriture ni rafraîchissement de credential : ni `.credentials.json`, ni Keychain, ni `auth.json`, ni variable de token. Aucune requête HTTP ajoutée.
- Aucune I/O synchrone ni sous-processus sur le thread de rendu (`AGENTS.md`, section Boundaries).
- Linux, macOS et Windows sont des cibles. Le code `cfg(windows)` n'est linté que par la lane Windows de la CI ; tout nouvel item se déclare avant un éventuel `mod tests`.
- Aucun commentaire dans le code source.
- L'état de Paneflow passe par `crates/paneflow-home` ; jamais par `dirs::config_dir`. Les dossiers des agents passent par `crates/paneflow-agent-config`.
- `paneflow-ai-hook` ne prend aucune dépendance nouvelle et reste sous 384 000 octets (`.github/workflows/run_tests.yml:321`).
- `window-state.json` est plafonné à 4096 octets (`src-app/src/window_state.rs:13`) : aucune donnée d'usage n'y va.
- Le rendu suit DESIGN.md et l'artifact validé, qui fait foi. Il réutilise les primitives existantes (`squircle_skin`, `menu_surface`, `menu_panel`, `menu_row`, `menu_reveal`).
- `docs/user/` est un miroir généré depuis le dépôt du site : la documentation utilisateur s'écrit dans `paneflow-web`.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration

Gates additionnels :
- Stories qui touchent `paneflow-ai-hook` (US-005, US-017) : build `release-min` de `paneflow-ai-hook` pour `x86_64-unknown-linux-gnu`, taille sous 384 000 octets. C'est le job `Release build + binary-size budget (Linux x86_64)`, reproductible dans WSL.
- Stories qui ajoutent une clé de configuration (US-006, US-011) : `cargo test -p paneflow-config --locked`, dont le test qui compare les clés sérialisées à `schemas/paneflow.schema.json`.
- Stories qui modifient l'installation des hooks (US-006, US-017) : `cargo test -p paneflow-agent-config --locked`.
- Stories qui touchent du code `#[cfg(windows)]` : relecture de l'ordre des items par inspection. La lane `Windows x86_64 libghostty check` fait foi après le push.
- Stories d'interface (US-010 à US-015, US-017) : revue par le skill `/better-interface` sur la surface modifiée. La story ne passe en IN_REVIEW qu'avec un verdict `Approve`, sans finding `HIGH`. Ce qui exige l'app rendue est marqué `Not verified` et revient à la passe visuelle.
- Stories d'interface (EP-003, EP-004, US-017) : passe visuelle manuelle par Arthur sur macOS et Windows, comparée à l'artifact validé. Pour EP-003 et EP-004, la passe macOS se fait dans US-021 (amendements 1.4 et 1.7). L'agent d'implémentation ne lance pas l'application et ne fait pas de capture.

## Epics & User Stories

### EP-001: Sources de données et comptes

Établit, sans interface, d'où viennent les limites et à quel compte elles appartiennent : deux validations d'hypothèses, l'inventaire des comptes, le modèle pur, le relais status line et son installation.

**Definition of Done:** les rapports de US-001 et US-002 sont consignés dans leurs stories ; l'inventaire renvoie un compte par couple (agent, dossier canonique) ; le modèle pur passe ses tests ; le relais, lancé comme le fait Claude Code, envoie `rate_limits` au host, prouvé par test d'intégration (la preuve en pane réel sous Windows est portée par US-007, amendement 1.2) ; l'installation pose et retire un `statusLine` géré sans toucher un status line étranger ; aucun fichier de credential n'est référencé.

#### US-001: Validate assumption: le status line de Claude Code reçoit `rate_limits` dans un pane Paneflow
**Description:** As a mainteneur, I want prouver qu'une commande `statusLine` installée dans un dossier de config reçoit `rate_limits` et l'environnement du pane so that la source des limites Claude ne dépende d'aucun credential.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given un compte Claude Pro, Max ou Ultra et un script de status line qui recopie son stdin dans un fichier, when un message reçoit une réponse dans un pane Paneflow, then le JSON contient `rate_limits.five_hour.used_percentage`, `rate_limits.five_hour.resets_at`, `rate_limits.seven_day.used_percentage`, `rate_limits.seven_day.resets_at`, `session_id` et `transcript_path`. Le constat est consigné avec la version de Claude Code, sous Windows. Linux et macOS sont validés par US-020 et US-021 (amendement 1.1).
- [ ] Given le même script, when il s'exécute, then son environnement contient `PANEFLOW_SESSION_ID` et `PANEFLOW_HOST_ENDPOINT`, plus `CLAUDE_CONFIG_DIR` quand l'espace le définit.
- [ ] Given un objet `statusLine` qui porte la clé `_paneflow_managed: true`, when Claude Code démarre, then il exécute la commande sans erreur ni avertissement bloquant. Sinon, la story consigne le rejet, et le marqueur retenu pour US-006 devient un suffixe de la commande.
- [ ] Given Windows avec Git Bash, puis sans Git Bash, when la commande est le chemin absolu de `paneflow-ai-hook` suivi de `StatusLine`, chemin avec espaces compris, then elle s'exécute dans les deux shells (Git Bash, puis PowerShell).
- [ ] Given la première ouverture d'une session, when aucune réponse n'est encore arrivée, then l'absence de `rate_limits` est consignée.
- ~~Given un compte Team, Enterprise ou une clé API, si l'un est disponible, when le script tourne, then la présence ou l'absence de `rate_limits` et de `spend_limit` est consignée.~~ Retiré par l'amendement 1.1 : aucun compte de ce type n'est disponible. L'absence de `rate_limits` est couverte par le code (US-005 envoie `rate_limits: null` une fois par session, US-007 en fait l'état `NoLimitsReported`) et par ses tests, sans preuve réelle. L'Open Question 2 reste ouverte.
- [ ] Given `disableAllHooks: true` dans `settings.json`, when une réponse arrive, then le script ne s'exécute pas, et ce fait est consigné.

**Rapport de validation (2026-10-31, Windows 11, Claude Code 2.1.316) :**

Sonde : la commande `statusLine` de `~/.claude/settings.json` a été remplacée quelques secondes par un script qui recopiait son stdin et son environnement, puis le fichier a été restauré à l'octet près (`cmp`). Quatre sessions Claude Code tournaient dans des panes Paneflow (build installée), sous Git Bash (`MSYSTEM=MINGW64`, `SHELL=/bin/bash.exe`). Captures locales, hors dépôt.

- `rate_limits.five_hour` et `rate_limits.seven_day` arrivent, chacun avec `used_percentage` (entier) et `resets_at` (secondes Unix), ainsi que `session_id`, `transcript_path` et `version`. Linux et macOS : US-020 et US-021.
- L'environnement contient `PANEFLOW_SESSION_ID` et `PANEFLOW_HOST_ENDPOINT`. Une session lancée dans un pane avec `CLAUDE_CONFIG_DIR` défini, comme le fait un espace, le transmet aussi au status line (`CLAUDE_CONFIG_DIR=C:\Users\Arthur\.claude` relu dans l'environnement capturé).
- Un `statusLine` portant `_paneflow_managed: true` s'exécute sans erreur, dans les sessions en cours comme dans une nouvelle session lancée avec `--settings` : aucun avertissement au démarrage, et le status line s'affiche dès le premier rendu. US-006 garde la clé comme marqueur, et reconnaît aussi le relais à son chemin de programme.
- Commande sous Windows : sans Git Bash, Claude Code exécute le status line via PowerShell ([statusline, Windows configuration](https://code.claude.com/docs/en/statusline)). Le chemin absolu en barres obliques suivi de `StatusLine` s'exécute dans Git Bash, PowerShell 7 et PowerShell 5.1. Entre apostrophes (chemin avec espaces), PowerShell le rejette (`ParserError`). Correction technique de US-006 : quand le chemin absolu exigerait des apostrophes et que le relais est sous le dossier personnel, la commande devient `~/.paneflow/bin/paneflow-ai-hook.exe StatusLine`, sans guillemets. Cette forme s'exécute dans les trois shells avec un dossier personnel qui contient une espace, et Claude Code 2.1.316 l'a exécutée sous Git Bash (`$0` vaut `/usr/bin/bash`). `CLAUDE_CODE_GIT_BASH_PATH` invalide ne force pas PowerShell : Claude retrouve Git Bash à son emplacement standard. Le routage de Claude vers PowerShell repose donc sur la documentation, la commande étant prouvée dans PowerShell lui-même.
- Première ouverture : le tout premier appel du status line d'une nouvelle session n'a pas de `rate_limits` ; l'appel suivant, quelques secondes plus tard et toujours sans réponse, les porte (20 % et 61 %). Hypothèse infirmée : il n'est pas nécessaire qu'une réponse arrive, la session reçoit les valeurs du compte (`cost.total_api_duration_ms: 0`). Le relais ne s'appuie donc pas sur « première réponse » pour la présence des limites ; il n'envoie `rate_limits: null` qu'après une réponse (`total_api_duration_ms > 0`).
- Constat pour US-007 : une session inactive continue de transmettre des valeurs anciennes. Elle annonçait `seven_day` à 55 % (même `resets_at`) quand les autres en étaient à 59 %, et omettait `five_hour`. « Le dernier frame l'emporte » ferait reculer la jauge ; pour un même `resets_at`, il faut garder le pourcentage le plus haut.
- Team, Enterprise, clé API : critère retiré par l'amendement 1.1.
- `disableAllHooks: true` : aucune exécution du status line en 11 s, contre environ 3 par seconde avant. Le status line est coupé avec les hooks.

#### US-002: Validate assumption: les rollouts Codex portent `rate_limits` et les totaux de tokens
**Description:** As a mainteneur, I want vérifier sur la version courante de Codex ce que contient un rollout so that les limites Codex se lisent sur disque, sans `auth.json`.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given la version courante de Codex, avec le daemon app-server puis avec `--no-daemon`, when un tour se termine dans un pane Paneflow, then le dernier `rollout-*.jsonl` sous `<CODEX_HOME>/sessions/` contient un événement `token_count`. La story consigne le nom exact des champs de `rate_limits.primary` et `rate_limits.secondary` : pourcentage, durée de fenêtre et heure ou délai de remise à zéro.
- [ ] Given le même fichier, then la story confirme ou infirme que `info.total_token_usage` est cumulatif par fichier et que le premier `token_count` porte `info: null`.
- [ ] Given au moins un compte ChatGPT (Free, Plus ou Pro), when la fenêtre secondaire est mensuelle, then sa durée est consignée.
- [ ] Given un rollout sans aucun `token_count` (cas observé le 2026-10-31 sur deux rollouts locaux de 22 Ko), when la story l'analyse, then elle en donne la cause : version, mode daemon ou session sans réponse.
- [ ] Given qu'aucune version courante n'écrit `rate_limits` dans ses rollouts, then US-008 passe en BLOCKED et l'Open Question 3 tranche le repli.

**Rapport de validation (2026-10-31, Windows 11) :**

- Codex 0.166.0, compte ChatGPT `plan_type: "free"` : un tour `codex exec` lancé depuis un pane Paneflow écrit dans `sessions/2026/10/31/rollout-*.jsonl` un événement `event_msg` de type `token_count`. Champs de `rate_limits` : `limit_id`, `limit_name`, `primary` et `secondary`, chacun `{used_percent (flottant), window_minutes (entier), resets_at (secondes Unix)}`, puis `credits`, `individual_limit`, `spend_control_reached`, `plan_type`, `rate_limit_reached_type`. Ici `secondary` vaut `null`.
- Codex 0.160.0 en TUI (rollouts du 2026-10-02, `plan_type: "prolite"`) : même forme, `primary.window_minutes: 10080` (semaine), `secondary: null`.
- `info.total_token_usage` est cumulatif par fichier : 113 événements d'un même rollout, croissance monotone. Champs : `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens`, `total_tokens`. Hypothèse infirmée : le premier `token_count` porte déjà `info` dans les deux versions. Le parseur tolère quand même `info: null`.
- Fenêtre mensuelle : le compte free a `primary.window_minutes: 43200`, soit 30 jours, libellé « Month ».
- Rollouts sans `token_count` : la tentative du 2026-10-31 refusée par le serveur (modèle non pris en charge) a laissé un rollout de 108 Ko sans `token_count`, et les deux rollouts de 22 Ko du 2026-10-30 ne contiennent que `session_meta`. Cause : aucune réponse réussie, pas la version.
- Constat pour US-008 et US-018 : les rollouts terminés sont recompressés plus tard en `rollout-*.jsonl.zst` (74 fichiers sur 76). La lecture des limites vise le `.jsonl` actif ; le rattrapage de l'historique demande un décodeur zstd, donc une dépendance à décider.
- Nouveau type de ligne en 0.166.0 : `token_usage_record`.
- Codex 0.166.0 en TUI dans un pane Paneflow (`originator: codex-tui`), avec le daemon app-server puis avec `--no-daemon` : chaque tour écrit un `token_count` avec `rate_limits.primary` (2 % puis 3 %, `window_minutes: 43200`) et `secondary: null`. Le mode du daemon ne change ni le fichier ni les champs.
- `rate_limits` est bien présent : US-008 n'est pas bloquée.

#### US-003: Inventaire des comptes par dossier de config
**Description:** As a utilisateur à deux comptes, I want que Paneflow liste chaque compte Claude Code et Codex une seule fois so that la bulle et la page montrent tous mes abonnements sans configuration.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given aucun espace ni profil personnalisé, when l'inventaire s'exécute, then il renvoie un compte Claude (`~/.claude`, ou `$CLAUDE_CONFIG_DIR` de l'environnement de Paneflow) et un compte Codex (`~/.codex`, ou `$CODEX_HOME`), résolus par `crates/paneflow-agent-config/src/io.rs:8-65`.
- [ ] Given un espace « Work » avec `CLAUDE_CONFIG_DIR=~/.claude-work` et un profil « Claude perso » avec `~/.claude-perso`, then l'inventaire renvoie trois comptes Claude. Chacun porte son origine (défaut, espace ou profil) et le nom affiché de cette origine.
- [ ] Given deux origines qui désignent le même dossier après canonicalisation (`~` développé, casse ignorée sous Windows, liens symboliques résolus), then un seul compte est renvoyé, avec ses deux origines.
- [ ] Given un dossier Claude dont `.claude.json` contient `oauthAccount.emailAddress`, then le compte prend cet email pour libellé. Le fichier est `~/.claude.json` pour le dossier par défaut et `$CLAUDE_CONFIG_DIR/.claude.json` sinon. Il est lu hors du thread de rendu, plafonné à 1 Mo par `paneflow_home::read_regular_capped`.
- [ ] Given un `.claude.json` absent, illisible, invalide ou de plus de 1 Mo, then le libellé retombe sur le nom de l'origine, sans erreur visible.
- [ ] Given une variable de `SHADOWING_CREDENTIAL_KEYS` (`src-app/src/space_env.rs:8-15`) active pour l'origine (ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN ou CLAUDE_CODE_OAUTH_TOKEN pour Claude, OPENAI_API_KEY pour Codex), then le compte est marqué `api_billing`.
- [ ] Given un `CLAUDE_CONFIG_DIR` relatif dans l'environnement de Paneflow, then il est ignoré avec un avertissement journalisé, comme dans `absolute_env_dir`.
- [ ] Un test de garde échoue si un module de l'usage contient `.credentials.json`, `auth.json`, `find-generic-password` ou `Claude Code-credentials`.

#### US-004: Modèle d'usage pur : fenêtres, seuils, projection et remise à zéro
**Description:** As a développeur sur abonnement, I want des règles uniques pour libeller, colorer et projeter une fenêtre so that la capsule, la bulle et la page disent la même chose.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Une fenêtre se libelle d'après sa durée : 28 jours ou plus donnent « Month », 5 jours ou plus « Week », moins « Session ».
- [ ] Le niveau vaut `Normal` sous 0,80, `Warn` de 0,80 inclus à 0,95 exclu, et `Critical` à partir de 0,95. Un pourcentage hors de [0, 100] est borné.
- [ ] La fenêtre contraignante d'un ensemble de comptes est la plus consommée ; à égalité, Week l'emporte sur Session.
- [ ] La projection d'une fenêtre Week vaut `None` quand moins de 8 % de la fenêtre s'est écoulé ou que l'usage est nul. Si `usage / écoulé ≥ 1`, elle renvoie l'heure de dépassement arrondie au quart d'heure et l'avance sur la remise à zéro ; sinon, le pourcentage projeté à la remise à zéro.
- [ ] Cas testé : 54 % utilisés, remise à zéro jeudi 5 novembre 2026 à 09:00, lu le samedi 31 octobre à 14:30. La projection donne un dépassement le lundi vers 12:00, 2 j 21 h avant la remise à zéro.
- [ ] Given un `resets_at` passé au moment de la lecture, then la fenêtre vaut 0 %, porte l'état `ResetSinceSnapshot`, et sa projection vaut `None`.
- [ ] Le texte de remise à zéro est relatif sous 24 h (« in 3 h 12 min ») et absolu au-delà (jour abrégé et heure sur 24 h, « Thu 09:00 »).
- [ ] Given un JSON de status line ou un `token_count` Codex avec des champs absents, `null`, des nombres en chaîne, flottants ou entiers, then le parseur renvoie `None` pour la fenêtre fautive, garde les autres, et ne panique jamais. Un test couvre chaque cas.

#### US-005: Relais status line dans `paneflow-ai-hook`
**Description:** As a développeur sur abonnement, I want que Claude Code transmette ses limites à Paneflow à chaque réponse so that la capsule se mette à jour sans que Paneflow touche à mon token.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `paneflow-ai-hook StatusLine` lancé par Claude Code avec le JSON sur stdin et `PANEFLOW_SESSION_ID` et `PANEFLOW_HOST_ENDPOINT` définis, when le relais s'exécute, then il envoie un frame par le transport existant (`crates/paneflow-ai-hook/src/transport.rs`). Le frame contient `session_id`, le dossier de config (`CLAUDE_CONFIG_DIR`, ou le défaut), l'objet `rate_limits` tel que reçu et l'heure de réception.
- [ ] Le frame ne contient aucun autre champ du JSON : ni `transcript_path`, ni coût, ni contenu.
- [ ] Sans commande chaînée, le relais n'écrit rien sur stdout et sort avec le code 0 dans tous les cas.
- [ ] Given une session lancée hors de Paneflow (variables absentes), then le relais sort avec 0 sans ouvrir de connexion.
- [ ] Given un host arrêté ou lent, then le processus du relais rend la main en moins de 100 ms au p95 sur 100 exécutions, la livraison passant en arrière-plan comme pour les hooks (`crates/paneflow-ai-hook/src/background.rs`).
- [ ] Given un stdin plus grand que `MAX_STDIN_BYTES` ou un JSON invalide, then rien n'est envoyé et le code de sortie vaut 0.
- [ ] Given un JSON sans `rate_limits` après une première réponse (Team, Enterprise ou clé API), then un frame avec `rate_limits: null` est envoyé au plus une fois par session.
- [ ] `crates/paneflow-ai-hook/Cargo.toml` ne gagne aucune dépendance.

#### US-006: Installation du relais dans chaque dossier Claude
**Description:** As a utilisateur à deux comptes, I want que chaque dossier de config Claude reçoive le relais avec les hooks so that les limites de chaque compte arrivent sans réglage manuel.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003, US-005

**Acceptance Criteria:**
- [ ] Given un dossier Claude de l'inventaire dont `settings.json` n'a pas de `statusLine`, when Paneflow installe ses hooks (Settings › Agents, sauvegarde d'un espace, ou rafraîchissement au lancement d'une build release, `docs/user/settings.md:161-167`), then il ajoute `statusLine` avec `"type": "command"` et `"command"` égal au chemin absolu de `paneflow-ai-hook` suivi de `StatusLine`, marqué comme géré selon le constat de US-001.
- [ ] Le reste de `settings.json` est préservé : seule la clé `statusLine` change, et les commentaires JSONC sont conservés.
- [ ] Given un `statusLine` étranger, then l'installateur ne le modifie pas et le compte passe à l'état `ForeignStatusLine`.
- [ ] Given un `statusLine` géré qui pointe vers un ancien chemin de `paneflow-ai-hook`, then sa commande est mise à jour.
- [ ] Given `usage.plan_limits: false` dans `paneflow.json`, then l'installateur retire le `statusLine` géré et n'en pose aucun. La clé vaut `true` par défaut et figure dans `schemas/paneflow.schema.json`.
- [ ] Retirer l'intégration Claude (`remove_hooks_lenient`, `crates/paneflow-agent-config/src/claude_hooks.rs:62`) retire aussi le `statusLine` géré.
- [ ] Given `disableAllHooks: true` dans `settings.json`, then le compte passe à l'état `HooksDisabled`.
- [ ] Given un `settings.json` invalide, then l'installateur ne l'écrit pas, et l'erreur s'affiche dans Agent setup comme pour les hooks.

---

### EP-002: Magasin d'usage dans le worker

Le worker reçoit les frames du relais, lit les rollouts Codex, tient le journal quotidien, persiste le tout sous `~/.paneflow/usage/` et le publie à l'app.

**Definition of Done:** les instantanés Claude et Codex survivent à un redémarrage du worker ; le journal compte les tours et le temps actif d'une session scriptée sans erreur ; aucun fichier n'est écrit plus d'une fois toutes les 10 s pour `plans.json` et toutes les 60 s pour `ledger.json` ; aucune lecture n'a lieu sur le thread de rendu.

#### US-007: Instantanés de limites Claude reçus du relais
**Description:** As a développeur sur abonnement, I want que le worker garde les dernières limites de chaque compte so that la capsule les affiche même après un redémarrage de Paneflow.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-005

**Acceptance Criteria:**
- ~~Given Windows, un compte Claude Pro, Max ou Ultra dont le dossier de config porte le `statusLine` géré (pas un status line étranger), when un message reçoit une réponse dans un pane Paneflow réel, then le worker reçoit le frame `plan_limits` et met à jour l'instantané du compte. Le constat est consigné avec la version de Claude Code (amendement 1.2 : preuve reprise de la Definition of Done de EP-001).~~ Déplacé vers US-011 par l'amendement 1.3.
- [ ] Given un frame du relais, when le worker le reçoit, then il remplace l'instantané du compte (Claude, dossier de config) par ses fenêtres, leur `resets_at` et l'heure de réception, et publie l'ensemble des instantanés aux apps connectées.
- [ ] Given deux sessions du même compte, then le frame reçu en dernier l'emporte.
- [ ] L'ensemble est écrit dans `~/.paneflow/usage/plans.json` (`~/.paneflow-dev/usage/` en debug) par `paneflow_home::write_atomically`, au plus une fois toutes les 10 s, et rechargé au démarrage du worker.
- [ ] Given un `plans.json` corrompu ou de plus de 256 Ko, then le worker démarre sans instantané, journalise un avertissement et ne plante pas.
- [ ] Given une fenêtre dont le pourcentage sort de [0, 100] ou dont `resets_at` est à plus de 8 jours dans le futur ou le passé, then cette fenêtre est ignorée.
- [ ] Given un frame dont `rate_limits` vaut `null`, then le compte passe à l'état `NoLimitsReported`, et l'instantané précédent est conservé avec son heure.
- [ ] Given un dossier de config absent de l'inventaire (Claude lancé avec un `CLAUDE_CONFIG_DIR` ponctuel), then l'instantané est conservé et le compte apparaît avec le chemin comme libellé.
- [ ] Un test vérifie que `plans.json` ne contient que des pourcentages, des horodatages, des chemins de dossier et des libellés.

**Notes d'implémentation (2026-11-01) :**

- Trajet : le host publie `plan_limits` sur `agent.follow`, le worker le reçoit (`CoreFrame::PlanLimits`, `crates/paneflow-serve/src/core_link.rs`) et l'applique au magasin (`crates/paneflow-serve/src/usage/plans.rs`). Les apps reçoivent un frame `{"type": "usage"}` sur le flux du worker, et l'en-tête de `agent.follow` porte `usage`. La méthode `usage.snapshot` renvoie les instantanés et le journal.
- « Le dernier frame l'emporte » est appliqué fenêtre par fenêtre, avec les deux constats de US-001 : pour un même `resets_at`, le pourcentage le plus haut est gardé, et une fenêtre absente du frame garde la valeur détenue.
- Bornes : une remise à zéro future est acceptée jusqu'à `max(8 jours, durée de la fenêtre + 1 jour)`, sinon la fenêtre mensuelle du free tier Codex (US-002, jusqu'à 30 jours) serait toujours rejetée. Pour Claude (5 h et 7 jours), la borne reste 8 jours. Validé par Arthur et inscrit dans US-008 par l'amendement 1.3.

#### US-008: Limites Codex lues dans les rollouts
**Description:** As a développeur sur abonnement, I want que les limites Codex viennent des fichiers que Codex écrit lui-même so that Paneflow n'ait jamais à lire `auth.json`.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002, US-004

**Acceptance Criteria:**
- [ ] Given un compte Codex, when un tour Codex se termine dans un pane, when la bulle ou la page s'ouvre, ou toutes les 5 min tant qu'une session Codex vit, then le worker lit hors du thread de rendu les 256 derniers Ko du `rollout-*.jsonl` le plus récent sous `<CODEX_HOME>/sessions/` et retient le dernier `token_count` qui porte `rate_limits`.
- [ ] Deux lectures d'un même compte sont espacées d'au moins 30 s, sauf à la fin d'un tour.
- [ ] Le compte est attribué d'après l'emplacement du rollout sous `CODEX_HOME`, jamais d'après l'environnement du hook : le daemon partagé de Codex 0.159+ transmet aux hooks l'environnement du premier lanceur.
- [ ] Les fenêtres reçoivent leur libellé par la règle de durée de US-004, si bien que la fenêtre mensuelle du free tier se lit « Month ».
- [ ] Given aucun rollout, un fichier illisible ou aucun `token_count` avec `rate_limits`, then l'instantané existant est conservé et le compte passe à l'état `NoCodexData`.
- [ ] Les instantanés Codex suivent la même persistance et les mêmes bornes que US-007, sauf la borne future de remise à zéro, qui vaut `max(8 jours, durée de la fenêtre + 1 jour)` (amendement 1.3).

**Notes d'implémentation (2026-11-01) :**

- Déclencheurs : `Stop` d'un hook Codex ou sortie de working d'une session Codex (lecture immédiate), méthode du worker `usage.refresh` pour l'ouverture de la bulle ou de la page (plancher de 30 s, appelée par US-012 et US-014), et balayage du worker toutes les 5 min par compte qui a une session Codex vivante.
- Le compte Codex d'une session vient de `CODEX_HOME` dans l'environnement de lancement de la session (porté par l'entrée `account_env` du snapshot du host), sinon de celui du worker ou de `~/.codex`. L'environnement du hook n'est jamais lu.

#### US-009: Journal quotidien des tours et du temps actif
**Description:** As a développeur multi-agents, I want que Paneflow compte mes tours et mon temps d'agent par jour et par compte so that je voie mon rythme sur un an, au-delà des 30 jours de transcripts.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Un réducteur pur reçoit, par session, les transitions d'état OSC 7501 et les frames de hook `PromptSubmit` et `Stop`. Il ouvre un tour au premier `PromptSubmit` ou à la première entrée dans l'état working, quand aucun tour n'est ouvert. Il le ferme à `Stop`, à la sortie de working, à la fin de session, ou après 4 h sans événement.
- [ ] Le temps actif est la durée passée en working. Pour un agent sans OSC 7501, c'est l'intervalle de `PromptSubmit` à `Stop`. Il est plafonné à 4 h par tour.
- [ ] Un tour compte pour le jour local de son début ; un temps actif qui traverse minuit est réparti entre les deux jours.
- [ ] Chaque tour est attribué au compte (agent, dossier de config) d'après l'environnement de lancement de la session (environnement de l'espace, puis du profil), ou au compte par défaut.
- [ ] Le journal est écrit dans `~/.paneflow/usage/ledger.json` par `write_atomically`, au plus une fois toutes les 60 s et à l'arrêt du worker. Par jour et par compte, il ne stocke que `turns`, `active_ms`, `tokens_in` et `tokens_out` : aucun prompt, aucun titre, aucun chemin de projet.
- [ ] Les jours de plus de 400 jours sont élagués au chargement ; avec 4 comptes, le fichier reste sous 1 Mo.
- [ ] Given un redémarrage du worker pendant un tour, then le tour compte une seule fois, et son temps actif s'arrête au dernier événement connu.
- [ ] Given un `ledger.json` corrompu, then il est renommé `ledger.json.bad-<horodatage>`, un journal vide le remplace, et un avertissement est journalisé.
- [ ] Given un agent qui n'émet ni OSC 7501 ni hook, then rien n'est compté pour lui.
- [ ] Un test rejoue une séquence enregistrée de 20 tours Claude Code et trouve 20 tours et un temps actif exact à la seconde près.

**Notes d'implémentation (2026-11-01) :**

- Un passage de working à blocked (demande de permission) suspend le temps actif sans fermer le tour ; le tour se ferme à la sortie vers idle, done ou error. Sans cela, chaque demande de permission compterait un tour de plus (risque 6).
- Anti-rejeu : chaque session garde le couple (heure de réception, révision du hook) du dernier hook compté, persisté dans `ledger.json` avec les tours ouverts (`open_turns`, `watermarks`, sans contenu). Deux hooks reçus dans la même milliseconde restent ordonnés par leur révision.
- Après un redémarrage, l'intervalle working ouvert est clos au dernier événement connu ; si la session est encore en working, le même tour reprend sans être recompté.
- La séquence de 20 tours du test (`state::tests::a_replayed_twenty_turn_claude_session_counts_twenty_turns_and_its_exact_active_time`) est construite sur la forme des frames réels de Claude Code (hooks et OSC 7501), pas capturée dans une session réelle.
- L'heure locale vient de `chrono` (`Local`), déjà compilé dans l'app par GPUI ; `paneflow-serve` en devient dépendant direct.

---

### EP-003: Capsule et bulle dans la sidebar (variante A)

Affiche les limites dans le pied de la sidebar et dans une bulle, avec tous leurs états.

**Definition of Done:** la capsule et la bulle reproduisent la variante A de l'artifact validé ; chaque état de US-013 a son texte ; le parcours clavier complet fonctionne ; `/better-interface` rend `Approve` ; Arthur a fait la passe visuelle sur Windows. La passe macOS est reprise par US-021 (amendement 1.4).

#### US-010: Primitive d'anneau GPUI
**Description:** As a mainteneur, I want une seule primitive d'anneau so that la capsule et les tuiles de la page dessinent la même jauge.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Un composant dessine une piste et un arc de valeur à extrémités arrondies, de midi dans le sens horaire, avec `PathBuilder::stroke` (même mécanisme que `src-app/src/ui_primitives/squircle.rs:165`). Il existe en tailles 16 (trait 2,5), 44 (4,5) et 76 (7), plus une variante de deux anneaux concentriques séparés de 3 px.
- [ ] La piste est `text` à 0,13. L'arc prend `accent` en `Normal`, la teinte d'avertissement `hsl(40 85% 55%)` de DESIGN.md 4.3 en `Warn`, et `agent_error` en `Critical`.
- [ ] Une valeur nulle ne montre que la piste ; 1,0 dessine un cercle fermé ; une valeur hors de [0, 1] est bornée.
- [ ] L'anneau n'a aucune animation, donc rien à couper sous `reduce_motion`.
- [ ] Given une taille inférieure à 8 px ou un trait supérieur ou égal au rayon, then rien n'est dessiné et rien ne panique.
- [ ] Un test unitaire vérifie la géométrie de l'arc (angle de départ, sens, angle final) pour 0, 0,25, 0,54 et 1,0.

#### US-011: Capsule dans le pied de sidebar
**Description:** As a développeur sur abonnement, I want voir en permanence le pourcentage le plus contraignant de mon espace actif so that je sache où j'en suis sans rien ouvrir.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007, US-010

**Acceptance Criteria:**
- [ ] Given au moins un compte de l'espace actif avec un instantané, then une capsule de 36 px de haut, au rayon 12 (`squircle_skin`, avec les teintes de survol et d'activité du bouton Settings, `src-app/src/app/sidebar_actions_menu.rs:203-229`), s'insère entre le filtre et le bouton Settings. Elle contient l'anneau de 16 px et le pourcentage contraignant en police système 13 px Medium, `tabular-nums`, 7 px entre les deux.
- [ ] Les comptes de l'espace actif sont ceux dont l'origine est cet espace. À défaut, ce sont les comptes par défaut.
- [ ] Après 800 ms (`delayed_tooltip`), une infobulle affiche « Plan usage: {libellé du compte}, {fenêtre} {pourcentage} ».
- [ ] Given des comptes sans aucun instantané, then la capsule montre la piste seule, sans chiffre, avec l'infobulle « Plan usage: no data yet ».
- [ ] Given aucun compte Claude ni Codex, ou `usage.plan_limits: false`, then la capsule est absente et le filtre reprend la place.
- [ ] La préférence `sidebar_show.usage` (vrai par défaut) figure dans Customize sidebar sous Agents. Elle est persistée comme `sidebar_show.agents` (`crates/paneflow-config/src/schema.rs:616-650`) et déclarée dans `schemas/paneflow.schema.json`.
- [ ] Given la publication d'un instantané, then la capsule se met à jour en moins de 1 s, sans reconstruire l'arbre des workspaces (vue en cache, `src-app/src/app/sidebar/view.rs:28`).
- [ ] Le nom accessible commence par le texte visible : « 54% plan usage, Claude Code week ». L'état déplié est exposé, la capsule est atteinte par Tab depuis le filtre, et Entrée ou Espace ouvre la bulle.
- [ ] Given la fenêtre minimale de 800 px avec une sidebar de 300 px, then le filtre garde au moins 120 px ; en dessous, la capsule ne montre que l'anneau et garde son nom accessible.
- [ ] Given Windows et un compte Claude Pro, Max ou Ultra, when la bulle s'ouvre (sonde de US-022) ou qu'un message reçoit une réponse dans un pane Paneflow réel (`statusLine` géré), then le worker met à jour l'instantané du compte et la capsule affiche son pourcentage. Le constat est consigné avec la version de Claude Code et la source observée (amendement 1.3 : preuve reprise de US-007 ; amendement 1.5 : la sonde est une source valable).
  - Constat du 2026-11-01 (Windows 11 Pro 10.0.26300, build de dev, Claude Code 2.1.316, Codex 0.166.0) : à l'ouverture de la bulle, la sonde de US-022 a mis à jour les comptes `~/.claude` (`agent_version` 2.1.316, état `reported`) et `~/.codex` (`agent_version` 0.166.0) de `~/.paneflow-dev/usage/plans.json` à 11:09, et Arthur a vu la capsule afficher le pourcentage pendant sa passe visuelle. Source observée : la sonde. Le même jour, un lancement direct des binaires installés a répondu en 1,52 s (Claude, semaine à 68 %) et 1,23 s (Codex, mois à 3 %), sans passer par le shim de Paneflow.

#### US-012: Bulle d'usage
**Description:** As a utilisateur à deux comptes, I want ouvrir le détail de chaque compte depuis la capsule so that je voie les deux fenêtres, la remise à zéro et mon rythme.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Un clic, Entrée ou Espace sur la capsule ouvre une bulle ancrée au-dessus du pied, de 284 px de large, construite sur `menu_surface` (squircle 18, bordure à 0,6) avec l'apparition `menu_reveal`. Échap, un clic à l'extérieur ou un second clic la ferment, et le focus revient à la capsule.
- [ ] L'en-tête montre « Plan usage » en eyebrow 11 px Semibold `muted` et « Updated {temps relatif} » en 11 px `muted`, calculé sur l'instantané le plus ancien affiché.
- [ ] Il y a un bloc par compte : ceux de l'espace actif d'abord, puis les autres dans l'ordre de l'inventaire. Chaque bloc a le logo en 15 px (`#D97757` pour Claude Code), le nom de l'agent en 12,5 px Medium et le libellé en 11,5 px `muted`, tronqué avec infobulle.
- [ ] Chaque fenêtre est une jauge : libellé sur 52 px, barre capsule de 4 px dans la couleur du niveau, pourcentage aligné à droite sur 34 px, et texte de remise à zéro dans l'infobulle de la jauge.
- [ ] Chaque fenêtre Week qui a une projection ajoute une ligne. En dépassement : le glyphe d'avertissement et « At this pace you reach the weekly limit {Mon 12:00}, {2 d 21 h} before it resets. » Sinon : « On track: about {66%} of the week used when it resets. » en `muted`.
- [ ] La dernière ligne, une `menu_row` « Open agent settings », ouvre Settings › Agents et ferme la bulle (amendement 1.4 : US-014 la fait pointer vers Settings › Usage).
- [ ] L'ouverture demande au worker une relecture Codex (US-008, avec son plancher de 30 s) sans jamais bloquer le rendu.
- [ ] Haut et Bas passent d'un bloc de compte à l'autre puis à la dernière ligne, avec l'anneau de focus `text` à 0,35.
- [ ] Given plus de 4 comptes, then la liste défile au-delà de 420 px, avec l'en-tête et la dernière ligne fixes.
- [ ] Given un email de 64 caractères, then il est tronqué avec infobulle, sans débordement horizontal.

#### US-013: États de la bulle
**Description:** As a utilisateur, I want qu'un compte sans données dise pourquoi et quoi faire so that je ne prenne jamais une jauge vide pour un usage nul.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006, US-008, US-012

**Acceptance Criteria:**
- [ ] Given un compte Claude sans instantané, then le bloc affiche « No plan data yet. Send a message to Claude Code in a Paneflow pane to load your limits. »
- [ ] Given un compte Codex à l'état `NoCodexData`, then le bloc affiche « No plan data yet. Finish a Codex turn in a Paneflow pane to load your limits. »
- [ ] Given l'état `ForeignStatusLine` et aucun instantané, then le bloc affiche « Claude Code has not reported plan limits for {dossier} yet. Check that it is installed and signed in for this folder, then reopen this panel. » (amendement 1.6) Avec un instantané (sonde de US-022), les jauges s'affichent normalement (amendement 1.5 ; le bouton « Chain status line » disparaît avec US-017).
- [ ] Given l'état `HooksDisabled` et aucun instantané, then le bloc affiche le même texte (amendement 1.6). Avec un instantané, les jauges s'affichent normalement (amendement 1.5).
- [ ] Given un compte `api_billing`, then le bloc affiche « Billed through an API key. This account has no plan limits. »
- [ ] Given l'état `NoLimitsReported`, then le bloc affiche « Claude Code reports no plan limits for this account. »
- [ ] Given une fenêtre `ResetSinceSnapshot`, then la jauge affiche 0 %, avec « Reset at {09:00}. Updates at your next message. » en `muted`.
- [ ] Given un instantané de plus de 1 h, then l'en-tête affiche son âge (« Updated 3 h ago ») et les jauges restent affichées.
- [ ] Chaque texte est en sentence case, sans point d'exclamation, et nomme l'action qui corrige l'état quand elle existe. La capsule ne prend jamais une couleur de niveau à partir d'un compte dans l'un de ces états.

#### US-022: Sonde d'usage par le binaire Claude Code
**Description:** As a utilisateur qui a son propre status line, I want que Paneflow lise mes limites Claude sans toucher à ma configuration ni à mes credentials so that la capsule montre mes vrais comptes dès l'ouverture.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003, US-007, US-012 (amendement 1.5)

**Acceptance Criteria:**
- [ ] Le worker lance le binaire Claude Code réel (résolu comme le lanceur d'agents, jamais le shim de Paneflow) avec `-p --input-format stream-json --output-format stream-json`, envoie `initialize` puis la requête de contrôle `get_usage` avec `skip_behaviors: true`, lit le `control_response`, ferme stdin et termine le processus en moins de 2 s après la réponse. Aucun message utilisateur n'est envoyé ; `session.total_cost_usd` reste à 0.
- [ ] La sonde tourne avec le `CLAUDE_CONFIG_DIR` du compte (absent pour le dossier par défaut), sans aucune variable `PANEFLOW_*`, hooks coupés (`disableAllHooks`), sans persistance de session, sans serveur MCP (`ENABLE_CLAUDEAI_MCP_SERVERS=false`), comme [`buildClaudeCapabilitiesProbeQueryOptions`](https://github.com/pingdotgg/t3code/blob/37d7615a91d880c3776d575e1152b9c955104de2/apps/server/src/provider/ClaudeProvider.ts#L198-L228) de T3 Code ; l'appel lui-même est dans [`ClaudeProvider.ts:337-363`](https://github.com/pingdotgg/t3code/blob/37d7615a91d880c3776d575e1152b9c955104de2/apps/server/src/provider/ClaudeProvider.ts#L337-L363).
- [ ] L'ouverture de la bulle demande au worker, par `usage.refresh`, de sonder chaque compte Claude de l'inventaire, avec un plancher de 30 s par compte et au plus 2 sondes simultanées. Rien ne s'exécute sur le thread de rendu.
- [ ] `rate_limits.five_hour` et `rate_limits.seven_day` (`utilization` de 0 à 100, `resets_at` en ISO 8601) deviennent les fenêtres de l'instantané, avec les bornes de US-008 ; une fenêtre `null` est absente ; `rate_limits_available: false` donne l'état `NoLimitsReported`. Quand le relais et la sonde rapportent tous deux, l'observation la plus récente gagne.
- [ ] Given un binaire absent, un délai de 10 s dépassé, une réponse d'erreur ou un schéma inconnu, then l'instantané précédent est conservé, un avertissement est journalisé, et rien ne panique.
- [ ] La version de Claude Code rapportée par `initialize` est conservée avec l'instantané.
- [ ] Un test pilote un faux binaire qui rejoue le protocole stream-json à partir d'une réponse enregistrée sur Claude Code 2.1.316, et couvre succès, erreur, délai et schéma inconnu. Le test de garde de US-003 couvre le module de la sonde.

#### US-023: Sonde d'usage par `codex app-server`
**Description:** As a utilisateur de Codex, I want voir mes limites Codex à jour dès l'ouverture de la bulle so that je n'aie pas à finir un tour pour les rafraîchir.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008, US-022 (amendement 1.5)

**Acceptance Criteria:**
- [ ] Le worker lance le binaire Codex réel (jamais le shim de Paneflow) en `codex app-server`, avec le `CODEX_HOME` du compte et sans variable `PANEFLOW_*`, fait la poignée de main JSON-RPC, appelle `account/rateLimits/read`, puis ferme le processus en moins de 2 s après la réponse. Aucun tour n'est démarré. Référence : [`CodexProvider.ts:580`](https://github.com/pingdotgg/t3code/blob/37d7615a91d880c3776d575e1152b9c955104de2/apps/server/src/provider/CodexProvider.ts#L580) de T3 Code.
- [ ] La sonde partage le déclencheur, le plancher de 30 s par compte et la limite de 2 sondes simultanées de US-022, et ne s'exécute jamais sur le thread de rendu.
- [ ] Les fenêtres primaire et secondaire de la réponse deviennent l'instantané du compte, avec les bornes et les libellés de US-008 ; quand une lecture de rollout et la sonde rapportent toutes deux, l'observation la plus récente gagne.
- [ ] Given un binaire absent, une connexion par clé API, un délai de 10 s dépassé, une erreur JSON-RPC ou un schéma inconnu, then l'instantané précédent est conservé (ou l'état `NoCodexData` reste affiché), un avertissement est journalisé, et rien ne panique.
- [ ] Un test pilote un faux `codex app-server` qui rejoue une réponse enregistrée sur la version de Codex installée, et couvre succès, erreur, délai et schéma inconnu. Le test de garde de US-003 couvre le module de la sonde.

---

### EP-004: Page Settings › Usage (variante P2)

Ajoute la page Usage : tuiles de limites à anneaux, registres d'activité par compte, et métrique Tokens.

**Definition of Done:** la page reproduit la variante P2 de l'artifact validé, avec un registre de 53 semaines, le libellé de l'agent au-dessus de chaque registre ; les trois métriques fonctionnent ; le registre se parcourt au clavier ; `/better-interface` rend `Approve` ; Arthur a fait la passe visuelle Windows (la passe macOS est portée par US-021, amendement 1.7).

#### US-014: Entrée Usage et tuiles de limites
**Description:** As a utilisateur à deux comptes, I want une page qui réunit les limites de tous mes comptes so that je compare mes abonnements d'un coup d'œil.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007, US-010, US-013

**Acceptance Criteria:**
- [ ] `SettingsSection::Usage` s'ajoute (`src-app/src/app/window.rs:4`), et l'entrée « Usage » s'insère dans Integrations entre Agents et Plugins (`src-app/src/settings/chrome.rs:119-138`). Elle a un glyphe de graphique en traits de 15 px, la taille commune de la navigation (amendement 1.7) (nouveau SVG dans `src-app/assets/icons/`) et les mots-clés « usage », « limits », « plan », « quota », « subscription », « tokens » et « activity ».
- [ ] La page a un titre de 26 px « Usage », l'introduction 12 px `muted` « Plan limits come from Claude Code and Codex while they run in Paneflow. Activity is counted by Paneflow on this computer. », la mention « Updated {temps relatif} » et un bouton secondaire « Refresh », qui relance la lecture Codex et recharge le journal.
- [ ] Sous l'eyebrow « Plan limits », une grille de deux colonnes de tuiles (squircle 20 sur `card`) présente chaque compte : anneaux concentriques de 76 px (Week dehors, Session dedans, logo au centre), nom en 13 px Medium, libellé, légende (pastille, fenêtre, pourcentage, remise à zéro) et ligne de projection.
- [ ] Sous 560 px de largeur de page, la grille passe à une colonne.
- [ ] Les tuiles reprennent les états et les textes de US-013.
- [ ] La recherche des Settings trouve la page avec « usage », « quota » ou « limit », via des entrées `SettingCopy` déclarées dans `src-app/src/settings/search.rs`.
- [ ] Given aucun compte, then une carte unique affiche « No Claude Code or Codex account found. Install an agent in Settings › Agents. », avec une ligne qui ouvre Agents.
- [ ] La dernière ligne de la bulle de US-012 devient « Open usage settings » et ouvre Settings › Usage (amendement 1.4).

#### US-015: Registres d'activité sur 53 semaines
**Description:** As a développeur multi-agents, I want un registre quotidien par compte, à la GitHub so that je voie mon rythme et compare mes agents sur un an.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-009, US-014

**Acceptance Criteria:**
- [ ] La ligne d'eyebrow « Activity by agent » porte à droite un contrôle segmenté Turns, Tokens, Active time, fait de pilules pleines, Turns par défaut. Le choix n'est pas persisté.
- [ ] Il y a un registre par compte qui a de l'activité dans les 53 dernières semaines, Claude Code d'abord, puis Codex, puis les autres par ordre alphabétique. Au-dessus de chaque registre, une ligne montre le logo en 14 px, le nom de l'agent en 12,5 px Medium, le libellé du compte en 11 px `muted`, et à droite le total (« {1,284} turns in the last year »).
- [ ] Le registre compte 53 colonnes et 7 lignes, lundi en premier, en cases de 8 px avec 2 px d'écart et un rayon de 2. Les mois sont libellés au-dessus en 10,5 px `muted`, et Mon, Wed et Fri à gauche. Les jours futurs de la semaine en cours ne sont pas dessinés.
- [ ] L'échelle est commune : les niveaux 1 à 4 viennent des quartiles des valeurs non nulles de tous les registres affichés pour la métrique ; le niveau 0 est `subtle`. La rampe de Claude Code part de `#D97757`, celle des autres agents de `text`, mélangées à 30, 52, 76 et 100 %.
- [ ] Sous les registres, une ligne de détail donne le jour survolé ou focalisé, aujourd'hui par défaut : « {Tue, Aug 18}: Claude Code 41 turns · Codex 12 turns · Gemini none ».
- [ ] Chaque registre est une grille au clavier : Tab y entre sur la case du jour, les flèches changent de jour ou de semaine, Début et Fin vont au bout de la ligne. Chaque case a pour nom « {Tuesday, August 18, 2026}: {41 turns} », ou « no activity ».
- [ ] Given aucune activité nulle part, then la page affiche « No activity yet. Paneflow counts turns from agents that run in its panes. » au lieu de registres vides.
- [ ] Given la métrique Tokens et un compte sans données de tokens, then son registre est remplacé par « No token data for this agent. »
- [ ] Quatre registres se rendent en moins de 16 ms dans une build release, mesuré par un journal de temps dans un test ou un bench.

#### US-016: Tokens tirés des transcripts
**Description:** As a développeur sur abonnement, I want que le journal compte mes tokens par jour so that la métrique Tokens montre ce que je consomme vraiment.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008, US-009

**Acceptance Criteria:**
- [ ] Claude : à chaque frame `Stop` dont le `transcript_path` est sous un dossier connu, le worker lit hors du thread de rendu les octets ajoutés depuis le dernier offset. Il additionne `message.usage.input_tokens` et `output_tokens` des entrées assistant, dédoublonnées par `message.id`. Les tokens de cache sont exclus.
- [ ] Codex : à chaque lecture de rollout (US-008), le worker ajoute l'écart de `info.total_token_usage.input_tokens` et `output_tokens` depuis le dernier `token_count` vu pour ce fichier. Le premier `info: null` est ignoré, et l'entrée mise en cache est exclue.
- [ ] Les tokens comptent pour le jour local de l'horodatage de l'entrée.
- [ ] Les offsets par fichier sont persistés dans `ledger.json` sous `cursors`, avec au plus 200 entrées et une éviction des plus anciennes.
- [ ] Given un fichier plus court que son offset (tronqué ou remplacé), then la lecture repart de zéro, et la déduplication par `message.id` évite un double compte dans la journée.
- [ ] Given une ligne de plus de 1 Mo ou un JSON invalide, then la ligne est ignorée et la lecture continue.
- [ ] Une passe lit au plus 8 Mo par fichier ; le reste attend la passe suivante.

---

### EP-005: Chaînage, rattrapage et documentation

Couvre les utilisateurs qui ont déjà un status line, le rattrapage de l'historique existant, et les contrats de documentation.

**Definition of Done:** (US-017 annulée par l'amendement 1.5) le rattrapage remplit le journal sans double compte ; DESIGN.md, ARCHITECTURE.md, le CHANGELOG et la documentation du site décrivent la fonctionnalité.

#### US-017: Chaînage d'un status line existant, sur demande

**Status:** CANCELLED (amendement 1.5) : la sonde de US-022 lit les limites quel que soit le status line, sans réécrire `settings.json`.
**Description:** As a utilisateur qui a déjà un status line, I want garder le mien tout en donnant ses limites à Paneflow so that je n'aie pas à choisir entre les deux.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006, US-013

**Acceptance Criteria:**
- [ ] Dans l'état `ForeignStatusLine`, le bouton « Chain status line » affiche sur place la commande d'origine et un bouton de confirmation. La confirmation réécrit `statusLine.command` en chemin de `paneflow-ai-hook`, suivi de `StatusLine --then` et de la commande d'origine correctement échappée, et marque l'objet comme géré.
- [ ] Le relais exécute la commande d'origine avec le même stdin, dans le shell que Claude Code utilise : `sh -c` sur macOS et Linux ; sous Windows, Git Bash s'il est installé, sinon PowerShell. Il écrit sa sortie standard telle quelle.
- [ ] Le chaînage ajoute au plus 30 ms au p95, sur 50 exécutions par OS, par rapport à la commande d'origine seule.
- [ ] « Unchain », ou `usage.plan_limits: false`, ou le retrait de l'intégration, restaure la commande d'origine à l'octet près.
- [ ] Given une commande d'origine qui échoue, then le relais n'écrit rien et sort avec 0.
- [ ] Given une commande d'origine qui dépasse 10 s, then le relais l'arrête, n'écrit rien et sort avec 0.
- [ ] Given un `settings.json` modifié entre l'affichage et la confirmation, then l'opération s'annule, le fichier est relu et l'état réaffiché.
- [ ] `paneflow-ai-hook` ne gagne aucune dépendance.

#### US-018: Rattrapage initial depuis les transcripts
**Description:** As a nouvel utilisateur, I want que mon registre montre déjà mes dernières semaines so that la page ne s'ouvre pas sur une année vide.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] Au premier démarrage du worker avec un journal vide, ou par « Import past activity » sur la page, le worker parcourt les transcripts Claude `<dossier>/projects/**/*.jsonl` et les rollouts Codex de chaque compte de l'inventaire. Il compte les tours (messages utilisateur qui ne sont pas des résultats d'outil) et les tokens par jour.
- [ ] Le parcours tourne en arrière-plan, à basse priorité, et peut s'interrompre. La page affiche « Importing past activity… {n} files » pendant l'opération, sans jamais bloquer le rendu.
- [ ] Les jours importés portent un drapeau par compte, si bien qu'un second import n'ajoute rien.
- [ ] Given plus de 10 000 fichiers ou plus de 2 Go, then l'import s'arrête à la limite, et la page affiche « Imported the last {30} days ».
- [ ] La page précise sous les registres : « Claude Code keeps transcripts for 30 days by default. »

#### US-019: Contrats de design, d'architecture et documentation
**Description:** As a mainteneur, I want que la fonctionnalité soit décrite là où les agents et les utilisateurs la cherchent so that la prochaine modification respecte la règle « aucun credential ».

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-013, US-015

**Acceptance Criteria:**
- [ ] DESIGN.md décrit la capsule (5.2, pied de sidebar), la bulle (5.6), la page Usage (5.5), l'anneau (4.4 et 4.7) et les couleurs de niveau.
- [ ] ARCHITECTURE.md décrit le trajet relais, host, worker, app, les fichiers `usage/plans.json` et `usage/ledger.json`, et la règle qui interdit toute lecture de credential.
- [ ] Le CHANGELOG annonce la capsule, la page, le relais status line et la clé `usage.plan_limits`.
- [ ] La documentation utilisateur est rédigée dans le dépôt `paneflow-web`, jamais seulement dans le miroir `docs/user/`. Elle couvre la page, le relais, ce qui est stocké et la limite de 30 jours des transcripts.
- [ ] Given un lecteur qui a son propre status line, then la documentation explique l'état `ForeignStatusLine`, le chaînage et comment le défaire.

---

### EP-006: Validation Linux

Rejoue sur Linux, dans un pane Paneflow réel, les constats que US-001 a faits sous Windows.

**Definition of Done:** le rapport de US-020 est consigné dans sa story, avec la distribution, le serveur d'affichage et la version de Claude Code.

#### US-020: Validate on Linux: le status line et le relais dans un pane Paneflow
**Description:** As a mainteneur, I want rejouer US-001 sur Linux so that la source des limites Claude soit prouvée sur cette cible livrée.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given Linux (Wayland ou X11) et un compte Claude Pro, Max ou Ultra, when un script de status line recopie son stdin dans un pane Paneflow, then le JSON contient `rate_limits.five_hour` et `rate_limits.seven_day` avec `used_percentage` et `resets_at`. La distribution, le serveur d'affichage et la version de Claude Code sont consignés.
- [ ] Given le même script, then son environnement contient `PANEFLOW_SESSION_ID` et `PANEFLOW_HOST_ENDPOINT`, plus `CLAUDE_CONFIG_DIR` quand l'espace le définit.
- [ ] Given le `statusLine` géré posé par US-006 (`paneflow-ai-hook.sh StatusLine`), when une réponse arrive, then le host accepte le frame `status_line` et le publie en `plan_limits` sur le flux `agent.follow`.
- [ ] Given `disableAllHooks: true`, then le status line ne s'exécute pas.

---

### EP-007: Validation macOS

Rejoue sur macOS les constats de US-001, en dernier, sur une machine Scaleway.

**Definition of Done:** le rapport de US-021 est consigné dans sa story, avec la version de macOS, l'architecture et la version de Claude Code ; Arthur a fait la passe visuelle macOS de la capsule et de la bulle.

#### US-021: Validate on macOS: le status line et le relais dans un pane Paneflow
**Description:** As a mainteneur, I want rejouer US-001 sur macOS so that la source des limites Claude soit prouvée sur la dernière cible livrée.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006, US-020

**Acceptance Criteria:**
- [ ] Given macOS (Apple Silicon, sur une machine Scaleway) et un compte Claude Pro, Max ou Ultra, when un script de status line recopie son stdin dans un pane Paneflow, then le JSON contient `rate_limits.five_hour` et `rate_limits.seven_day` avec `used_percentage` et `resets_at`. La version de macOS et celle de Claude Code sont consignées.
- [ ] Given le même script, then son environnement contient `PANEFLOW_SESSION_ID` et `PANEFLOW_HOST_ENDPOINT`, plus `CLAUDE_CONFIG_DIR` quand l'espace le définit.
- [ ] Given le `statusLine` géré posé par US-006, when une réponse arrive, then le host accepte le frame `status_line` et le publie en `plan_limits` sur le flux `agent.follow`.
- [ ] Given `disableAllHooks: true`, then le status line ne s'exécute pas.
- [ ] Arthur fait sur macOS la passe visuelle de la capsule et de la bulle de EP-003 (amendement 1.4) et de la page Settings › Usage de EP-004 (amendement 1.7), comparées à l'artifact validé.

---

## Functional Requirements

- FR-01: Le système doit lister un compte par couple (agent, dossier de config canonique), en réunissant le dossier par défaut, ceux des espaces et ceux des profils.
- FR-02: Le système doit obtenir les limites Claude uniquement à partir du JSON que Claude Code passe à sa commande de status line, ou de la réponse à la requête de contrôle `get_usage` du binaire Claude Code non modifié (amendement 1.5).
- FR-03: Le système doit obtenir les limites Codex uniquement à partir des événements `token_count` des rollouts écrits par Codex, ou de la méthode `account/rateLimits/read` de `codex app-server` non modifié (amendement 1.5).
- FR-04: Le système ne doit jamais lire, copier, écrire ni rafraîchir `.credentials.json`, une entrée de Keychain, `auth.json` ou une variable de token, ni émettre de requête HTTP pour cette fonctionnalité. Seuls les binaires Claude Code et Codex, lancés sans modification par les sondes de US-022 et US-023, contactent leur fournisseur avec leur propre login (amendement 1.5).
- FR-05: Le système doit libeller une fenêtre d'après sa durée et lui attribuer un niveau aux seuils 0,80 et 0,95.
- FR-06: Le système doit afficher dans le pied de sidebar le pourcentage de la fenêtre la plus consommée des comptes de l'espace actif.
- FR-07: Quand l'utilisateur ouvre la capsule, le système doit afficher chaque compte avec ses fenêtres, leur remise à zéro et une projection pour la semaine.
- FR-08: Le système doit installer un `statusLine` géré dans chaque dossier Claude qui n'en a pas, et ne jamais modifier un `statusLine` étranger sans confirmation.
- FR-09: Le système doit compter les tours et le temps actif par jour local et par compte, à partir d'OSC 7501 et des hooks, sans stocker de contenu.
- FR-10: Le système doit conserver 400 jours de journal et élaguer au-delà.
- FR-11: Le système doit présenter dans Settings › Usage une tuile par compte et un registre de 53 semaines par compte, sur une échelle commune.
- FR-12: Quand une source manque, le système doit afficher l'état du compte et l'action qui le corrige, jamais une jauge à 0 % sans explication.
- FR-13: Le système doit permettre de désactiver la lecture des limites (`usage.plan_limits`) et l'affichage de la capsule (`sidebar_show.usage`).

## Non-Functional Requirements

- **Performance:** le relais rend la main en moins de 100 ms au p95 quand le host est arrêté ; le chaînage ajoute au plus 30 ms au p95 ; la capsule se met à jour moins de 1 s après un frame ; la page Usage se rend en moins de 16 ms avec 4 registres (build release) ; une lecture Codex lit au plus 256 Ko ; 0 I/O synchrone sur le thread de rendu.
- **Security:** 0 lecture de fichier ou d'entrée de credential, vérifié par un test de garde ; 0 requête réseau ajoutée ; les frames et les fichiers ne contiennent que des pourcentages, des horodatages, des compteurs, des chemins de dossier et des libellés ; les fichiers de `~/.paneflow/usage/` héritent des permissions privées du home Paneflow (0600 sous Unix).
- **Accessibility:** WCAG 2.2 AA. Chaque contrôle a un nom accessible qui commence par son texte visible ; capsule, bulle, contrôle segmenté et registres sont entièrement utilisables au clavier ; aucun niveau n'est porté par la seule couleur, puisque le pourcentage est toujours écrit.
- **Scalability:** jusqu'à 20 comptes et 400 jours ; `ledger.json` sous 1 Mo pour 4 comptes et sous 5 Mo pour 20 ; `plans.json` sous 256 Ko.
- **Reliability:** le relais sort avec 0 dans 100 % des cas ; un fichier corrompu n'empêche jamais le démarrage du worker ; tout parseur tolère des champs absents ou mal typés sans paniquer ; l'écriture se fait au plus une fois toutes les 10 s pour `plans.json` et toutes les 60 s pour `ledger.json`.
- **Binary size:** `paneflow-ai-hook` reste sous 384 000 octets en `release-min` (Linux x86_64).

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Premier lancement | Aucun instantané, journal vide | Capsule en piste seule ; registres remplacés par l'état vide | "No plan data yet. Send a message to Claude Code in a Paneflow pane to load your limits." |
| 2 | Attente de la première réponse | Session ouverte, aucune réponse encore | Instantané précédent conservé avec son âge | "Updated 2 h ago" |
| 3 | Status line personnel | `statusLine` étranger dans `settings.json` | Aucune écriture ; état `ForeignStatusLine` ; seule la sonde de US-022 lit les limites | "Claude Code has not reported plan limits for {dossier} yet. Check that it is installed and signed in for this folder, then reopen this panel." |
| 4 | Hooks coupés | `disableAllHooks: true` | État `HooksDisabled` ; seule la sonde de US-022 lit les limites | "Claude Code has not reported plan limits for {dossier} yet. Check that it is installed and signed in for this folder, then reopen this panel." |
| 5 | Facturation par clé API | `ANTHROPIC_API_KEY` ou `OPENAI_API_KEY` actif pour l'origine | Compte `api_billing`, aucune jauge | "Billed through an API key. This account has no plan limits." |
| 6 | Siège Team ou Enterprise | `rate_limits` absent après une réponse | État `NoLimitsReported` | "Claude Code reports no plan limits for this account." |
| 7 | Remise à zéro passée | `resets_at` < maintenant | Fenêtre à 0 %, projection masquée | "Reset at 09:00. Updates at your next message." |
| 8 | Usage ailleurs | Consommation sur claude.ai ou une autre machine | Valeur juste au prochain message dans Paneflow ; âge affiché | "Updated 40 min ago" |
| 9 | Codex sans rollout exploitable | Aucun `token_count` avec `rate_limits` | État `NoCodexData` | "No plan data yet. Finish a Codex turn in a Paneflow pane to load your limits." |
| 10 | Fichier corrompu | `plans.json` ou `ledger.json` invalide | Démarrage à vide ; journal renommé `.bad-<horodatage>` | None (avertissement journalisé) |
| 11 | Bornes | Pourcentage hors de [0, 100], `resets_at` à plus de 8 jours | Fenêtre ignorée | None |
| 12 | Deux origines pour un dossier | Espace et profil sur `~/.claude-work` | Un seul compte, deux origines | None |
| 13 | Session hors de Paneflow | Claude lancé dans un autre terminal | Relais sans effet, status line chaîné affiché normalement | None |
| 14 | Redémarrage pendant un tour | Worker relancé | Tour compté une fois, temps actif jusqu'au dernier événement | None |
| 15 | Fenêtre minimale | 800 px de large | Filtre d'au moins 120 px ; capsule réduite à l'anneau | None |
| 16 | Plus de 4 comptes | 5 dossiers de config ou plus | Bulle défilante au-delà de 420 px | None |
| 17 | Agent muet | Gemini sans OSC 7501 ni hook | Absent du journal et des registres | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Claude Code retire ou renomme `rate_limits` dans le JSON du status line | Low | High | Parsing tolérant (US-004) ; état `NoLimitsReported` ; version consignée par US-001 et vérifiée à chaque montée de version de Claude Code |
| 2 | Beaucoup d'utilisateurs avancés ont déjà un status line (ccstatusline et autres) | High | Medium | Installation seulement en l'absence de status line ; chaînage sur demande et réversible (US-017) |
| 3 | Les rollouts Codex n'écrivent pas `rate_limits` dans la version courante | Medium | High | Validation par US-002 avant US-008 ; repli décidé par l'Open Question 3 |
| 4 | Les sièges Team et Enterprise ne reçoivent aucune limite | High | Medium | État explicite ; question ouverte 2 ; aucune jauge trompeuse |
| 5 | `paneflow-ai-hook` dépasse son plafond de taille | Medium | Medium | Aucune dépendance nouvelle ; gate de taille sur US-005 et US-017 |
| 6 | Double compte ou oubli de tours entre OSC 7501 et les hooks | Medium | Medium | Réducteur pur ; rejeu d'une séquence enregistrée de 20 tours (US-009) |
| 7 | L'utilisateur prend une valeur ancienne pour l'état réel (usage sur le web) | High | Low | Âge toujours affiché ; sémantique de remise à zéro ; documentation (US-019) |
| 8 | Différences de shell sous Windows pour le status line | Medium | Medium | US-001 couvre Git Bash et PowerShell ; US-017 reproduit la règle de Claude Code |
| 9 | Changement de la politique d'Anthropic sur les commandes de status line | Low | High | Point d'extension documenté par Anthropic ; aucune donnée transmise hors de la machine |
| 10 | La requête `get_usage` change ou disparaît (le SDK la nomme `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET`) | Medium | Medium | Parsing tolérant ; le relais du status line reste une seconde source ; version de Claude Code conservée ; échec explicite sans jauge trompeuse (US-022) |

## Non-Goals

- Aucun appel à `GET api.anthropic.com/api/oauth/usage` ni à `GET chatgpt.com/backend-api/wham/usage`, et aucune lecture de credential : la page légale de Claude Code l'interdit. À revoir seulement avec un accord écrit d'Anthropic.
- Pas de multi-compte avec basculement à la Zeron : un compte est un dossier de config, et les espaces et profils existants suffisent.
- Pas de coûts en dollars, pas d'affichage de `spend_limit` des gateways : prévu après la v1 si un utilisateur d'entreprise le demande.
- Pas de synchronisation du journal entre machines.
- Pas de notification ni de toast au franchissement d'un seuil.
- Pas de limites pour Gemini, Grok, OpenCode ou Pi : leur activité seule est comptée, quand ils émettent OSC 7501 ou des hooks. Leur CLI n'expose pas le quota hors du mode interactif ; les lire exigerait le token de l'utilisateur (amendement 1.5). À revoir quand un de ces CLI publie une méthode headless ou ACP pour le quota.
- Pas de courbe de consommation façon CodexBar : la phrase de projection la remplace en v1.

## Files NOT to Modify

- `native/libghostty/**`, `crates/paneflow-libghostty-sys/**`, `crates/paneflow-terminal-ghostty/**`: moteur unique, hors sujet.
- `src-app/src/terminal/pty_session/session_backend.rs`, `src-app/src/terminal/types.rs`: frontière du moteur.
- `crates/paneflow-shim/**`, `crates/paneflow-mcp/**`: helpers plafonnés en taille, sans lien avec l'usage.
- `crates/paneflow-host/src/program_status.rs`: gestion de la spec OSC 7501 ; l'usage lit ses sorties, il ne la modifie pas.
- `src-app/Cargo.toml` (valeurs `rev` de GPUI et feature `font-kit`): épinglage de GPUI.
- `src-app/src/window_state.rs`: plafond de 4096 octets ; aucune donnée d'usage n'y va.
- `docs/user/**`: miroir généré ; la documentation s'écrit dans `paneflow-web`.

## Technical Considerations

- **Architecture:** le worker possède les instantanés et le journal, l'app affiche. Recommandé : étendre le flux d'état existant du worker vers l'app plutôt qu'ajouter une méthode IPC. Engineering to confirm que le worker connaît l'environnement de lancement de chaque session.
- **Data Model:** `ledger.json` indexé par jour puis par compte, ou un journal JSONL en ajout seul ? Recommandé : JSON indexé, réécrit au plus une fois par minute, parce que 400 jours × 20 comptes restent sous 5 Mo. Trade-off : le JSONL survit mieux à une coupure brutale, mais demande un compactage.
- **API Design:** le relais réutilise `METHOD_AGENT_EVENT` avec une nouvelle variante `StatusLine` d'`AiHookMethod`, ou une méthode du host dédiée ? Recommandé : la variante, pour garder un seul transport et sa livraison en arrière-plan.
- **Inventory location:** l'inventaire se calcule-t-il dans l'app, qui connaît la configuration, les espaces et les profils, puis se pousse au worker ? Recommandé : oui, à chaque changement de `paneflow.json`.
- **Rendering:** GPUI offre-t-il un arc natif dans `PathBuilder`, ou faut-il approcher l'arc par des courbes de Bézier comme le squircle ? À confirmer pour US-010.
- **Dependencies:** aucune bibliothèque nouvelle. `serde_json` est déjà présent dans `paneflow-ai-hook` ; `ureq` n'est pas utilisé ici.
- **Migration:** aucune donnée existante. Retour arrière : supprimer `~/.paneflow/usage/` et passer `usage.plan_limits` à `false`, ce qui retire le `statusLine` géré.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Délai entre une réponse de Claude Code et la capsule | N/A (new) | < 1 s | Month-1 | Horodatages du frame et de la publication, journalisés en debug |
| Lectures de credential | N/A (new) | 0 | Chaque PR | Test de garde de US-003 |
| Exactitude du journal | N/A (new) | 20 tours sur 20 dans la séquence rejouée | Month-1 | Test de US-009 |
| Taille de `paneflow-ai-hook` | Plafond actuel 384 000 octets | ≤ 384 000 octets | Chaque PR | Job de budget de taille de la CI |
| Rendu de la page Usage | N/A (new) | < 16 ms avec 4 registres | Month-1 | Mesure en build release (US-015) |
| Comptes avec limites visibles chez Arthur | 0 | 100 % des comptes Claude Max et Codex Pro de ses espaces | Month-1 | Passe visuelle manuelle |

## Open Questions

- **Source des limites (Arthur, avant EP-001) :** le 2026-10-31, Arthur avait retenu la lecture du login du CLI, comme Zeron. Ce PRD passe au status line et aux rollouts, à cause de la page légale de Claude Code. Si Arthur maintient l'endpoint OAuth, US-005, US-006, US-007 et US-008 deviennent des sondes HTTP, et l'interface ne change pas.
- **Team et Enterprise (Arthur, pendant US-001) :** que reçoit le status line d'un siège d'entreprise ? `rate_limits`, `spend_limit` ou rien ? Le texte de l'état `NoLimitsReported` en dépend.
- **Repli Codex (Arthur, avant US-008) :** si US-002 montre que les rollouts n'ont pas `rate_limits`, faut-il appeler `wham/usage` avec `auth.json`, en lisant un credential, et alors sous quelles conditions d'OpenAI ? Elles n'ont pas été vérifiées.
- **`spend_limit` (après la v1) :** afficher la fenêtre de dépense des gateways d'entreprise ?
[/PRD]
