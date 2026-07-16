# Diagnostic - non-hooking d'un agent lancé via `send "claude"` (EP-002 US-004)

> Spike du hardening du control plane agent. Statut : analyse code
> complète + instrumentation livrée ; US-005 a ensuite corrigé la cause la plus
> dangereuse : un hook persistant Paneflow stale ne supprime plus l'injection
> project-local.

## Symptôme

Un agent démarré via `paneflow send <shell> "claude" --submit` (dans un shell nu)
apparaît `state: unknown_running` / `hooked:false` dans `fleet.list`, n'émet
jamais d'`ai.stop`, n'a pas de `last_result`. Un agent démarré via `paneflow up`
(agent=claude) est hooké. Pourquoi la différence ?

## La chaîne « un agent est-il hooké ? » (6 maillons, file:line)

1. **`paneflow up`** (`src-app/src/cli/up_cmd.rs:64-103`, `411-430`) : lance le
   binaire agent comme **enfant direct** de la pane PTY. Ne crée **PAS** de
   session proactivement (`ai.session_start` est un no-op : `ipc_handler.rs`
   `let _ = (pid, tool, ws)`). La session naît au 1er `ai.prompt_submit`.
2. **`send "claude"`** (`src-app/src/cli/send_cmd.rs`) : écrit `claude\r` dans un
   shell existant. L'agent est enfant du **shell**, pas de la pane. Pas de
   session proactive non plus.
3. **Install du hook (shim)** : `HookConfigGuard::install()`
   (`crates/paneflow-shim/src/hooks.rs:389`) résout `.claude/` contre
   `env::current_dir()` puis `install_at`. **Branches `None` :**
   `current_dir()` échoue (`:390`) ; **IPC injoignable** (`:405`) ; **persistent
   paneflow hook présent dans `~/.claude/settings.json`** (`:421`, skip volontaire
   EP-004 US-018) ; `.claude` symlinké / non-dir / cwd non-writable / write fail
   (dans `install_at`/`install_hook_config_file`).
4. **Hook -> app** : `paneflow-ai-hook` a besoin de `PANEFLOW_SOCKET_PATH` +
   `PANEFLOW_WORKSPACE_ID`, **injectés au niveau PTY de la pane**
   (`pty_session.rs` `assemble_pty_env`), donc **hérités par tout enfant**, y
   compris un `claude` lancé via `send`. **L'env n'est PAS le différenciateur.**
5. **Création de session par event** : `ai.prompt_submit` -> `upsert_session_state`
   (`ipc_handler.rs`). Crée une row si le `workspace_id` résout et le tool parse.
   Clé = PID (pas de session_id requis).
6. **Dérivation `hooked`** : `build_fleet_rows` (`ipc_handler.rs:792`) : `hooked:true`
   depuis `ws.sessions` (sessions trackées) ; `hooked:false` + `unknown_running`
   depuis le **scan /proc** `ws.detected`. `surface.status` ne fabrique pas
   thinking/idle depuis le scan.

**Conclusion mécanique** : `unknown_running`/`hooked:false` signifie qu'**aucune
AgentSession n'existe pour ce PID** -> le hook n'a jamais délivré d'`ai.prompt_submit`.
Ce n'est PAS l'env (hérité correctement) ni la résolution de surface (qui donnerait
`hooked:true` sans surface). C'est donc **l'install du hook qui a retourné `None`**
(maillon 3).

## Causes racines rangées

1. **Skip « persistent hooks » silencieux** (`hooks.rs:421-435`). `install()`
   skip l'injection projet-local si `~/.claude/settings.json` contient déjà un
   hook **paneflow-managed**. Si ce hook persistant est **périmé** (pointe un
   binaire déplacé/absent) ou ne joint pas le socket, l'agent reste hooké à
   `false` en permanence. **C'était la seule branche `None` sans aucun
   diagnostic** (corrigé, voir ci-dessous). `up` et `send` seraient affectés
   identiquement ici.
2. **Divergence de cwd** (`hooks.rs:390`). `up` donne un cwd canonique writable
   (depuis la spec) ; un `send` dans un shell pré-existant utilise le cwd où le
   shell a navigué : symlinké -> `None`, non-writable -> `None`. L'install
   échoue pour `send` alors qu'elle réussissait pour `up`.
3. **IPC injoignable** (`hooks.rs:405`) : déjà loggé via le `ipc_reachable` de
   `main.rs`. Peu probable (le socket est joignable dans la trace de démo).

## Instrumentation livrée (US-004)

`diagnose()` est désormais `pub(crate)` (`shim/main.rs`), et **chaque branche
`None` de `HookConfigGuard::install()` se nomme** dans `$PANEFLOW_HOOK_LOG`
(`hooks.rs:389`) :
- `claude: hook install skipped - current_dir() failed: …`
- `claude: hook install skipped - no Paneflow IPC socket reachable …`
- `claude: project-local hook install suppressed - a Paneflow-managed persistent hook in ~/.claude/settings.json takes precedence …`
- `claude: hook install_at(<path>) returned None - filesystem refused …`

Couplé au `install_hook_guard(tool) = installed/None; ipc_reachable = …` déjà
émis par `main.rs`, le log montre exactement **quel** maillon casse.

## Procédure live (à exécuter par Arthur)

```bash
export PANEFLOW_HOOK_LOG=/tmp/pf-hook.log
: > "$PANEFLOW_HOOK_LOG"
# 1) cas qui échoue
paneflow send <shell-pane> "claude" --submit
# 2) cas qui marche
paneflow up   # (agent=claude) dans un workspace.toml
# 3) comparer
grep claude /tmp/pf-hook.log
cat ~/.claude/settings.json | jq .hooks   # cause #1 : un hook paneflow y est-il présent ?
```

Lire la ligne `install_hook_guard(claude) = …` + la raison nommée. La ligne
discriminante :
- `… persistent hook … takes precedence` -> **cause #1** (vérifier/supprimer le
  hook persistant périmé, ou réparer son chemin).
- `… install_at(<path>) returned None …` -> **cause #2** (cwd non-writable /
  symlink ; voir `<path>`).
- aucune ligne `claude:` mais `install_hook_guard(claude) = installed` -> le hook
  s'installe ; chercher ailleurs (le hook ne joint pas le socket : voir le log de
  `paneflow-ai-hook` sous le même `PANEFLOW_HOOK_LOG`).

## Orientation US-005 (fix livré)

- **Cause #1 (persistent skip)** : corrigée. Le shim ne skippe l'injection
  project-local que si le hook persistant est **vérifié vivant** (binaire
  existe). Si le hook persistant pointe vers un binaire absent, il est traité
  stale et le shim retombe sur l'install projet-local. Le skip vivant reste en
  place pour éviter le double-fire EP-004 US-018.
- **Cause #2 (cwd)** : installer depuis un emplacement stable n'est PAS trivial -
  Claude Code lit `.claude/` depuis **son** cwd ; le seul fallback "stable" est
  `~/.claude/settings.json` (user-global, invasif : affecte toutes les sessions).
  À trancher selon la fréquence réelle.
- **Quoi qu'il en soit (livré)** : tout échec d'install est désormais **journalisé
  avec sa raison** (US-005 AC2, « jamais silencieux »), et un agent non-hooké est
  exposé `hooked:false` + `reason:"no_hook"` (US-006), donc « pas un état ambigu »
  (US-005 AC3). La vérification live reste utile pour valider l'environnement
  d'Arthur, mais le correctif de précédence n'est plus déféré.
