[PRD]
# PRD: Espaces, regrouper les workspaces et séparer les comptes des agents

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-10-20 | Arthur Jean | PRD initial : espaces à la Arc en release 1 (groupes nommés de workspaces, sélecteur en bas de la sidebar, attention entre espaces, IPC et CLI), puis environnement et comptes par espace en release 2 (env injectée au spawn, hooks installés dans les dossiers de config de l'espace, sous-processus de l'app sous le bon compte). Vérification Windows réel en dernier epic. |
| 1.1 | 2026-10-21 | Arthur Jean | US-015 amendée : la preuve à deux comptes réels connectés en même temps est retirée, faute de second compte disponible. La story vérifie désormais que chaque variable déplace le login de sa CLI vers un dossier séparé, et note comment `gh` et git choisissent leurs identifiants. L'isolation entre deux comptes connectés repose sur la documentation des CLIs. |
| 1.2 | 2026-10-21 | Arthur Jean | US-021 amendée comme US-015 : sur Windows, la vérification porte sur le déplacement du login de Claude Code et de Codex vers le dossier de l'espace, et non plus sur deux comptes claude.ai connectés en même temps. |

## Problem Statement

Un utilisateur (Olivier M, Discord, 2026-10-20) demande de pouvoir « créer des espaces, à la manière d'Arc, pour décorréler le pro et le perso ». Paneflow n'a qu'une liste plate de workspaces et un seul environnement pour toutes ses sessions.

1. **Aucun regroupement des workspaces.** `PaneFlowApp` garde `workspaces: Vec<Workspace>` et `active_idx` (`src-app/src/main.rs:170-171`). La sidebar ne regroupe que les workspaces d'un même dépôt (`compute_display_order`, `src-app/src/app/sidebar/mod.rs:233`). Projets pro et perso s'affichent mélangés, et `SelectWorkspace1..9` (`src-app/src/app/actions.rs:31`) parcourt l'ensemble.
2. **Un seul compte par machine pour les agents et les outils.** Chaque terminal reçoit la même env : `terminal.env` fusionné à l'env de la surface (`src-app/src/terminal/pty_session/spawn_env.rs:367-376`). Lancer un agent du travail avec le compte perso, ou committer sous la mauvaise identité, ne demande qu'un oubli. Les profils d'agent (`agent_profiles`, `crates/paneflow-config/src/schema/config.rs:563`) portent une env par agent, pas par contexte de travail.
3. **Changer le dossier de config d'un agent casse Paneflow sans signal.** Les hooks et le MCP s'installent dans le dossier résolu depuis l'env du process de l'app (`ConfigPaths::resolve`, `crates/paneflow-mcp-install/src/integrations.rs:100-110`) : un agent lancé avec un autre `CLAUDE_CONFIG_DIR` ou `CODEX_HOME` n'a pas de hooks, donc plus d'état réflexion ou attente. La découverte des sessions (`src-app/src/claude_sessions.rs:39`), l'auto-nommage (`src-app/src/auto_naming.rs:214`, `:273`), le badge PR (`src-app/src/app/pull_request.rs:178`), le clone (`src-app/src/app/clone_repo.rs:253`, `:364`) et le git interne (`src-app/src/git_command.rs:74`) tournent tous avec l'env de l'app, donc avec le compte par défaut. L'auto-nommage fait résumer le contenu d'un terminal par `claude -p` ou `codex exec` sous ce compte.

**Why now:** la demande vient d'un utilisateur actif, et Paneflow se vend comme cockpit multi-agents : plusieurs contextes de travail menés en parallèle sont son cas d'usage. Claude Code documente désormais le multi-compte par `CLAUDE_CONFIG_DIR`, Keychain macOS compris ([Authentication](https://code.claude.com/docs/en/authentication)), ce qui rend fiable une séparation par environnement. Les outils actuels (direnv, `includeIf` de git) sont des hooks de shell qui ne s'appliquent pas aux processus que Paneflow lance lui-même.

## Overview

Un espace est un groupe nommé de workspaces, avec une couleur, puis en release 2 un environnement. La liste des workspaces reste plate et indexée globalement : l'espace est une métadonnée de chaque workspace, et seuls la sidebar, le sélecteur et la navigation filtrent sur l'espace actif. L'IPC, la file d'attention, `find_pane_by_surface_id` et `DetachedPaneLocation` restent intacts. Les définitions vivent dans `paneflow.json` (`spaces`, sur le modèle d'`agent_profiles`), l'appartenance et l'espace actif dans `session.json`, en champs additifs qui gardent la version de schéma 3. Un workspace sans espace appartient à l'espace par défaut, implicite et nommé « Default » : tant qu'aucun espace n'est créé, rien ne change à l'écran.

Le sélecteur se place en bas de la sidebar, au-dessus du pied de page, comme les icônes d'espaces d'Arc. Changer d'espace remplace la liste sans animation et ramène le dernier workspace affiché de cet espace ; un espace vide montre l'état vide existant. Arc n'a aucun signal entre Spaces ; ici chaque espace caché affiche un badge quand un agent y attend, a échoué ou a fini, et la file d'attention comme `workspace.select` basculent d'espace pour amener en un geste vers un agent caché.

En release 2, chaque espace porte une env injectée au spawn de chaque terminal de ses workspaces, avec la précédence global < espace < profil d'agent < surface. Quand cette env définit `CLAUDE_CONFIG_DIR` ou `CODEX_HOME`, Paneflow crée le dossier, y installe ses hooks et son MCP, et affiche leur état par espace. Les sous-processus de l'app (badge PR, clone, git, auto-nommage, découverte et reprise des sessions) reçoivent une liste blanche de cette env. Un espace organise le travail ; ce n'est pas une frontière de sécurité, et la documentation le dit.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Rendus de la sidebar par changement d'espace | 1 | 1 |
| Processus git lancés par un changement d'espace, au-delà de la sonde du workspace affiché | 0 | 0 |
| Délai entre l'entrée en attente d'un agent dans un espace caché et l'apparition du badge | ≤ 1 s (rendu suivant l'événement d'état) | ≤ 1 s |
| Agents d'un espace à `CLAUDE_CONFIG_DIR` propre dont l'état réflexion ou attente s'affiche (release 2) | 100 % sur Linux | 100 % sur Linux et Windows |
| Chemins de sous-processus de l'app lancés sans l'env de l'espace concerné (release 2) | 0 sur 5 | 0 sur 5 |

## Target Users

### Développeur multi-contextes
- **Role:** Développeur qui mène travail salarié ou missions et projets personnels dans la même instance Paneflow (le cas d'Olivier, et celui du mainteneur avec StriveX et Paneflow).
- **Behaviors:** Des dizaines de workspaces ouverts, des agents en arrière-plan, deux comptes Claude ou Codex, deux identités git ou GitHub.
- **Pain points:** Liste mélangée ; risque de lancer un agent pro sur le compte perso (facturation, données du client) ; `Ctrl+1..9` traverse tout.
- **Current workaround:** Alias shell `CLAUDE_CONFIG_DIR=~/.claude-work claude`, profils d'agent Paneflow, direnv ou `includeIf` git ; ou une seconde instance avec `PANEFLOW_HOME`, qui double host, worker et fenêtre.
- **Success looks like:** Un geste pour passer du pro au perso, et tout ce qui se lance dans l'espace pro utilise les comptes pro, statut des agents compris.

### Freelance multi-clients
- **Role:** Consultant avec un compte ou une identité git par client.
- **Behaviors:** 3 à 5 contextes, alternés dans la journée.
- **Pain points:** Identités git et clés SSH à jongler ; un agent lancé dans le mauvais contexte travaille sous le mauvais compte.
- **Current workaround:** `includeIf "gitdir:"`, `gh auth switch` global, alias par client.
- **Success looks like:** Un espace par client avec son env, et les agents d'un client caché restent visibles quand ils attendent.

### Script et agent conducteur
- **Role:** Scripts `paneflow` et agents conducteurs (skill paneflow-conductor) qui pilotent l'instance par IPC.
- **Behaviors:** `paneflow new`, `paneflow select`, `paneflow up`, `workspace.select` par index.
- **Pain points:** Aucun moyen de cibler un contexte de travail.
- **Current workaround:** Aucun.
- **Success looks like:** `--space` à la création, l'espace visible dans `workspace.list`, une sélection qui bascule d'espace.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- Arc Browser : un Space contient onglets épinglés et du jour, avec nom, icône et couleur ; un Profile isole logins, cookies, historique, extensions et favoris, peut servir plusieurs Spaces, et un profil « Default » existe ; Ctrl+N sur macOS et Alt+N sur Windows vont au space N, Cmd+Opt+←/→ et Ctrl+Alt+←/→ au voisin ([Profiles](https://resources.arc.net/hc/en-us/articles/19227964556183-Profiles-Separate-Work-Personal-Browsing), [Keyboard Shortcuts](https://resources.arc.net/hc/en-us/articles/20595231349911-Keyboard-Shortcuts)). Aucun badge d'activité entre Spaces n'est documenté. Arc ne reçoit plus que les mises à jour Chromium ([arc.net](https://arc.net/)).
- Zen Browser : workspaces liés à un conteneur par défaut ; les utilisateurs veulent regroupement et isolation d'identité dans la même fenêtre, pas une fenêtre par profil ([Zen workspaces](https://docs.zen-browser.app/user-manual/workspaces)).
- VS Code Profiles : réglages, extensions et MCP par profil, activés par dossier, sans variables d'environnement ni isolation de comptes ([VS Code profiles](https://code.visualstudio.com/docs/configure/profiles)).
- tmux et WezTerm : tmux applique `set-environment` aux seuls panes ouverts après le changement, piège souvent critiqué ; les workspaces de WezTerm sont une simple étiquette ([WezTerm](https://wezterm.org/recipes/workspaces.html)).
- direnv, mise, `includeIf` git, `gh auth switch` : hooks de shell ou de git, ignorés par un processus lancé directement par une app ; `gh auth switch` bascule un compte global et ses mainteneurs refusent le basculement par dossier ([gh multiple accounts](https://github.com/cli/cli/blob/4b077daf7e74dbe6bf933f69e220066e3a12e8b4/docs/multiple-accounts.md)).
- **Market gap:** aucun outil ne combine regroupement de terminaux, env appliquée au spawn de chaque processus, agents compris, et visibilité des agents des contextes cachés.

### Best Practices Applied
- Séparer le conteneur visuel de l'isolation, comme Arc ; en v1 l'env reste portée par l'espace et l'entité « profil partagé » est différée (Non-Goals).
- Appliquer l'env au spawn de chaque PTY, de façon déterministe (leçon de tmux), au point de passage unique `resolve_spawn_launch` (`src-app/src/terminal/pty_session/spawn_env.rs:318`).
- Pointer des dossiers de config plutôt qu'injecter des secrets : `CLAUDE_CONFIG_DIR` est le mécanisme multi-compte officiel de Claude Code et isole `.credentials.json` comme l'entrée Keychain macOS ([Authentication](https://code.claude.com/docs/en/authentication)) ; `CODEX_HOME` isole `config.toml`, `auth.json` et `sessions/` mais doit exister avant le lancement (Context7 `/openai/codex`, `codex-rs/utils/home-dir`).
- Les variables `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_API_KEY` et `CLAUDE_CODE_OAUTH_TOKEN` passent devant le login stocké ([Authentication](https://code.claude.com/docs/en/authentication), « Authentication precedence ») ; `GH_TOKEN` passe devant les comptes de `GH_CONFIG_DIR`.
- Un seul espace laisse l'interface inchangée : une source tierce rapporte qu'environ 5,5 % des utilisateurs actifs d'Arc utilisaient plusieurs Spaces ([remotesynthesis](https://remotesynthesis.com/posts/migrating-from-arc-to-zen)).

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- `CLAUDE_CONFIG_DIR` isole deux connexions claude.ai dans la version courante de Claude Code, d'après la doc officielle ; des rapports plus anciens (miroir claudeissues.com, #20553, #70697, #47661) décrivaient des collisions. US-015 vérifie sur Linux que la variable déplace le login et ses fichiers vers le dossier ; la coexistence de deux comptes connectés repose sur la doc, faute de second compte pour la vérifier (v1.1). Sur Windows, US-021 vérifie le même déplacement du login ; macOS repose sur la doc tant qu'aucun Mac n'est disponible.
- `CODEX_HOME` isole la connexion Codex tant que `cli_auth_credentials_store` ne range pas l'auth dans le trousseau du système. US-015 vérifie le déplacement du login vers le dossier.
- `GH_CONFIG_DIR` isole les comptes `gh`, sauf quand `GH_TOKEN` ou `GITHUB_TOKEN` est défini ; `git push` n'en profite que si le helper d'identifiants de git est `gh auth git-credential`. US-015 vérifie le déplacement du login et relève le helper configuré ; documenté.
- `paneflow-ai-hook`, lancé par l'agent dans le pane, hérite de l'env du pane : une fois installés dans le dossier de l'espace, les hooks remontent comme aujourd'hui. `crates/paneflow-agent-config/src/io.rs:27-66` résout déjà `CLAUDE_CONFIG_DIR` et `CODEX_HOME`.
- Les utilisateurs veulent l'env par espace, pas seulement le regroupement. La question est posée à Olivier ; la release 1 a une valeur seule.
- Neuf espaces suffisent : chiffres 1 à 9, largeur de la sidebar.
- Le chantier resource-efficiency en cours dans le worktree (`SidebarView` en cache, compteur `sidebar_renders`, `src-app/src/app/sidebar/view.rs`) est livré avant EP-002, qui se construit dessus.

### Hard Constraints
- Linux, macOS et Windows : chaque changement a un chemin sur les trois OS ; le code `#[cfg(windows)]` n'est linté que par le job Windows.
- Le thread de rendu ne bloque jamais : installation des hooks, création de dossiers et sous-processus passent par `smol::unblock` ou le bootstrap.
- Aucune nouvelle dépendance ; GPUI reste au rev épinglé.
- Aucun commentaire dans le code source ; tout nouvel item se déclare avant `mod tests`.
- `SESSION_SCHEMA_VERSION` reste 3, et un `session.json` écrit par cette version reste lisible par la version précédente.
- `paneflow-shim`, `paneflow-ai-hook` et `paneflow-mcp` gardent leurs plafonds de taille.
- Humain dans la boucle : un espace ne lance ni ne soumet jamais un agent tout seul.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatage canonique, gate CI sur les quatre builds
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, cibles de test comprises
- `cargo test --workspace --locked` - tests unitaires et d'intégration
- `cargo deny check advisories licenses sources` - uniquement quand une dépendance change (aucune n'est prévue)

Gates additionnels :
- Le job CI `Performance gates (Linux x86_64)` passe.
- Stories UI (US-006 à US-012, US-016, US-017) : passe visuelle d'Arthur sous Linux sur un build debug, capture ou enregistrement court joint à la PR. L'agent livre le changement sans lancer l'app pour le vérifier.
- Stories qui touchent du code `#[cfg(windows)]` : le job « Windows x86_64 libghostty check » passe, et la PR dit si Windows a été vérifié par inspection ou sur le matériel réel.
- Chaque story met à jour la page de `docs/user/` qu'elle concerne et ajoute son entrée sous `[Unreleased]` dans `CHANGELOG.md`.

## Epics & User Stories

Release 1 : EP-001 à EP-004, puis US-020. Release 2 : EP-005, puis US-021.

### EP-001: Socle des espaces

Donner à chaque workspace une appartenance à un espace, la persister, et garantir que le workspace affiché appartient toujours à l'espace actif.

**Definition of Done:** les espaces se lisent depuis `paneflow.json` avec rechargement à chaud ; appartenance et espace actif se restaurent depuis `session.json` ; basculer d'espace change le workspace affiché ; aucun chemin d'activation ni de création ne laisse le workspace affiché hors de l'espace actif, et chaque chemin a son test unitaire.

#### US-001: Déclarer les espaces dans paneflow.json
**Description:** As a développeur multi-contextes, I want que mes espaces (nom, couleur) soient déclarés dans `paneflow.json` so that ils survivent aux redémarrages et se rechargent à chaud comme le reste de la config.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] `crates/paneflow-config/src/schema/identity.rs` déclare `SpaceId` avec la macro `uuid_identity!`, à côté de `WorkspaceId`.
- [ ] `PaneFlowConfig` gagne `spaces: Vec<SpaceConfig>`, avec `SpaceConfig { id: SpaceId, name: String, color: Option<SpaceColor> }`, désérialisé avec le même lenient qu'`agent_profiles` (`config.rs:89-94`) et omis à l'écriture quand il est vide.
- [ ] `SpaceColor` vaut `red`, `green`, `yellow`, `blue`, `magenta` ou `cyan`, résolus plus tard sur la palette ANSI du thème actif ; une valeur inconnue devient `None` sans écarter l'espace.
- [ ] Le loader écarte, avec un `warn!` par entrée : un `id` absent ou invalide, un nom vide après trim ou de plus de 32 caractères, un nom déjà pris (comparaison insensible à la casse, la première entrée gagne), le nom réservé « Default », toute entrée au-delà de la neuvième.
- [ ] Given `paneflow.json` modifié à la main pendant que l'app tourne, when le watcher recharge, then la nouvelle liste d'espaces est disponible sans redémarrage.
- [ ] Échec : given une clé `spaces` qui n'est pas un tableau, when la config se charge, then `spaces` vaut une liste vide, le reste de la config se charge, et un seul `warn!` le signale.
- [ ] `docs/user/configuration.md` documente la clé `spaces` et ses limites.

#### US-002: Persister l'appartenance et l'espace actif
**Description:** As a développeur multi-contextes, I want que Paneflow retienne l'espace de chaque workspace et l'espace actif so that je retrouve mon organisation et mon contexte au redémarrage.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `Workspace` (`src-app/src/workspace/mod.rs:91`) gagne `space: Option<SpaceId>`, `None` désignant l'espace par défaut, et `PaneFlowApp` gagne `active_space: Option<SpaceId>`.
- [ ] `WorkspaceSession` (`crates/paneflow-config/src/schema/session.rs:201`) gagne `space: Option<SpaceId>` et `SessionState` gagne `active_space: Option<SpaceId>`, tous deux `#[serde(default, skip_serializing_if = "Option::is_none")]` ; `SESSION_SCHEMA_VERSION` reste 3.
- [ ] `build_session_state` (`src-app/src/app/session.rs:47`) écrit les deux champs ; `restore_workspaces` (`src-app/src/app/session.rs:529`) les relit.
- [ ] Given un `space` ou un `active_space` qui ne correspond à aucun espace de la config, when la session se restaure, then le workspace ou l'espace actif retombe sur l'espace par défaut et un seul `info!` compte les références orphelines.
- [ ] Given un `active_workspace` hors de l'espace actif restauré, when la session se restaure, then le workspace affiché devient le premier de l'espace actif dans l'ordre d'affichage, ou l'état vide si l'espace n'en a aucun.
- [ ] Un test aller-retour couvre trois workspaces répartis sur deux espaces et l'espace par défaut.
- [ ] Échec : given un `session.json` écrit avant ce PRD, when il se charge, then tous ses workspaces sont dans l'espace par défaut et aucune erreur de corruption n'est signalée.
- [ ] Un test montre qu'un JSON contenant `space` et `active_space` se désérialise dans un `SessionState` qui ignore ces champs, ce qui garantit la lecture par la version précédente.

#### US-003: Basculer d'espace et filtrer la navigation
**Description:** As a développeur multi-contextes, I want basculer d'un espace à l'autre et que la navigation entre workspaces reste dans l'espace actif so that `Ctrl+1..9` et `Ctrl+Tab` ne m'emmènent plus dans l'autre contexte.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] `PaneFlowApp::activate_space(Option<SpaceId>)` mémorise le workspace affiché de l'espace quitté dans une table en mémoire (espace vers id de workspace), puis affiche le workspace mémorisé de l'espace cible s'il existe encore et lui appartient toujours, sinon le premier de l'espace dans l'ordre d'affichage.
- [ ] `compute_display_order` (`src-app/src/app/sidebar/mod.rs:233`) ne renvoie que les workspaces de l'espace actif, en gardant le regroupement par dépôt.
- [ ] `handle_select_ws` (`src-app/src/app/workspace_ops/mod.rs:1202`) et `NextWorkspace` (`:1190`) indexent et parcourent cet ordre filtré.
- [ ] `active_workspace()` et `active_workspace_mut()` (`src-app/src/app/workspace_ops/mod.rs:461`, `:469`) renvoient `None` quand le workspace d'`active_idx` n'appartient pas à l'espace actif.
- [ ] Given un espace actif sans workspace, when la fenêtre se dessine, then la zone principale montre l'état vide existant (`src-app/src/app/render.rs:261`) au lieu d'un workspace d'un autre espace.
- [ ] Un changement d'espace déclenche la sonde git du workspace affiché par `probe_workspace_git_if_shown_changed`, et aucun autre processus git.
- [ ] Échec : given le workspace mémorisé fermé entre-temps, when l'utilisateur revient sur l'espace, then le premier workspace de l'espace s'affiche, sans panique ni index hors bornes.

#### US-004: Garder le workspace affiché dans l'espace actif sur chaque chemin d'activation
**Description:** As a développeur multi-contextes, I want que tout ce qui active un workspace respecte l'espace actif, ou bascule d'espace explicitement so that je ne me retrouve jamais devant un workspace d'un autre contexte sans le voir.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] `activate_workspace_at` (`src-app/src/app/workspace_ops/mod.rs:482`) bascule d'abord sur l'espace du workspace cible quand il diffère de l'espace actif ; c'est le seul chemin qui change d'espace implicitement.
- [ ] Fermer un workspace (`src-app/src/app/workspace_ops/mod.rs:1056`) affiche le voisin le plus proche dans le même espace, ou l'état vide si l'espace n'en a plus.
- [ ] Ouvrir un dossier déjà ouvert dans un autre espace (`src-app/src/app/workspace_ops/mod.rs:611`) passe par `activate_workspace_at`, donc bascule sur cet espace au lieu de créer un doublon.
- [ ] Le rattachement d'une session hébergée (`src-app/src/app/hosted_sessions.rs:784`), la réouverture d'un pane fermé (`src-app/src/app/workspace_ops/mod.rs:893`) et le lancement d'un template (`src-app/src/app/workspace_ops/template_launch.rs:59`) passent par `activate_workspace_at` au lieu d'écrire `active_idx`.
- [ ] Hors des accesseurs `active_workspace()` et `active_workspace_mut()`, plus aucun accès `workspaces[self.active_idx]`, `workspaces.get(self.active_idx)` ou `workspaces.get_mut(self.active_idx)` ne reste dans `src-app/src` (15 occurrences aujourd'hui, accesseurs compris).
- [ ] Un test par chemin vérifie qu'après l'action, le workspace affiché appartient à l'espace actif.
- [ ] Échec : given un espace actif vide, when un raccourci agit sur le workspace actif (split, nouvel onglet, fermeture), then l'action ne fait rien et aucun workspace d'un autre espace n'est modifié.

#### US-005: Créer chaque workspace dans l'espace actif
**Description:** As a développeur multi-contextes, I want qu'un workspace créé par n'importe quel chemin rejoigne l'espace actif so that mon organisation tienne sans que je déplace chaque nouveau workspace.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] `open_workspace_folders` (`src-app/src/app/workspace_ops/mod.rs:598`) assigne l'espace actif, ce qui couvre le nouveau workspace, le sélecteur de dossier, l'accueil, les récents, le clone et le dépôt d'un dossier sur la sidebar.
- [ ] Le lancement d'un template qui crée un workspace (`src-app/src/app/workspace_ops/template_launch.rs`) assigne l'espace actif.
- [ ] `workspace.create` et `workspace.up` (`src-app/src/app/ipc_handler/workspace_methods.rs:332`, `:460`) assignent l'espace actif quand l'appel ne précise pas d'espace (paramètre ajouté par US-013).
- [ ] Ouvrir une ligne de « Other sessions » sans workspace ouvert (`src-app/src/app/hosted_sessions.rs:740`) crée le workspace dans l'espace actif.
- [ ] Échec : given un espace actif supprimé de `paneflow.json` pendant que l'app tourne, when un workspace se crée, then il rejoint l'espace par défaut, qui devient l'espace actif.

---

### EP-002: Interface des espaces

Rendre les espaces visibles et manipulables : sidebar filtrée, sélecteur, déplacement d'un workspace, palette, raccourcis et réglages.

**Definition of Done:** avec au moins un espace créé, le sélecteur apparaît en bas de la sidebar, la liste ne montre que l'espace actif, un workspace se déplace par menu ou palette, deux raccourcis par plateforme changent d'espace, et Settings > Spaces crée, renomme, colore et supprime un espace ; passe visuelle d'Arthur sous Linux.

#### US-006: Filtrer la sidebar par espace
**Description:** As a développeur multi-contextes, I want que la sidebar ne liste que les workspaces de l'espace actif so that je ne voie que le contexte sur lequel je travaille.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] `sidebar_rows` (`src-app/src/app/sidebar/mod.rs:519`) et `all_workspaces_expanded` (`:560`) ne considèrent que les workspaces de l'espace actif.
- [ ] `sidebar_order_signature` (`src-app/src/app/sidebar/mod.rs:219`) inclut l'espace actif et l'espace de chaque workspace, si bien qu'un changement d'espace ou un déplacement invalide l'ordre en cache.
- [ ] Un changement d'espace remplace les lignes sans animation : la topologie passée à `sidebar_row_motion` (`:588-604`) est filtrée comme les lignes, et aucune ligne cachée ne s'anime.
- [ ] Les emplacements de dépôt (`src-app/src/app/sidebar/drop.rs:17-26`) se convertissent en index globaux, et réordonner deux workspaces d'un espace ne déplace aucun workspace d'un autre espace.
- [ ] Le curseur clavier de la sidebar (`src-app/src/app/sidebar/keyboard.rs:88`) revient sur le workspace affiché après un changement d'espace.
- [ ] Le groupe « Other sessions » (`src-app/src/app/sidebar/session_rows.rs:89`) s'affiche dans tous les espaces.
- [ ] Given un espace actif vide, when la sidebar se dessine, then l'état vide indique « No workspaces in {space} » avec le bouton d'ouverture de dossier existant.
- [ ] Given le scénario `desktop_tui_30_workspaces` de `perf_gates` avec ses 30 workspaces répartis sur 3 espaces, when il mesure, then `sidebar_renders` reste dans son budget actuel.
- [ ] Échec : given un filtre texte actif dans la sidebar, when l'utilisateur change d'espace, then le filtre s'applique au nouvel espace et le message « No workspaces or tabs match » s'affiche s'il ne trouve rien.

#### US-007: Afficher le sélecteur d'espaces en bas de la sidebar
**Description:** As a développeur multi-contextes, I want une rangée d'espaces en bas de la sidebar so that je voie l'espace actif et que j'en change d'un clic.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-003

**Acceptance Criteria:**
- [ ] `render_space_switcher` s'insère entre la liste et `render_sidebar_settings_footer` (`src-app/src/app/sidebar/mod.rs:390`), et ne s'affiche que si `paneflow.json` déclare au moins un espace.
- [ ] La rangée montre « Default » en premier puis les espaces dans l'ordre de la config ; chaque bouton fait 22 px, porte la première lettre du nom en majuscule, teintée par sa couleur résolue sur la palette ANSI du thème, ou `muted` sans couleur.
- [ ] Le bouton de l'espace actif porte `sidebar_tab_active_background` et les autres le survol de `sidebar_tab_hover_background` (DESIGN.md §4.3) ; une info-bulle donne le nom complet.
- [ ] Cliquer un bouton appelle `activate_space` ; un bouton `+` en fin de rangée ouvre Settings > Spaces sur un nouvel espace dont le champ nom a le focus.
- [ ] Chaque bouton expose le rôle bouton et le libellé accessible « Space {name} », suivi de « current » pour l'espace actif.
- [ ] Échec : given neuf espaces déclarés, when la rangée se dessine, then le bouton `+` est désactivé avec l'info-bulle « 9 spaces maximum ».
- [ ] Échec : given une sidebar à sa largeur minimale avec dix boutons, when la rangée se dessine, then elle tient sur une ligne sans chevauchement ni débordement du pied de page.

#### US-008: Déplacer un workspace vers un autre espace
**Description:** As a développeur multi-contextes, I want déplacer un workspace d'un espace à l'autre so that je range ce qui existait avant les espaces et corrige une erreur de classement.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Le menu contextuel d'un workspace (`src-app/src/app/sidebar/context_menu.rs:155`) gagne un sous-menu « Move to space » qui liste les autres espaces, « Default » compris, puis « New space… ».
- [ ] « New space… » crée un espace nommé « Space N » (N le premier entier libre), y déplace le workspace et ouvre Settings > Spaces sur son nom.
- [ ] Déplacer le workspace affiché le retire de la liste, affiche son voisin dans l'espace courant, et montre un toast « Moved {workspace} to {space} » avec un bouton « Show » qui bascule sur l'espace cible.
- [ ] Le déplacement est persisté par la sauvegarde de session existante, sans écriture synchrone sur le thread de rendu.
- [ ] Échec : given le dernier workspace d'un espace déplacé, when le déplacement se termine, then l'espace courant montre son état vide et l'espace n'est pas supprimé.

#### US-009: Changer d'espace au clavier et par la palette
**Description:** As a développeur multi-contextes, I want des raccourcis et des commandes de palette pour les espaces so that je change de contexte sans souris.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Les actions `next_space`, `previous_space`, `new_space` et `move_workspace_to_space` existent dans `src-app/src/app/actions.rs` et dans le registre des raccourcis (`src-app/src/keybindings/registry.rs`).
- [ ] `MACOS_PLATFORM_DEFAULTS` lie `cmd-alt-right` et `cmd-alt-left` à `next_space` et `previous_space` ; `OTHER_PLATFORM_DEFAULTS` lie `ctrl-alt-pagedown` et `ctrl-alt-pageup` (`src-app/src/keybindings/defaults.rs:420`).
- [ ] Le passage au voisin boucle de la dernière position à la première, « Default » compris.
- [ ] La palette gagne un sélecteur « Switch space » et les commandes « New space » et « Move workspace to space » ; le test `every_command_action_exists_in_the_registry` (`src-app/src/app/command_palette/catalog.rs:1470`) passe.
- [ ] Le sélecteur de workspaces de la palette (`workspace_values`, `catalog.rs:215`) liste d'abord les workspaces de l'espace actif, puis les autres avec le nom de leur espace ; en choisir un d'un autre espace bascule d'espace.
- [ ] `docs/user/keybindings.md` documente les deux raccourcis par plateforme et leur remappage.
- [ ] Échec : given aucun espace déclaré, when `next_space` ou `previous_space` se déclenche, then rien ne change et aucun toast ne s'affiche.

#### US-010: Gérer les espaces dans Settings
**Description:** As a développeur multi-contextes, I want créer, renommer, colorer et supprimer mes espaces dans Settings so that je n'aie pas à éditer `paneflow.json` à la main.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] `SettingsSection` (`src-app/src/app/window.rs:4`) gagne `Spaces`, présent dans la navigation de Settings, le sélecteur de sections de la palette (`catalog.rs:268`) et la recherche de Settings.
- [ ] La section liste « Default », non modifiable, puis chaque espace avec un champ nom, un choix de couleur parmi les six teintes ou aucune, et un bouton de suppression ; un bouton « New space » ajoute « Space N ».
- [ ] Les écritures passent par `config_writer`, comme `persist_agent_profiles` (`src-app/src/settings/tabs/agents.rs:1146`), et le rechargement à chaud met à jour le sélecteur.
- [ ] Supprimer un espace ouvre une confirmation « Its {n} workspaces move to Default » ; après confirmation, ses workspaces passent dans l'espace par défaut, et l'espace par défaut devient actif si l'espace supprimé l'était.
- [ ] Échec : given un nom vide, déjà pris (insensible à la casse), égal à « Default » ou de plus de 32 caractères, when l'utilisateur valide, then le champ affiche l'erreur en ligne et rien n'est écrit.
- [ ] Échec : given neuf espaces, when la section s'affiche, then « New space » est désactivé avec la raison.
- [ ] `docs/user/settings.md` décrit la section.

---

### EP-003: Attention entre espaces

Un agent qui attend, échoue ou finit dans un espace caché reste visible, et un geste y mène.

**Definition of Done:** chaque espace caché montre un badge quand l'un de ses agents attend une entrée, a échoué ou a fini sans être vu ; la file d'attention et les notifications nomment l'espace ; sauter vers un agent caché bascule d'espace.

#### US-011: Afficher un badge d'attention par espace
**Description:** As a développeur multi-contextes, I want un badge sur un espace caché dont un agent attend, a échoué ou a fini so that aucun agent ne reste bloqué parce que son espace n'est pas affiché.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] La classification par workspace d'`attention_queue_rows` (`src-app/src/app/attention_queue.rs:48`) est extraite en une fonction réutilisée par la file et par le sélecteur, qui compte par espace les approbations en attente, les agents en attente d'entrée, les statuts déclarés en erreur et les fins non vues.
- [ ] Le bouton d'un espace caché affiche un point teinté de la couleur de l'état le plus urgent (entrée, puis erreur, puis fin), avec les couleurs de statut de la sidebar (DESIGN.md, lignes d'agent), et le nombre d'éléments quand il dépasse 1.
- [ ] L'espace actif n'affiche pas de badge, ses lignes portant déjà leur état.
- [ ] Le badge se met à jour sur la notification qui met à jour les lignes, sans minuterie ajoutée.
- [ ] Le libellé accessible du bouton ajoute « {n} agents need attention » quand le badge est visible.
- [ ] Échec : given un workspace caché en sourdine (`muted`), when un de ses agents finit, then le badge ne compte pas cette fin, comme la notification aujourd'hui (`src-app/src/app/host_agents.rs:855`).

#### US-012: Nommer l'espace dans la file d'attention et les notifications
**Description:** As a développeur multi-contextes, I want que la file d'attention et les notifications disent dans quel espace un agent attend so that je sache où je vais avant d'y sauter.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Quand au moins un espace est déclaré, chaque ligne de la file d'attention affiche le nom de l'espace avant le titre du workspace.
- [ ] `notification_context_body` (`src-app/src/agents/notifications.rs:214`) ajoute le nom de l'espace quand au moins un espace est déclaré.
- [ ] Given un agent en attente dans un espace caché, when l'utilisateur l'active depuis la file (`attention_queue_activate`, `src-app/src/app/attention_queue.rs:169`), then Paneflow bascule sur son espace et donne le focus à son pane, et un test le vérifie.
- [ ] Échec : given le pane de la ligne fermé entre-temps, when l'utilisateur l'active, then l'espace actif ne change pas et la file se rafraîchit.

---

### EP-004: Espaces dans l'IPC et la CLI

Les scripts et les agents conducteurs voient et ciblent les espaces.

**Definition of Done:** `workspace.list` et `workspace.current` exposent l'espace, `workspace.select` bascule d'espace, `workspace.create` et `workspace.up` acceptent un espace, la CLI suit, et `docs/user/scripting.md` documente le tout, y compris le fait qu'un espace n'est pas une frontière de sécurité.

#### US-013: Exposer et cibler les espaces par IPC
**Description:** As a script ou agent conducteur, I want voir l'espace de chaque workspace et créer un workspace dans un espace donné so that j'organise l'instance comme l'humain.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] `workspace.list` et `workspace.current` (`src-app/src/app/ipc_handler/workspace_methods.rs:622`, `:639`) ajoutent `space: { id, name }`, ou `null` pour l'espace par défaut ; les index restent globaux.
- [ ] `workspace.select` (`:653`) sur un workspace d'un espace caché bascule sur cet espace.
- [ ] `workspace.create` et `workspace.up` acceptent un paramètre optionnel `space`, nom (insensible à la casse) ou id ; sans lui, l'espace actif s'applique.
- [ ] Échec : given un `space` qui ne correspond à aucun espace, when `workspace.create` est appelé, then la réponse est une erreur de paramètres invalides « unknown space: {value} » et aucun workspace n'est créé.
- [ ] Les tests du handler couvrent la liste, la sélection qui bascule et la création ciblée.

#### US-014: Cibler les espaces depuis la CLI et documenter le scripting
**Description:** As a script ou agent conducteur, I want une option `--space` et l'espace dans les sorties de la CLI so that un script crée ses workspaces au bon endroit.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] `paneflow new` accepte `--space <name>` (`#[arg(help = ...)]`, `src-app/src/cli/mod.rs:244`).
- [ ] `WorkspaceSpec` (`src-app/src/cli/workspace_spec.rs:9`) accepte une clé optionnelle `space` lue par `paneflow up`.
- [ ] `paneflow ls` (`src-app/src/cli/read_cmds.rs:164`) ajoute l'objet `space` à chaque surface dans sa sortie JSON par défaut, et une colonne espace avec `--human`.
- [ ] `docs/user/scripting.md` documente `--space`, la clé `space` de `paneflow.workspace.toml`, la bascule de `workspace.select`, et précise qu'un espace organise le travail sans isoler : `PANEFLOW_MCP_SCOPE=all` et `write_pane` restent valables sur toute l'instance.
- [ ] Échec : given `--space` avec un nom inconnu, when la commande s'exécute, then elle sort avec le code d'erreur de la CLI et le message « unknown space: {name} ».

---

### EP-005: Environnement et comptes par espace

Chaque espace porte une env appliquée à tout ce qui se lance pour ses workspaces, terminaux, agents et sous-processus de l'app, sans casser le statut des agents.

**Definition of Done:** un espace définit son env dans Settings ; chaque terminal de ses workspaces la reçoit au spawn ; quand elle déplace `CLAUDE_CONFIG_DIR` ou `CODEX_HOME`, les hooks et le MCP de Paneflow y sont installés et leur état est visible ; badge PR, clone, git, auto-nommage, découverte et reprise des sessions tournent sous l'env de l'espace ; sous Linux, chaque variable de dossier déplace le login de sa CLI vers ce dossier (US-015).

#### US-015: Valider l'hypothèse : une variable de dossier isole deux comptes par CLI
**Description:** As a mainteneur, I want vérifier que `CLAUDE_CONFIG_DIR`, `CODEX_HOME` et `GH_CONFIG_DIR` déplacent chacune le login de leur CLI vers un dossier séparé so that la release 2 ne promette pas une isolation que les CLIs ne tiennent pas.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Sous Linux, avec le compte par défaut connecté, `claude auth status` lancé sous un `CLAUDE_CONFIG_DIR` vide affiche `loggedIn: false` et un `configDirectory` et un `projectsDirectory` qui suivent la variable, et aucun `.credentials.json` n'apparaît dans ce dossier.
- [ ] Même vérification avec un `CODEX_HOME` vide (`codex login status`, pas d'`auth.json`, en notant la valeur de `cli_auth_credentials_store`) et avec un `GH_CONFIG_DIR` vide (`gh auth status` avec `GH_TOKEN` et `GITHUB_TOKEN` retirés, pas de `hosts.yml`), plus le constat qu'un `GITHUB_TOKEN` défini passe devant `GH_CONFIG_DIR`.
- [ ] Le helper d'identifiants de git configuré pour github.com est relevé, et la documentation dit quand `git push` suit `GH_CONFIG_DIR` (HTTPS avec `gh auth git-credential`) et quand il ne la suit pas (SSH, autre helper).
- [ ] Le rapport, preuve locale non versionnée sous `tasks/`, note les versions de Claude Code, Codex et gh, la version de Claude Code vérifiée, et dit explicitement que la coexistence de deux comptes connectés n'est pas mesurée et repose sur la documentation des CLIs.
- [ ] Échec : given une CLI dont le login reste visible sous un dossier vide, when le spike conclut, then le PRD passe en version suivante avec la limite documentée dans US-016 et l'avertissement correspondant dans Settings, au lieu de promettre l'isolation pour cette CLI.

#### US-016: Injecter l'env de l'espace dans chaque terminal
**Description:** As a développeur multi-contextes, I want définir une env par espace et que chaque terminal de ses workspaces la reçoive so that un agent lancé dans l'espace pro utilise mes comptes pro.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010, US-015

**Acceptance Criteria:**
- [ ] `SpaceConfig` gagne `env: BTreeMap<String, String>`, édité dans Settings > Spaces avec les lignes d'env des profils (`EnvRowInputs`, `src-app/src/settings/tabs/agents.rs:57`) et étendu (`~`, `$VAR`) comme les profils d'agent (`src-app/src/agent_launcher.rs:452-467`).
- [ ] `HostedLaunch` (`src-app/src/terminal/view.rs:746`) porte l'env résolue de l'espace du workspace, et `resolve_spawn_launch_in` (`src-app/src/terminal/pty_session/spawn_env.rs:343`) fusionne global < espace < surface, l'env d'un profil d'agent arrivant comme env de surface.
- [ ] Settings et le loader refusent une clé invalide (`is_valid_env_name`), interdite (`is_forbidden_child_env_key`), protégée (`PROTECTED_ENV_KEYS`, `spawn_env.rs:15-27`) ou préfixée `PANEFLOW_`, avec la raison en ligne ; sous Windows les clés sont mises en majuscules comme aujourd'hui (`spawn_env.rs:244`).
- [ ] Quand l'env définit `CLAUDE_CONFIG_DIR`, `CODEX_HOME` ou `GH_CONFIG_DIR` vers un chemin absolu absent, Paneflow crée le dossier hors du thread de rendu avant le spawn, en mode 0700 sous Unix ; un chemin relatif est refusé dans Settings.
- [ ] Settings avertit sous l'espace quand l'env de Paneflow contient `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `OPENAI_API_KEY`, `GH_TOKEN` ou `GITHUB_TOKEN` sans que l'espace la redéfinisse, puisque ces variables passent devant le compte du dossier.
- [ ] Une valeur dont la clé contient `KEY`, `TOKEN`, `SECRET` ou `PASSWORD` affiche l'avertissement « Stored in plain text in paneflow.json; prefer a config directory ».
- [ ] Déplacer vers un espace d'env différente un workspace qui a des terminaux vivants affiche le toast « Open terminals keep their previous environment » ; aucun terminal n'est relancé.
- [ ] L'env injectée est persistée dans le manifest du host (`crates/paneflow-host/src/manifest.rs:31`), si bien qu'un rattachement garde l'env du lancement.
- [ ] Échec : given un dossier impossible à créer (droits), when un terminal se lance, then le terminal démarre quand même, un toast nomme le dossier et l'erreur, et le log garde le détail.
- [ ] `docs/user/settings.md` et `docs/user/configuration.md` documentent l'env d'espace, sa précédence, et le fait qu'un shell qui exporte la même variable dans son rc la remplace.

#### US-017: Installer les hooks et le MCP dans les dossiers de l'espace
**Description:** As a développeur multi-contextes, I want que Paneflow installe ses hooks dans le dossier de config d'agent de chaque espace so that l'état réflexion et attente de mes agents fonctionne dans tous mes espaces.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] `paneflow-mcp-install` expose une installation à partir de chemins explicites, construits par `ClaudePaths::from_env` et `codex_home_from` (`crates/paneflow-agent-config/src/io.rs:27`, `:60`) sur l'env de l'espace ; `ConfigPaths::resolve` (`crates/paneflow-mcp-install/src/integrations.rs:99`) n'est plus le seul point d'entrée.
- [ ] Les marqueurs d'un espace vivent sous `integrations/spaces/<space-id>/<slug>.json` dans le home Paneflow, sans toucher aux marqueurs actuels.
- [ ] Enregistrer un espace dont l'env définit `CLAUDE_CONFIG_DIR` ou `CODEX_HOME` installe, hors du thread de rendu, les intégrations des runtimes déjà installés pour l'espace par défaut.
- [ ] Au lancement, `adopt_and_refresh_installed` s'exécute aussi pour chaque dossier d'espace distinct du dossier par défaut, chaque dossier n'étant traité qu'une fois.
- [ ] Settings > Spaces affiche par espace l'état de chaque intégration (`state_label`, `integrations.rs:607`), avec un bouton « Retry » en cas d'erreur.
- [ ] Supprimer un espace ne retire pas les hooks de son dossier ; la documentation indique comment les retirer.
- [ ] Given un agent Claude Code lancé dans un espace à `CLAUDE_CONFIG_DIR` propre, when il réfléchit puis attend, then sa ligne de sidebar et son badge d'espace passent en réflexion puis en attente, et un test d'intégration le vérifie avec un dossier temporaire.
- [ ] Échec : given un dossier de config en lecture seule, when l'installation tourne, then l'état passe en erreur avec la cause, l'app reste utilisable et « Retry » relance l'installation.

#### US-018: Faire suivre l'env de l'espace aux sous-processus de l'app
**Description:** As a développeur multi-contextes, I want que le badge PR, le clone, git et l'auto-nommage tournent sous les comptes de l'espace concerné so that aucune donnée pro ne passe par mon compte perso.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] Une liste blanche unique couvre `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `GIT_COMMITTER_NAME`, `GIT_COMMITTER_EMAIL`, `GIT_CONFIG_GLOBAL`, `GIT_SSH_COMMAND`, `SSH_AUTH_SOCK`, `GH_CONFIG_DIR`, `GH_TOKEN`, `GITHUB_TOKEN`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `ANTHROPIC_API_KEY` et `OPENAI_API_KEY`.
- [ ] La recherche de PR par `gh` (`src-app/src/app/pull_request.rs:178`) reçoit la liste blanche de l'espace du workspace dont le dépôt est sondé.
- [ ] Le clone (`src-app/src/app/clone_repo.rs:253`, `:364`) reçoit la liste blanche de l'espace actif au moment du clone.
- [ ] `git_command` (`src-app/src/git_command.rs:74`) applique la liste blanche de l'espace du workspace concerné, après le retrait existant d'`INHERITED_REPOSITORY_ENV`.
- [ ] `summarizer_environment` (`src-app/src/auto_naming.rs:192`) applique la liste blanche de l'espace du workspace dont le terminal est résumé, en gardant la liste fermée de Codex (`CODEX_ALLOWED`).
- [ ] Un test par chemin vérifie qu'une variable de l'espace arrive au sous-processus et qu'une variable hors liste blanche n'y arrive pas.
- [ ] Échec : given un workspace de l'espace par défaut, when un de ces sous-processus se lance, then il reçoit exactement l'env d'aujourd'hui.

#### US-019: Retrouver et reprendre les sessions sous le compte de l'espace
**Description:** As a développeur multi-contextes, I want que « Resume Ended Sessions » trouve les conversations de chaque espace et les reprenne sous le même compte so that reprendre une session pro ne la cherche pas dans mon compte perso.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] `project_dir_for_cwd` (`src-app/src/claude_sessions.rs:38`) reçoit les `ClaudePaths` résolus sur l'env de l'espace du workspace, au lieu de `ClaudePaths::current()`.
- [ ] La reprise (`src-app/src/app/conversation_restore.rs:393-400`) lance la nouvelle surface avec l'env de lancement de la surface d'origine quand elle est connue, profil d'agent compris, au lieu de `env: None`.
- [ ] Given deux espaces dont les `CLAUDE_CONFIG_DIR` contiennent chacun une session pour le même dossier, when l'utilisateur ouvre « Resume Ended Sessions » dans chaque espace, then chaque liste ne montre que les sessions de son espace.
- [ ] Échec : given une session dont le dossier de config a été supprimé, when l'utilisateur la reprend, then la reprise échoue avec le message de l'agent dans le pane, sans retomber silencieusement sur le compte par défaut.

---

### EP-006: Vérifier sur le matériel réel

Vérifier sur Windows réel ce que Linux ne peut pas compiler ni exercer, release par release.

**Definition of Done:** US-020 vérifiée avant la release 1, US-021 avant la release 2, chaque défaut trouvé corrigé ou suivi par une issue.

#### US-020: Vérifier les espaces sur Windows réel (release 1)
**Description:** As a mainteneur, I want vérifier les espaces sur Windows 11 so that la release 1 ne livre pas de raccourci ni de chemin Windows cassé.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-009, US-012, US-014

**Acceptance Criteria:**
- [ ] Sur Windows 11 en dual boot, avec un clavier AZERTY : `ctrl-alt-pageup` et `ctrl-alt-pagedown` changent d'espace et ne produisent aucun caractère AltGr dans le terminal.
- [ ] Le sélecteur, le déplacement, la suppression d'un espace et la restauration après redémarrage se comportent comme sous Linux.
- [ ] `paneflow new --space` fonctionne sur le pipe nommé.
- [ ] Échec : given un défaut propre à Windows, when la vérification le trouve, then il est corrigé dans la release ou suivi par une issue citée dans la PR, et la release 1 n'est pas taguée avant.

#### US-021: Vérifier l'env et les comptes sur Windows réel (release 2)
**Description:** As a mainteneur, I want vérifier l'env par espace et l'installation des hooks sur Windows 11 so that la promesse des comptes séparés tienne sur les trois OS livrés.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-017, US-018, US-019

**Acceptance Criteria:**
- [ ] Avec le compte par défaut connecté, un terminal d'un espace dont le `CLAUDE_CONFIG_DIR` pointe vers un dossier vide montre `loggedIn: false` dans `claude auth status`, avec un `configDirectory` qui suit la variable, et aucun `.credentials.json` n'apparaît dans ce dossier ; même vérification pour `CODEX_HOME` avec `codex login status` et `auth.json`. La coexistence de deux comptes connectés n'est pas mesurée et repose sur la documentation des CLIs.
- [ ] Les hooks s'installent dans les dossiers des espaces, et l'état réflexion et attente s'affiche pour Claude Code et Codex dans chaque espace.
- [ ] Les clés d'env saisies en minuscules arrivent en majuscules dans le terminal, et `CODEX_HOME` est créé avant le lancement de Codex.
- [ ] La documentation indique que `SSH_AUTH_SOCK` n'a pas d'effet sous Windows, où l'agent OpenSSH passe par un pipe nommé.
- [ ] Échec : given un défaut propre à Windows, when la vérification le trouve, then il est corrigé ou suivi par une issue, et la release 2 n'est pas taguée avant.

## Functional Requirements

- FR-01: Le système doit permettre de déclarer jusqu'à 9 espaces nommés, plus l'espace par défaut implicite « Default ».
- FR-02: Chaque workspace doit appartenir à exactement un espace, l'espace par défaut quand aucun n'est assigné.
- FR-03: Quand au moins un espace est déclaré, la sidebar ne doit lister que les workspaces de l'espace actif.
- FR-04: Le workspace affiché doit toujours appartenir à l'espace actif, ou aucun workspace ne doit être affiché si l'espace actif est vide.
- FR-05: Quand l'utilisateur change d'espace, le système doit afficher le dernier workspace affiché de cet espace s'il existe encore.
- FR-06: Tout workspace créé sans espace explicite doit rejoindre l'espace actif.
- FR-07: Activer un workspace d'un autre espace depuis la file d'attention, la palette, l'IPC ou l'ouverture d'un dossier déjà ouvert doit basculer sur son espace.
- FR-08: Un espace caché doit afficher un badge tant qu'un de ses agents attend une entrée, a échoué ou a fini sans être vu.
- FR-09: Supprimer un espace doit déplacer ses workspaces dans l'espace par défaut, sans fermer de terminal.
- FR-10: L'appartenance et l'espace actif doivent survivre à un redémarrage ; une référence à un espace disparu doit retomber sur l'espace par défaut.
- FR-11: `workspace.list` et `workspace.current` doivent exposer l'espace ; `workspace.create`, `workspace.up` et la CLI doivent accepter un espace cible.
- FR-12: En release 2, chaque terminal d'un workspace doit recevoir l'env de son espace, avec la précédence global < espace < profil d'agent < surface.
- FR-13: En release 2, quand l'env d'un espace définit `CLAUDE_CONFIG_DIR` ou `CODEX_HOME`, le système doit installer ses hooks et son MCP dans ce dossier et en afficher l'état.
- FR-14: En release 2, le badge PR, le clone, git, l'auto-nommage et la découverte des sessions doivent recevoir la liste blanche de l'env de l'espace concerné.
- FR-15: Le système ne doit jamais relancer un terminal vivant à cause d'un changement d'espace ou d'env.
- FR-16: Le système ne doit jamais présenter un espace comme une frontière de sécurité ; le MCP et `write_pane` gardent leur portée actuelle.
- FR-17: Sans espace déclaré, l'interface, la navigation et les sorties IPC (hors champ `space: null`) doivent rester identiques à la version précédente.

## Non-Functional Requirements

- **Performance:** un changement d'espace coûte exactement 1 rendu de sidebar (`sidebar_renders`) et 0 processus git au-delà de la sonde du workspace affiché ; `sidebar_renders` et `git_spawns` du scénario `desktop_tui_30_workspaces` restent dans leurs budgets actuels avec les workspaces répartis sur 3 espaces ; 0 lecture ou écriture de fichier synchrone ajoutée sur le thread de rendu.
- **Security:** les dossiers créés pour un espace le sont en mode 0700 sous Unix ; 100 % des clés interdites ou protégées (`is_forbidden_child_env_key`, `PROTECTED_ENV_KEYS`, préfixe `PANEFLOW_`) sont refusées par Settings et par le loader ; les permissions de `paneflow.json` restent celles d'aujourd'hui ; un avertissement s'affiche pour 100 % des valeurs dont la clé contient `KEY`, `TOKEN`, `SECRET` ou `PASSWORD`.
- **Accessibility:** chaque bouton du sélecteur a un rôle bouton et un libellé accessible qui donne le nom, l'état actif et le nombre d'agents en attente ; le changement d'espace se fait entièrement au clavier (2 raccourcis par plateforme et la palette) ; aucune animation n'est ajoutée, Reduce motion n'a donc rien à couper.
- **Scalability:** 9 espaces et 100 workspaces s'affichent sans chevauchement à la largeur minimale de la sidebar ; la table des derniers workspaces par espace reste en mémoire et compte au plus 10 entrées.
- **Reliability:** une référence orpheline (`space` ou `active_space`) ne provoque ni panique ni corruption signalée, dans 100 % des cas couverts par les tests de restauration ; un `session.json` écrit par cette version se lit par la version précédente ; un échec d'installation des hooks ou de création de dossier laisse l'app utilisable et propose « Retry ».

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Aucun espace déclaré | Installation existante, premier lancement de cette version | Interface identique à aujourd'hui, sélecteur masqué | None |
| 2 | Espace actif vide | Nouvel espace, ou dernier workspace déplacé ou fermé | Zone principale et sidebar en état vide, bouton d'ouverture de dossier | "No workspaces in {space}" |
| 3 | Installation des hooks en cours | Enregistrement d'un espace à `CLAUDE_CONFIG_DIR` ou `CODEX_HOME` | État « Installing » par intégration dans Settings, travail hors du thread de rendu | None |
| 4 | Installation des hooks impossible | Dossier en lecture seule ou disque plein | État d'erreur avec la cause, bouton « Retry », app utilisable | "Could not install hooks in {dir}: {error}" |
| 5 | Dossier de config impossible à créer | Chemin absolu sous un dossier sans droit d'écriture | Le terminal démarre quand même, toast avec le dossier et l'erreur | "Could not create {dir}: {error}" |
| 6 | Espace supprimé à la main dans `paneflow.json` | Édition pendant que l'app tourne | Ses workspaces passent dans l'espace par défaut, qui devient actif si besoin | None |
| 7 | Référence orpheline au démarrage | `session.json` qui cite un espace disparu | Retour à l'espace par défaut, un `info!` compte les références | None |
| 8 | Dixième espace | Création depuis le sélecteur, Settings ou « New space… » | Création refusée | "9 spaces maximum" |
| 9 | Nom invalide | Vide, en double, « Default » ou plus de 32 caractères | Erreur en ligne, rien n'est écrit | "Name already used" / "Name required" / "32 characters maximum" |
| 10 | Workspace déplacé avec des terminaux vivants (release 2) | « Move to space » vers un espace d'env différente | Aucun terminal relancé, nouveaux terminaux sous la nouvelle env | "Open terminals keep their previous environment" |
| 11 | Identifiant hérité qui écrase le compte de l'espace (release 2) | `ANTHROPIC_API_KEY` ou `GH_TOKEN` dans l'env de Paneflow | Avertissement sous l'espace dans Settings | "{VAR} is set in Paneflow's environment and overrides this space's account" |
| 12 | Agent en attente dans un espace caché | Événement d'état d'un agent | Badge sur l'espace, notification qui nomme l'espace | "{agent} is waiting in {space} · {workspace}" |
| 13 | Saut vers un pane fermé entre-temps | Activation d'une ligne obsolète de la file | Pas de changement d'espace, file rafraîchie | None |
| 14 | Dossier déjà ouvert dans un autre espace | Ouverture du même dossier | Bascule sur l'espace et le workspace existants, pas de doublon | None |
| 15 | Fenêtre détachée d'un workspace caché | Changement d'espace | La fenêtre reste ouverte ; la rattacher bascule sur son espace | None |
| 16 | Shell qui exporte la même variable (release 2) | `CLAUDE_CONFIG_DIR` exporté dans `.zshrc` | La valeur du shell gagne, comportement documenté | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Une version de Claude Code, Codex ou gh partage la connexion entre deux dossiers | Med | High | Spike US-015 avant toute promesse, version minimale documentée, état des hooks visible par espace |
| 2 | Une variable d'identifiant héritée écrase le compte de l'espace sans que l'utilisateur le voie | Med | High | Avertissement dans Settings (US-016), précédence documentée |
| 3 | L'utilisateur prend un espace pour une frontière de sécurité | Med | Med | `docs/user/scripting.md` et Settings le disent explicitement ; MCP et `write_pane` inchangés (US-014) |
| 4 | Un chemin d'activation laisse `active_idx` hors de l'espace actif | High | Med | Accès centralisé par `active_workspace()`, bascule implicite unique dans `activate_workspace_at`, un test par chemin (US-004) |
| 5 | Peu d'utilisateurs créent un second espace | Med | Low | Fonction invisible sans espace déclaré, coût de performance nul mesuré par `perf_gates` |
| 6 | Raccourcis capturés par le bureau, l'AltGr ou absents des claviers sans PgUp | Med | Low | Raccourcis par plateforme remappables, palette, sélecteur cliquable, vérification AZERTY (US-020) |
| 7 | Secrets en clair dans `paneflow.json` | Med | Med | Avertissement par valeur sensible, doc qui recommande les dossiers de config ; même stockage que les profils d'agent aujourd'hui |
| 8 | Hooks écrits dans un dossier de config que l'utilisateur n'attendait pas | Low | Med | Installation seulement quand l'env de l'espace désigne le dossier et pour les runtimes déjà installés, marqueurs par espace, état visible |
| 9 | Le chantier resource-efficiency en cours change la sidebar sous EP-002 | Med | Med | EP-002 démarre après la livraison de `SidebarView` en cache et du compteur `sidebar_renders` |

## Non-Goals

What this version explicitly does NOT include:

- Pas d'entité « profil » partagée entre plusieurs espaces comme dans Arc : l'env vit sur l'espace. À revoir si des utilisateurs demandent la même env pour plusieurs espaces.
- Pas de règles de routage façon Air Traffic Control (assigner un workspace à un espace selon son chemin ou son dépôt) : l'espace actif suffit en v1.
- Pas de choix d'icône ni d'emoji : la première lettre du nom et une teinte du thème.
- Pas de recoloration de la fenêtre ni du thème par espace, contrairement à Arc : DESIGN.md réserve la teinte de ligne à l'élément actif.
- Pas de frontière de sécurité : le MCP (`PANEFLOW_MCP_SCOPE=all`) et `write_pane` restent valables sur toute l'instance.
- Pas de relance automatique des terminaux quand l'env change.
- Pas de clic de notification qui saute vers l'agent : ce manque existe déjà sans les espaces et se traite à part.
- Pas d'export, d'import ni de synchronisation des espaces entre machines.
- Pas de fenêtre principale par espace.
- Pas de renommage de l'espace par défaut en v1 (Open Questions).

## Files NOT to Modify

- `native/libghostty/` et `src-app/src/terminal/types.rs` : frontière du moteur de terminal, sans rapport avec les espaces.
- `crates/paneflow-shim/`, `crates/paneflow-ai-hook/` et `crates/paneflow-mcp/` : plafonds de taille, et le hook hérite déjà de l'env du pane ; la portée du MCP ne change pas.
- `crates/paneflow-host/src/env.rs` : les listes de clés interdites et héritées ne s'assouplissent pas ; les espaces les réutilisent.
- `src-app/Cargo.toml` : les quatre `rev` de GPUI restent épinglés.
- `crates/paneflow-config/src/schema/session.rs` : `SESSION_SCHEMA_VERSION` et `DetachedPaneLocation` ne changent pas, seuls des champs additifs s'ajoutent.
- `src-app/src/app/workspace_ops/git_watch.rs` : le plan de sonde du seul workspace affiché couvre déjà les espaces cachés.

## Technical Considerations

- **Architecture:** espace en métadonnée de chaque workspace dans une liste plate, plutôt qu'un vecteur de workspaces par espace ; recommandé pour garder les index globaux de l'IPC, de la file d'attention et de `DetachedPaneLocation`. Engineering to confirm qu'aucun autre appelant de `compute_display_order` n'attend l'ordre global.
- **Data Model:** définitions dans `paneflow.json`, appartenance et espace actif dans `session.json`. `config_writer` sait-il réécrire un tableau d'objets comme `spaces` avec le même helper qu'`agent_profiles`, ou faut-il un helper dédié ?
- **API Design:** paramètre `space` par nom ou id sur `workspace.create` et `workspace.up`, sans nouvel espace de noms `space.*`. Faut-il un `space.list` pour les conducteurs, ou l'objet `space` de `workspace.list` suffit-il ?
- **Dependencies:** aucune nouvelle dépendance. L'installation par chemins explicites réutilise `install_integration_at` ; reste-t-il ailleurs dans `paneflow-mcp-install` un appel à `ClaudePaths::current()` à rendre paramétrable (`crates/paneflow-mcp-install/src/agents/support.rs:13`) ?
- **Migration:** aucune migration ; compatibilité ascendante et descendante de `session.json` par champs optionnels. Rollback : une version précédente ignore `space` et `active_space` et montre tous les workspaces dans une seule liste.
- **Empty space:** faut-il conserver `active_idx: usize` avec un accès filtré, ou passer à `Option<usize>` pour rendre l'espace vide explicite dans les types ? Le PRD recommande l'accès filtré, moins invasif.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Utilisateurs qui confirment utiliser les espaces | N/A (new) | Olivier et au moins 2 autres utilisateurs | Month-1 après la release 1 | Retours Discord et GitHub, relevé manuel |
| Issues « statut d'agent absent dans un espace » | N/A (new) | 0 | Month-1 après la release 2 | Issues GitHub étiquetées `spaces` |
| `sidebar_renders` sur 30 s de sortie, 30 workspaces sur 3 espaces | Budget actuel du scénario `desktop_tui_30_workspaces` | Inchangé | Month-1 | Job `Performance gates (Linux x86_64)` |
| Rendus de sidebar par changement d'espace | N/A (new) | 1 | Month-1 | `system.counters`, test dédié |
| Chemins de sous-processus sous l'env de l'espace (release 2) | 0 sur 5 | 5 sur 5 | Month-1 après la release 2 | Tests d'US-018 et US-019 |

## Open Questions

- Olivier veut-il surtout séparer visuellement, ou aussi les comptes ? Arthur, après la release 1 ; la réponse décide du lancement de la release 2.
- L'espace par défaut doit-il être renommable (« Perso ») ? Arthur, avant US-007 ; dépend de la passe visuelle.
- `ctrl-alt-pageup` et `ctrl-alt-pagedown` restent-ils utilisables sur un portable sans touches PgUp et PgDn dédiées ? Arthur, pendant US-020.
- Un Mac est-il disponible pour vérifier l'isolation Keychain de `CLAUDE_CONFIG_DIR` avant la release 2 ? Sinon macOS repose sur la doc officielle. Arthur, avant US-021.
- Quelle version minimale de Claude Code documenter ? Fixée par US-015.
- Le nom de l'espace doit-il apparaître dans la zone centrale de la barre de titre (DESIGN.md, point et nom du workspace) ? Arthur, après la passe visuelle d'US-007.
[/PRD]
