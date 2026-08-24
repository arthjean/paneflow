[PRD]
# PRD: Chat in-app pour l'interface Agents, alimenté par le transcript de l'agent - 2026-Q3

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-08-17 | Arthur Jean | PRD initial: le bouton "New Thread" ouvre une vue chat alimentée par le tail-follow du transcript de session du CLI, terminal conservé en coexistence |

## Problem Statement

1. Le bouton "New thread" d'un projet ne produit aucune conversation lisible. `create_agents_thread_in` (`src-app/src/app/agents_sidebar/affordances.rs:360`) ne fait que vider `agents_target` et armer un picker; le clic sur une tuile monte un `TerminalView` plein cadre (`agents_view_actions.rs:1101-1179`). L'utilisateur lit donc une TUI en alternate screen, avec spinners, redraws et scrollback plafonné à 4000 lignes, là où il attend un fil de conversation.
2. Le contenu existe déjà sur le disque et Paneflow ne l'exploite pas. Les CLIs écrivent un transcript complet et structuré (Claude Code un bloc de contenu par ligne, Codex un item complet par ligne avec `ordinal` monotone, pi un message complet par ligne). Paneflow lit déjà ces fichiers, mais uniquement pour des métadonnées: `claude_sessions.rs`, `codex_sessions.rs`, `pi_sessions.rs` en extraient un `SessionMeta` (`agent_sessions.rs:705-735`) et `extract_last_result_from_transcript` (`app/ipc_handler.rs:225-330`) n'y prend que le dernier message assistant.
3. La coquille visuelle du chat est déjà construite et sert à afficher un terminal. `render_terminal_thread_surface` (`agents_view_actions.rs:1412-1445`) pose déjà un fond `ui.base`, une bande de toolbar réservée, une colonne centrée plafonnée à `agent_panel.max_content_width` (760) et un overlay d'environnement cwd/branche/diffstat. La configuration du chat existe aussi: `agent_panel.thinking_display` (`crates/paneflow-config/src/schema/agent_panel.rs:25`) décrit au pixel le rendu des rafales de thinking, sans aucun consommateur qui rende ces rafales.
4. Le composer du chat existe et sert à renommer des lignes de sidebar. `widgets/text_area.rs:205` a été écrit comme Composer (US-016 de `prd-agents-view`): multi-ligne, soft-wrap avec hit testing, `on_submit` sur Enter, Shift+Enter, `Ctrl+Shift+Enter` pour la mise en file pendant un tour en vol. Son seul point d'usage aujourd'hui est le champ de renommage inline (`main.rs:931`).
5. Les tentatives précédentes ont échoué par le mauvais bout. Le chat ACP in-app a été supprimé en juin 2026 (crate `paneflow-acp` absente du disque et des membres du workspace, store sqlite de threads supprimé dont `Thread.store_id` reste le fossile) parce qu'il transformait l'agent en boîte noire headless. Rien dans le repo ne permet aujourd'hui de reconstruire une conversation, et la reconstruire depuis la grille PTY est exclu: `extract_scrollback` (`terminal/pty_session.rs:3653`) ne rend que du texte plat sans notion de rôle.

**Why now:** la mesure a levé le seul inconnu qui bloquait la décision. Sur la session courante, les timestamps à `requestId` constant sont échelonnés (`thinking` 20:54:56.050 puis `text` 20:55:31.976 pour la même requête), et Codex expose un `ordinal` monotone avec des timestamps étalés à l'intérieur d'un même `turn_id`. L'écriture est donc incrémentale pendant le tour, ce qui rend un chat qui coule atteignable sans réintroduire le headless. Par ailleurs Paneflow forge lui-même l'UUID de session Claude à la création du thread (`project/mod.rs:295-323`, `--session-id` splicé en premier dans `agent_launcher.rs:349-361`): le lien thread vers transcript est déjà établi par construction.

## Overview

La solution traite le transcript de l'agent comme la source de vérité de la conversation, et le PTY comme la source de vérité de l'exécution. Un lecteur par agent normalise son format en une suite de blocs Paneflow, un tail incrémental suit le fichier avec un curseur, et une nouvelle surface `ChatView` remplace la branche terminal de `render_agents_main` (`agents_view_actions.rs:266-306`) en héritant sans modification de la toolbar, de la colonne centrée, de l'overlay d'environnement, du dock diff et du dock terminal.

Le sens de l'écriture ne change pas: le composer du chat écrit dans le vrai CLI via `TerminalView::inject_text` en bracketed paste, avec le `\r` de soumission en geste explicite séparé (`terminal/input.rs:1399`, `:2911`). Aucun agent n'est piloté autrement que par son PTY, aucune requête modèle n'est émise par Paneflow, aucun sandbox d'agent n'est désarmé par cette fonctionnalité. La règle human-in-loop et la décision de juin restent donc intactes: ce PRD ne rouvre pas ACP.

Le périmètre V1 est délibérément aligné sur ce qui est prouvé et non sur ce qui est plausible. Trois agents ont un transcript en fichier avec append incrémental démontré: Claude Code, Codex, pi. OpenCode stocke ses parts dans `opencode.db` en SQLite avec mise à jour en place, ce qui exige un polling de base et non un tail de fichier: reporté. Gemini CLI, Cursor et Hermes n'ont aucune donnée sur la machine de référence et Grok n'offre qu'un échantillon d'une session: hors scope, sans inférence de format. Les treize agents restants conservent exactement le comportement actuel, le terminal plein cadre, qui reste aussi le fallback des trois agients supportés quand leur transcript est absent, illisible ou d'un format non reconnu.

La sensation visuelle est portée depuis zeron (`/home/arthur/dev/comet`), dont l'ergonomie de chat est la référence retenue: rows à granularité de bloc avec ids stables et cache par empreinte de contenu (`crates/ui/src/transcript.rs:1-25`), veil de fade paint-only qui ne peut pas provoquer de reflow (`crates/ui/src/markdown/veil.rs`), stick-to-bottom à ressort avec rupture sur input utilisateur (`transcript.rs:146`) et catalogue de motion explicite (`crates/ui/src/motion.rs:4-13`). Ce portage est un choix d'ergonomie, pas une dépendance: aucun code de zeron n'est importé, seules les techniques sont reproduites.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Rendre une conversation lisible sans TUI | Chat disponible pour Claude Code, Codex et pi, avec bascule vers le terminal en un geste | Chat disponible aussi pour OpenCode et Grok |
| Afficher un tour pendant qu'il se déroule | Latence p95 entre l'écriture d'un bloc sur disque et son affichage < 300 ms | < 150 ms |
| Ne jamais régresser sur le terminal | 0 changement de comportement pour les 13 agents non couverts, 0 modification du backend PTY | Idem, y compris après extension du scope |
| Dégrader sans casser | 100 % des transcripts absents, tronqués ou non reconnus retombent sur le terminal sans crash ni écran vide | Idem, plus bandeau de diagnostic actionnable |
| Tenir la mémoire | RSS additionnel < 40 Mo pour un chat de 500 blocs ouvert, plat après fermeture | < 25 Mo, budget de cache borné et vérifié en CI |

## Target Users

### Développeur pilotant plusieurs agents en parallèle

- **Role:** utilisateur principal de Paneflow, ouvre plusieurs threads d'agents sur un ou plusieurs projets, sous Linux, macOS ou Windows.
- **Behaviors:** lance Claude Code ou Codex sur un projet, part faire autre chose, revient lire ce que l'agent a fait, relit un tour ancien pour comprendre une décision, réagit par un nouveau prompt.
- **Pain points:** la TUI efface son propre historique, le scrollback est plafonné, les spinners et redraws rendent la relecture pénible, et rien ne distingue visuellement un raisonnement d'un appel d'outil ou d'une réponse.
- **Current workaround:** scroller dans le terminal, ou ouvrir le fichier JSONL à la main dans un éditeur.
- **Success looks like:** un fil de conversation lisible qui se remplit pendant que l'agent travaille, avec les appels d'outils repliés, et le terminal toujours accessible en un geste quand il faut voir la TUI réelle.

### Développeur qui revient sur une session ancienne

- **Role:** même personne, quelques heures ou quelques jours plus tard, souvent après un `--resume`.
- **Behaviors:** rouvre un thread existant, cherche le passage où une décision a été prise, vérifie quels fichiers ont été touchés.
- **Pain points:** le terminal d'un thread relancé ne contient plus rien de l'historique, alors que le transcript sur disque le contient intégralement.
- **Current workaround:** relire le JSONL manuellement, ou redemander à l'agent ce qu'il a fait.
- **Success looks like:** l'ouverture du thread affiche la conversation complète, même si le processus a été tué entre-temps, sans avoir à relancer l'agent.

## Research Findings

Key findings that informed this PRD:

### Competitive Context

- **Zed** (`/home/arthur/dev/zed`, HEAD 2026-08-17): unifie agent natif et agents externes derrière `acp_thread::AgentConnection`, persiste chaque thread en blob JSON zstd dans une table SQLite (`crates/agent/src/db.rs:438-545`), rend le transcript via `ListState` avec une entité `Markdown` long-lived par bloc, et reparse le document markdown entier à chaque chunk sur un thread de fond (`crates/markdown/src/markdown.rs:957`, `:1169`). Sa surface de review est un multibuffer avec Keep/Reject par hunk. Nous différons sur deux points: Paneflow ne possède pas les buffers et ne peut pas offrir cette review, et Paneflow ne parle pas ACP aux agents mais lit leur transcript.
- **zeron** (`/home/arthur/dev/comet`): rows par bloc markdown avec ids stables `{msgId}#{partId}.{blockIx}`, cache par empreinte de contenu, splice minimal par `(id, version)`, veil de fade paint-only, stick-to-bottom à ressort. C'est l'ergonomie de référence de ce PRD. zeron n'a en revanche aucun terminal réel à offrir en fallback, là où Paneflow en a un.
- **Anthropic et OpenAI:** aucun des deux n'expose ACP en first-party. Anthropic pousse l'Agent SDK avec un `SessionStore` officiel, OpenAI le `codex app-server` en JSON-RPC. Les deux écrivent malgré tout un transcript local append-only, ce qui est précisément le contrat de fait exploité ici.
- **Market gap:** aucun produit lu pendant cette recherche ne construit une UI de chat au-dessus du transcript d'un CLI tiers en laissant le CLI piloter. Les concurrents choisissent soit d'être l'agent, soit de relayer une session terminal.

### Best Practices Applied

- Parsing tolérant et additif: ignorer les champs inconnus, sauter une entrée malformée sans faire tomber la vue. C'est la philosophie malformed-entry de zeron et la stratégie `serde(default)` de Zed, tous deux confrontés à des schémas mouvants.
- Traiter la sortie d'agent comme non fiable: le bridge MCP de Paneflow fence déjà le texte lu comme untrusted (`crates/paneflow-mcp/src/tools.rs:117`). Le chat applique la même règle et ne dérive aucune action d'un contenu de transcript.
- Curseur persistant plutôt que relecture: Codex fournit `ordinal`, les autres un offset de fichier. Zeron a payé cher la divergence entre contenu et curseur (`docs/chat2-sync.md` C2), d'où la règle de ne jamais avancer un curseur avant d'avoir intégré le contenu correspondant.
- Lecture hors du render thread: tous les lecteurs de session existants passent par `smol::unblock`, et une passe antérieure a déjà causé un blocage de la boucle GPUI par une lecture synchrone. Le tail hérite de cette contrainte.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)

- L'append incrémental par bloc pendant un tour est vérifié pour Claude Code, Codex et pi sur la machine de référence, sur des sessions réelles. Il n'est pas garanti par un contrat public: c'est un comportement observé, à re-vérifier à chaque montée de version des CLIs.
- Les hooks `ai.*` suffisent comme déclencheurs de relecture, sans polling permanent, parce qu'ils marquent début de tour, appels d'outils et fin de tour. La latence effective reste à mesurer et conditionne l'ajout d'un poll de mtime borné pendant qu'un tour est actif.
- La granularité d'un bloc est un grain de rendu acceptable. Un paragraphe long apparaîtra d'un coup, sans progression intra-bloc, ce qui est compensé cosmétiquement par le veil et non par du vrai streaming de tokens.
- Le format de transcript de Gemini CLI, Cursor, Grok et Hermes reste inconnu: aucune donnée exploitable n'existe sur la machine de référence, et aucune inférence n'est faite.

### Hard Constraints

- Aucun agent n'est piloté autrement que par son PTY. Pas de client ACP, pas de requête modèle émise par Paneflow, pas de mode headless.
- Le contrat des hooks ne change pas. `UserPromptSubmit` droppe volontairement le texte du prompt (`crates/paneflow-ai-hook/src/event.rs:203`, verrouillé par un test) et cette décision de confidentialité est maintenue: le contenu ne doit jamais transiter par les hooks.
- Le backend terminal reste intouché. Ni `terminal/pty_session.rs`, ni `ghostty_session.rs`, ni les moteurs VT ne sont modifiés par ce PRD.
- Compatibilité Linux, macOS et Windows obligatoire, y compris la résolution des chemins de transcript, qui doit passer par `dirs` et `PathBuf` sans séparateur ni chemin POSIX codé en dur.
- Le contenu du transcript est untrusted: jamais exécuté, jamais interprété comme commande, aucun HTML brut rendu, aucune ressource distante chargée depuis un contenu de message.
- Aucune persistance propre des messages. Pas de résurrection du store sqlite de threads; `Thread.store_id` reste un champ de compatibilité de session.json.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate de release sur les quatre legs de la matrice
- `cargo clippy --workspace -- -D warnings` - aucun warning toléré
- `cargo test --workspace` - suite complète de l'espace de travail
- `cargo build` - compilation du binaire principal

Pour les stories UI, gates additionnels:
- Vérification visuelle GUI par l'auteur sur au moins un agent réel (Claude Code) avec un tour complet contenant thinking, appel d'outil et réponse
- Vérification du chemin de dégradation: thread dont le transcript est absent, doit afficher le terminal sans écran vide

## Epics & User Stories

### EP-001: Socle transcript

Construire le pipeline de données: un modèle de blocs normalisé, un lecteur par agent supporté, un tail incrémental à curseur, et la résolution du transcript d'un thread. Aucune UI dans cet épic.

**Definition of Done:** pour un thread Claude Code, Codex ou pi, le moteur produit une suite de blocs normalisés complète et se met à jour pendant un tour, prouvé par des tests sur fixtures et par une trace de latence mesurée, sans aucune lecture sur le render thread.

#### US-001: Modèle de blocs normalisé
**Description:** As a développeur de Paneflow, I want un modèle de conversation neutre indépendant du format de chaque agent so that l'UI et les lecteurs évoluent séparément.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given les trois formats supportés, when un lecteur les normalise, then chaque entrée produite porte un id stable, un rôle (user, assistant, tool, system), un type de bloc (text, thinking, tool_call, tool_result, error, attachment), un timestamp et un identifiant de tour
- [ ] Given un bloc d'appel d'outil et son résultat émis sur deux lignes distinctes, when le modèle les représente, then le résultat est rattachable à son appel par un identifiant sans dépendre de l'ordre d'arrivée
- [ ] Given un champ inconnu dans le format source, when le lecteur le rencontre, then il est ignoré sans erreur et le bloc reste exploitable
- [ ] Given un type de bloc non reconnu, when le modèle le reçoit, then il est conservé comme bloc opaque affichable en repli plutôt que supprimé
- [ ] Le modèle est couvert par des tests unitaires purs, sans I/O

#### US-002: Lecteur Claude Code complet
**Description:** As a utilisateur de Claude Code, I want que Paneflow lise l'intégralité de mon transcript so that ma conversation soit affichable en entier.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un fichier de session Claude Code, when le lecteur le parse, then chaque ligne assistant produit exactement un bloc du type correspondant (thinking, text ou tool_use) et les lignes user, system et attachment sont classées correctement
- [ ] Given des lignes partageant un même `requestId`, when le lecteur les groupe, then elles appartiennent au même tour, ce qui permet le repli par tour côté UI
- [ ] Given une ligne portant `isSidechain`, when elle est lue, then le bloc est marqué comme provenant d'un sous-agent et distinguable du fil principal
- [ ] Given les champs `uuid` et `parentUuid`, when le lecteur construit la suite, then le chaînage est exposé et l'ordre reste déterministe même si deux lignes ont le même timestamp
- [ ] Given une ligne JSON invalide ou tronquée au milieu du fichier, when le lecteur la rencontre, then elle est comptée et sautée, et le parsing continue jusqu'à la fin
- [ ] Fixture figée en `tests/` avec la version de CLI observée, sans aucun contenu de conversation réel

#### US-003: Lecteur Codex avec curseur ordinal et déduplication
**Description:** As a utilisateur de Codex, I want que Paneflow lise mes rollouts so that mes tours Codex s'affichent comme ceux de Claude Code.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un fichier rollout, when le lecteur le parse, then les lignes `response_item` produisent les blocs (message avec ses blocs de contenu, reasoning, function_call, function_call_output, custom_tool_call et son output) et la ligne `session_meta` fournit l'en-tête de session
- [ ] Given que le même contenu apparaît en `response_item` et en `event_msg/item_completed`, when le lecteur produit les blocs, then un seul canal est retenu et aucun bloc n'est affiché deux fois
- [ ] Given un `call_id`, when un `function_call_output` arrive, then il est rattaché à son appel correspondant
- [ ] Given le champ `ordinal`, when le lecteur reprend une lecture, then il reprend au-delà du dernier ordinal intégré et ne relit pas le début du fichier
- [ ] Given un burst de replay (fichier de plusieurs milliers de lignes pour peu de timestamps distincts, observé sur resume et fork), when le lecteur l'ingère, then il traite par lots bornés et ne bloque jamais le render thread
- [ ] Given un fichier dont l'en-tête `session_meta` est absent, when le lecteur le lit, then il produit quand même les blocs disponibles et signale l'en-tête manquant

#### US-004: Lecteur pi
**Description:** As a utilisateur de pi, I want que mes sessions pi s'affichent en chat so that le chat ne soit pas réservé à deux agents.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given un fichier de session pi, when le lecteur le parse, then les lignes `message` produisent les blocs de leur tableau `content` (text, thinking, toolCall) et les lignes `toolResult` sont rattachées par `toolCallId`
- [ ] Given les lignes `session`, `model_change` et `thinking_level_change`, when elles sont lues, then elles alimentent l'en-tête de session sans produire de bloc de conversation
- [ ] Given `parentId`, when le lecteur ordonne les entrées, then l'ordre reste déterministe
- [ ] Given un message dont le tableau `content` est vide, when il est lu, then aucun bloc vide n'est produit et le tour reste cohérent

#### US-005: Tail incrémental à curseur, hors render thread
**Description:** As a utilisateur, I want voir la conversation se remplir pendant que l'agent travaille so that je n'aie pas à attendre la fin du tour.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-002, US-003

**Acceptance Criteria:**
- [ ] Given un thread actif, when l'agent écrit un nouveau bloc dans son transcript, then ce bloc apparaît dans le modèle sans relecture complète du fichier, en repartant du curseur persistant en mémoire
- [ ] Given les hooks `ai.*` (prompt_submit, tool_use, stop), when ils arrivent, then ils déclenchent une relecture incrémentale, et la latence p95 entre écriture disque et disponibilité du bloc est mesurée et consignée
- [ ] Given un statut de thread `Thinking` sans hook reçu depuis un délai borné, when le tour est en cours, then un poll de mtime borné complète les hooks, et il s'arrête dès le retour à `Idle`
- [ ] Given toute lecture de fichier, when elle est effectuée, then elle passe par un exécuteur hors du thread principal et aucune I/O synchrone n'est faite dans un chemin de rendu
- [ ] Given une ligne encore en cours d'écriture (JSON incomplet en fin de fichier), when le tail la rencontre, then elle est ignorée et le curseur n'avance pas au-delà, jusqu'à ce qu'elle soit complète
- [ ] Given trois échecs consécutifs de lecture, when ils se produisent, then le tail s'arrête proprement, l'état déjà affiché est conservé et un diagnostic est disponible
- [ ] Le curseur n'avance jamais avant l'intégration effective du contenu correspondant

#### US-006: Résolution du transcript d'un thread, y compris après resume
**Description:** As a utilisateur, I want que Paneflow trouve le bon transcript pour le thread ouvert so that je ne voie jamais la conversation d'une autre session.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002, US-003, US-004

**Acceptance Criteria:**
- [ ] Given un thread Claude Code créé par Paneflow, when la résolution s'exécute, then elle utilise l'UUID de session déjà forgé à la création du thread et le slug de cwd, sans deviner
- [ ] Given un thread Codex ou pi, when la résolution s'exécute, then elle sélectionne la session dont le cwd correspond et dont l'horodatage est le plus proche du démarrage du thread, et refuse de choisir en cas d'ambiguïté plutôt que de se tromper
- [ ] Given un thread relancé avec `--resume` ou `resume`, when l'utilisateur ouvre le chat, then la conversation affichée est celle de la session effectivement reprise
- [ ] Given aucun transcript résolvable, when le chat est demandé, then la surface retombe sur le terminal et l'état est explicitement "transcript introuvable", jamais un fil vide silencieux
- [ ] Given des chemins de données sur les trois plateformes, when la résolution compose un chemin, then elle passe par `dirs` et `PathBuf` sans séparateur codé en dur

---

### EP-002: Surface chat

Remplacer la branche terminal du panneau Agents par une vue de conversation virtualisée, en conservant intacte la chrome existante et en gardant le terminal accessible.

**Definition of Done:** un thread supporté affiche sa conversation complète en rows par bloc, avec appels d'outils repliables et thinking conforme au réglage existant, et l'utilisateur peut basculer vers le terminal et revenir sans perdre d'état.

#### US-007: ChatView et intégration dans le panneau Agents
**Description:** As a utilisateur, I want que la sélection d'un thread affiche un chat so that l'interface Agents ressemble à une conversation et non à un terminal.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given un thread supporté sélectionné, when le panneau Agents se rend, then la branche de contenu affiche `ChatView` au lieu du `TerminalView`, sans modifier la bande de toolbar, l'overlay d'environnement, le dock diff ni le dock terminal
- [ ] Given la configuration `agent_panel.max_content_width`, when le chat se rend, then la colonne de conversation respecte cette largeur maximale et reste centrée
- [ ] Given un thread d'un agent non supporté, when il est sélectionné, then le terminal plein cadre actuel est affiché exactement comme aujourd'hui
- [ ] Given un thread sans aucun bloc encore écrit, when le chat s'affiche, then un état d'attente explicite est rendu et l'utilisateur peut passer au terminal
- [ ] Given le cache de vues existant, when l'utilisateur change de thread puis revient, then la position de lecture du chat est conservée

#### US-008: Rows par bloc, ids stables et virtualisation
**Description:** As a utilisateur avec de longues conversations, I want un défilement fluide so that relire un fil de plusieurs centaines de blocs reste agréable.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given une conversation de 500 blocs, when elle est affichée, then seules les rows visibles et une marge de surdessin sont construites, via une liste virtualisée
- [ ] Given un bloc dont le contenu n'a pas changé, when la vue se rafraîchit, then sa row est réutilisée depuis un cache indexé par empreinte de contenu et n'est pas reconstruite
- [ ] Given l'arrivée d'un nouveau bloc pendant un tour, when la vue se met à jour, then seules les rows affectées changent et l'identité des rows existantes est préservée
- [ ] Given une row dont la hauteur change au-dessus du viewport, when elle est mesurée, then la position de lecture est compensée et le contenu lu ne saute pas
- [ ] Given une conversation de 500 blocs ouverte puis fermée, when la mémoire est mesurée, then le surcoût RSS reste sous 40 Mo et redescend à la fermeture

#### US-009: Rendu markdown des blocs de texte
**Description:** As a utilisateur, I want lire du markdown mis en forme so that les réponses soient aussi lisibles que dans un client de chat.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given un bloc de texte markdown, when il est rendu, then titres, listes, emphase, code inline et blocs de code sont mis en forme, avec coloration syntaxique appliquée en peinture et sans influer sur la hauteur calculée
- [ ] Given le choix d'implémentation retenu (entité markdown du fork Zed déjà compilée, ou constructeur depuis une chaîne sur le viewer interne), when la décision est prise, then elle est consignée dans le PRD et un seul chemin de rendu markdown existe pour le chat
- [ ] Given un bloc contenant du HTML brut ou une ressource distante, when il est rendu, then aucun HTML n'est interprété et aucune requête réseau n'est émise
- [ ] Given un bloc de code très long, when il est rendu, then il défile dans son propre conteneur et la page ne défile jamais horizontalement
- [ ] Given un markdown malformé, when il est rendu, then le texte reste lisible en repli et aucun panic ne se produit

#### US-010: Appels d'outils groupés et repliables
**Description:** As a utilisateur, I want que les appels d'outils soient repliés par défaut so that je lise la conversation et non les mécanismes.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given des appels d'outils consécutifs dans un même tour, when ils sont rendus, then ils forment un groupe replié affichant le nombre d'appels et les noms d'outils
- [ ] Given un groupe déplié, when un appel a un résultat, then l'appel et son résultat sont affichés appariés, avec la sortie plafonnée en hauteur et défilable
- [ ] Given un appel d'outil en erreur, when il est rendu, then il est visuellement distinct et son message d'erreur est lisible sans dépliage supplémentaire
- [ ] Given un appel sans résultat encore écrit, when il est rendu, then son état en cours est visible et ne bloque pas le rendu du reste du tour
- [ ] Given un bloc marqué comme venant d'un sous-agent, when il est rendu, then il est visuellement rattaché à son parent et repliable indépendamment

#### US-011: Thinking conforme à ThinkingDisplayMode
**Description:** As a utilisateur, I want contrôler l'affichage du raisonnement so that je choisisse entre suivre le détail et garder un fil propre.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given `agent_panel.thinking_display` en mode par défaut, when une rafale de thinking arrive, then la rafale en cours est dépliée et les rafales précédentes se replient à l'arrivée du chunk suivant, conformément à la documentation du réglage
- [ ] Given le mode aperçu, when une rafale est rendue, then elle affiche un en-tête et un corps de hauteur plafonnée avec le dégradé décrit par le réglage
- [ ] Given un changement de ce réglage à chaud, when la configuration est rechargée, then le chat applique le nouveau mode sans redémarrage
- [ ] Given un agent qui n'émet aucun bloc de raisonnement, when le chat se rend, then aucun espace vide ni en-tête orphelin n'est affiché
- [ ] Aucun nouveau réglage de configuration n'est introduit pour le thinking

#### US-012: Bascule chat et terminal
**Description:** As a utilisateur, I want passer du chat au terminal réel en un geste so that je garde l'accès à la TUI quand j'en ai besoin.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given un thread supporté, when l'utilisateur déclenche la bascule, then la surface passe du chat au terminal et inversement, sans tuer le processus ni perdre le scrollback
- [ ] Given une bascule effectuée, when l'utilisateur quitte puis revient au thread, then la vue choisie pour ce thread est retrouvée
- [ ] Given un thread affichant le terminal parce que son transcript est introuvable, when le transcript devient disponible, then la bascule vers le chat devient possible sans redémarrer le thread
- [ ] Given un agent non supporté, when l'utilisateur cherche la bascule, then aucune affordance de chat n'est proposée et le motif est indiqué

---

### EP-003: Composer et envoi

Rendre la conversation bidirectionnelle en écrivant dans le CLI, sans jamais contourner le PTY ni soumettre à l'insu de l'utilisateur.

**Definition of Done:** l'utilisateur écrit un prompt depuis le chat, il arrive pré-rempli dans le CLI, la soumission reste un geste explicite, et l'état du bouton reflète l'état réel du tour.

#### US-013: Composer du chat câblé sur l'injection PTY
**Description:** As a utilisateur, I want écrire mon prompt dans le chat so that je n'aie pas à viser le terminal.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given un prompt multi-ligne saisi dans le composer du chat, when l'utilisateur le délivre, then il arrive dans l'input du CLI en bracketed paste avec les retours à la ligne littéraux, et n'est pas soumis
- [ ] Given le geste explicite de soumission documenté, when il est déclenché, then le retour chariot est envoyé séparément et le tour démarre
- [ ] Given un thread dont le PTY n'est pas encore promu, when un prompt est délivré, then il est mis en tampon et délivré à la promotion, sans perte
- [ ] Given un thread dont le processus est mort, when un prompt est délivré, then l'échec est visible et le texte reste récupérable dans le composer
- [ ] Le composer réutilise le widget de saisie multi-ligne existant plutôt qu'une nouvelle implémentation

#### US-014: Morph du bouton et mise en file pendant un tour
**Description:** As a utilisateur, I want que le bouton reflète ce que je peux faire maintenant so that je n'envoie pas un prompte au mauvais moment.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given le statut du thread, when il vaut au repos, en génération ou en attente d'entrée, then le bouton affiche respectivement l'action d'envoi, l'action d'interruption et l'action de réponse, et change avec le statut sans rechargement
- [ ] Given un tour en cours, when l'utilisateur délivre un prompt, then il est mis en file selon la politique latest-wins existante et délivré à la sortie de l'état de génération
- [ ] Given un prompt en file, when l'utilisateur le remplace, then seul le dernier est délivré et l'utilisateur voit qu'un prompt est en attente
- [ ] Given un thread en échec, when le composer est rendu, then l'action proposée n'est pas l'envoi silencieux et l'état d'échec est explicite

#### US-015: Contexte replié en texte
**Description:** As a utilisateur, I want joindre un fichier ou un hunk de diff à mon prompt so that l'agent ait le contexte sans que je le recopie.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given une référence de contexte ajoutée au composer, when le prompt est délivré, then le contexte est replié en texte brut dans le prompt, puisque le PTY ne transporte que du texte
- [ ] Given un prompt envoyé contenant un bloc de contexte, when il est réaffiché dans le chat, then le bloc est re-extrait et rendu comme une pastille compacte plutôt que comme du texte brut
- [ ] Given un contexte trop volumineux, when il est ajouté, then il est plafonné ou refusé avec un message explicite, sans tronquer silencieusement
- [ ] Aucun second modèle de données n'est introduit pour le contexte: le texte du prompt reste la seule source

---

### EP-004: Sensation visuelle

Porter les techniques de rendu qui rendent une conversation agréable à lire pendant qu'elle s'écrit, sans jamais faire dépendre le layout d'une couleur ou d'une animation.

**Definition of Done:** l'arrivée d'un bloc est visuellement douce, le fil suit la fin sans arracher le contrôle à l'utilisateur, et le réglage de mouvement réduit est respecté.

#### US-016: Veil de fade à l'arrivée d'un bloc
**Description:** As a utilisateur, I want que le texte apparaisse en douceur so that le remplissage du fil ne soit pas une succession de sauts.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given un bloc nouvellement arrivé, when il est rendu, then son opacité progresse jusqu'à l'opacité normale sur une durée bornée, et l'effet ne s'applique qu'une fois par bloc
- [ ] Given l'effet appliqué, when le layout est mesuré, then aucune position ni hauteur ne dépend de l'animation, l'effet est purement de peinture
- [ ] Given plusieurs blocs arrivant rapidement, when ils sont rendus, then plusieurs fades peuvent coexister sans que le texte déjà stabilisé ne rejoue son animation
- [ ] Given le mouvement réduit activé, when un bloc arrive, then il apparaît directement à son état final sans animation planifiée

#### US-017: Suivi de fin de fil à ressort
**Description:** As a utilisateur, I want que le fil suive la génération sans me voler le défilement so that je puisse remonter lire sans lutter.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given le fil accroché à la fin, when du contenu arrive, then le viewport glisse vers la fin de manière continue plutôt que par sauts à chaque mise à jour
- [ ] Given un défilement initié par l'utilisateur vers le haut, when il se produit, then l'accroche se rompt immédiatement et le viewport ne bouge plus tout seul
- [ ] Given l'utilisateur revenu près de la fin, when il entre dans une bande de tolérance, then l'accroche se rétablit
- [ ] Given l'envoi d'un prompt par l'utilisateur, when il est délivré, then l'accroche se rétablit et le fil défile vers la fin
- [ ] Given un contenu qui grandit au-dessus du viewport, when l'utilisateur est détaché, then sa position de lecture reste stable

#### US-018: Entrées animées et mouvement réduit
**Description:** As a utilisateur, I want une interface qui bouge de façon cohérente so that l'ensemble paraisse fini.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given l'apparition d'un groupe d'outils, d'une pastille ou d'un état d'attente, when il entre, then il utilise une entrée unique et cohérente définie une seule fois, pas des durées ad hoc par site d'appel
- [ ] Given le mouvement réduit activé, when une entrée se produit, then aucune image d'animation n'est planifiée
- [ ] Given une animation en cours, when le layout est mesuré, then aucune dimension de layout n'en dépend

---

### EP-005: Robustesse et bords

Garantir que le chat dégrade proprement et ne devient jamais un piège: pas d'écran vide, pas de fuite mémoire, pas de crash sur un format inattendu.

**Definition of Done:** chaque scénario du tableau des cas limites est couvert par un comportement explicite et vérifié, et les budgets mémoire sont bornés et mesurés.

#### US-019: Dégradation explicite sur transcript inutilisable
**Description:** As a utilisateur, I want comprendre pourquoi je n'ai pas de chat so that je ne croie pas que l'application est cassée.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] Given un transcript absent, supprimé pendant la session, ou d'un format non reconnu, when le chat est demandé, then le terminal est affiché avec un motif lisible et actionnable, jamais un fil vide
- [ ] Given un transcript dont la version de format a changé et dont le parsing échoue majoritairement, when il est lu, then le chat le signale comme format non reconnu au lieu d'afficher une conversation partielle trompeuse
- [ ] Given un processus d'agent tué sans événement d'arrêt, when le balayage de PID périmés le détecte, then le tail s'arrête et le dernier état reste affiché
- [ ] Given une bascule pendant une dégradation, when l'utilisateur la déclenche, then aucun panic ni état incohérent n'est produit

#### US-020: Budgets bornés et gros transcripts
**Description:** As a utilisateur avec des milliers de sessions, I want que l'ouverture d'un chat reste rapide et bornée so that l'application ne gonfle pas avec l'usage.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-008, US-005

**Acceptance Criteria:**
- [ ] Given un transcript dépassant le plafond de taille retenu, when le chat s'ouvre, then seule la queue est chargée d'abord et une affordance permet de charger l'historique plus ancien
- [ ] Given l'ouverture d'un thread au transcript déjà volumineux, when le premier rendu se produit, then il survient sous 150 ms au p95 sur la machine de référence
- [ ] Given le cache de rows et de rendus, when l'utilisateur parcourt une longue conversation, then le cache est plafonné par un budget et ne croît pas indéfiniment avec les rows déjà dépassées
- [ ] Given un thread fermé, when ses ressources sont libérées, then le modèle, le tail et les caches associés sont relâchés et la mémoire redescend
- [ ] Given douze threads ouverts successivement, when la mémoire est échantillonnée, then la croissance reste bornée et n'est pas monotone

---

## Functional Requirements

- FR-01: le système doit afficher, pour un thread d'agent supporté, la conversation reconstruite depuis le transcript de session de cet agent.
- FR-02: le système doit mettre à jour la conversation pendant un tour, sans relire l'intégralité du transcript à chaque mise à jour.
- FR-03: le système ne doit jamais piloter un agent autrement qu'en écrivant dans son PTY.
- FR-04: le système ne doit jamais soumettre un prompt sans geste explicite de l'utilisateur, le comportement par défaut de la délivrance étant le pré-remplissage.
- FR-05: le système doit conserver le terminal accessible pour tout thread affichant un chat, et l'utiliser comme rendu par défaut pour tout agent non supporté.
- FR-06: le système doit traiter tout contenu de transcript comme non fiable, sans l'exécuter, sans l'interpréter comme commande, sans rendre de HTML brut et sans charger de ressource distante.
- FR-07: le système doit ignorer une entrée de transcript malformée ou incomplète sans interrompre l'affichage du reste de la conversation.
- FR-08: le système ne doit persister aucun message de conversation dans un stockage propre à Paneflow.
- FR-09: le système doit effectuer toute lecture de transcript hors du thread de rendu.
- FR-10: le système doit refuser d'associer un transcript à un thread en cas d'ambiguïté, plutôt que d'afficher une conversation potentiellement erronée.
- FR-11: le système doit respecter les réglages `agent_panel` existants pour la largeur de contenu et l'affichage du raisonnement, sans introduire de réglage concurrent.
- FR-12: le système ne doit pas modifier le contrat des hooks, en particulier ne pas y faire transiter le contenu des prompts.

## Non-Functional Requirements

- **Performance:** premier rendu d'un thread au transcript déjà écrit sous 150 ms au p95 sur la machine de référence; latence p95 entre l'écriture d'un bloc sur disque et son affichage sous 300 ms; aucune image de rendu dépassant 16,6 ms sur le chemin du chat pendant un tour actif; parsing par lots bornés de sorte qu'un burst de 3000 lignes ne produise aucun gel visible.
- **Sécurité:** contenu de transcript traité comme non fiable, jamais exécuté ni interprété comme commande; aucun HTML brut rendu; aucune requête réseau émise depuis un contenu de message; aucune donnée de conversation écrite hors des fichiers déjà écrits par les agents; aucune extension du contrat de hooks au contenu des prompts.
- **Accessibilité:** navigation clavier complète du fil et du composer, y compris repli et dépliage des groupes d'outils; réglage de mouvement réduit honoré sur toutes les animations ajoutées; contraste des états d'erreur et d'attente conforme au thème existant.
- **Scalabilité:** ouverture correcte d'un répertoire de sessions contenant au moins 2500 fichiers sans dégradation perceptible de la résolution; conversation de 500 blocs affichée sans perte de fluidité; plafond explicite de taille de transcript chargé d'un coup avec chargement de la queue en premier.
- **Reliabilité:** dégradation vers le terminal dans 100 % des cas de transcript absent, illisible ou non reconnu; arrêt propre du tail après trois échecs consécutifs de lecture avec conservation de l'état affiché; aucun panic sur entrée malformée, prouvé par des fixtures dédiées.
- **Portabilité:** résolution des chemins de transcript fonctionnelle sur Linux, macOS et Windows, sans séparateur ni chemin POSIX codé en dur.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Thread neuf | Agent lancé, aucun bloc encore écrit | État d'attente explicite, terminal accessible | "En attente du premier tour de l'agent" |
| 2 | Transcript introuvable | Session non résolue ou fichier absent | Bascule terminal, motif affiché | "Transcript introuvable pour ce thread, affichage du terminal" |
| 3 | Ligne incomplète en fin de fichier | Tail pendant une écriture en cours | Ligne ignorée, curseur non avancé, réessai | Aucun |
| 4 | Ligne JSON invalide au milieu | Corruption ou champ inattendu | Ligne comptée et sautée, parsing poursuivi | Aucun, compteur en diagnostic |
| 5 | Format non reconnu | Montée de version du CLI changeant le schéma | Bascule terminal, signalement explicite | "Format de transcript non reconnu pour cette version de l'agent" |
| 6 | Burst de replay | `resume` ou `fork` réécrivant des milliers de lignes | Ingestion par lots bornés, aucun gel | Aucun |
| 7 | Transcript volumineux | Fichier au-delà du plafond retenu | Chargement de la queue, affordance d'historique | "Afficher les tours plus anciens" |
| 8 | Doublon de canal Codex | Même contenu en `response_item` et `item_completed` | Un seul canal retenu, aucun doublon affiché | Aucun |
| 9 | Agent non supporté | Thread Gemini, Cursor, Grok, Hermes, OpenCode ou autre | Terminal plein cadre inchangé, aucune affordance de chat | "Chat non disponible pour cet agent" |
| 10 | Session reprise | Relance avec `--resume` ou `resume` | Chat suit la session effectivement reprise | Aucun |
| 11 | Processus tué sans arrêt propre | Balayage de PID périmés | Tail arrêté, dernier état conservé | "Agent arrêté" |
| 12 | Prompt pendant un tour | Délivrance alors que le statut est en génération | Mise en file latest-wins, indication d'attente | "Prompt en attente de la fin du tour" |
| 13 | PTY non encore promu | Délivrance juste après création du thread | Tampon puis délivrance à la promotion | Aucun |
| 14 | Processus mort à la délivrance | Agent terminé, prompt délivré | Échec visible, texte conservé dans le composer | "Impossible de délivrer, l'agent n'est plus actif" |
| 15 | Bloc image ou pièce jointe | Contenu non textuel dans le transcript | Emplacement réservé, aucun chargement distant | "Pièce jointe non affichable" |
| 16 | Mouvement réduit | Réglage système actif | Aucune animation planifiée, états finaux directs | Aucun |
| 17 | Réglage changé à chaud | Modification de `thinking_display` ou de la largeur | Application sans redémarrage | Aucun |
| 18 | Bloc de type inconnu | Nouveau type introduit par le CLI | Rendu opaque en repli, jamais perdu | Aucun |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Le format de transcript n'est pas un contrat public et peut casser à toute mise à jour de CLI | High | High | Parsing tolérant et additif, fixtures figées par version observée, détection de format non reconnu avec dégradation vers le terminal, et couverture par tests de la bascule de dégradation. C'est le risque structurel de l'approche et il est assumé en échange de la préservation du human-in-loop |
| 2 | Absence de progression intra-bloc, un long paragraphe apparaît d'un coup | High | Med | Veil de fade à l'arrivée du bloc, affichage immédiat des blocs de raisonnement et des appels d'outils qui, eux, arrivent au fil du tour, donnant la sensation de progression |
| 3 | Divergence perçue entre ce que montre le chat et ce que montre la TUI | Med | Med | Une seule vue active à la fois par thread, bascule explicite, et indication claire de la vue courante plutôt que deux rendus concurrents |
| 4 | Dérive du scope vers un vrai client d'agent, donc retour au headless | Med | High | Contrainte dure inscrite dans les FR: aucune interaction avec l'agent en dehors de son PTY. Toute story qui exigerait un canal direct est rejetée plutôt qu'adaptée |
| 5 | Explosion mémoire sur gros transcripts et longues sessions | Med | High | Plafond de taille chargée, chargement de la queue en premier, budgets de cache bornés, libération à la fermeture du thread, et mesure de croissance sur douze threads |
| 6 | Coût de maintenance de trois lecteurs plus l'UI, sur un repo déjà large | Med | Med | Un seul modèle normalisé, lecteurs réduits à de la traduction pure et testés par fixtures, aucun format inféré sans données réelles |
| 7 | Latence des hooks insuffisante, chat en retard sur le terminal | Med | Med | Poll de mtime borné pendant les tours actifs en complément des hooks, avec mesure de latence consignée en critère d'acceptation |

## Non-Goals

Explicit boundaries - what this version does NOT include:

- Aucun retour à ACP ni à un mode headless. La crate supprimée n'est pas ressuscitée et Paneflow ne devient pas un client d'agent.
- Aucune persistance propre des messages. Le transcript de l'agent est la persistance; le store sqlite de threads reste supprimé.
- Aucun support de OpenCode en V1. Son état vit dans `opencode.db` avec mise à jour en place, ce qui exige un polling de base de données plutôt qu'un tail de fichier. Réexaminable dès que la V1 est stable.
- Aucun support de Gemini CLI, Cursor, Grok et Hermes en V1. Aucune donnée exploitable n'existe sur la machine de référence, et aucun format ne sera inféré sans échantillon réel.
- Aucune édition ni suppression d'un message passé. Le transcript appartient à l'agent et reste en lecture seule.
- Aucun affichage de fenêtre de contexte ni compaction. La compaction appartient au CLI. L'affichage d'un compteur de tokens par tour est une extension évidente, volontairement laissée hors V1.
- Aucune minimap de prompts en V1. Reportée après validation de la lisibilité du fil.
- Aucune review d'édition par hunk depuis le chat. Paneflow ne possède pas les buffers; le dock diff existant reste la surface de lecture des changements.
- Aucun multi-appareils ni synchronisation de conversation.

## Files NOT to Modify

- `src-app/src/terminal/pty_session.rs`, `src-app/src/terminal/ghostty_session.rs` et les moteurs VT - le backend terminal est hors scope, seul le point d'injection déjà public est consommé
- `crates/paneflow-ai-hook/src/**` et `crates/paneflow-shim/src/**` - le contrat des hooks est figé, en particulier le drop volontaire du texte des prompts
- `crates/paneflow-mcp/src/**` - le bridge MCP est indépendant de cette fonctionnalité
- `src-app/Cargo.toml` pins du fork Zed - toute décision de rendu markdown doit se faire sans bouger la révision épinglée
- `src-app/src/app/agents_diff/**` et `src-app/src/agents_view/skills.rs` - surfaces voisines hors scope
- `src-app/src/project/mod.rs` champ `Thread.store_id` - fossile de compatibilité session.json, à laisser tel quel

## Technical Considerations

- **Architecture:** un modèle de blocs normalisé plus un lecteur par agent, sur le patron déjà employé par `agent_sessions.rs` qui normalise neuf agents en `SessionMeta`. Recommandé: étendre ce patron avec un producteur de blocs et un curseur, plutôt que créer une hiérarchie parallèle. Engineering à confirmer.
- **Data Model:** aucun schéma persistant nouveau. Le curseur de lecture est un état en mémoire par thread ouvert. Alternative écartée: persister le curseur, qui rouvrirait la classe de divergence contenu/curseur documentée par zeron.
- **Rendu markdown:** deux options réelles, à trancher en EP-002. Instancier l'entité markdown du fork Zed déjà compilée, dont l'optimisation de streaming vise exactement ce cas d'usage, ou ajouter un constructeur depuis une chaîne au viewer markdown interne dont le parser prend déjà des octets. Compromis: la première réutilise un composant éprouvé mais ancre davantage le pin du fork; la seconde garde un seul chemin de rendu maison mais demande de porter les manques.
- **Déclenchement:** hooks `ai.*` en source primaire, poll de mtime borné en complément pendant les tours actifs. Alternative écartée: watcher de système de fichiers récursif, dont un incident antérieur a montré le coût sur ce repo.
- **Dependencies:** aucune nouvelle dépendance attendue. `pulldown-cmark` et le fork Zed sont déjà présents, et les lecteurs de session existants fournissent déjà la lecture bornée de lignes.
- **Migration:** aucune. La fonctionnalité est additive et la vue par défaut d'un agent non supporté reste le terminal actuel. Plan de retrait: désactiver la branche chat et revenir au terminal plein cadre sans toucher aux données.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Threads dont la conversation est lisible sans TUI | 0 % | 100 % des threads Claude Code, Codex et pi | Month-1 | Comptage manuel sur un échantillon de threads réels des trois agents |
| Latence bloc écrit vers bloc affiché | N/A (nouveau) | p95 < 300 ms | Month-1 | Trace horodatée entre mtime du transcript et notification de rendu, consignée par US-005 |
| Premier rendu d'un thread existant | N/A (nouveau) | p95 < 150 ms | Month-1 | Mesure sur la machine de référence avec un transcript volumineux, US-020 |
| RSS additionnel pour un chat de 500 blocs | N/A (nouveau) | < 40 Mo, redescend à la fermeture | Month-1 | Échantillonnage RSS avant, pendant et après ouverture, US-020 |
| Régressions terminal sur agents non couverts | 0 | 0 | Month-1 | Suite de tests existante plus vérification manuelle sur deux agents non couverts |
| Écrans vides sur transcript inutilisable | N/A (nouveau) | 0 | Month-1 | Scénarios du tableau des cas limites 2, 5 et 9 rejoués manuellement |

## Open Questions

- Quel geste porte la bascule chat vers terminal, et la préférence est-elle mémorisée par thread ou globale? Arthur, avant le début d'EP-002, car cela conditionne US-012 et la chrome de la toolbar.
- Rendu markdown: entité du fork Zed ou constructeur depuis chaîne sur le viewer interne? À trancher au début d'EP-002 par mesure sur un tour réel, car cela détermine la justification durable du pin du fork.
- Le compteur de tokens par tour, déjà lisible dans les trois formats et déjà extrait partiellement par `agent_sessions.rs`, entre-t-il dans un incrément ultérieur ou reste-t-il définitivement hors scope? Arthur, avant la clôture de la V1.
- OpenCode: un polling borné de `opencode.db` est-il acceptable comme exception au principe du tail de fichier, ou attend-on un flux d'événements côté OpenCode? Décision requise avant tout élargissement du scope.
- La machine de référence pour les seuils de performance est-elle la station Fedora d'Arthur, ou faut-il un seuil distinct pour le dual-boot Windows? Nécessaire avant la validation de US-020.
[/PRD]
