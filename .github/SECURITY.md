# Security Policy

## Supported versions

Security fixes land on the latest release. Always run the most recent version
from the [releases page](https://github.com/arthjean/paneflow/releases/latest)
or your package manager.

## Reporting a vulnerability

**Please do not open a public issue for security reports.**

Email **arthur.jean@strivex.fr** with:

- a description of the issue and its impact,
- steps to reproduce (or a proof of concept),
- the Paneflow version (`paneflow --version`), OS, and display server
  (Wayland / X11) where relevant.

You can expect an initial acknowledgement within 72 hours. Once a fix is ready,
a patched release is published and the report is credited unless you prefer to
stay anonymous.

## Scope

Areas most relevant to Paneflow's threat model:

- the **JSON-RPC IPC server** (Unix socket / named pipe) and any method it
  exposes,
- the **MCP bridge** (`list_panes` / `read_pane` / `search_pane`) and how pane
  output is wrapped as untrusted data,
- the **in-app updater** (download, signature verification, atomic install),
- PTY handling and any path where untrusted agent or terminal output reaches a
  privileged surface (for example OS notifications).

Verifying release artifact signatures is documented per platform in
[`docs/release/`](../docs/release).
