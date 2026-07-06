#!/usr/bin/env bash
# Choregraphie Linux/Wayland pour le GIF demo du README
# (cf. tasks/hn-launch-playbook.md §1.1).
#
# Usage :
#   1. Lance Paneflow avec une fenetre PROPRE (pas de workspaces persos visibles).
#   2. Demarre l'enregistrement (Kooha/OBS, region = fenetre Paneflow).
#   3. ./tasks/record-demo.sh /chemin/vers/un/repo/de/demo
#   4. Sur camera : Enter sur le prompt prerempli de Claude, idem Codex,
#      reponds a une question d'agent, Ctrl+Shift+J pour le jump.
#   5. Stoppe l'enregistrement, convertis (commande affichée à la fin).
#
# Les prompts sont PREREMPLIS, jamais soumis: c'est toi qui presses Enter
# (human-in-the-loop, c'est le pitch).

set -euo pipefail

DEMO_REPO="${1:?usage: record-demo.sh /path/to/demo/repo}"

if [[ ! -d "$DEMO_REPO" ]]; then
  echo "repo demo introuvable: $DEMO_REPO" >&2
  exit 1
fi

command -v paneflow >/dev/null || { echo "paneflow requis dans PATH"; exit 1; }

toml_string() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  printf '"%s"' "$value"
}

echo ">> ping"
paneflow ps >/dev/null

workspace_dir="$(mktemp -d "${TMPDIR:-/tmp}/paneflow-demo.XXXXXX")"
workspace_file="$workspace_dir/workspace.toml"
trap 'rm -rf "$workspace_dir"' EXIT

cat >"$workspace_file" <<EOF
name = "demo"
layout = "even_h"

[[panes]]
name = "claude"
cwd = $(toml_string "$DEMO_REPO")
agent = "claude"
prompt = "Add a --json flag to the export command, with tests"
focus = true

[[panes]]
name = "codex"
cwd = $(toml_string "$DEMO_REPO")
agent = "codex"
prompt = "Profile the startup path and list the 3 slowest spans"
EOF

echo ">> workspace demo + panes agents (prompts preremplis, non soumis)"
paneflow up "$workspace_file"

cat <<'EOF'

Choregraphie lancee. A toi : Enter sur chaque prompt, laisse tourner,
reponds a une question, Ctrl+Shift+J, 2 s sur la vue d'ensemble, coupe.

Conversion (vise < 10 Mo) :
  gifski --fps 12 --quality 80 --width 960 -o assets/images/demo.gif capture.mp4
EOF
