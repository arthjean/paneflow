[PRD]
# PRD: Codex Native Chat dans les panes CLI (2026-Q3)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-07-10 | Arthur Jean | PRD initial pour un Chat Codex natif dans `AppMode::Cli`, alimenté par Codex App Server et l'abonnement ChatGPT existant. Le passage CLI/Chat crée toujours une nouvelle session et ne transfère jamais le thread actif. |

## Problem Statement

1. Paneflow organise aujourd'hui les coding agents comme des processus dans des terminaux. Dans `AppMode::Cli`, un onglet peut afficher un terminal, du Markdown ou un diff, mais aucune surface ne transforme les événements structurés de Codex en messages, reasoning, commandes, modifications de fichiers et approvals.
2. Un utilisateur possédant déjà un abonnement ChatGPT/Codex doit rester dans le TUI Codex ou fournir une API key à une autre intégration. Paneflow ne propose aucun chemin natif utilisant l'authentification ChatGPT gérée par Codex sans manipuler les tokens.
3. Le Composer actuel injecte du texte dans un PTY. Cette plomberie ne peut pas fournir les garanties nécessaires pour corréler un turn, afficher une approval, interrompre un agent ou reconstruire un transcript structuré.
4. Une action nommée comme une bascule CLI/Chat laisserait croire que le contexte courant est conservé. Le handoff réel exige un App Server multi-client et un transport WebSocket encore expérimental. Cette dépendance augmenterait le risque cross-platform et le couplage aux versions de Codex avant même d'avoir validé l'usage du Chat natif.
5. Le PRD `prd-agents-ui-codex-redesign-2026-Q3.md` emploie déjà le mot « Chats » pour des terminaux libres dans `AppMode::Agents`. Sans frontière explicite, une nouvelle implémentation pourrait modifier le mauvais mode ou réintroduire le chat ACP supprimé.

**Why now:** Codex App Server expose désormais les primitives nécessaires à un client riche: authentification ChatGPT gérée, catalogue dynamique, threads, turns, événements structurés, approvals, interruption, usage et quotas. Paneflow possède déjà les panes, onglets, menus contextuels, persistence de session et lifecycle PTY nécessaires pour intégrer cette surface dans `AppMode::Cli`. Le choix « nouvelle session » retire le risque multi-client du MVP et permet d'évaluer le Chat natif sur le transport stdio par défaut.

## Overview

Ce PRD ajoute une surface `Codex Chat` dans les panes du mode CLI traditionnel de Paneflow. Un runtime dédié lance le binaire Codex installé avec `codex app-server` sur stdio JSONL, initialise une connexion v2, laisse Codex posséder l'authentification et les tokens, puis expose à GPUI un état typé pour les comptes, modèles, threads, turns, items, approvals, usage et erreurs.

Un onglet Chat démarre toujours un thread neuf avec `thread/start`. Le menu contextuel d'un onglet Terminal propose `Start new Codex Chat`; celui d'un onglet Codex Chat propose `Start new Codex CLI`. Chaque action conserve l'emplacement de l'onglet, son identité Paneflow, son CWD, son nom et les réglages compatibles, mais ferme la surface courante et crée une nouvelle session sans contexte conversationnel. L'ancienne session Codex n'est ni supprimée ni archivée et reste disponible dans l'historique Codex.

Codex reste la source de vérité du compte, des modèles et du transcript. Paneflow persiste uniquement le binding de l'onglet, les réglages sélectionnés et un cache UI borné. Le MVP utilise App Server v2 sur stdio. Il exclut WebSocket, la continuité CLI/Chat, les routes ChatGPT privées reproduites par Pi, les autres providers, `AppMode::Agents`, `AppMode::Diff` et les commandes standalone `paneflow <verb>`.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Démarrage d'un Chat Codex authentifié | 20/20 scénarios smoke réussis avec un compte ChatGPT déjà connecté | ≥ 99 % des tentatives dogfooding aboutissent sans relancer Paneflow |
| Sémantique « nouvelle session » | 20/20 actions CLI/Chat produisent un nouvel identifiant de thread ou un nouveau processus CLI sans `resume` | 0 rapport de continuité implicite ou de thread remplacé silencieusement |
| Compatibilité Codex | 1 version minimale et 1 plage testée documentées avant US-002 | Matrice maintenue pour les 3 dernières versions supportées de Codex |
| Sécurité des credentials | 0 token ou API key dans les logs, la télémétrie et `session.json` | 0 régression détectée par les tests de redaction et de persistence |
| Réactivité du renderer | p95 notification JSON reçue vers frame GPUI demandée < 50 ms à 100 événements/s | p95 < 50 ms sur 16 Chat tabs chargés |
| Cross-platform | Smoke complet sur Linux Wayland, Linux X11, macOS arm64 et Windows 11 x64 | 100 % des plateformes de release passent le runbook à chaque release mineure |

## Target Users

### Orchestrateur local de coding agents
- **Role:** développeur qui utilise Paneflow pour exécuter plusieurs agents locaux en parallèle dans des panes.
- **Behaviors:** alterne entre shells, TUI d'agents et tâches structurées; utilise un abonnement ChatGPT/Codex; conserve plusieurs sessions par workspace.
- **Pain points:** le TUI est efficace mais rend difficile la lecture séparée du reasoning, des diffs, commandes et approvals; une intégration API indépendante demanderait une seconde facturation ou une API key.
- **Current workaround:** lancer `codex` dans un terminal, parcourir l'ANSI et reprendre les sessions avec `codex resume`.
- **Success looks like:** créer un Chat Codex dans le pane courant, se connecter avec l'abonnement existant, choisir un modèle, suivre les actions structurées et répondre aux approvals sans quitter `AppMode::Cli`.

### Utilisateur terminal-first évaluant le Chat natif
- **Role:** utilisateur attaché au vrai TUI Codex mais souhaitant employer un Chat structuré pour certaines tâches.
- **Behaviors:** choisit le mode selon la tâche et change plusieurs fois de surface pendant une journée.
- **Pain points:** un handoff implicite peut perdre un turn, dupliquer un agent ou masquer la création d'un nouveau contexte.
- **Current workaround:** fermer manuellement un terminal, ouvrir un autre onglet et relancer une session sans indication sur ce qui est conservé.
- **Success looks like:** le menu annonce `Start new ...`, la confirmation liste ce qui sera fermé, et l'ancienne session reste retrouvable dans l'historique Codex.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Super:** les captures fournies par Arthur montrent une même zone de travail pouvant accueillir un terminal ou un Chat. Elles établissent la valeur produit de deux présentations, mais pas un contrat public démontrant un handoff du même processus vivant.
- **Zed Terminal Threads:** Zed sépare explicitement les threads terminal des threads agent natifs. Le CLI/TUI possède son authentification et sa configuration, tandis que l'éditeur possède l'organisation de la surface. Cette frontière confirme que deux types de sessions peuvent cohabiter sans partager leur runtime. Source: [Zed Terminal Threads](https://zed.dev/docs/ai/terminal-threads).
- **VS Code Chat Sessions:** une nouvelle session possède son propre contexte, workspace, modèle et niveau de permissions. Les approvals sont rendues dans le Chat et leur portée est liée à la session. Sources: [VS Code Chat Sessions](https://code.visualstudio.com/docs/chat/chat-sessions), [VS Code Approvals](https://code.visualstudio.com/docs/agents/approvals).
- **Pi:** Pi démontre une UX browser/device code et un reducer événementiel, mais réimplémente OAuth, appelle une route ChatGPT privée et maintient son catalogue de modèles. Ce PRD reprend les patterns UX, pas ces contrats internes.
- **Market gap:** Paneflow peut offrir terminal natif et Chat Codex natif dans les mêmes panes, sur Linux, macOS et Windows, en gardant une frontière de session explicite et l'abonnement Codex existant.

### Best Practices Applied
- **Contrat de session explicite:** les actions utilisent `Start new ...`, jamais `Switch`, et affichent une confirmation avant de fermer une surface active.
- **Credentials possédés par Codex:** Paneflow utilise `account/read` et les flows ChatGPT d'App Server; il ne lit ni n'écrit les tokens.
- **Catalogue dynamique:** modèles et reasoning efforts viennent de `model/list`; aucun identifiant de modèle n'est hardcodé.
- **Approvals structurées:** l'UI répond aux requêtes serveur par leur request ID, puis traite `serverRequest/resolved` et `item/completed` comme états autoritaires.
- **Isolation du renderer:** l'identité Paneflow de l'onglet ne dépend pas de l'EntityId du Terminal ou du Chat monté.
- **Dégradation explicite:** un binaire Codex absent, trop ancien, un login annulé ou un App Server mort produit un état actionnable; aucune nouvelle session silencieuse n'est créée comme fallback.

*Sources principales: [Codex App Server README](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md), [Zed Agents](https://zed.dev/docs/ai/agents), [VS Code Trust and Safety](https://code.visualstudio.com/docs/agents/concepts/trust-and-safety).*

## Assumptions & Constraints

### Assumptions (to validate)
- La version minimale retenue de Codex expose les méthodes v2 stables nécessaires sans activer `experimentalApi`; US-001 doit établir la matrice exacte.
- Un thread créé avec `thread/start` est persisté dans le même `CODEX_HOME` et reste découvrable par les outils d'historique Codex; US-001 doit le prouver par test.
- Une connexion stdio App Server peut gérer au moins 16 threads Chat chargés sans nécessiter un processus par onglet; US-002 et US-017 doivent mesurer ce scénario.
- Les utilisateurs ciblés disposent d'un binaire Codex installé ou acceptent un message expliquant comment l'installer; le bundling du binaire est hors scope.
- Les champs inconnus ajoutés au protocole peuvent être ignorés sans perdre les événements indispensables; US-001 doit définir la politique de compatibilité.

### Hard Constraints
- `AppMode::Cli` est la seule surface produit modifiée. `AppMode::Agents` et `AppMode::Diff` restent inchangés.
- Le changement Terminal/Chat démarre toujours une nouvelle session. Aucun `thread/resume` ne relie la session fermée à la surface nouvellement créée.
- Le Chat utilise l'App Server officiel sur stdio v2. WebSocket, routes ChatGPT privées et API expérimentales sont interdits dans le MVP.
- Paneflow ne persiste, ne journalise et ne transmet à sa télémétrie aucun access token, refresh token, device token ou API key.
- Le catalogue de modèles et les reasoning efforts proviennent exclusivement d'App Server.
- L'implémentation doit fonctionner sur Linux Wayland/X11, macOS Intel/Apple Silicon et Windows 10/11 x64; toute limitation ARM64 doit être explicitement documentée.
- Les dépendances GPUI et Alacritty locales restent des path dependencies; aucune migration de dépendance n'est incluse.
- Le transcript Codex reste la source de vérité. Le cache Paneflow est borné et reconstructible.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - vérifie le formatage Rust, obligatoire avant tout commit ou push touchant Rust.
- `cargo check --workspace` - vérifie le typage et la compilation de tous les crates du workspace.
- `cargo clippy --workspace -- -D warnings` - bloque toute nouvelle alerte Clippy.
- `cargo test --workspace` - exécute les tests unitaires et d'intégration du workspace.
- `cargo test -p paneflow-app --test flex_nchild -- --nocapture` - protège les invariants de layout GPUI pour toute story touchant panes ou tabs.

For UI stories, additional gates:
- Vérification manuelle du focus clavier, scroll, menu contextuel, resize, thème clair/sombre et `prefers-reduced-motion`.
- Smoke documenté sur Linux Wayland, Linux X11, macOS arm64 et Windows 11 x64 avant clôture d'un epic UI ou lifecycle.

## Epics & User Stories

### EP-001: Contrat Codex et runtime App Server

Établir une frontière versionnée et un processus stdio long-lived que les surfaces GPUI peuvent consommer sans connaître le transport, les credentials ou le lifecycle OS.

**Definition of Done:** la plage Codex supportée est documentée; App Server démarre, initialise, route les messages, s'arrête et récupère d'un crash sur les trois familles d'OS sans fuite de processus ni donnée sensible.

#### US-001: Valider le contrat App Server v2 supporté
**Description:** As a mainteneur Paneflow, I want une matrice de compatibilité Codex vérifiée so that l'implémentation cible un protocole testable au lieu de dépendre de la branche `main` implicite.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given le binaire/source Codex de référence, when le spike inspecte les schemas et le README App Server, then `initialize`, `account/*`, `model/list`, `thread/start|read|resume`, `turn/start|steer|interrupt`, les notifications d'items, les approvals, usage et rate limits sont classés stable ou exclus avec une preuve `file:line` dans `docs/codex-app-server-compatibility.md`.
- [ ] Given la matrice, then une version minimale Codex et une plage initiale testée sont fixées avec la méthode utilisée pour obtenir `codex --version`.
- [ ] Given un thread App Server créé avec `thread/start`, when le processus est arrêté proprement puis l'historique Codex est interrogé, then le spike prouve si le thread reste découvrable et documente son `sourceKind`.
- [ ] Given des champs JSON inconnus et une notification inconnue, when les fixtures de compatibilité sont désérialisées, then les champs sont tolérés et la notification est ignorée avec un diagnostic borné.
- [ ] Given une méthode indispensable absente ou renommée, when une version non supportée est testée, then le verdict est `unsupported` avec la méthode manquante et aucune tentative de fallback expérimental.
- [ ] Given `codex --version` absent, non UTF-8 ou non parsable, when la détection s'exécute, then elle retourne une erreur typée et ne panique pas.

#### US-002: Superviser un App Server stdio long-lived
**Description:** As a utilisateur de Chat Codex, I want un runtime App Server géré par Paneflow so that plusieurs onglets Chat partagent compte et catalogue sans processus orphelin.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un binaire Codex supporté, when le premier Chat demande un runtime, then Paneflow lance `codex app-server` avec stdin writable, stdout drainé en continu et stderr drainé séparément.
- [ ] Given deux Chat tabs avec la même identité `(binary path, version, CODEX_HOME canonique)`, when ils demandent le runtime, then un seul processus est créé et les deux obtiennent des handles indépendants.
- [ ] Given deux `CODEX_HOME` différents, when deux Chat tabs démarrent, then deux runtimes isolés sont créés et aucun état de compte n'est croisé.
- [ ] Given un shutdown Paneflow, when le runtime reçoit l'arrêt, then il dispose de 2 000 ms pour quitter proprement avant kill, puis son child est reaped sur Linux, macOS et Windows.
- [ ] Given la fermeture du dernier Chat tab, when le délai idle configurable de 30 s expire sans nouveau client, then le runtime est arrêté; une nouvelle demande pendant le délai annule l'arrêt.
- [ ] Given le binaire absent, non exécutable ou un spawn refusé, when la création est demandée, then aucun handle partiel n'est publié et l'erreur inclut le chemin testé et une action Retry.
- [ ] Given la mort de Paneflow, when l'OS nettoie l'arbre enfant, then les descendants App Server sont terminés via le Job Object Windows ou le guard de processus Unix prévu par le design.

#### US-003: Corréler JSON-RPC, notifications et requêtes serveur
**Description:** As a développeur de surfaces GPUI, I want un client App Server typé so that les vues consomment des événements de domaine sans parser du JSON ou gérer les request IDs.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] Given une connexion neuve, when le client démarre, then il envoie exactement un `initialize` avec `clientInfo`, attend la réponse, envoie `initialized` et bloque toute autre requête avant ce handshake.
- [ ] Given plusieurs requêtes concurrentes, when les réponses arrivent dans un ordre différent, then chaque réponse est livrée au bon caller par un ID unique sans fuite de waiter.
- [ ] Given une notification turn/item/account/model, when elle est reçue, then elle est convertie en événement de domaine typé et routée aux subscribers concernés par `threadId`.
- [ ] Given une approval ou un `requestUserInput` initié par le serveur, when il arrive avec un request ID, then le client conserve un responder one-shot et interdit une seconde réponse.
- [ ] Given une ligne JSONL vide ou une notification inconnue, when elle est lue, then elle est ignorée ou diagnostiquée sans arrêter le runtime.
- [ ] Given une frame invalide ou supérieure à 64 MiB, when elle est lue, then la connexion passe en erreur protocolaire, la frame n'est pas journalisée et les requêtes pendantes reçoivent la même erreur terminale.
- [ ] Given 100 notifications/s pendant 60 s, when le benchmark s'exécute, then la queue reste bornée à 2 048 événements et aucune notification terminale `turn/completed` ou `serverRequest/resolved` n'est perdue.

#### US-004: Récupérer d'un crash App Server
**Description:** As a utilisateur avec plusieurs Chats ouverts, I want un état de récupération déterministe so that un crash Codex ne ferme pas mes panes et ne crée pas de turns en double.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002, US-003

**Acceptance Criteria:**
- [ ] Given un App Server qui quitte, when l'exit est observé, then tous les Chat tabs liés passent en `Disconnected` en moins de 500 ms sans être fermés.
- [ ] Given un runtime crashé, when la politique de récupération s'exécute, then Paneflow effectue au maximum 1 restart automatique après 250 ms et expose ensuite un bouton Retry manuel.
- [ ] Given un `thread_id` connu après restart, when le tab se reconnecte, then `thread/resume` reconstruit le thread sans envoyer le draft ni relancer le dernier turn.
- [ ] Given un thread introuvable ou une auth expirée, when la reprise échoue, then le tab conserve son identité et affiche `Sign in`, `Retry` ou `Start new Chat` selon l'erreur; aucune nouvelle session n'est créée automatiquement.
- [ ] Given des requêtes pendantes au moment du crash, when le processus meurt, then elles échouent une seule fois et leurs contrôles UI sont réactivés.
- [ ] Given 3 crashs successifs en moins de 60 s, when l'utilisateur clique Retry, then chaque tentative reste manuelle et aucun restart loop background n'est créé.
- [ ] Given stderr contenant une chaîne ressemblant à un token, when le diagnostic est conservé, then les valeurs sensibles sont remplacées et le ring stderr reste ≤ 1 MiB.

---

### EP-002: Compte ChatGPT et catalogue Codex

Permettre à l'utilisateur d'employer son abonnement Codex sans API key Paneflow et choisir uniquement les modèles/efforts déclarés par le runtime actif.

**Definition of Done:** un utilisateur peut lire son état de compte, se connecter par navigateur ou device code, annuler, se déconnecter, choisir un modèle/effort valide et consulter les limites disponibles sans que Paneflow ne possède un secret.

#### US-005: Projeter l'état du compte Codex
**Description:** As a utilisateur abonné ChatGPT, I want voir si Codex est connecté so that je comprends pourquoi un Chat peut ou non démarrer.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Given un runtime initialisé, when `account/read` réussit, then l'UI distingue `Signed out`, `ChatGPT managed`, `API key external`, `Personal access token external` et `Provider without OpenAI auth`.
- [ ] Given un compte ChatGPT, when les métadonnées sont disponibles, then email et plan type sont affichés uniquement dans la surface compte et ne sont pas envoyés en télémétrie.
- [ ] Given `account/updated`, when l'auth mode change, then tous les Chat tabs du même runtime reflètent l'état au repaint suivant.
- [ ] Given `requiresOpenaiAuth=false`, when aucun compte OpenAI n'est présent, then le runtime n'affiche pas un blocage de login artificiel.
- [ ] Given un mode API key déjà configuré hors Paneflow, when il est lu, then Paneflow peut l'utiliser via App Server mais ne propose aucun champ de saisie ou d'édition de clé.
- [ ] Given `account/read` en timeout après 5 000 ms, when l'état est rendu, then il affiche Retry et ne déduit pas `Signed out`.
- [ ] Given une réponse contenant des champs de secret inconnus, when elle est convertie en domaine, then ces champs ne sont ni conservés ni sérialisés.

#### US-006: Gérer le login ChatGPT browser et device code
**Description:** As a utilisateur sans session Codex active, I want me connecter avec mon abonnement ChatGPT so that je n'ai aucune API key à fournir à Paneflow.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given `Signed out`, when l'utilisateur choisit browser login, then Paneflow appelle `account/login/start` avec le type ChatGPT, ouvre l'`authUrl` via le mécanisme OS et affiche un état d'attente lié au `loginId`.
- [ ] Given le choix device code, when App Server retourne verification URL et user code, then l'UI affiche le code, une action Copy et une action Open Browser sans poller directement les endpoints OAuth.
- [ ] Given un login en cours, when l'utilisateur clique Cancel, then `account/login/cancel` utilise le `loginId`, l'état revient à `Signed out` et une completion tardive ne reconnecte pas l'UI silencieusement.
- [ ] Given `account/login/completed` réussi puis `account/updated`, when les notifications arrivent, then le compte est rechargé et le catalogue de modèles est invalidé puis refetché une fois.
- [ ] Given un navigateur indisponible, when l'ouverture de l'URL échoue, then l'URL peut être copiée et le device code reste proposé.
- [ ] Given un login refusé, expiré ou en erreur réseau, when la completion arrive, then l'erreur actionnable est affichée sans URL/token dans les logs.
- [ ] Given un compte connecté, when l'utilisateur confirme Logout, then `account/logout` est appelé; si le revoke distant échoue mais le logout local réussit, l'UI reflète l'état retourné par `account/updated`.

#### US-007: Charger modèles et reasoning efforts dynamiquement
**Description:** As a utilisateur Codex, I want choisir parmi les modèles réellement disponibles pour mon compte so that le Chat ne dépend jamais d'un catalogue Paneflow périmé.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003, US-005

**Acceptance Criteria:**
- [ ] Given un compte utilisable, when `model/list` est appelé, then toutes les pages sont chargées, les modèles hidden sont exclus et l'ordre serveur est conservé.
- [ ] Given un modèle `isDefault`, when aucun choix Paneflow valide n'existe, then ce modèle est sélectionné; sinon le premier modèle visible devient le fallback.
- [ ] Given un modèle sélectionné, when ses `supportedReasoningEfforts` sont rendus, then l'ordre serveur et l'effort par défaut sont conservés sans dérivation lexicographique.
- [ ] Given un changement de modèle pendant un thread idle, when il est confirmé, then les réglages du prochain turn utilisent le modèle/effort sélectionnés et attendent l'acknowledgement requis.
- [ ] Given le modèle courant retiré après refetch, when le catalogue change, then l'UI sélectionne le nouveau default et affiche une notification unique.
- [ ] Given un catalogue vide, un timeout de 10 000 ms ou une page invalide, when le sélecteur est ouvert, then l'envoi est bloqué et Retry est disponible; aucun modèle hardcodé n'est injecté.
- [ ] Given deux runtimes avec des comptes différents, when les catalogues arrivent, then chaque tab utilise seulement le catalogue de son runtime.

#### US-008: Afficher limites et usage disponibles
**Description:** As a utilisateur d'un abonnement Codex, I want voir les limites fournies par mon compte so that je distingue une panne d'une limite atteinte.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given un compte ChatGPT managed, when `account/rateLimits/read` réussit, then les fenêtres, resets et crédits disponibles sont affichés dans le footer ou popover du Chat.
- [ ] Given une notification sparse `account/rateLimits/updated`, when elle arrive, then ses champs sont fusionnés au dernier snapshot au lieu de remplacer les valeurs absentes.
- [ ] Given `account/usage/read` disponible, when l'utilisateur ouvre les détails, then l'activité agrégée est affichée sans convertir les tokens en estimation de coût inventée.
- [ ] Given un auth mode sans rate limits ChatGPT, when la lecture est refusée, then les contrôles de quota sont masqués et le Chat reste utilisable.
- [ ] Given une erreur 429 pendant un turn, when elle est reçue, then l'UI affiche le reset connu ou un message sans reset si App Server n'en fournit pas.
- [ ] Given une erreur de quota temporaire, when l'utilisateur clique Retry avant le reset annoncé, then une confirmation évite une boucle automatique; aucun retry background n'est lancé.

---

### EP-003: Surface Chat native et boucle agent

Rendre dans un onglet GPUI une conversation Codex structurée, pilotée par threads/turns/items plutôt que par parsing ANSI.

**Definition of Done:** l'utilisateur peut créer un Chat, envoyer ou orienter un turn, suivre messages et actions, répondre aux approvals, interrompre le travail et conserver une UI cohérente après erreurs ou resize.

#### US-009: Introduire un onglet Codex Chat à identité stable
**Description:** As a utilisateur de panes, I want un onglet Chat qui se comporte comme les autres tabs so that je peux le déplacer, le nommer, le fermer et le restaurer sans casser son identité Paneflow.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un pane, when un Codex Chat est ajouté, then il possède une identité `SurfaceId` stable distincte de l'EntityId de son renderer GPUI.
- [ ] Given l'onglet Chat, when les fonctions titre, icône, focus, render, close, move-to-pane et drag sont appelées, then chaque match exhaustif de `TabContent` traite le Chat explicitement.
- [ ] Given un move inter-pane, when le Chat est déplacé, then `surface_id`, CWD, runtime binding, thread binding, draft et projection restent attachés au même onglet.
- [ ] Given la fermeture d'un Chat, when son Entity GPUI est drop, then seuls ses subscribers et états UI sont libérés; le thread Codex n'est ni supprimé ni archivé.
- [ ] Given un ancien layout Terminal/Markdown/Diff, when il est restauré, then son rendu et son identité restent identiques à la baseline.
- [ ] Given un type de surface inconnu dans `session.json`, when le restore s'exécute, then il produit un placeholder d'erreur ou ignore la surface selon le contrat documenté; il ne la transforme jamais silencieusement en terminal.
- [ ] Given le Chat sans runtime prêt, when il est rendu, then un état déterministe `Starting`, `Sign in`, `Unsupported Codex` ou `Failed` remplace le transcript vide.

#### US-010: Démarrer un thread et envoyer depuis le Composer
**Description:** As a utilisateur Chat, I want envoyer un prompt texte avec le modèle et le CWD sélectionnés so that Codex démarre une conversation neuve dans le bon workspace.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-007, US-009

**Acceptance Criteria:**
- [ ] Given runtime, compte et catalogue prêts, when un Chat neuf devient actif, then `thread/start` reçoit le CWD canonique, le modèle, l'effort, la sandbox et l'approval policy configurés, puis le `thread_id` retourné est lié au tab.
- [ ] Given un draft texte non vide, when l'utilisateur soumet, then `turn/start` est appelé une seule fois et le draft n'est effacé qu'après acceptation de la requête.
- [ ] Given un turn actif et un `expectedTurnId`, when l'utilisateur choisit Send follow-up, then `turn/steer` porte ce turn ID; sans turn actif, l'action n'est pas proposée.
- [ ] Given un prompt vide ou composé uniquement d'espaces, when Submit est invoqué, then aucun RPC n'est émis et le focus reste dans le Composer.
- [ ] Given `thread/start` refusé, when l'erreur revient, then aucun `thread_id` fantôme n'est persisté et le draft reste présent.
- [ ] Given `turn/start` timeout après 10 000 ms, when le statut du thread reste inconnu, then le bouton Send reste bloqué jusqu'à `thread/read` ou reconnexion; Paneflow ne renvoie pas automatiquement le prompt.
- [ ] Given un changement de CWD après création, when le pane/workspace actif change, then le thread conserve son CWD initial et l'UI demande une nouvelle session pour cibler un autre CWD.

#### US-011: Rendre messages, reasoning et lifecycle du turn
**Description:** As a utilisateur Chat, I want lire une projection structurée du turn so that je distingue entrée utilisateur, raisonnement, réponse, statut et usage.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given `turn/started`, item lifecycle et deltas agent, when les événements arrivent, then le transcript conserve leur ordre par turn/item et met à jour une seule cellule par item.
- [ ] Given un message utilisateur ou agent, when son Markdown est rendu, then headings, listes, liens et blocs de code sont supportés; HTML brut, escapes terminal et URI non autorisées ne sont pas exécutés.
- [ ] Given des deltas de reasoning/summary, when ils arrivent, then ils sont affichés dans une section distincte et repliable sans inventer de contenu absent du protocole.
- [ ] Given `turn/completed`, when le statut est completed, interrupted ou failed, then la cellule de turn affiche le statut et l'usage retournés par le serveur.
- [ ] Given l'utilisateur à moins de 48 px du bas, when un delta arrive, then le scroll suit; au-delà de 48 px, la position reste stable et une action Jump to latest apparaît.
- [ ] Given une notification dupliquée ou tardive pour un item terminé, when le reducer la reçoit, then il reste idempotent et ne duplique pas la cellule.
- [ ] Given un item inconnu, when il est projeté, then une cellule `Unsupported Codex item` affiche son type et l'ID sans exposer le JSON brut ni paniquer.

#### US-012: Rendre commandes, outils et modifications de fichiers
**Description:** As a utilisateur Chat, I want voir les actions de Codex séparément so that je peux comprendre ce qui a été exécuté, modifié ou refusé.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given un item command execution, when il progresse, then commande, CWD, environnement, statut, sortie bornée et exit code sont rendus dans une cellule dédiée.
- [ ] Given un item file change, when il progresse, then chemins, type de changement, résumé de diff et statut sont rendus sans appliquer une seconde fois la modification côté Paneflow.
- [ ] Given un tool/MCP/web item stable, when il arrive, then nom, arguments expurgés, progression et résultat sont affichés selon son type; les champs inconnus restent masqués par défaut.
- [ ] Given une sortie supérieure à 2 MiB pour un item, when elle est projetée, then l'affichage est tronqué à 2 MiB avec le nombre d'octets omis et le cache du tab reste ≤ 64 MiB.
- [ ] Given du contenu ANSI, bidi ou zero-width dans une commande, un path ou une sortie, when il est affiché, then les contrôles actifs sont neutralisés et le contenu ne peut pas changer le chrome Paneflow.
- [ ] Given un command/file/tool item failed ou declined, when `item/completed` arrive, then ce résultat devient autoritaire même si un delta antérieur annonçait un succès intermédiaire.
- [ ] Given un chemin hors workspace, when il est affiché, then son chemin absolu reste visible pour décision utilisateur mais n'est jamais ouvert automatiquement.

#### US-013: Gérer approvals, questions et interruption
**Description:** As a utilisateur Chat, I want répondre aux demandes bloquantes de Codex so that les actions sensibles ne continuent qu'après une décision explicite.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010, US-012

**Acceptance Criteria:**
- [ ] Given une command approval, when la requête serveur arrive, then commande, CWD, raison et `availableDecisions` sont affichés inline dans le turn concerné.
- [ ] Given une file-change approval, when elle arrive, then le résumé de changements et les décisions autorisées sont affichés avant toute réponse.
- [ ] Given `requestUserInput`, when la question arrive, then champs et options supportés sont rendus; une réponse invalide est bloquée localement.
- [ ] Given une décision utilisateur, when elle est envoyée, then le responder request ID est consommé une fois, les contrôles sont désactivés et `serverRequest/resolved` clôt l'état pending.
- [ ] Given `item/completed` après approval, when le résultat arrive, then il remplace le statut optimiste et reste la source autoritaire.
- [ ] Given un turn actif, when Stop est cliqué, then `turn/interrupt` est envoyé avec le thread/turn correct et l'UI attend `turn/completed` avant de revenir idle.
- [ ] Given le tab fermé ou le runtime déconnecté avec une approval pending, when la cleanup s'exécute, then la requête est annulée ou réconciliée à la reconnexion; aucune décision `accept` n'est inventée.
- [ ] Given une seconde action sur une approval déjà résolue, when elle est déclenchée, then elle est ignorée avec un état `Already resolved` et aucun second RPC n'est émis.

---

### EP-004: Nouvelle session depuis le menu et restauration

Offrir les actions contextuelles choisies par Arthur sans continuité implicite, puis restaurer les vrais Chat tabs après un restart Paneflow.

**Definition of Done:** Terminal vers Chat et Chat vers CLI créent des sessions neuves transactionnellement, gardent l'identité de tab et l'ancienne session dans l'historique; les Chat tabs survivant à un restart reprennent leur propre thread sans renvoyer de turn.

#### US-014: Démarrer un nouveau Chat depuis un onglet Terminal
**Description:** As a utilisateur dans un terminal, I want démarrer un nouveau Codex Chat au même emplacement so that je change de workflow sans laisser croire que le contexte du terminal est transféré.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006, US-007, US-009, US-010

**Acceptance Criteria:**
- [ ] Given un onglet Terminal dans `AppMode::Cli`, when son menu contextuel s'ouvre, then il propose `Start new Codex Chat`; Markdown, Diff et `AppMode::Agents` ne montrent pas cette action.
- [ ] Given l'action choisie, when la confirmation s'affiche, then elle indique que le terminal et ses processus seront fermés, qu'aucun contexte ne sera transféré et que toute session Codex existante restera dans l'historique.
- [ ] Given Cancel, when l'utilisateur ferme la confirmation, then le Terminal, son PTY, son focus et ses prompts pending restent inchangés.
- [ ] Given Confirm, when le runtime/auth/catalogue preflight réussit, then le PTY est arrêté par son chemin cross-platform existant, le renderer Chat remplace le Terminal dans le même `surface_id`, et `thread/start` crée un nouvel identifiant.
- [ ] Given un terminal Codex avec un session ID connu, when le nouveau Chat démarre, then le nouveau `thread_id` est différent et aucun `thread/resume` n'est émis.
- [ ] Given runtime, login ou catalogue indisponible avant Confirm, when l'action est demandée, then la surface Terminal reste vivante et l'utilisateur reçoit l'action corrective avant toute fermeture.
- [ ] Given un échec après fermeture du PTY mais avant création du thread, when le Chat failed state apparaît, then Retry et `Start new terminal` sont disponibles; Paneflow ne prétend pas restaurer le processus fermé.

#### US-015: Démarrer un nouveau Codex CLI depuis un Chat
**Description:** As a utilisateur dans un Chat, I want démarrer un nouveau TUI Codex dans le même onglet so that je retrouve l'expérience terminal sans reprendre le thread Chat.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009, US-010, US-013

**Acceptance Criteria:**
- [ ] Given un Codex Chat dans `AppMode::Cli`, when son menu contextuel s'ouvre, then il propose `Start new Codex CLI` et n'utilise jamais le mot Switch ou Continue.
- [ ] Given l'action choisie, when la confirmation s'affiche, then elle indique qu'un nouveau TUI sera lancé sans transcript et que le Chat courant restera dans l'historique Codex.
- [ ] Given un turn actif, when l'utilisateur confirme Stop and start, then Paneflow appelle `turn/interrupt`, attend `turn/completed` jusqu'à 5 000 ms, puis remplace la surface; Cancel laisse le turn actif.
- [ ] Given un Chat idle confirmé, when le PTY est créé, then il lance un nouveau `codex` sans argument `resume`, avec le CWD, `CODEX_HOME`, modèle et effort compatibles transmis par arguments structurés ou environnement validé.
- [ ] Given le nouveau CLI lancé, when le remplacement termine, then le `surface_id`, l'emplacement, le nom personnalisé et le focus restent identiques; le binding `thread_id` Chat n'est plus celui de la surface active.
- [ ] Given le binaire CLI absent ou le spawn refusé, when le lancement échoue, then le Chat existant reste monté et aucune interruption n'est envoyée s'il était idle.
- [ ] Given le timeout d'interruption après 5 000 ms, when le turn ne confirme pas son arrêt, then le Chat reste actif et l'utilisateur peut Retry ou Cancel; le PTY n'est pas lancé en parallèle.

#### US-016: Persister et restaurer un Codex Chat
**Description:** As a utilisateur qui redémarre Paneflow, I want retrouver mes Chat tabs so that le layout et les conversations ouvertes survivent sans renvoyer mes prompts.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003, US-009, US-011, US-013

**Acceptance Criteria:**
- [ ] Given un Chat tab sauvegardé, when `session.json` est écrit, then il contient un discriminant de surface, `surface_id`, `thread_id`, runtime identity non secrète, CWD, modèle, effort, nom, draft et position de scroll bornée.
- [ ] Given un restart Paneflow, when le runtime et le compte sont prêts, then le tab appelle `thread/read` puis `thread/resume` si nécessaire et reconstruit le transcript sans envoyer de nouveau turn.
- [ ] Given un transcript plus grand que le cache persisté, when le restore s'exécute, then Codex reste la source de vérité et le cache local est remplacé par la projection lue, dans la limite de 64 MiB par tab.
- [ ] Given un `thread_id` manquant, archivé, supprimé ou illisible, when le restore échoue, then l'onglet reste présent avec `Retry`, `Open Codex history` et `Start new Chat`; aucun thread neuf n'est créé automatiquement.
- [ ] Given une auth expirée, when le restore s'exécute, then le tab affiche Sign in et conserve draft, titre et binding jusqu'au succès ou à une action explicite.
- [ ] Given une session Paneflow antérieure au feature, when elle est chargée, then Terminal, Markdown et Diff retrouvent leur comportement antérieur et les nouveaux champs prennent leurs defaults.
- [ ] Given un Chat remplacé par un CLI avant la sauvegarde, when le restore suivant s'exécute, then seule la surface Terminal active est restaurée; l'ancien `thread_id` Chat n'est pas rattaché silencieusement.

---

### EP-005: Validation produit, sécurité et cross-platform

Prouver que le parcours complet fonctionne dans la matrice Paneflow, que les limites mesurables sont tenues et que les erreurs n'exposent ni secret ni action destructrice implicite.

**Definition of Done:** le runbook reproductible couvre auth, Chat, actions contextuelles, crash/recovery et restore sur les plateformes cibles; les seuils performance, accessibilité et sécurité sont mesurés et documentés.

#### US-017: Exécuter le runbook end-to-end Codex Chat
**Description:** As a mainteneur Paneflow, I want une validation reproductible du parcours complet so that la feature peut être livrée sans dépendre d'une démonstration manuelle unique.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004, US-006, US-007, US-008, US-011, US-012, US-013, US-014, US-015, US-016

**Acceptance Criteria:**
- [ ] Given un compte ChatGPT test, when le runbook browser login, device code, model selection, prompt, command approval, file approval, interrupt et logout est exécuté, then chaque étape produit l'état attendu et une preuve expurgée.
- [ ] Given 20 cycles Terminal vers Chat puis Chat vers CLI, when ils s'exécutent, then chaque Chat utilise un nouveau thread ID, chaque CLI démarre sans `resume`, aucun processus n'est orphelin et aucune ancienne session n'est supprimée.
- [ ] Given 16 Chat tabs chargés et 100 événements/s injectés pendant 60 s, when les probes mesurent le renderer, then p95 event-to-notify < 50 ms, aucune frame terminale n'est perdue et le cache total respecte 64 MiB/tab.
- [ ] Given un App Server tué pendant streaming, approval et idle, when les trois scénarios sont exécutés, then les états Disconnected/Retry apparaissent, aucune approval n'est auto-acceptée et aucun prompt n'est renvoyé.
- [ ] Given Linux Wayland, Linux X11, macOS arm64 et Windows 11 x64, when le smoke est exécuté, then spawn, login callback/device flow, clipboard, browser open, shutdown et restore passent; les chemins macOS Intel/Windows ARM64 non exécutés sont documentés comme compilation vérifiée ou gap.
- [ ] Given navigation clavier uniquement, when le parcours complet est exécuté, then tab strip, menu contextuel, Composer, sélecteurs et approvals sont accessibles; focus visible et contraste texte ≥ 4.5:1 dans les deux thèmes.
- [ ] Given les fixtures contenant token, email, prompt, path et sortie ANSI hostile, when logs, télémétrie et session persistence sont inspectés, then aucun token/prompt/email n'est présent, les paths sont absents de la télémétrie et les contrôles ANSI/bidi sont neutralisés.
- [ ] Given une étape du runbook en échec, when le rapport est produit, then la story reste IN_REVIEW ou BLOCKED avec plateforme, version Codex et étape exacte; aucune exception n'est marquée « acceptable » sans décision tracée.

## Functional Requirements

- FR-01: La surface Codex Chat existe uniquement dans `AppMode::Cli` et ne modifie pas le comportement de `AppMode::Agents` ou `AppMode::Diff`.
- FR-02: Paneflow lance exclusivement l'App Server officiel sur stdio v2 pour le MVP.
- FR-03: Paneflow DOIT envoyer `initialize` puis `initialized` avant toute autre requête App Server.
- FR-04: Paneflow NE DOIT PAS lire, écrire, sérialiser ou journaliser les credentials Codex.
- FR-05: Le login UI DOIT proposer ChatGPT browser et device code, cancellation et logout; aucune saisie d'API key Paneflow n'est incluse.
- FR-06: Les modèles et reasoning efforts DOIVENT venir de `model/list`, dans l'ordre retourné par Codex.
- FR-07: Un nouveau Chat DOIT utiliser `thread/start`; un restore du même Chat PEUT utiliser `thread/read`/`thread/resume`.
- FR-08: `Start new Codex Chat` et `Start new Codex CLI` DOIVENT créer une nouvelle session et NE DOIVENT JAMAIS reprendre la session remplacée.
- FR-09: L'ancienne session Codex NE DOIT PAS être supprimée ou archivée lors d'un remplacement de surface.
- FR-10: Toute fermeture de Terminal ou interruption de Chat liée au remplacement DOIT être précédée d'une confirmation explicite.
- FR-11: Le `surface_id` Paneflow DOIT rester stable lors du remplacement Terminal/Chat.
- FR-12: Le transcript, les items et approvals DOIVENT être dérivés des événements structurés App Server, jamais du parsing PTY/ANSI.
- FR-13: `item/completed`, `turn/completed` et `serverRequest/resolved` DOIVENT être les états autoritaires de fin.
- FR-14: Une approval DOIT accepter au maximum une réponse utilisateur et ne DOIT JAMAIS default sur `accept`.
- FR-15: Un prompt dont le statut après timeout est inconnu NE DOIT PAS être renvoyé automatiquement.
- FR-16: Les sorties et caches DOIVENT être bornés selon les NFR; les dépassements produisent une troncature visible ou une erreur, pas un OOM.
- FR-17: Les erreurs de version, auth, réseau, protocole et subprocess DOIVENT produire un état actionnable sans fallback vers une API privée ou expérimentale.
- FR-18: La persistence Paneflow DOIT rester backward-compatible avec les sessions antérieures.
- FR-19: Les commandes CLI sont construites comme arguments structurés; aucun `thread_id`, path, modèle ou effort non validé n'est interpolé dans une commande shell.
- FR-20: La télémétrie DOIT exclure prompts, réponses, reasoning, commandes, diffs, paths, email et identifiants de compte.

## Non-Functional Requirements

- **Performance:** p95 entre réception d'une notification App Server et `cx.notify()` < 50 ms à 100 événements/s; initialisation d'un runtime déjà authentifié < 2 000 ms p95 hors latence login; 16 Chat tabs chargés sans dépasser 64 MiB de cache projeté par tab; sortie rendue limitée à 2 MiB par item.
- **Security:** 0 credential Codex stocké par Paneflow; frames JSONL limitées à 64 MiB; stderr diagnostic limité à 1 MiB et redacted; 100 % des approvals avec side effects exigent une décision App Server autorisée; aucun HTML brut, ANSI actif, bidi ou zero-width ne modifie le chrome.
- **Accessibility:** contraste texte ≥ 4.5:1; toutes les actions réalisables au clavier; focus visible sur 100 % des contrôles interactifs; aucune information de statut portée uniquement par la couleur; animations désactivées quand `prefers-reduced-motion` est actif.
- **Scalability:** 1 runtime stdio supporte 16 threads Chat chargés et 32 requêtes concurrentes corrélées; event queue bornée à 2 048; cache projeté borné à 64 MiB/tab; aucun buffer global non borné.
- **Reliability:** handshake timeout 5 000 ms; requête catalogue/turn timeout 10 000 ms selon le contrat story; shutdown gracieux 2 000 ms puis kill/reap; 1 restart automatique maximum par crash; 0 retry automatique d'un prompt ou d'une approval.
- **Cross-platform:** 100 % des chemins spawn, browser open, clipboard, shutdown, paths et persistence possèdent une implémentation ou un fallback explicite Linux/macOS/Windows; aucun path POSIX, séparateur ou shell hardcodé.
- **Compatibility:** une version minimale et 3 versions Codex testées sont maintenues à Month 6; les champs inconnus sont tolérés; une méthode obligatoire absente bloque le runtime avant création de thread.

## Edge Cases & Error States

Systematic coverage of unhappy paths. Evidence shows earlier defect discovery significantly reduces cost (Boehm 1981, NIST 2002).

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Codex absent | Action Start new Chat/CLI | La surface courante reste intacte; installation ou path configurable proposés | `Codex CLI was not found` |
| 2 | Version Codex non supportée | Preflight runtime | Aucun App Server utilisé; version courante et plage supportée affichées | `Codex {version} is not supported` |
| 3 | Login browser impossible | OS refuse d'ouvrir l'URL | URL copiable et device code disponible | `Could not open the browser` |
| 4 | Login annulé/expiré | Cancel ou completion error | Retour Signed out, aucune session créée | `Sign-in was cancelled` |
| 5 | Catalogue vide | `model/list` vide/failed | Send bloqué, Retry disponible | `No Codex models are available` |
| 6 | Terminal avec processus actif | Start new Chat | Confirmation avant fermeture; Cancel no-op | `This closes the terminal and starts a new Chat without its context` |
| 7 | Chat avec turn actif | Start new CLI | Interrupt explicite puis attente de completion | `Stop the current turn and start a new Codex CLI?` |
| 8 | Interrupt timeout | Pas de `turn/completed` en 5 s | Chat conservé, aucun CLI parallèle | `Codex did not confirm the stop` |
| 9 | App Server crash | Exit child | Tabs Disconnected, 1 restart max, Retry | `Codex App Server stopped` |
| 10 | Frame JSONL invalide/oversize | Parser | Runtime protocol error, contenu non loggé | `Codex sent an unsupported response` |
| 11 | Approval résolue ailleurs/tardive | Double clic ou reconnect | Contrôle disabled, état Already resolved | `This request is already resolved` |
| 12 | Thread supprimé au restore | `thread/read` not found | Tab conservé, options history/new/retry | `This Codex session is no longer available` |
| 13 | Auth expirée au restore | `account/read` signed out | Draft/binding conservés, Sign in demandé | `Sign in to restore this Chat` |
| 14 | Modèle retiré | Refetch catalogue | Default sélectionné, notification unique | `The previous model is no longer available` |
| 15 | Output très volumineux | Item > 2 MiB/cache > 64 MiB | Troncature visible, pas d'OOM | `{N} bytes omitted` |
| 16 | CWD supprimé | Thread start/restore | Création bloquée ou état recovery, aucun fallback home | `The working directory no longer exists` |
| 17 | Workspace/pane déplacé | Chat tab déplacé | CWD du thread inchangé; nouveau CWD exige nouvelle session | `Start a new session to change working directory` |
| 18 | Paneflow ferme pendant login | Shutdown | Login cancel best-effort, child reaped, aucun secret persisté | Aucun message après fermeture |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | App Server évolue sans version de protocole explicite dans initialize | High | High | US-001 fixe une version minimale, des fixtures et une matrice; runtime bloque avant thread/start si une méthode obligatoire manque; experimentalApi reste false. |
| 2 | Le refactor d'identité de tab casse IPC, prompts pending ou move-to-pane | Med | High | US-009 sépare SurfaceId/EntityId et exige des tests de round-trip, move, close et ancien layout avant les stories de remplacement. |
| 3 | Un child long-lived devient orphelin sur Unix ou Windows | Med | High | Supervisor dédié, Job Object Windows, guard Unix, shutdown 2 s, kill/reap et runbook crash US-017. |
| 4 | Un prompt est renvoyé après timeout et déclenche des actions doubles | Low | High | Aucun retry automatique; état Unknown bloque Send jusqu'à réconciliation thread/read ou reconnect. |
| 5 | Un contenu agent hostile affecte le renderer ou les logs | Med | High | Caps frame/item/cache, redaction, neutralisation ANSI/bidi/zero-width, aucun HTML brut, fixtures hostiles US-017. |
| 6 | Les labels de menu laissent croire à une continuité de contexte | Med | Med | Labels `Start new`, confirmations avec exclusions, assertions de thread ID distinct et interdiction de `resume` dans US-014/015. |
| 7 | Le PRD chevauche le mode Agents existant et réintroduit ACP chat | Low | High | Hard constraint AppMode::Cli; fichiers Agents et crate ACP dans Files NOT to Modify; tests de non-régression. |
| 8 | Un cache Paneflow diverge du transcript Codex | Med | Med | Codex reste autoritaire; cache borné/reconstructible; thread/read/resume remplace la projection au restore. |
| 9 | L'auth ChatGPT ou les modèles ne sont pas disponibles pour certains comptes | Med | Med | États account/provider explicites, catalogue dynamique, aucune supposition de modèle, erreurs actionnables. |
| 10 | 17 stories dérivent vers un framework multi-provider prématuré | Med | Med | Adapter Codex concret; autres providers et protocole universel en Non-Goals; chaque story garde un output Codex-first. |

## Non-Goals

Explicit boundaries - what this version does NOT include:

- Aucune continuité de thread entre CLI et Chat. Le thread ou processus remplacé n'est jamais repris par la nouvelle surface.
- Aucun App Server WebSocket, daemon partagé externe, remote-control ou client multi-connexion.
- Aucun support Claude, Pi, OpenCode, Gemini, ACP ou provider générique dans le Chat natif v1.
- Aucune route privée `chatgpt.com/backend-api`, client OAuth copié de Pi, parsing JWT ou catalogue de modèles observé/hardcodé.
- Aucune API key saisie ou stockée par Paneflow. Les auth modes configurés hors Paneflow peuvent être projetés par App Server.
- Aucun changement à `AppMode::Agents`, au rail Agents, à ses « Chats » terminal-only ou au PRD Agents existant.
- Aucun changement à `AppMode::Diff`, aux vues Diff ou au workflow Review.
- Aucune nouvelle commande standalone `paneflow <verb>` pour piloter le Chat.
- Aucun realtime/audio, raw Responses item, dynamic tools, collaboration/multi-agent, external-token auth, credits ou champ expérimental App Server.
- Aucun bundling ou updater du binaire Codex dans Paneflow v1.
- Aucun navigateur Paneflow complet de tout l'historique Codex. L'ancienne session reste dans l'historique Codex existant.
- Aucune pièce jointe, mention de fichier ou drag-and-drop dans le Composer v1; input texte uniquement.
- Aucun transfert du scrollback TUI, draft, processus shell ou modal entre Terminal et Chat.

## Files NOT to Modify

- `src-app/src/app/agents_view_actions.rs` et `src-app/src/app/agents_sidebar/**` - `AppMode::Agents` est hors scope et terminal-only.
- `src-app/src/project/**` - modèle Project/Thread de la vue Agents, sans rapport avec les tabs Chat de `AppMode::Cli`.
- `src-app/src/diff/**` - `AppMode::Diff` et le renderer Diff sont gelés.
- `src-app/src/cli/**` - binaire scriptable `paneflow <verb>` hors scope; il ne doit pas devenir un host Chat.
- `crates/paneflow-acp/**` - ACP chat a été retiré; ne pas réutiliser ce crate comme adaptateur Codex App Server.
- `crates/paneflow-process/src/lib.rs` - runner one-shot avec stdin null et timeout; préserver son contrat et créer un supervisor long-lived dédié.
- `src-app/src/update/**` et `.github/workflows/release.yml` - update/release signing hors scope.
- `src-app/src/terminal/hera_dogfood/**` et `src-app/src/terminal/element/golden/hera_*.txt` - surfaces Hera sans rapport avec Codex Chat et actuellement sensibles aux changements de rendu.
- `tasks/prd-agents-ui-codex-redesign-2026-Q3.md` et son status JSON - PRD distinct déjà exécuté; ne pas le réécrire ou réouvrir.

## Technical Considerations

Frame as questions for engineering input - not mandates:

- **Architecture:** le protocole/processus doit-il vivre dans un nouveau crate `paneflow-codex`, avec un adapter GPUI dans `src-app`, ou dans un module `src-app/src/codex`? Recommandé: crate sans dépendance GPUI pour process, RPC et types de domaine; engineering doit confirmer le coût d'un nouveau membre workspace.
- **Tab identity:** faut-il introduire un wrapper `PaneTab { surface_id, content }` pour toutes les tabs ou un `CodexSessionView` stable qui possède ses renderers? Recommandé: choisir pendant US-009 l'option qui maintient l'identité publique sans refactor non nécessaire des tabs Markdown/Diff.
- **Runtime key:** quelles composantes déterminent l'isolation du runtime? Recommandé: chemin canonique du binaire, version et `CODEX_HOME` canonique; config overrides susceptibles de changer l'auth doivent être ajoutés à la clé.
- **Protocol types:** faut-il générer des structs depuis le schema Codex supporté ou maintenir un sous-ensemble Rust manuel? Recommandé: fixtures/schema générés pour détection, types Rust MVP explicites et désérialisation tolérante; US-001 doit mesurer le churn.
- **Process lifecycle:** comment étendre le parent-death coverage Unix sans changer le runner one-shot? Recommandé: supervisor dédié avec trait OS et tests child/reap; Windows réutilise le pattern Job Object existant.
- **Transcript projection:** faut-il persister des cellules rendues ou uniquement thread binding/draft/scroll? Recommandé: cache borné optionnel pour first paint, immédiatement réconcilié par thread/read; jamais source de vérité.
- **Replacement transaction:** à quel moment détruire le renderer courant? Recommandé: preflight runtime/auth/catalogue avant confirmation, remplacement atomique après confirmation, failed state récupérable si l'étape irréversible a déjà fermé le PTY.
- **Composer:** faut-il généraliser le Composer actuel avec `PromptTarget` ou créer un Composer interne au Chat? Recommandé: UI partagée si elle n'affaiblit pas la règle terminal prefill-only; dispatch Terminal et Chat doit rester explicite.
- **History discoverability:** `sourceKind=appServer` apparaît-il dans le picker Codex standard de la version minimale? Recommandé: US-001 tranche et la documentation utilisateur donne la commande/chemin exact si `--include-non-interactive` est nécessaire.
- **Migration:** les nouvelles surfaces doivent être backward-compatible avec `session.json`; rollback recommandé: une version ancienne ignore ou conserve les surfaces inconnues sans convertir un Chat en Terminal. Engineering doit confirmer la stratégie avant US-016.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Chat Codex natif utilisable avec abonnement | 0 surface | 20/20 parcours auth-cached -> prompt -> réponse | Month-1 | Runbook US-017 |
| Créations accidentelles avec continuité implicite | N/A (feature absente) | 0/20 cycles; thread ID distinct et CLI sans `resume` | Month-1 | Instrumentation test + inspection commande |
| Tokens/PII dans persistence, logs ou télémétrie | Baseline feature N/A | 0 occurrence dans fixtures adversariales | Avant READY release | Tests redaction + inspection artefacts |
| Latence event App Server vers repaint demandé | N/A | p95 < 50 ms à 100 événements/s | Month-1 | Probe déterministe US-017 |
| Runtime orphelin après fermeture/crash | N/A | 0/30 scénarios Linux/macOS/Windows | Month-1 | Tests process tree + runbook |
| Restore d'un Chat existant sans prompt dupliqué | 0 | 20/20 restores; 0 `turn/start` automatique | Month-1 | Test intégration session restore |
| Compatibilité Codex maintenue | 0 version déclarée | version minimale + 3 versions testées | Month-6 | Matrice docs + CI/validation release |
| Adoption dogfooding | 0 utilisation | Chat utilisé dans ≥ 30 % des sessions Codex d'Arthur pendant 2 semaines | Month-1 + 2 semaines | Compteur local opt-in sans contenu |
| Plateformes validées | Linux principal seulement | Linux Wayland/X11, macOS arm64, Windows 11 x64 | Avant release | Runbook signé par plateforme |

## Open Questions

- **Version minimale Codex:** owner engineering, résolue par US-001 avant US-002. Bloque le schema, les fixtures et le message unsupported.
- **Découvrabilité des threads App Server dans l'historique Codex standard:** owner engineering, résolue par US-001. Bloque le texte exact de confirmation et l'action `Open Codex history`, pas la création du Chat.
- **Forme de l'identité stable de tab:** owner engineering, résolue au début de US-009 par un mini-ADR comparant wrapper global et `CodexSessionView`. Bloque US-014/015/016.
- **Version ancienne de Paneflow ouvrant un `session.json` contenant un Chat:** owner engineering, résolue avant US-016. Le minimum acceptable est une erreur non destructive, jamais une conversion silencieuse en Terminal.
[/PRD]
