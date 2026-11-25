# Coverage audit of docs/user against the macOS qualification (2026-11-25)

This matrix closes the macOS qualification in [../macos-20261124.md](../macos-20261124.md). It walks every page of `docs/user/`, section by section, lists each concrete claim a user can check (a behavior, a command, a key, a setting, an error), and gives it a verdict with its evidence. Blog posts are included only where they state current behavior that no other page states.

The replay ran on a debug build of `main` at `0cd67ce4` (code state `f9b79a3b`, every fix of the report included), on a fresh home `~/.paneflow-dev-cover` (`scripts/dev.sh --tag cover` layout: own IPC endpoint, host and worker, every inherited `PANEFLOW_*` cleared), on the same Apple M4 Mac, macOS 27.2.1. It was driven with the `paneflow` CLI, raw JSON-RPC, a fake agent binary named `amp` (the cli/06 recipe) and, for the window, `agent-desktop` on the dev window only: headless accessibility presses, physical keys, and right clicks by accessibility bounds. Every write into an agent configuration went to a throwaway `HOME`. The installed app and its home were never touched.

A second pass worked down the 132 rows still uncovered, on a debug build of `fd627b91` and a fresh home `~/.paneflow-dev-cover2` (same isolation, launched by `$SCRATCH/cover-fx/launch3.sh`). The window was driven with real mouse input on the dev window only (`agent-desktop --headed`), System Events keys, the `paneflow` CLI and raw JSON-RPC. Every agent alias (`claude`, `codex`, `gemini`, `opencode`, `amp` and the rest) resolved to a stand-in script under `$SCRATCH/cover-fx/sbin`: the app's `PATH` started there, and before any launcher was pressed a dry check showed that the launcher and Paneflow's shim resolved to the stand-in. No real agent binary ran. Its transcripts and screenshots are `final/51-*`, and every row it settled names `fd627b91`. Fixes that landed on `main` after `fd627b91` (from `347305f9` to `81b35db0`) were not replayed in that pass; the third pass below re-verified C8 and C15.

A third pass ran on a debug build of current `main` at `1825a6fc` (it includes `81b35db0` and `9cc22b78`), on a fresh home `~/.paneflow-dev-cover3`. The app, its host and its worker ran with `HOME` set to a throwaway directory, so the default Claude Code and Codex directories were throwaway too, and a space pointed `CLAUDE_CONFIG_DIR` and `CODEX_HOME` at other scratch directories. The real `~/.claude/settings.json`, the conductor skill, and the Paneflow parts of `~/.claude.json` (its MCP entries) were checksummed before and after and did not change; the Codex, Gemini, opencode and fx files stayed absent. Its transcripts and screenshots are `final/52-*`, and the rows it settled name `1825a6fc`.

## Verdicts

| Verdict | Meaning | Rows |
|---|---|---|
| `covered-final` | Observed on the final code: evidence in `final/` (the regression pass at `a330868a` and later, the mouse pass, this audit's `final/50-*`, and the second pass's `final/51-*` on `fd627b91`) or in `120-`, `121-`, `122-` | 376 |
| `covered-earlier` | Observed in the main pass, before the late fixes (`70126a19` onward); none of them touches the code path, or it was replayed here (then the row is `covered-final`) | 196 |
| `unavailable` | Cannot be exercised on this Mac, with the reason; rows the report did not already mark say "not marked by the report" | 65 |
| `fail` | Observed and does not match the page; "(docs)" when the product behaves sensibly and the page is wrong | 15 |
| `uncovered` | No evidence yet | 0 |
| Total | | 652 |

Evidence names are relative to [macos-20261124/](.). This audit's transcripts and screenshots are `final/50-*`; each transcript states its setup and ends with its verdict. Paths are shown with `~` for the home directory and `$SCRATCH` for the session scratchpad.

## Failures found by this audit

Every failure below was fixed after the audit, in the product or in the site docs; the Status column names the commit. Each fix carries a test that failed before it and a live check on a dev build (`tasks/evidence/macos-qualification-20261124/124-` to `133-`, copied next to the report).

| Id | Page | Failure | Evidence | Status |
|---|---|---|---|---|
| C1 | features.md, scripting/reference.md (`tmux.compat`) | With `ipc.scripting=true`, `tmux send-keys -t %1 Enter` (or `C-m`, or `'text' Enter`) from a Claude Code team is always refused with `text contains CR or LF; multiline surface.send_text requires active bracketed paste`: the shim maps Enter to CR and calls `surface.send_text` with `paste: false` and no submit, so a team can type into a teammate but never submit a line. `send-keys -l text` works | `final/50-tmux-send-keys-scripting.txt` | fixed `b06c1499` |
| C2 | scripting.md, scripting/reference.md (flow) | In a `foreach` fan-out of two panes with a `prompt`, the second pane's prompt is often never typed and its `ready` times out (5 of 9 runs; the first pane always passes) | `final/50-flows.txt` | fixed `40fe223a` |
| C3 | scripting/reference.md (flow) | A `send` step whose text holds a multiline capture, toward an agent pane without bracketed paste, aborts the whole flow as `instance unreachable: ... text contains CR or LF ...` (exit 1) instead of failing the step; the page's own example sends a 20-line `${summary}` | `final/50-flows.txt` | fixed `347305f9` |
| C4 | scripting/reference.md (flow) | Once in ten runs a fan-out pane failed with `paneflow error -32070: the layout op was refused: invalid` and no reason in any log; not reproduced afterwards | `final/50-flows.txt` | fixed `40fe223a` (cause) and `915888ad` (reason in the error) |
| C5 | index.md | (docs or product decision) "On first launch, Paneflow opens a workspace with a terminal pane": a fresh home opens the welcome screen with no workspace, also when `paneflow` runs from a project directory | `final/50-first-launch.txt` | docs fixed in the site (`93899df`), mirror synced |
| C6 | settings.md | (docs) The Settings map lists a Notifications page that does not exist (the switch is on General) and has no row for Editor and Usage | `final/50-settings-pages.txt` | docs fixed in the site (`93899df`), mirror synced |
| C7 | scripting.md | (docs) "one started by `paneflow actions run` acts as its caller": there is no `actions run`; `paneflow do <name> key=value` runs an action, with origin `cli` | `final/50-mcp-actions.txt` | docs fixed in the site (`93899df`), mirror synced |
| C8 | features.md (notifications) | With `notify_when_agent_waiting` set to `PrimaryScreen`, the first desktop notification opens a modal "Choose Application: Where is use_default?" dialog (the AppleScript path of mac-notification-sys), the window is frozen until Cancel, and nothing is delivered. The space button counts and "an error stops counting once seen" work. Fixed by `81b35db0`, re-verified on `1825a6fc`: one log warning that a debug build has no app bundle, no modal, the window keeps answering | `final/51-notifications-fail.txt`, `final/51-notification-choose-application.jpg`, `final/52-notifications-retest.txt`, `final/52-notifications-no-modal.jpg` | fixed `81b35db0`, re-tested in the third pass |
| C9 | features.md (send files and code) | Cmd+click on a path that matches two files of the project (`q/helpers.rs:3` with `p/q/helpers.rs` and `r/q/helpers.rs`, or a bare `twin.rs`) does nothing: no underline, no file finder, no tab. A unique suffix (`inner/uniq.rs:2`) opens at its line | `final/51-cmd-click-ambiguous.txt`, `final/51-cmd-click-ambiguous.jpg` | fixed `d902e72e` |
| C10 | configuration/schema.md (`option_as_meta`) | With `option_as_meta: true`, Option+B sends ESC b and also the composed `∫` (`cat -v` shows `^[b∫`, Option+F `^[fƒ`). Keys were synthesized through System Events, not a physical keyboard | `final/51-config-hot-reload.txt`, `final/51-option-as-meta.jpg` | fixed `605c202e`, and `bc528ed0` for Option+Shift+B |
| C11 | configuration/schema.md (`commands`) | A `commands[]` entry with only `name` and `command` appears nowhere: the command palette answers "No matching command" and Settings > Workspaces lists only templates; the schema calls these entries "Command palette entries". Custom command buttons (workspace menu > Manage custom buttons…) are a separate feature and work | `final/51-layouts-templates.txt` | fixed `55a3b0e7` |
| C12 | settings.md (Workspaces), configuration/schema.md (`commands`) | Launching a template from Settings > Workspaces runs it in an already open workspace of the same folder, replaces that workspace's active tab without asking (seven panes of `hover` in one run), creates no workspace named after `workspace.name`, and leaves the replaced sessions running unseen (29 host sessions for 21 panes afterwards) | `final/51-layouts-templates.txt`, `final/51-template-launch-stacked.jpg` | fixed `4cd65e2d` |
| C13 | worktrees.md (switch the project checkout) | The New branch form refuses to switch the checkout while an agent works there, but the Changes dock branch picker switched the same checkout to another branch at once, under a working agent, with no warning | `final/51-worktrees.txt`, `final/51-checkout-switch-guard.jpg` | fixed `e0f51cb2` |
| C14 | keybindings.md (`jump_next_waiting`) | The order is as documented (waiting, error, unseen finished with the most recent first, active space first, wraps), but a target in a tab that is not selected is never shown: the jump selects the workspace (and its space) and leaves the other tab in front, so the agent stays "not yet seen" and keeps coming back | `final/51-jump-next-waiting.txt`, `final/51-jump-next-waiting.jpg` | fixed `0eac70d8` |
| C15 | features.md (Claude Code (team)) | "Claude Code (team)" from the pane tab bar's + menu starts the lead without `PANEFLOW_TMUX_TEAM`, so every `tmux` call fails with "PANEFLOW_TMUX_TEAM is not set; this pane is not a Paneflow team pane". The same entry from the sidebar's New tab palette works. Fixed by `9cc22b78`, re-verified on `1825a6fc` (the tab bar entry gives the lead its token and a teammate opens) | `final/51-team-lead.txt`, `final/51-team-tabbar-no-token.jpg`, `final/52-team-tabbar-retest.txt` | fixed `9cc22b78`, re-tested in the third pass |
| C16 | configuration/schema.md (`window_backdrop`) | `PANEFLOW_WINDOW_BACKDROP=opaque` has no effect on macOS (chrome stays Blended with `macos_chrome_material: true`), while the schema says the variable overrides `window_backdrop` for one launch; the code reads it only on Windows. The config values behave as documented | `final/51-restart2.txt`, `final/51-backdrop-env-opaque.jpg`, `final/51-window-backdrop.jpg` | docs fixed in the site: the variable is Windows-only by decision D7 of `tasks/prd-unified-window-backdrop.md`; mirror synced |
| C17 | settings.md (Spaces, Agents), scripting.md, conductor.md | (docs) Settings > Spaces describes an **Agent setup** block with **Install** and **Retry**; the space page has an **Accounts** section instead: an account picker per agent, "Paneflow's hooks are not in this account yet" and **Add hooks**, which does install hooks, MCP and the skill into the space's directories. The pages also name a **Paneflow hooks** toggle in Settings > Agents; it is the **Hooks and MCP** switch in each agent's settings dialog | `final/52-agent-config-dirs.txt`, `final/52-space-accounts.jpg`, `final/52-hooks-toggle.jpg` | docs fixed in the site (`05be402`, `373ee60`), mirror synced |

Smaller observations, none of them contradicting a page: `paneflow host status` against a host whose 128 connections are taken answers `faulted` with `Broken pipe` and `serve status` answers `faulted`, rather than busy (`final/50-dropped-busy.txt`); a layout's surface `name` is not applied by `workspace.restore_layout` (`final/50-restore-layout.txt`); the palette's red sandbox refusal, the worktree removal dialog, and the pressed state of the dock's Unified and Split buttons are not exposed to accessibility (`final/50-sandbox.txt`, `final/50-worktree-tab-menu.txt`, `final/50-changes-dock.txt`); `terminal.scrollback_lines` trims whole pages, so 500 lines fit under a setting of 100 (`final/50-config-keys.txt`). The report's themes row says an unknown theme keeps the previous one; the product falls back to Paneflow Dark, as `themes.md` says (`final/50-themes.txt`).

Second pass observations, none of them contradicting a page: in the Markdown pane, find-next moved to match 2 without scrolling it into view (`final/51-markdown-config.txt`); the read-only file banner text is clipped; Open With is disabled when no editor is detected; the language server banner buttons, the custom button dialog, the New branch refusal and the Cmd+click file finder are not exposed to accessibility; rust-analyzer exited twice in the fixture and stayed stopped, as documented; `window_decorations: "server"` looks the same as `client` on macOS; the dev binary shows a generic icon in the Dock for the default app icon; the files tree's maximum width is capped by the window; Option+F moves by word only inside the search overlay; an agent row stays Error after its process ended; `workspace.create` ignores a `title=` key; Cmd+W in a team lead closes the whole tab, teammates included (with an Undo toast).

Third pass observations: a debug build installs nothing when a space is saved or `agent_prompts` changes, and says so in the log (`Settings > Spaces > Retry installs`, `paneflow integrations install <runtime> --force`); a space account appears in Settings > Usage only once its directory exists; a space that reuses another space's directory adds no account card and the card keeps the first space's name; after the agent setup ran, the space's Codex row stayed on "Checking <dir>..." (`final/52-agent-config-dirs.txt`).

## Matrix

### index.md

| page | claim | verdict | evidence |
|---|---|---|---|
| index#intro | Runs on macOS Apple Silicon | covered-earlier | report Environment (Apple M4, macOS 27.2.1), `100-dmg-provenance.txt` |
| index#intro | Runs on Linux x86_64/ARM64 and Windows x86_64 | unavailable | Linux and Windows: report: Unavailable, Windows and Linux |
| index#download-links | macOS DMG link for v0.20.0 resolves to the signed, notarized DMG | covered-earlier | `100-dmg-provenance.txt` |
| index#download-links | Homebrew `brew install --cask arthjean/paneflow/paneflow` | unavailable | report: Homebrew not installed, cask pin checked through the GitHub API (`100-dmg-provenance.txt`) |
| index#open-paneflow | Open from Applications, Spotlight or `paneflow` in a project directory | unavailable | not marked by the report: not run: launching the installed app would touch its live sessions; `paneflow` is not on PATH after install (`100-dmg-provenance.txt`) |
| index#open-paneflow | On first launch Paneflow opens a workspace with a terminal pane | fail | (docs) final/50-first-launch.txt: a first launch opens the welcome screen, no workspace or terminal |
| index#open-paneflow | No account or API key needed; telemetry off by default, only enabled with consent | covered-final | final/51-restart-settings.txt (no telemetry log line and no network socket with telemetry unset; no account or key asked by any flow of this audit) (fd627b91) |
| index#split-the-terminal-into-panes | Cmd+Shift+D / Cmd+Shift+E split, new pane shell in same cwd | covered-earlier | report row: Split (workspace.current readings); `final/36-split.jpg` |
| index#split-the-terminal-into-panes | Alt+Arrow moves focus; Cmd+Shift+N opens another workspace | covered-earlier | report row: Focus; `02-open-folder.jpg`, `57-new-workspace-open-panel.jpg` |
| index#start-an-installed-agent | `claude` runs in a pane and Paneflow shows its activity | covered-earlier | `13-changes-dock.jpg`, `67-waiting-agent-sidebar.jpg` |
| index#start-an-installed-agent | `codex` / `opencode` run in a pane | unavailable | not marked by the report: Codex and OpenCode detected but not signed in (report Environment) |
| index#review-the-changes | Changes dock: diff against HEAD with staged, unstaged and untracked files | covered-final | `final/40-mouse-pass.txt` section 10, `final/40-57-changes-view.jpg`; earlier `14-changes.jpg`, `89-dock-open.jpg` |
| index#review-the-changes | Files opens the integrated editor | covered-final | `final/40-mouse-pass.txt` section 9, `final/40-54-files-a-txt-open.jpg`, `121-ax-prompt-palette.txt` (3) |
| index#keep-sessions-running | Quit dialog Keep sessions running, reopen reconnects | covered-final | final/50-quit-keep-relaunch.txt; `46-quit-dialog.jpg`, `47-relaunch-reconnected.jpg`; `on_quit: keep` set from Settings in `final/40-mouse-pass.txt` section 3 |
| index#keep-sessions-running | Stop everything and quit ends the sessions | covered-final | final/50-stop-everything.txt; earlier: none (`46-quit-dialog.jpg` shows the choice only) |
| index#system-requirements | macOS 13 Ventura or later, Apple Silicon, Metal | covered-final | final/51-macos-minimum.txt (installed 0.20.0 declares LSMinimumSystemVersion 13.0, arm64 only, links Metal; a run on macOS 13 itself is unavailable) (fd627b91) |
| index#system-requirements | Linux Vulkan, Windows 10 1809+ DirectX 11 | unavailable | Linux and Windows: report: Unavailable |
| index#system-requirements | Windows: cmd.exe does not report cwd through OSC 7, use PowerShell or Git Bash | unavailable | Windows-only: report: Unavailable |

### installation.md

| page | claim | verdict | evidence |
|---|---|---|---|
| installation#choose-your-platform | macOS: Homebrew or signed and notarized DMG, no Intel build | covered-earlier | `100-dmg-provenance.txt`; Homebrew and Intel unavailable (report) |
| installation#choose-your-platform | Linux formats, Windows MSI and WSL2 | unavailable | Linux and Windows: report: Unavailable |
| installation#where-settings-and-sessions-are-stored | State lives in ~/.paneflow; PANEFLOW_HOME moves it | covered-final | every final pass runs on `PANEFLOW_HOME=~/.paneflow-dev-final` (`final/01-verbs-send-up-wait.txt` header); default ~/.paneflow held by the installed app (report Environment) |
| installation#where-settings-and-sessions-are-stored | Sessions run in a separate host and survive a quit while host and machine run | covered-final | `final/06-host-serve-status.txt`; quit and reconnect `46`, `47-*.jpg` (earlier) |

### installation/macos.md

| page | claim | verdict | evidence |
|---|---|---|---|
| installation/macos#homebrew | brew cask installs PaneFlow.app in /Applications | unavailable | report: Homebrew not installed |
| installation/macos#dmg | DMG named paneflow-<version>-aarch64-apple-darwin.dmg holds PaneFlow.app and an Applications link | covered-earlier | `100-dmg-provenance.txt` |
| installation/macos#open-paneflow | Open from Applications, Spotlight or `open -a PaneFlow`; first-open confirmation | unavailable | not marked by the report: not run on the installed app, which holds live sessions |
| installation/macos#verify | `/Applications/PaneFlow.app/Contents/MacOS/paneflow --version` prints the version | covered-earlier | `100-dmg-provenance.txt` (`paneflow 0.20.0`) |
| installation/macos#provenance | `gh attestation verify` with --signer-workflow succeeds; prints Verification succeeded! in a terminal | covered-earlier | `100-dmg-provenance.txt` (verified; gh prints no success line off a TTY, report notes it) |
| installation/macos#path | `paneflow` not on PATH after install | covered-earlier | `100-dmg-provenance.txt` |
| installation/macos#path | sudo mkdir and ln -s into /usr/local/bin, File exists check, PATH export | unavailable | report: sudo symlink not run |
| installation/macos#gatekeeper | Official DMG signed and notarized, accepted by Gatekeeper | covered-earlier | `100-dmg-provenance.txt`, report row: installed app codesign, spctl, stapler |
| installation/macos#gatekeeper | Open Anyway path for a blocked app | unavailable | not marked by the report: the notarized app is never blocked here |
| installation/macos#intel | No Intel build, DMG does not run on Intel | unavailable | report: no Intel Mac |

### installation/linux.md

| page | claim | verdict | evidence |
|---|---|---|---|
| installation/linux#formats | AppImage, .deb, .rpm, .tar.gz install, verify, attestation, PATH fixes | unavailable | Linux-only: report: Unavailable, Linux |

### installation/windows.md

| page | claim | verdict | evidence |
|---|---|---|---|
| installation/windows#msi | MSI install with PATH option, signature check, attestation | unavailable | Windows-only: report: Unavailable, Windows |
| installation/windows#requirements | Windows 10/11 x64, WSL2 choice, known caveats, terminal backend, update and uninstall | unavailable | Windows-only: report: Unavailable, Windows |

### features.md

| page | claim | verdict | evidence |
|---|---|---|---|
| features#organize-projects-and-panes | Workspace, tabs, panes; several agents share a tab | covered-final | `final/01-verbs-send-up-wait.txt` (up fleet, 3 panes in one tab), `final/40-mouse-pass.txt` section 5 |
| features#organize-projects-and-panes | Split keys, same cwd, Alt+Arrow, Cmd+Shift+N | covered-earlier | report rows: Split, Focus; `02-open-folder.jpg`, `06-zoom.jpg` |
| features#organize-projects-and-panes | Four layout presets | covered-earlier | `05-tiled5.jpg`, `11-palette-layout.jpg`, `12-palette-layout-presets.jpg` |
| features#organize-projects-and-panes | Sidebar width drag 240 to 480 px, 432 px default notch, double-click returns to it, remembered across restarts, capped in a narrow window | covered-final | final/51-sidebar-scroll.txt, final/51-restart2.txt (300 px kept across a restart) (fd627b91) |
| features#organize-projects-and-panes | Space header shows circle and name; image, color gradient or outline; Default gradient | covered-earlier | `79-spaces-sidebar.jpg`, `80-space-work.jpg`, `81-space-color.jpg` |
| features#organize-projects-and-panes | Space menu lists Default, spaces in order, New space..., Manage spaces | covered-final | `final/51-spaces-menus.jpg`, `final/51-spaces.txt`: the space menu lists Default, Work, Play in paneflow.json order, New space…, Manage spaces (fd627b91) |
| features#organize-projects-and-panes | Cmd+Alt+Left/Right switch spaces | covered-earlier | report row: Spaces; `80-space-work.jpg` |
| features#organize-projects-and-panes | Ctrl+Alt+PageUp/PageDown switch spaces | unavailable | Linux and Windows:  |
| features#organize-projects-and-panes | Sidebar, Cmd+1..9 and Next workspace cover the active space only; each space remembers its last workspace | covered-final | `final/51-spaces.txt`: in Work the sidebar, Cmd+1/2 and Ctrl+Tab cover only w-one and w-two; each space comes back to its last workspace (fd627b91) |
| features#organize-projects-and-panes | Move to space from the workspace menu, toast Show switches | covered-final | `final/51-spaces-menus.jpg`, `final/51-spaces.txt`: Move to space > Play shows 'Moved other to Play' with Show; a real click on Show switched to Play (fd627b91) |
| features#organize-projects-and-panes | Hidden-space attention button (bell, red circle, blue dot, count above 1, tooltip Go to Work..., click opens pane); space menu marks; muted workspace excluded; notifications name the space | covered-final | `final/51-spaces-attention-button.jpg`, `final/51-spaces.txt`: bell, red crossed circle, blue dot, count only above 1, tooltip 'Go to Default: 1 agent waiting for input, 2 agents finished', click opens the waiting pane, space menu marks, a muted workspace's finish is left out; desktop notifications not checked (see C8) (fd627b91) |
| features#run-and-follow-coding-agents | Agent chat from ~ outside a project | covered-final | final/51-home-agent-chat.txt, final/51-home-agent-chat.jpg (stand-in agent opened from a workspace at ~) (fd627b91) |
| features#run-and-follow-coding-agents | `paneflow integrations install claude-code` / `codex` installs hooks, MCP bridge, conductor skill | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` |
| features#run-and-follow-coding-agents | Codex trusts the hooks through /hooks | unavailable | not marked by the report: Codex not signed in |
| features#run-and-follow-coding-agents | Paneflow hooks toggle in agent settings | covered-final | final/52-agent-config-dirs.txt, final/52-hooks-toggle.jpg: the per-agent 'Hooks and MCP' switch installs and removes hooks, MCP entry, status line and skill for Claude Code, hooks and MCP for Codex (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| features#run-and-follow-coding-agents | An entry another Paneflow home owns is left alone, both homes named, --force takes it over | covered-final | final/50-integrations-hooks-skills.txt |
| features#run-and-follow-coding-agents | Agent without OSC 7501 detected by process but shows no turn status | covered-final | `final/51-agents-section.txt`: a stand-in agent that emits no OSC 7501 shows 'starting', then 'No status' after about 3 s; `final/51-agents-groups.jpg` (fd627b91) |
| features#run-and-follow-coding-agents | `paneflow sessions` reads status with the window closed | covered-earlier | `cli/22-worker.txt`, `cli/23-windowless.txt` |
| features#run-and-follow-coding-agents | Agents section groups Need you first; Cmd+Shift+J jumps to waiting agents | covered-earlier | `67-waiting-agent-sidebar.jpg`, `68-jump-next-waiting.jpg` |
| features#run-and-follow-coding-agents | Group order Need you, Error, Done, Working, Idle, No status with counts; one-line rows; most recent first; Done moves to Idle when opened; No status after 3 s with version tooltip | covered-final | `final/51-agents-groups.jpg`, `final/51-agents-no-status-tooltip.jpg`, `final/51-agents-section.txt`: groups Need you, Error, Done, No status with counts, one-line rows, most recent first, Done to Idle when opened, the version tooltip sits on the No status badge (fd627b91) |
| features#run-and-follow-coding-agents | Click an agent row opens its pane | covered-final | `final/40-mouse-pass.txt` section 11, `final/40-59-agent-row-click.jpg` |
| features#run-and-follow-coding-agents | Agents header collapses; top edge drag and keyboard resize (Tab, arrows, Home, End, Enter); height and collapse remembered | covered-final | `final/51-agents-section.txt`: a real click collapses and expands the section; a mouse drag of the top edge and Up/Home/End/Enter resize it; the height and collapsed state survived the restart (final/51-restart-settings.txt) (fd627b91) |
| features#run-and-follow-coding-agents | After Cmd+Alt+S the keyboard cursor continues into the Agents section and Enter opens the pane | covered-final | `final/51-agents-keyboard-enter.jpg`: after Cmd+Alt+S, Down walks from the tree into the agent rows past the headers and Enter opens the Codex pane (fd627b91) |
| features#run-and-follow-coding-agents | Customize sidebar Show Agents / sidebar_show.agents; Customize agent rows logo and workspace | covered-final | `final/51-customize-sidebar-show.jpg`, `final/51-sidebar-show-keys.jpg`: Show > Agents writes sidebar_show.agents=false and hides the section; agent_logo and agent_workspace hide the logo and workspace name (fd627b91) |
| features#run-and-follow-coding-agents | Customize agent rows All spaces (sidebar_show.agent_all_spaces) | covered-final | `final/51-agents-all-spaces.jpg`: Customize agent rows > All spaces writes sidebar_show.agent_all_spaces=true and lists the Default space's agents from Play, each row naming its space (fd627b91) |
| features#run-and-follow-coding-agents | Shimmer toggles sidebar_show.shimmer and agent_shimmer | covered-final | `final/51-shimmer-tab-row.jpg` (working tab title gray with a moving band, then still and white with sidebar_show.shimmer=false) and `final/51-shimmer-agent-row.jpg` (same for the Agents row with agent_shimmer on, then off) (fd627b91) |
| features#run-and-follow-coding-agents | sidebar_show.status false drops tree status badges, restored while Agents is folded or hidden | covered-final | `final/51-sidebar-status-folded-hidden.jpg`: status=false drops the tree badge, which comes back while Agents is folded or hidden (fd627b91) |
| features#run-and-follow-coding-agents | Group by Status (menu, palette, sidebar_group_by) with New tab button | covered-final | `final/51-sidebar-group-by-status.jpg`, `final/51-agents-section.txt`: sidebar_group_by status groups tabs by status, hides Agents, adds a New tab button that asks for the workspace; Group by > Folder from the menu writes folder (fd627b91) |
| features#run-and-follow-coding-agents | Claude Code (team) launcher opens teammates as panes; tmux answered for that pane only; own panes only | covered-final | `final/08-tmux-compat.txt`, `final/26-team-lead.jpg` |
| features#run-and-follow-coding-agents | Team send-keys follows ipc.scripting (refused when false, typed when true) | fail | final/50-tmux-send-keys-scripting.txt: send-keys Enter or C-m is always refused, so a team can type into a teammate but never submit; `cli/42-tmux-compat.txt` (B3); `final/08-tmux-compat.txt` ran with scripting=false only |
| features#run-and-follow-coding-agents | Closing the lead pane leaves teammates as ordinary panes | covered-final | final/51-team-lead.txt, final/51-team-lead-close.jpg (closing the lead leaves the teammate as an ordinary pane); C15 (no team token from the tab bar + entry) fixed by 9cc22b78, re-tested on 1825a6fc in final/52-team-tabbar-retest.txt, final/52-team-tabbar-token.jpg |
| features#run-and-follow-coding-agents | Team entry absent on Windows | unavailable | Windows-only:  |
| features#run-and-follow-coding-agents | Program OSC 7501 state shown in pane header, also unfocused | covered-earlier | `72-osc7501-blocked.jpg`, `73-unfocused-pane-status-visible.jpg` |
| features#run-and-follow-coding-agents | Program blocked/error counts toward space button and notifies at most every 10 s per pane under notify_when_agent_waiting; error stops counting once seen | covered-final | final/52-notifications-retest.txt, final/52-notifications-no-modal.jpg: C8 fixed by 81b35db0, re-tested on 1825a6fc (one warning, no modal, the space button counts the error); delivery and the 10 s throttle need an app bundle, not observable on a debug build |
| features#run-and-follow-coding-agents | working/blocked/idle clear at the prompt; done/error stay until typing; state=clear; base64 msg | covered-final | `final/51-osc-lifetimes.jpg`, `final/51-osc-keystroke-clears.jpg`: working and blocked clear at the prompt, done and error stay until a real keystroke, state=clear clears, the base64 msg shows in the header (fd627b91) |
| features#keep-terminal-sessions-running | Keep sessions running reconnects to sessions and screens; layout and cwd saved | covered-final | final/50-quit-keep-relaunch.txt; `46-quit-dialog.jpg`, `47-relaunch-reconnected.jpg` |
| features#keep-terminal-sessions-running | Stop everything and quit ends the processes | covered-final | final/50-stop-everything.txt |
| features#keep-terminal-sessions-running | Lost session restores the agent conversation (claude --resume <id>) | covered-earlier | `cli/30-followup.txt` (window and host restart relaunched claude --resume) |
| features#keep-terminal-sessions-running | codex resume with check_for_update flag, fx --continue; Folder not found banner; Resume failed banner with New session; only first of several panes; restore_conversations false | unavailable | restore_conversations and resume type the agent's resume command after the host session is lost; that needs a real agent session to resume, and real agent binaries are off limits (fd627b91) |
| features#keep-terminal-sessions-running | Fork conversation from palette and pane context menu | covered-earlier | `98-fork-palette.jpg`, `99-fork-conversation.jpg` |
| features#keep-terminal-sessions-running | Closing a pane, tab or workspace takes one click and stops its sessions | covered-final | `final/40-mouse-pass.txt` section 7 (tab close, 4 to 3 panes); `58-close-workspace.jpg` |
| features#keep-terminal-sessions-running | Tab or workspace with unsaved dock files asks first | covered-final | final/51-editor-dock.txt, final/51-unsaved-close-dialog.jpg (workspace close asks Cancel / Don't Save / Save) (fd627b91) |
| features#keep-terminal-sessions-running | Multi-select rows (Cmd+click, Shift+click, Shift+Up/Down), Delete closes, menu Close N tabs / Clear selection, dialog for two or more, Escape clears | covered-final | final/51-sidebar-scroll.txt, final/51-multiselect-close-dialog.jpg (Cmd+click, Shift+Up/Down, menu Close N tabs / Clear selection, dialog for two, Escape, Delete; Shift+click not tried) (fd627b91) |
| features#keep-terminal-sessions-running | Ended shell stays read-only, Restart opens a new shell | covered-earlier | `75-exit-banner-fixed.jpg`, `76-after-restart.jpg` |
| features#keep-terminal-sessions-running | Undo after closing a tab, Cmd+Shift+T reopens tab or pane | covered-earlier | report row: Split, close, undo close |
| features#review-and-edit-changes | Changes dock title bar toggle | covered-final | `final/40-mouse-pass.txt` section 10, `120-a11y-gaps.txt` |
| features#review-and-edit-changes | Cmd+Alt+B and palette Toggle the Changes dock | covered-final | final/50-changes-dock.txt; earlier: none recorded |
| features#review-and-edit-changes | Dock opens on a list of surfaces, pick with C, T, F or arrows | covered-final | final/50-changes-dock.txt (C key; arrows not tried) |
| features#review-and-edit-changes | Split diff, intra-line highlight, untracked files | covered-final | `final/40-57-changes-view.jpg`; `14-changes.jpg`, `89-dock-open.jpg` |
| features#review-and-edit-changes | Unified switch, revert a block, trim or ignore whitespace, toolbar row (branch, diffstat, refresh, collapse all, tree) | covered-final | final/50-changes-dock.txt (unified, whitespace, toolbar; block revert not tried) |
| features#review-and-edit-changes | File header shows status, path, counts and folds; stays pinned while scrolling | covered-final | `121-ax-prompt-palette.txt` (4), `final/40-58-diff-row-collapsed.jpg`; pinning uncovered |
| features#review-and-edit-changes | Files tree at full dock width; opening a file moves the tree right | covered-final | `final/40-54-files-a-txt-open.jpg`, `121-ax-prompt-palette.txt` (3) |
| features#review-and-edit-changes | Tree resize 160 to 560 px by edge or arrows | covered-final | final/51-editor-dock.txt (edge drag: min 159 px held, grew to 495 px, the window width of this run capped the 560 px end; arrow keys not tried) (fd627b91) |
| features#review-and-edit-changes | Dot files shown; git-ignored entries dimmed and openable | covered-final | final/51-editor-dock.txt, final/51-files-tree-menus.jpg (dot files listed, target/ and run.log dimmed, run.log opens) (fd627b91) |
| features#review-and-edit-changes | Toolbar Refresh files, Search files (fuzzy, respects .gitignore), Expand all / Collapse all | covered-final | presence only: `final/40-mouse-pass.txt` section 8; search behavior uncovered |
| features#review-and-edit-changes | Tree reveals the active tab's file | covered-final | `121-ax-prompt-palette.txt` (3) (lib.rs selected after opening) |
| features#review-and-edit-changes | Entry context menu Open with Default App, Reveal in Finder, Open With | covered-final | final/51-editor-dock.txt, final/51-files-tree-menus.jpg (three entries present; Open With disabled because no code editor was detected here) (fd627b91) |
| features#review-and-edit-changes | Cmd+P file finder: recent files, fuzzy project search, opens in dock | covered-earlier | `102-file-finder-cmd-p-fixed.jpg` |
| features#review-and-edit-changes | File tab menu (Copy Path, Copy Relative Path, Close, Close Others, Close to the Right, Close All, Reopen Closed Tab with scroll), middle click, keeps modified files, tabs restored after restart | covered-final | final/51-editor-dock.txt, final/51-file-tab-menu.jpg, final/51-restart-settings.txt (all seven entries, middle click, modified file kept by Close All, tabs restored after restart; scroll position of a reopened tab not checked) (fd627b91) |
| features#review-and-edit-changes | Breadcrumb folder click reveals in tree; chevron browses | covered-final | final/51-editor-dock.txt (breadcrumb 'docs' reveals and selects the folder; chevron lists its 7 files) (fd627b91) |
| features#review-and-edit-changes | Editor save, undo, redo | covered-earlier | `16-editor.jpg`, `17-editor-save.jpg`, `18-editor-undo-redo.jpg` |
| features#review-and-edit-changes | Git gutter markers popup read, copy, revert | covered-final | final/51-editor-dock.txt, final/51-gutter-popup-revert.jpg (popup 'Modified line 10', Copy put the old line on the clipboard, Revert restored it) (fd627b91) |
| features#review-and-edit-changes | Save conflict on disk change | covered-earlier | `19-editor-disk-change.jpg`, `20-editor-save-conflict.jpg`, `21-dock-maximized-conflict.jpg` |
| features#review-and-edit-changes | Editor find bar Cmd+F: Match Case, Regular Expression, N of M, Enter/Shift+Enter, Replace / Replace All one undo with $1, none on read-only | covered-final | final/51-editor-dock.txt, final/51-editor-find.jpg, final/51-editor-replace-undo.jpg, final/51-markdown-config.txt (counts, Enter/Shift+Enter, regex, match case, Replace All with $1 undone in one step, no replace on a chmod 444 file) (fd627b91) |
| features#review-and-edit-changes | :120 Enter jumps to line; Word Wrap menu and editor.word_wrap | covered-final | final/51-editor-dock.txt, final/51-editor-jump-wrap.jpg, final/51-word-wrap-config.jpg (fd627b91) |
| features#review-and-edit-changes | Viewers: images zoom and pan, CSV/TSV table with Show source (editor.csv_table), Markdown rendered/source (editor.markdown_rendered), zip/tar listing, 10 to 256 MB text viewer, No preview card | covered-final | final/51-editor-dock.txt, final/51-viewers.jpg, final/51-viewer-no-preview.jpg (image controls, CSV table and Show source -> csv_table=false, Markdown rendered -> markdown_rendered=true, zip listing, 15.6 MB text viewer, binary card; image zoom/pan gestures and TSV not exercised) (fd627b91) |
| features#send-files-and-code-to-an-agent | Send to Agent types @path or bare path without Enter; list with several agents; disabled with none; drag onto agent or shell | covered-final | final/51-editor-dock.txt, final/51-files-tree-menus.jpg, final/51-markdown-config.txt (@path without Enter, two-agent list, 'No agent in this workspace', drag onto agent and onto shell) (fd627b91) |
| features#send-files-and-code-to-an-agent | Gutter + comment, Comment Selection Cmd+Shift+M, Send to Agent Cmd+Enter | covered-final | final/51-editor-dock.txt, final/51-comment-send.jpg (Cmd+Shift+M on the caret line, Cmd+Enter, agent list, path:line + comment + quoted lines pasted after the bracketed-paste prompt) (fd627b91) |
| features#send-files-and-code-to-an-agent | Cmd+click a path:line:col in terminal opens the dock at that position; workspace-root and suffix lookup; several matches open the finder; File not found | fail | C9: final/51-cmd-click-ambiguous.txt, final/51-cmd-click-ambiguous.jpg (several matches never open the finder; root, suffix and line:col work, final/51-editor-dock.txt, final/51-cmd-click-paths.jpg; an open tab of a deleted file shows the deleted-on-disk banner) (fd627b91) |
| features#send-files-and-code-to-an-agent | Shift+click or terminal.open_links_in external_editor opens the external editor | unavailable | not marked by the report: no supported external editor installed (report lists editors unavailable) |
| features#keep-project-context-at-hand | Branch display and tree follow the active checkout | covered-earlier | `107-fixed-tab-title-branch.jpg`, `worktrees/31-changes-dock-feat.jpg` |
| features#keep-project-context-at-hand | Copy absolute or relative path | covered-earlier | report row: copy path |
| features#keep-project-context-at-hand | Cmd+Shift+I finds a file and inserts its quoted path | covered-earlier | `77-insert-path.jpg` |
| features#keep-project-context-at-hand | Open in Zed, Cursor, VS Code, WebStorm; WebStorm discovery | unavailable | report: editors not installed |
| features#keep-project-context-at-hand | external_editor windsurf falls back to auto | unavailable | external_editor opens the user's own editor app outside the dev sandbox (fd627b91) |
| features#keep-project-context-at-hand | Pasted image saved under cache/clipboard-images and its path inserted | covered-earlier | `77-insert-path.jpg` |
| features#keep-project-context-at-hand | Listening ports tied to panes, dev server URL entry | covered-earlier | `78-ports.jpg`, `78-ports-context-menu.jpg` |
| features#keep-project-context-at-hand | Custom command buttons | covered-final | final/51-layouts-templates.txt (Manage custom buttons… creates a button; it shows in the pane + chooser and runs its command in a new pane) (fd627b91) |
| features#separate-tasks-with-git-worktrees | Worktrees per task, cleanup and snapshots | covered-final | `final/07-worktrees.txt`; `worktrees/*` |
| features#share-pane-output-through-mcp | `paneflow mcp install` for the supported agents | covered-final | final/50-integrations-hooks-skills.txt; `cli/16-mcp.txt`, `cli/20-hooks.txt` |
| features#share-pane-output-through-mcp | list_panes, read_pane, search_pane return untrusted framed output | covered-final | `final/11-flow-watch-mcp.txt`; `cli/16-mcp.txt` |
| features#share-pane-output-through-mcp | write_pane only after a human allows the pair | covered-final | `final/04-mcp-write-approval.txt`, `final/40-mouse-pass.txt` section 12 |
| features#share-pane-output-through-mcp | ask_human prompts in Paneflow; run_action runs an init.lua action marked agents = true under the write gates | covered-final | final/50-mcp-actions.txt; earlier: tools listed only (`cli/16-mcp.txt`) |
| features#coordinate-panes-with-the-cli | `paneflow up` from a file; `flow run` | covered-final | `final/01-verbs-send-up-wait.txt`, `final/11-flow-watch-mcp.txt` |
| features#coordinate-panes-with-the-cli | `send` prefills, `--submit` presses Enter, gated by permissions | covered-final | `final/01-verbs-send-up-wait.txt` |
| features#adjust-paneflow-to-your-workflow | Settings pages for shell, theme, terminal, shortcuts, notifications apply live | covered-final | `final/30-settings-general.jpg`..`final/34-theme-light.jpg`, `final/10-theme.txt`, `final/40-mouse-pass.txt` section 3 |
| features#adjust-paneflow-to-your-workflow | A new default shell applies to new terminals | covered-final | final/50-config-keys.txt |
| features#adjust-paneflow-to-your-workflow | Interface style Classic default, Rounded selectable | covered-final | `121-ax-prompt-palette.txt` (2) (interface_style written) |
| features#adjust-paneflow-to-your-workflow | Background style keeps Themed, Mica, Blended values | covered-earlier | `31`..`44-*.jpg`, `43-settings-blended-both.jpg` |
| features#adjust-paneflow-to-your-workflow | App icon from twenty-one icons; Workspace icon glyph | covered-final | final/51-restart-settings.txt, final/51-app-icon-dock.jpg, final/51-config-hot-reload.jpg (app icons black and melon, six workspace icon glyphs; the 21-icon gallery itself not opened) (fd627b91) |

### layouts.md

| page | claim | verdict | evidence |
|---|---|---|---|
| layouts#Choose and apply a layout | Cmd+Alt+1..4 apply Even horizontal, Even vertical, Main vertical, Tiled to the active tab | covered-earlier | 05-tiled5.jpg; report row: Layouts Cmd+Alt+1..4 |
| layouts#Choose and apply a layout | Command palette Pane layout lists the four presets by the same names | covered-earlier | 11-palette-layout.jpg, 12-palette-layout-presets.jpg |
| layouts#Choose and apply a layout | Main vertical keeps focus on the selected pane; other presets move focus to the first pane | covered-final | final/51-layouts-templates.txt (fd627b91) |
| layouts#Choose and apply a layout | With one pane, a preset leaves the arrangement unchanged | covered-final | final/51-layouts-templates.txt (fd627b91) |
| layouts#Balance sizes | Cmd+Shift+= (split_equalize) balances sizes within each split, keeps the arrangement | covered-earlier | fixed 266e5122; 70b-split-equalize.jpg, 71b-equalize-shortcut-fixed.jpg |
| layouts#Keep working in the same sessions | A preset reuses panes: shells, running commands, history and cwd survive; other tabs keep their layouts | covered-earlier | 05-tiled5.jpg; report row: Layouts Cmd+Alt+1..4 |
| layouts#Keep working in the same sessions | Keep sessions running on quit, reopen reconnects | covered-final | final/50-quit-keep-relaunch.txt; 46-quit-dialog.jpg, 47-relaunch-reconnected.jpg |
| layouts#Configure layouts and shortcuts | Rebinding a layout action (layout_tiled etc.) under shortcuts | covered-final | final/50-config-keys.txt (secondary+alt+9 to layout_tiled frees Cmd+Alt+4); earlier: none (only new_tab and split rebinding observed: 92, 93) |
| layouts#Configure layouts and shortcuts | paneflow up file key layout takes even_h, even_v, main_vertical, tiled | covered-final | final/01-verbs-send-up-wait.txt (tiled); cli/14-up.txt (even_h, even_v, main_vertical) |
| layouts#Configure layouts and shortcuts | Workspace template commands[].workspace.layout_preset sets the initial arrangement | covered-final | final/51-layouts-templates.txt, final/51-template-launch-stacked.jpg (even_v -> Stacked) (fd627b91) |

### worktrees.md

| page | claim | verdict | evidence |
|---|---|---|---|
| worktrees#What a worktree is | Worktrees only for projects in a Git repository; a non-Git project is refused clearly | covered-earlier | worktrees/30-walkthrough.txt; report row: project without git refused |
| worktrees#Open a pane on a new branch | A new tab's New pane palette opens with a branch row above the agents | covered-final | final/40-mouse-pass.txt step 5-6, final/25-new-tab-palette-branch-row.jpg, final/40-46-new-tab-palette.jpg |
| worktrees#Open a pane on a new branch | Adding a pane (split) opens the New pane palette with the branch row | covered-earlier | fixed 9a8080e5; 108-fixed-split-palette-branch-row.jpg |
| worktrees#Open a pane on a new branch | Branch row opens a menu with New branch... and the branches | covered-final | final/40-mouse-pass.txt step 6, final/40-47-palette-branch-list.jpg |
| worktrees#Open a pane on a new branch | New branch form: name, from defaulting to the project branch, Worktree toggle on, pick a launcher | covered-final | final/23-tab-menu-new-branch-form.jpg, final/24-tab-menu-new-branch-bound.jpg; worktrees/73-new-branch-form.jpg, worktrees/82-mouse-flows.txt |
| worktrees#Open a pane on a new branch | Paneflow creates the branch and worktree from the base, binds the tab, the tab shows the branch name, the pane starts inside the worktree | covered-final | final/07-worktrees.txt (tab_title feat/final-one, pwd in worktree); fixed e6f85806, 107-fixed-tab-title-branch.jpg |
| worktrees#Open a pane on a new branch | The tab context menu offers New branch... for an existing tab (repository with several branches) | covered-final | final/07-worktrees.txt F06 Shift+F10, final/22-shift-f10-tab-menu.jpg..final/24 |
| worktrees#Open a pane on a new branch | The tab context menu offers New branch... in a repository with only its default branch | covered-final | fixed f5827d8c (pin test only; final/40-mouse-pass.txt note C before); no observation on a build with the fix |
| worktrees#Open a pane on a new branch | paneflow up, flows and the API (workspace.up, surface.split) take worktree and from | covered-final | final/07-worktrees.txt (surface.split); cli/14-up.txt (up worktree) |
| worktrees#Open a pane on a new branch | Scripted paths refuse a branch checked out outside Paneflow: branch NAME already checked out at PATH | covered-earlier | worktrees/14-reuse-existing-branch.txt, worktrees/14b-reuse-via-up.txt |
| worktrees#Reuse a branch that already exists | Picking a branch already in a worktree reuses it; a branch checked out nowhere gets its own worktree; from ignored | covered-earlier | worktrees/80-reuse-existing-branch.jpg, worktrees/81-existing-b-new-worktree.jpg, worktrees/14-reuse-existing-branch.txt |
| worktrees#Reuse a branch that already exists | The app never fails with already used by worktree at ... | covered-earlier | worktrees/80-reuse-existing-branch.jpg |
| worktrees#Start detached | Empty branch name gives a worktree detached at the base, named BASE-SHA | covered-earlier | worktrees/77-detached-worktree.jpg, worktrees/82-mouse-flows.txt |
| worktrees#Start detached | Tab right-click Create branch here... runs git switch -c and keeps uncommitted files | covered-earlier | worktrees/78-create-branch-here.jpg, worktrees/82-mouse-flows.txt |
| worktrees#Switch the project checkout instead | Worktree toggle off switches the project checkout in place, pane at the repo root, nothing listed | covered-earlier | worktrees/79-worktree-toggle-off.jpg, worktrees/82-mouse-flows.txt |
| worktrees#Switch the project checkout instead | The toggle choice is remembered in worktrees.for_new_branches | covered-earlier | worktrees/82-mouse-flows.txt (for_new_branches false) |
| worktrees#Switch the project checkout instead | Switching the checkout is refused while an agent is working in it | fail | C13: final/51-worktrees.txt, final/51-checkout-switch-guard.jpg (the New branch form refuses; the Changes dock branch picker switched the checkout while the agent was working) (fd627b91) |
| worktrees#Where worktrees live | Managed worktrees under HOME/worktrees/REPO-HASH/BRANCH | covered-final | final/07-worktrees.txt |
| worktrees#Where worktrees live | Nothing written in the checkout (git status clean); owner marker in the worktree's git metadata | covered-final | final/07-worktrees.txt (status --short empty); worktrees/10-new-branch-split.txt, worktrees/11-new-branch-worktree-contents.txt (marker) |
| worktrees#Where worktrees live | worktrees.dir (Settings or json) applies to new worktrees; older ones keep working and stay listed | covered-earlier | worktrees/16-worktrees-dir-setting.txt, worktrees/20-settings.jpg |
| worktrees#Copy ignored local files | .worktreeinclude paths, directories, # comments; default .env* and AGENTS.override.md; existing files never overwritten | covered-earlier | worktrees/15-worktreeinclude.txt, worktrees/16-worktrees-dir-setting.txt |
| worktrees#Cleanup | Default keep limit 15; keep_limit and auto_remove in Settings or json | covered-earlier | worktrees/26-keep-limit.txt, worktrees/27-auto-remove-off.txt; fixed 56812e94 |
| worktrees#Cleanup | Removed when its workspace closes | covered-earlier | worktrees/18-close-workspace-cleanup.txt..22; fixed 65b4ab10, a8338540 |
| worktrees#Cleanup | Oldest trimmed to stay within the keep limit, also while the window is inactive | covered-earlier | worktrees/26-keep-limit.txt; fixed 84243b18 (pin) |
| worktrees#Cleanup | Never removed while a tab of an open workspace uses it; the branch is never deleted | covered-earlier | worktrees/28-shared-worktree-close.txt, worktrees/23-close-after-shell-exit.txt..27; fixed 30f9832b |
| worktrees#Cleanup | Manual removal from Settings > Worktrees with its dialog | covered-earlier | worktrees/61-manual-remove.txt, worktrees/61-remove-asks.jpg..63 |
| worktrees#Cleanup | Manual removal from the tab's context menu | covered-final | final/50-worktree-tab-menu.txt |
| worktrees#Cleanup | Worktrees made with git worktree add are never removed | covered-earlier | worktrees/40-foreign-worktrees.txt |
| worktrees#Snapshots | Snapshot of tracked edits, new files and branch before removal; a clean worktree leaves none | covered-earlier | worktrees/23-close-after-shell-exit.txt, worktrees/24-snapshot-contents.txt |
| worktrees#Snapshots | Snapshots under refs/paneflow/snapshots/, never pushed, readable with git log | covered-earlier | worktrees/24-snapshot-contents.txt, worktrees/25-snapshot-not-pushed.txt |
| worktrees#Snapshots | Settings > Worktrees lists snapshots as soon as it opens | covered-final | final/33-settings-worktrees.jpg; fixed fce04a13 |
| worktrees#Snapshots | Restore recreates the worktree at its path and branch, changes uncommitted, .worktreeinclude copied again | covered-earlier | worktrees/57-snapshot-restore.txt, worktrees/57-after-restore.jpg |
| worktrees#Snapshots | Delete drops the snapshot | covered-earlier | worktrees/59-snapshot-delete.txt, worktrees/58-before-delete.jpg, worktrees/59-after-delete.jpg, worktrees/60-delete-armed.jpg |
| worktrees#FAQ | ~ in worktrees.dir expands; PANEFLOW_HOME moves the worktrees | covered-final | final/07-worktrees.txt (worktrees under the tagged home); worktrees/16-worktrees-dir-setting.txt (~) |
| worktrees#FAQ | Picking the branch again after removal gives a fresh worktree from its committed state | covered-final | final/51-worktrees.txt (fd627b91) |
| worktrees#FAQ | Worktrees made by the user are listed in the branch row and panes open in them | covered-earlier | worktrees/40-foreign-worktrees.txt, worktrees/reuse-foreign.toml |
| worktrees#Two tasks, then one integration | Two branches from one base, checkout paths confirmed, project WIP not carried, full branch diff, integration | covered-earlier | worktrees/30-walkthrough.txt, worktrees/31-changes-dock-feat.jpg..33 |
| worktrees#Existing branch, non-Git project or unsaved editor | Snapshots read disk files, not unsaved editor text | covered-final | final/51-worktrees.txt, final/51-worktree-snapshot.jpg (fd627b91) |

### settings.md

| page | claim | verdict | evidence |
|---|---|---|---|
| settings#Settings map | Settings pages render; navigation is a tab list | covered-final | final/09-settings-nav-ax.txt, 120-a11y-gaps.txt, final/30-settings-general.jpg |
| settings#Settings map | General: external_editor and default_shell apply to new launches | covered-final | final/50-config-keys.txt (default_shell; external editors unavailable, none installed); earlier: 82-settings-General.jpg (render only) |
| settings#Settings map | Appearance theme row writes theme and theme_mode and hot-reloads | covered-final | final/10-theme.txt, final/34-theme-light.jpg |
| settings#Settings map | A Notifications page holds native notifications (agent_panel.notify_when_agent_waiting) | fail | (docs) final/50-settings-pages.txt: no Notifications page, the switch is on General; the map also omits Editor and Usage |
| settings#Settings map | Keyboard Shortcuts page: change a binding and reset to defaults, written to shortcuts | covered-final | final/50-settings-pages.txt; earlier: 82-settings-Keyboard.jpg (render only) |
| settings#Settings map | Terminal page: cursor shape (next terminal), color, font family, size, weight, line height, cell width, glyphs, emoji hot-reload | covered-final | final/50-settings-pages.txt (font size, line height, glyphs, cursor shape written); earlier: 82-settings-Terminal.jpg (render only) |
| settings#Settings map | Workspaces page: templates run through the paneflow up path | fail | C12: final/51-layouts-templates.txt (Launch runs in an open workspace of the same folder, replaces its active tab without asking and leaves the replaced sessions running unseen) (fd627b91) |
| settings#Settings map | Agents page: launcher visibility, versions, hook state, Paneflow hooks toggle, profiles, full access, ai_unrestricted, injection fence | covered-final | final/51-restart-settings.txt (launcher visibility switches, Update, hooks switch state, presets, full access, AI free access; versions not exposed to AX; hooks not toggled) (fd627b91) |
| settings#Settings map | Plugins installs or repairs paneflow-mcp in agent configs | covered-earlier | 82-settings-Plugins.jpg (Stale path observation; Repair not clicked); mcp install in cli/16-mcp.txt |
| settings#Interface style | Classic is the default edge-to-edge layout | covered-earlier | 82-settings-*.jpg, 22-settings.jpg |
| settings#Interface style | Picking Rounded writes interface_style and applies at once | covered-final | 121-ax-prompt-palette.txt (2) |
| settings#Interface style | Comfortable density applies only to Rounded | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (density has no effect under classic; cards vs 1 px dividers under rounded) (fd627b91) |
| settings#Background style | Themed default; Blended blur and veil; Themed kept in fullscreen and under Reduce transparency | covered-earlier | 31-blended-dark.jpg..53; fixed 357a9a4a (45, 49, 50) |
| settings#Background style | Legacy keys read when background_style is absent; picking a style removes them | covered-earlier | 51-upgrade-macos-chrome-material.jpg, 52-upgrade-no-keys.jpg |
| settings#Background style | Linux Wayland blur, X11 thicker veil warning | unavailable | Linux-only:  |
| settings#Workspace icon | Workspace icon styles (Default, Standard, Metal, Windows, Windows 95, Pixel) with open and closed glyphs | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (six styles) (fd627b91) |
| settings#App icon | App icon gallery; the choice shows in the Dock and app switcher at once on macOS | covered-final | final/51-restart-settings.txt, final/51-app-icon-dock.jpg (Dock tile changes at once; Settings picker not opened) (fd627b91) |
| settings#App icon | Windows taskbar, Start menu copy; Linux X11 and Wayland behavior | unavailable | Windows-only, Linux-only:  |
| settings#Terminal contrast | Minimum contrast Auto, Off, 45..90; values above 90 capped, invalid fall back to Auto; hot-reload | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (Auto, 0 off, 90, 120 capped to 90, invalid -> Auto, hot reload) (fd627b91) |
| settings#Spaces | Spaces in the sidebar, colors as gradient or outline | covered-earlier | 79-spaces-sidebar.jpg, 80-space-work.jpg, 81-space-color.jpg |
| settings#Spaces | New space adds Space N; rename with Enter/Escape; name rules (1 to 32, not Default, unique ignoring case); 9 spaces maximum | covered-final | final/50-settings-pages.txt; earlier: 82-settings-Spaces.jpg (render only) |
| settings#Spaces | Delete space asks, moves workspaces to Default without closing terminals | covered-final | final/50-settings-pages.txt |
| settings#Space image | Upload, replace, remove image; 96 px PNG in avatars/; unreadable or too large refused | unavailable | the space image is picked through the system Open panel, which neither CGEvent nor System Events keystrokes could drive in this session; Cancel closed it (`final/51-space-env.txt`) (fd627b91) |
| settings#Space environment | Edit env rows; Save refuses invalid names, PANEFLOW_ names, TERM, LD_PRELOAD, CLAUDECODE, relative config dirs, unknown $NAME; plain-text and token warnings | covered-final | `final/51-space-env-refusals.jpg`, `final/51-space-env.txt`: Save refuses an invalid name, PANEFLOW_, TERM, LD_PRELOAD, CLAUDECODE, a relative CLAUDE_CONFIG_DIR and an unset $NAME, warns on a secret-looking name; the messages are not exposed to accessibility (fd627b91) |
| settings#Space environment | Space env applies to new terminals; open terminals keep theirs with a notice | covered-final | `final/51-space-env.txt`, `final/51-space-env-move-notice.jpg`: an open terminal kept QUAL_SPACE=from-work after the env changed, a new one got from-work-2; moving a workspace shows 'Open terminals keep their previous environment' after the move toast (fd627b91) |
| settings#Space environment | Saving a space with CLAUDE_CONFIG_DIR or CODEX_HOME installs hooks and MCP there; Agent setup with Retry and Install | fail (docs) | C17: final/52-agent-config-dirs.txt, final/52-space-accounts.jpg: 'Add hooks' installs hooks and MCP in the space's CLAUDE_CONFIG_DIR and CODEX_HOME; saving does not on a debug build (logged); the page has an Accounts section with 'Add hooks', not the 'Agent setup' block with Install and Retry the page describes (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| settings#AI access vs MCP | claude_code_bypass_permissions launches Claude Code with --permission-mode bypassPermissions | covered-final | final/50-quit-keep-relaunch.txt |
| settings#Config-only controls | A malformed paneflow.json keeps the previous valid config | covered-final | final/50-quit-keep-relaunch.txt; 83-malformed-json.jpg |

### themes.md

| page | claim | verdict | evidence |
|---|---|---|---|
| themes#Choose a theme | Light, Dark, System and Preset apply live and write theme_mode | covered-final | final/10-theme.txt, final/34-theme-light.jpg; 23-appearance.jpg..26 |
| themes#Choose a theme | System follows the OS appearance and stays selected after relaunch | covered-earlier | 28-system-mode-false.jpg..30; fixed 5d08ac83 |
| themes#Choose a theme | Reset to default in the Theme section returns to Paneflow Dark | covered-final | final/50-settings-pages.txt |
| themes#Included palettes | Five palettes with light and dark variants | covered-earlier | 23-appearance.jpg, 24-light.jpg, 25-claude-light.jpg |
| themes#Included palettes | Names are case-insensitive; One Dark, PaneFlow Light, Vercel, Claude, Cursor aliases | covered-final | final/50-themes.txt; 26-one-dark-alias.jpg (One Dark only; case and the other aliases not shown) |
| themes#Set a theme in paneflow.json | Hand edit of theme and theme_mode hot-reloads | covered-final | final/50-themes.txt; report row: hand edit hot-reloads (23..27) |
| themes#Set a theme in paneflow.json | theme_mode omitted or null infers the mode from the name; removing both restores the default | covered-final | final/51-theme-mode.jpg (theme 'Claude Light' without theme_mode -> light UI; 'Claude Dark' with theme_mode null -> dark; both removed -> default Paneflow Dark) (fd627b91) |
| themes#What the theme changes | terminal.cursor_color overrides the cursor; null uses the theme's | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (cursor_color hot, underline + blink off on a new terminal) (fd627b91) |
| themes#If the theme does not change | An unknown name falls back to Paneflow Dark and logs a warning | covered-final | final/50-themes.txt; 27-unknown-theme.jpg; the report row says it keeps the previous theme, while the docs and src-app/src/theme/watcher.rs fall back to Paneflow Dark |
| themes#If the theme does not change | Changing window_decorations needs a restart | covered-final | final/51-restart-settings.txt (window_decorations is startup only; on macOS server looks identical to client, observation) (fd627b91) |
| themes#Custom palettes | theme.json and custom palettes in paneflow.json are not loaded | covered-final | final/50-themes.txt |

### usage.md

| page | claim | verdict | evidence |
|---|---|---|---|
| usage#Accounts | Default ~/.claude account listed | covered-earlier | 54-usage-capsule.jpg, 56-settings-usage.jpg |
| usage#Accounts | Accounts from spaces and profiles setting CLAUDE_CONFIG_DIR or CODEX_HOME; two spaces on one dir share an account | covered-final | final/52-agent-config-dirs.txt, final/52-usage-accounts.jpg (Work from a space, a profile account, Play on the same directory adds no account) (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| usage#Accounts | An API-key account has no plan limits and says so | covered-final | final/52-agent-config-dirs.txt, final/52-usage-accounts.jpg ('Billed through an API key. This account has no plan limits.' with a dummy key in a space) (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| usage#The sidebar capsule | Capsule between filter and Settings, ring and percentage, hover tooltip | covered-earlier | 54-usage-capsule.jpg |
| usage#The sidebar capsule | Ring accent below 80, amber from 80, red from 95 | unavailable | the capsule shows 'Plan usage: no data yet': plan percentages come from the real Claude usage probe, which this audit must not run (fd627b91) |
| usage#The sidebar capsule | Click, Enter or Space opens the bubble; meters, reset tooltip, pace line; Open usage settings opens the page | covered-final | 120-a11y-gaps.txt section 4 (capsule and Open usage settings pressed); 55-usage-bubble.jpg |
| usage#The sidebar capsule | Weekly limit reached line | unavailable | the capsule shows 'Plan usage: no data yet': plan percentages come from the real Claude usage probe, which this audit must not run (fd627b91) |
| usage#The sidebar capsule | Refresh every minute for readings older than five minutes | unavailable | the capsule shows 'Plan usage: no data yet': plan percentages come from the real Claude usage probe, which this audit must not run (fd627b91) |
| usage#The sidebar capsule | Settings > Usage > Sidebar switch per agent, stored in usage.sidebar_agents | covered-final | 120-a11y-gaps.txt section 4 (Codex switch toggled; the json key not shown) |
| usage#The sidebar capsule | sidebar_show.usage false or Customize sidebar hides the capsule | covered-final | final/51-restart-settings.txt, final/51-usage-capsule.jpg (fd627b91) |
| usage#The Usage page | One tile per account with week and session rings, reset times, pace; Refresh | covered-final | final/31-settings-usage.jpg; 56-settings-usage.jpg |
| usage#The Usage page | Activity registers, Turns, Tokens, Active time; arrow keys move through days | covered-final | final/51-usage-fx.txt (three registers, arrow keys move the focused day) (fd627b91) |
| usage#Import past activity | First-start import of transcripts; Import past activity reruns without double counting | covered-final | final/51-usage-fx.txt (first-start import of 57 files; rerun kept 168 turns) (fd627b91) |
| usage#Where the limits come from | Integration sets statusLine to paneflow-ai-hook StatusLine when none exists; relay to the capsule | covered-final | final/52-agent-config-dirs.txt (statusLine set to paneflow-ai-hook StatusLine where none existed; relay already observed earlier) (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| usage#Where the limits come from | rate_limits.five_hour | unavailable | report: Claude Code 2.1.331 sent none |
| usage#Where the limits come from | Codex app-server and Codex session-file limits | unavailable | Codex not signed in (not marked as a usage row in the report) |
| usage#If you have your own status line | A user status line is never changed; message until the binary reading succeeds | covered-final | final/52-agent-config-dirs.txt (a user statusLine kept through installs; the 'has not reported plan limits for <folder> yet' message) (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| usage#What Paneflow stores | usage/plans.json and usage/ledger.json in the home, no prompt text | covered-final | final/51-usage-fx.txt (both files present, numeric fields only) (fd627b91) |
| usage#Turn it off | usage.plan_limits false hides the capsule; integrations install or remove claude-code removes the status line | covered-final | final/50-integrations-hooks-skills.txt (status line removal; capsule hiding not observed); earlier: cli/20-hooks.txt runs install and remove but never reads statusLine |

### keybindings.md

| page | claim | verdict | evidence |
|---|---|---|---|
| keybindings#Essential shortcuts | Split Cmd+Shift+D and Cmd+Shift+E; close Cmd+Shift+W; undo close Cmd+Shift+T | covered-earlier | report row: workspace.current readings; final/36-split.jpg |
| keybindings#Default shortcuts | new_tab Cmd+Alt+T, previous_tab Cmd+[, close_tab Cmd+W | covered-earlier | 07-tabs.jpg, 08-cmd-w.jpg, 95-after-cmd-w.jpg |
| keybindings#Default shortcuts | next_tab Cmd+] | covered-final | final/51-terminal-keys.txt (Tab 2 -> Tab 1 -> Tab 2, wraps) (fd627b91) |
| keybindings#Default shortcuts | toggle_zoom Cmd+Shift+Z; toggle_detached_pane Cmd+Alt+Shift+D | covered-earlier | 06-zoom.jpg, 41-blended-detached.jpg |
| keybindings#Default shortcuts | Alt+Arrows focus | covered-earlier | report row: Focus (Alt+Arrows) |
| keybindings#Default shortcuts | new_workspace Cmd+Shift+N | covered-earlier | 02-open-folder.jpg |
| keybindings#Default shortcuts | close_workspace Cmd+Shift+Q | unavailable | report: Log Out key, verified from the Window menu (58-close-workspace.jpg) |
| keybindings#Default shortcuts | next_workspace Ctrl+Tab; select_workspace_1..9 Cmd+1..9 | covered-earlier | report row: Workspace switching |
| keybindings#Default shortcuts | next_space Cmd+Alt+Right, previous_space Cmd+Alt+Left | covered-earlier | 79-spaces-sidebar.jpg, 80-space-work.jpg |
| keybindings#Default shortcuts | Ctrl+Alt+PageDown and PageUp space keys; Ctrl+V paste; the Menu key | unavailable | Linux and Windows only:  |
| keybindings#Default shortcuts | jump_next_waiting Cmd+Shift+J | covered-earlier | 67-waiting-agent-sidebar.jpg, 68-jump-next-waiting.jpg |
| keybindings#Default shortcuts | focus_workspaces_sidebar Cmd+Alt+S | covered-final | final/21-focus-sidebar.jpg, final/40-mouse-pass.txt step 13 |
| keybindings#Default shortcuts | copy_workspace_path Ctrl+Shift+Alt+C, reveal_workspace_in_file_manager Ctrl+Alt+R | covered-earlier | report row: copy path, reveal in Finder (69-focus-sidebar.jpg) |
| keybindings#Default shortcuts | open_workspace_in_zed, cursor, vscode, webstorm | unavailable | report: editors not installed |
| keybindings#Default shortcuts | toggle_files_sidebar Cmd+Alt+F | covered-final | final/51-editor-dock.txt (Cmd+Alt+F via System Events opens the Files tree; agent-desktop press fails on the menu modifier) (fd627b91) |
| keybindings#Default shortcuts | toggle_diff_dock Cmd+Alt+B | covered-final | final/50-changes-dock.txt; earlier: dock toggled only by the title bar button (final/40-mouse-pass.txt step 10) |
| keybindings#Default shortcuts | toggle_diff_dock_maximize Cmd+Alt+M | covered-final | final/51-editor-dock.txt (Cmd+Alt+M toggles Maximize panel / Restore panel both ways) (fd627b91) |
| keybindings#Default shortcuts | open_file_finder Cmd+P: recent files, project search, opens in the dock | covered-earlier | 102-file-finder-cmd-p-fixed.jpg |
| keybindings#Default shortcuts | open_command_palette Cmd+Shift+P | covered-final | 121-ax-prompt-palette.txt, final/35-command-palette.jpg |
| keybindings#Default shortcuts | open_settings Cmd+, | covered-final | final/50-settings-pages.txt; earlier: none (Settings opened from the welcome row in final/40) |
| keybindings#Default shortcuts | diff_new_file_tab Cmd+G and diff_new_terminal_tab Cmd+J in the dock | covered-earlier | fixed 5c85556c; 101-dock-cmd-j-fixed.jpg, 90-dock-cmd-g.jpg |
| keybindings#Default shortcuts | Cmd+C, Cmd+V, Cmd+A in a terminal | covered-earlier | report row: copy, paste and select all |
| keybindings#Default shortcuts | Ctrl+Shift+C, Ctrl+Shift+V, Ctrl+Shift+A in a terminal on macOS | covered-final | final/51-terminal-keys.txt, final/51-terminal-keys.jpg (fd627b91) |
| keybindings#Default shortcuts | scroll_page_up Shift+PageUp | covered-earlier | 63-shift-pageup.jpg |
| keybindings#Default shortcuts | scroll_page_down Shift+PageDown | covered-final | final/51-terminal-keys.txt, final/51-terminal-keys.jpg (fd627b91) |
| keybindings#Default shortcuts | jump_prev_prompt and jump_next_prompt Cmd+Shift+Up and Down | covered-earlier | fixed e741a919; 64-prompt-jump-fixed.jpg |
| keybindings#Default shortcuts | insert_path Cmd+Shift+I | covered-earlier | 77-insert-path.jpg |
| keybindings#Default shortcuts | clear_scroll_history Cmd+K; reset_terminal Cmd+Shift+R | covered-earlier | 65-reset-terminal.jpg, 66-after-reset.jpg |
| keybindings#Default shortcuts | clear_scroll_history Cmd+Shift+K | covered-final | final/51-terminal-keys.txt, final/51-terminal-keys.jpg (history before the cleared screen gone) (fd627b91) |
| keybindings#Default shortcuts | toggle_search Cmd+F | covered-earlier | 59-cmd-f-search.jpg, 84-search-open.jpg |
| keybindings#Default shortcuts | toggle_search Ctrl+Shift+F on macOS | covered-final | final/51-terminal-keys.txt, final/51-terminal-keys.jpg (counts 11/11 and 1/11 step with Down, Up, Tab, Shift+Tab) (fd627b91) |
| keybindings#Default shortcuts | font_size_increase Cmd+= and font_size_reset Cmd+0 | covered-earlier | 60-font-bigger.jpg, 61-font-reset.jpg |
| keybindings#Default shortcuts | font_size_decrease Cmd+- | covered-final | final/51-terminal-keys.txt, final/51-terminal-keys.jpg (fd627b91) |
| keybindings#Default shortcuts | Search overlay Enter, Shift+Enter, Escape, Alt+R | covered-earlier | 85-search-next2.jpg, 86-search-prev.jpg, 87-search-regex.jpg, 88-search-dismissed.jpg |
| keybindings#Default shortcuts | Search overlay Down, Up, Tab, Shift+Tab | covered-final | final/51-terminal-keys.txt, final/51-terminal-keys.jpg (counts 11/11 and 1/11 step with Down, Up, Tab, Shift+Tab) (fd627b91) |
| keybindings#Default shortcuts | toggle_fleet_search Alt+F | covered-final | final/51-terminal-keys.txt, final/51-terminal-keys.jpg (Alt+F works inside the search overlay, its documented context; outside it types ƒ) (fd627b91) |
| keybindings#Default shortcuts | Markdown pane keys: Shift+PageUp/Down, Ctrl+F find, Ctrl+Shift+C, Enter, Shift+Enter, Escape | covered-final | final/51-markdown-config.txt, final/51-markdown-pane-keys.jpg (all seven keys; find-next scroll left match 2 below the viewport, observation) (fd627b91) |
| keybindings#Default shortcuts | quit Cmd+Q shows the quit dialog | covered-final | final/50-quit-keep-relaunch.txt; 46-quit-dialog.jpg |
| keybindings#Actions without a default shortcut | fork_conversation splits with claude --resume ID --fork-session | covered-earlier | 98-fork-palette.jpg, 99-fork-conversation.jpg |
| keybindings#Actions without a default shortcut | close_window, new_space, move_workspace_to_space, accept_conversation_notice, dismiss_conversation_notice | covered-final | final/51-terminal-keys.txt (new_space has no default and fires once bound; close_window, move_workspace_to_space, accept/dismiss_conversation_notice not pressed) (fd627b91) |
| keybindings#Action reference notes | jump_next_waiting order: waiting, then error, then unseen finished, most recent first, every workspace, wraps | fail | C14: final/51-jump-next-waiting.txt, final/51-jump-next-waiting.jpg (order waiting, error, unseen finished, active space first, wraps: as documented; but a target in a tab that is not selected is never shown) (fd627b91) |
| keybindings#Action reference notes | After Cmd+Alt+S, arrows, Home, End move through workspaces and Agents; Enter opens the agent's pane | covered-final | final/40-mouse-pass.txt step 13 (Down), final/21-focus-sidebar.jpg; Home, End and Enter on an agent row not recorded |
| keybindings#Action reference notes | Shift+F10 opens the row's context menu at the row; arrows, Enter, Escape inside | covered-final | final/22-shift-f10-tab-menu.jpg, final/40-62-shift-f10-menu.jpg..64, final/12-sidebar-ax.txt |
| keybindings#Action reference notes | open_workspace_in_windsurf no longer resolves | covered-final | final/50-config-keys.txt |
| keybindings#Action reference notes | Code editor Cmd+F find bar, Cmd+Shift+M comment, Cmd+Enter sends the comment to an agent | covered-final | final/51-editor-dock.txt, final/51-comment-send.jpg (Cmd+Shift+M on the caret line, Cmd+Enter, agent list, path:line + comment + quoted lines pasted after the bracketed-paste prompt) (fd627b91) |
| keybindings#Action reference notes | next_space and previous_space wrap through Default and the spaces, and do nothing with no space | covered-final | final/51-terminal-keys.txt (Default -> Work -> Play -> Default; spaces=[] removes the Space button and next_space does nothing; previous_space not pressed) (fd627b91) |
| keybindings#Customize a shortcut | A custom binding replaces the action's default: secondary+shift+t new_tab removes Cmd+Alt+T and takes Cmd+Shift+T from undo_close_pane | covered-final | final/50-config-keys.txt; 92-custom-shortcuts.jpg |
| keybindings#Customize a shortcut | none disables a default; removing the entry restores it | covered-final | final/50-config-keys.txt; 93-none-no-split.jpg, 93-tab1-not-split.jpg, 94-default-restored.jpg |
| keybindings#Customize a shortcut | + and - separators name the same combination; cmd and ctrl explicit modifiers; alt+1 select_workspace_1 | covered-final | final/51-terminal-keys.txt (cmd-shift-y with dash separator and explicit cmd, ctrl+shift+u, alt+1 -> select_workspace_1, hot reload) (fd627b91) |

### scripting.md

| page | claim | verdict | evidence |
|---|---|---|---|
| scripting.md#intro | `paneflow` verbs run as a CLI client and exit before GPUI starts | covered-final | `final/01-verbs-send-up-wait.txt` |
| scripting.md#tldr | `ps --json`, `status <target> --json`, `read <target> --lines N` from outside a pane | covered-final | `final/01-verbs-send-up-wait.txt` |
| scripting.md#tldr | Target panes by id and by name | covered-final | `final/01-verbs-send-up-wait.txt` |
| scripting.md#tldr | Target panes by `cmdline:<substr>` and `cwd:<path>` | covered-earlier | `cli/03-selectors.txt` |
| scripting.md#which-interface | Inside a pane `PANEFLOW_SOCKET_PATH` is injected; the CLI and MCP bridge share the socket | covered-earlier | `cli/09-principals.txt` (env), `cli/scripts/inpane-principal.out` |
| scripting.md#inspect | `ls --human` lists the panes of the active workspace | covered-final | `final/01-verbs-send-up-wait.txt`; report row: ls fixed (`2ae87dab`), `113-fix-verification.txt` |
| scripting.md#inspect | `search <target> <pattern> --max 5` | covered-earlier | `cli/04-read-search.txt`, `cli/28-blog.txt` (final/01 runs `search` without `--max`) |
| scripting.md#inspect | `status` and `read --json` carry `output_generation`, absent when unknown | covered-final | `final/01-verbs-send-up-wait.txt`, `cli/04-read-search.txt` |
| scripting.md#inspect | `status --json` fields `hooked`, `state`, `state_seq` (state never `unknown`) | covered-final | `final/01-verbs-send-up-wait.txt`, `cli/06-fake-agent.txt` |
| scripting.md#inspect | `hooked` is true only for a hook from the current launch | covered-earlier | `cli/21-lifecycle.txt`, `cli/30-followup.txt` |
| scripting.md#inspect | Without a window, `hooked` and `output_generation` come from the host and `state` from the worker | covered-earlier | `cli/23-windowless.txt`; report row: windowless status fixed (`4cecdd7f`) |
| scripting.md#inspect | `watch` streams NDJSON from `events.subscribe`; `--surface` | covered-final | `final/11-flow-watch-mcp.txt` |
| scripting.md#inspect | `watch --type ai.stop`, `--type ai.notification --type surface_changed` | covered-earlier | `cli/21-watch-stop-only.jsonl`, `cli/21-watch-notif-surface.jsonl` |
| scripting.md#write | `send` stages text without Enter; `--submit` presses it | covered-final | `final/01-verbs-send-up-wait.txt`, `cli/02-gates.txt` |
| scripting.md#write | `send --report-file <path> --submit` | covered-earlier | `cli/27-send-extras.txt` |
| scripting.md#write | `key <target> ctrl-c` | covered-final | `final/01-verbs-send-up-wait.txt` |
| scripting.md#who-is-calling | `api call system.whoami` prints the principal on window, worker and host | covered-final | `final/02-principals.txt` |
| scripting.md#who-is-calling | Principals `window` and `worker` held by the window and the worker | covered-final | `final/02-principals.txt` (F), `final/04-mcp-write-approval.txt`, `final/03-session-input-journal.txt` |
| scripting.md#who-is-calling | Principal `pane` for a pane process and its descendants | covered-final | `final/02-principals.txt` (C) |
| scripting.md#who-is-calling | Principal `external` for a process outside every pane | covered-final | `final/02-principals.txt` (A) |
| scripting.md#who-is-calling | `unverified` may call only `system.whoami`, `system.capabilities`, `system.describe` | covered-final | `final/02-principals.txt` (D), `cli/40-principals.txt` |
| scripting.md#who-is-calling | When the host is down, the window and worker read the session manifests, so a surviving pane process still reads `pane` | unavailable | final/51-host-down-whoami.txt: a pane descendant read 'pane' while the window was closed, but 'host stop --force' ends every pane process (a nohup'd, disowned probe included), so no pane process outlives the host; killing the host by signal is outside the audit's safety rules (fd627b91) |
| scripting.md#who-is-calling | A non-Paneflow process claiming window or worker (hello or `layout.attach`) is `external`, logged once | covered-final | `final/02-principals.txt` (B, E) |
| scripting.md#who-is-calling | A byte-identical copy of this installation's binary, or the process that started the server, is vouched for as window or worker | covered-final | `final/51-identity-hooks.txt` (1, 2): python3 claiming window or worker is answered external and logged once; a worker run from a byte-identical copy of the binary is vouched for, one from an altered copy is logged 'not Paneflow's, so it is treated as external' (fd627b91) |
| scripting.md#who-is-calling | Linux caller identity: `SO_PEERPIDFD`, `SO_PEERCRED` fallback, `hidepid=2` behavior | unavailable | Linux-only: report: `sandbox-matrix.sh --suite identity` must run on a Linux KVM host |
| scripting.md#who-is-calling | Windows Job Object per pane, `GetNamedPipeClientProcessId`, `OpenConsole.exe` outside | unavailable | Windows-only: none |
| scripting.md#who-is-calling | macOS `LOCAL_PEERTOKEN` identity; `setsid` plus double-fork reads `external` | covered-final | `final/02-principals.txt`, `cli/09-principals.txt` |
| scripting.md#who-is-calling | Intel Macs not measured | unavailable | no Intel Mac: report: Unavailable table |
| scripting.md#who-is-calling | An `init.lua` action from a key or the palette acts as the window; `paneflow actions run` acts as its caller | fail | (docs) final/50-mcp-actions.txt: the CLI has no 'paneflow actions run'; 'paneflow do <name>' runs an action |
| scripting.md#who-is-calling | A pane never reaches `layout.sweep`, `approval.follow`, `approval.decide`, `layout.attach` | covered-final | `final/02-principals.txt` (C) |
| scripting.md#who-is-calling | A pane never reaches `layout.launch`, `session.runtime.bind`, `session.workspace`, `host.shutdown`, `peer.classify` | covered-earlier | `cli/09-principals.txt`, `cli/scripts/inpane-reserved.out`, `cli/scripts/inpane-shutdown.out` |
| scripting.md#who-is-calling | A pane's `scope_session` is replaced by its own session; a foreign `source_session` is refused | covered-final | `final/51-scope-grants.txt` (1): from a pane, surface.list with another workspace's scope_session listed only the caller's workspace; pane.write with a foreign source_session refused 'source_session must be the calling pane's own session' (fd627b91) |
| scripting.md#who-is-calling | A pane-scoped write or stop stays in its workspace unless orchestration is open | covered-earlier | `cli/17-workspace-mismatch.txt`, `cli/scripts/inpane-scope.out`, `cli/scripts/inpane-scope-xws.out` |
| scripting.md#gates | Gates apply to the next request after saving `paneflow.json` | covered-final | `final/01-verbs-send-up-wait.txt` (scripting=true then send), `final/11-flow-watch-mcp.txt` |
| scripting.md#gates | `ipc.scripting` opens `surface.send_text`, `surface.send_keystroke`, `session.input` and implies orchestration | covered-final | `final/01-verbs-send-up-wait.txt`, `final/03-session-input-journal.txt` |
| scripting.md#gates | `ipc.orchestration` alone lets a request launch a command, prompt or env and `--scope all` | covered-final | final/50-restore-layout.txt; `cli/14-up.txt`, `cli/43-restore-layout.txt`, `cli/18-grants.txt` |
| scripting.md#gates | `ai_unrestricted` opens both gates for every caller | covered-earlier | `cli/26-config-keys.txt`, `cli/02-gates.txt` |
| scripting.md#gates | `ai_injection_fence` (default true) wraps peer output on the read path | covered-earlier | `cli/26-config-keys.txt`, `cli/04-read-search.txt` |
| scripting.md#gates | The window and worker always pass the gates | covered-final | `final/03-session-input-journal.txt`, `final/04-mcp-write-approval.txt` |
| scripting.md#gates | A malformed `ipc` value logs `invalid config key ipc.scripting: expected true or false` and keeps both gates closed for panes | covered-earlier | `cli/11-config-gates.txt` |
| scripting.md#gates | Deprecated `PANEFLOW_IPC_*` open gates for external callers, for panes only without an `ipc` section, with a warning | covered-earlier | `cli/24-envgate.txt`, `cli/02-gates.txt` |
| scripting.md#gates | A refused request keeps its former message naming `PANEFLOW_IPC_SCRIPTING` | covered-final | `final/03-session-input-journal.txt`, `final/08-tmux-compat.txt` |
| scripting.md#grants | Rights `write`, `spawn`, `layout`; `workspace` vs `instance` scope | covered-final | final/50-grants.txt; `cli/18-grants.txt`, `cli/46-grant-ceiling.txt` |
| scripting.md#grants | A `workspace` spawn grant opens only a split beside a surface of its own workspace; elsewhere `scope_refused` | covered-final | `final/51-scope-grants.txt` (4): after an allowed workspace spawn grant, a split with a command beside the pane's own surface succeeded and one in workspace other was refused ('covers its own workspace ... needs an instance grant'); no grant: -32601 (fd627b91) |
| scripting.md#grants | `session.create`, `workspace.create` and `layout` need an `instance` grant; `grant.request` refuses `layout` with `workspace` | covered-final | final/50-grants.txt; `cli/18-grants.txt` (layout workspace refused); session.create/workspace.create with a workspace grant not exercised |
| scripting.md#grants | A grant ends at expiry, when its session ends, or when the session restarts | covered-final | final/50-grants.txt (session end; expiry and restart not observed); earlier: none (expiry shown as `119s left` only in `final/05-grant-prompt.txt`) |
| scripting.md#grants | At most 128 grants, 16 per pane; past that `grant_limit` and no prompt | covered-final | final/50-grants.txt; `cli/46-grant-ceiling.txt` |
| scripting.md#grants | `grant.request` prompt names agent, pane, rights, scope and duration; title says so for `instance` or no expiry | covered-final | `final/05-grant-prompt.txt`, `final/13-grant-prompt.jpg`, `final/14-grant-instance-prompt.jpg` |
| scripting.md#grants | Reply `granted: true` with `id` and `expires_at_ms`, or `reason: denied` | covered-final | `final/05-grant-prompt.txt`, `121-ax-prompt-palette.txt` |
| scripting.md#grants | `unanswered` when nobody answers before the timeout | covered-final | final/50-grants.txt; `cli/18-grants.txt`, `cli/scripts/inpane-grant.out` |
| scripting.md#grants | `unanswered` when no window could show the prompt | covered-final | final/50-quit-keep-relaunch.txt |
| scripting.md#grants | Only a process in a pane may call `grant.request` | covered-final | final/50-grants.txt |
| scripting.md#grants | Profile grants under `agents.<name>.grants` reach the pane when the agent runs in its foreground | covered-final | final/50-grants.txt; `cli/18-grants.txt`, `cli/46-grant-ceiling.txt` |
| scripting.md#grants | An unknown right logs `invalid config key agents.claude.grants: unknown right` and stores no grant of that profile | covered-final | final/50-grants.txt; `cli/18-grants.txt` |
| scripting.md#grants | `grant.delegate` within the holder's grant, else `grant_exceeds_holder`; non-spawned pane `not_spawned_by_caller`; revocation cascades | covered-final | final/50-grants.txt; `cli/18-grants.txt`, `cli/46-grant-ceiling.txt`; report row: window split records its spawner fixed (`3d3fac6e`), tests only |
| scripting.md#grants | `grants ls` and `grants revoke <id>`; from a pane only own or delegated, `principal_refused` otherwise | covered-final | final/50-grants.txt; `cli/18-grants.txt`, `cli/46-grant-ceiling.txt`; `final/05-grant-prompt.txt` (ls from a pane) |
| scripting.md#grants | Grants survive a worker restart while their sessions run | covered-final | final/50-grants.txt |
| scripting.md#grants | `grants_unsynced` with `Grants are being synchronized; retry the call` while a mirror lags | covered-final | final/50-grants.txt; `cli/45-grants-unsynced.txt` |
| scripting.md#grants | Host logs each accepted control-client `session.input` at info with client, session, byte count, never content | covered-final | `final/03-session-input-journal.txt` |
| scripting.md#grants | Hook delivery (`agent.event`) is ungated and accepted only for the live launch generation | covered-final | `final/51-identity-hooks.txt` (3): with every gate closed, agent.event for the live generation is accepted; generation 8 is refused 'the event names a generation this session has not reached' (fd627b91) |
| scripting.md#sandbox | A sandboxed pane: own endpoints under `panes/<session>/`, `whoami --on host` answers `pane`, shield, `sandboxed: true`, children sandboxed, `principal_refused` list | unavailable | sandbox not offered on macOS: `cli/19-sandbox.txt` |
| scripting.md#sandbox | Linux bubblewrap, Landlock, runtime directory filtering, Ubuntu 24.04 AppArmor refusal | unavailable | Linux-only: none |
| scripting.md#sandbox | Windows logs `Sandbox 'paneflow' is not available on windows` | unavailable | Windows-only: none |
| scripting.md#sandbox | macOS logs `Sandbox 'paneflow' is not available on macos` and the profile does not start | covered-final | final/50-sandbox.txt; `cli/19-sandbox.txt` |
| scripting.md#sandbox | macOS Seatbelt measurements (sockets, signals, `ps` refused, no nesting) | unavailable | sandbox not offered on macOS: report: Unavailable table |
| scripting.md#sandbox | An unknown `sandbox` value logs an error naming the key and does not start | covered-final | final/50-sandbox.txt; `cli/19-sandbox.txt` |
| scripting.md#sandbox | A sandboxed profile never runs unsandboxed; a restored pane starts sandboxed or not at all | unavailable | sandbox not offered on macOS: none |
| scripting.md#scope | In a pane, `send` and `key` pass `PANEFLOW_SESSION_ID`; a write into another workspace is refused with a message naming it | covered-final | final/50-grants.txt; `cli/17-workspace-mismatch.txt`, `cli/scripts/inpane-scope-xws.out`, `cli/18-grants.txt` |
| scripting.md#scope | `--scope all` reaches every workspace and needs `ipc.orchestration` | covered-final | final/50-grants.txt; `cli/18-grants.txt`, `cli/30-followup.txt`, `cli/scripts/inpane-scope.out` |
| scripting.md#scope | Flow `send` steps to a pane the flow did not spawn carry the pane's session | covered-final | `final/51-scope-grants.txt` (2): a flow run inside pane left whose send step targets o1 (not spawned by the flow) was refused naming workspace other; the refusal is labeled 'instance unreachable' as in C3 (fd627b91) |
| scripting.md#scope | A caller session the instance does not know is refused | covered-final | `final/51-scope-grants.txt` (3): an external send with an unknown PANEFLOW_SESSION_ID was refused 'unknown caller session ... the write is refused' (fd627b91) |
| scripting.md#scope | A script outside Paneflow keeps the instance scope | covered-final | `final/01-verbs-send-up-wait.txt` (external sends across ws-a and fleet) |
| scripting.md#send-refusal | `send` exits 3 with no byte written toward an agent waiting for a decision or out of the foreground; `--force` logged | covered-earlier | `cli/08-send-refusal.txt`, `cli/30-followup.txt` |
| scripting.md#send-refusal | A pane without an agent is never held | covered-earlier | `cli/08-send-refusal.txt` |
| scripting.md#submit | `--submit` reply `delivered`, `started`, `reason`, `state` | covered-final | `final/01-verbs-send-up-wait.txt` |
| scripting.md#submit | `started: null` with `no_signal` when the agent declares no OSC 7501 | covered-final | `final/01-verbs-send-up-wait.txt`, `cli/02-gates.txt` |
| scripting.md#submit | `started: false` (`no_state_transition`) exits 1 | covered-earlier | `cli/06-fake-agent.txt`; report row: short turn fixed (`377d7517`), test only |
| scripting.md#submit | `inline_cr` reply stops at the delivery fields and exits 0 | covered-earlier | `cli/06-fake-agent.txt`, `cli/23-windowless.txt` |
| scripting.md#submit | `deferred_paste_cr` after a bracketed paste | covered-final | `final/01-verbs-send-up-wait.txt`, `final/04-mcp-write-approval.txt` |
| scripting.md#wait | `wait --idle` returns when an OSC 7501 agent turns idle, attention or blocked | covered-final | `final/01-verbs-send-up-wait.txt` |
| scripting.md#wait | `wait --idle` returns at once when the agent already waits; `--for` ignored there | covered-earlier | `cli/08-send-refusal.txt`, `cli/07-wait.txt` |
| scripting.md#wait | Other panes wait for `--for` ms of quiet output | covered-earlier | `cli/07-wait.txt` |
| scripting.md#send-options | `--raw` turns the fence off | covered-final | `final/04-mcp-write-approval.txt`, `final/07-worktrees.txt` |
| scripting.md#send-options | `--paste` forces bracketed-paste delivery | covered-earlier | `cli/27-send-extras.txt` |
| scripting.md#up | `paneflow up <file>` with panes, cwd, agent, command, prompt prefill, env, worktrees | covered-final | `final/01-verbs-send-up-wait.txt`, `cli/14-up.txt` |
| scripting.md#up | `up --dry-run` validates the resolved plan without mutating; prompts prefilled, not submitted | covered-final | `final/01-verbs-send-up-wait.txt`, `cli/14-up.txt` |
| scripting.md#spaces | `new --space <name>` in any case, by id, `Default`; without it the shown space | covered-final | final/50-spaces-config.txt; `cli/13-spaces.txt` |
| scripting.md#spaces | Top-level `space` key in a workspace TOML | covered-earlier | `cli/14-up.txt` (space.toml) |
| scripting.md#spaces | Unknown space fails before creating with `unknown space: <name>` and exit 1 | covered-final | final/50-spaces-config.txt; `cli/13-spaces.txt`, `cli/14-up.txt` |
| scripting.md#spaces | `workspace.list`, `workspace.current`, `ls` carry `space: {id, name}` or null; `ls --human` SPACE column | covered-final | final/50-spaces-config.txt; `cli/13-spaces.txt` |
| scripting.md#spaces | `select <index>` and `up` switch to the workspace's space; `new` never changes it | covered-final | final/50-spaces-config.txt; `cli/13-spaces.txt`, `cli/14-up.txt` |
| scripting.md#spaces | MCP with `PANEFLOW_MCP_SCOPE=all` reads across spaces | covered-earlier | `cli/16-mcp.txt`, `cli/28-blog.txt` |
| scripting.md#flow | `flow run` with a ready barrier | covered-final | `final/11-flow-watch-mcp.txt` |
| scripting.md#flow | `flow run` with capture, fan-out (`foreach`), a `send` step with `${var}` and a submitting pane, end to end | fail | final/50-flows.txt: passes end to end in 3 of 9 runs; F-flow-1 fan-out prompt lost, F-flow-2 -32070 once, F-flow-3 multiline send aborts the flow |
| scripting.md#flow | Submitting a step needs the write gate, checked up front including `--dry-run` | covered-final | final/50-flows.txt; `cli/15-flow.txt`; `final/11-flow-watch-mcp.txt` (orchestration message) |
| scripting.md#mcp | Tools `list_panes`, `read_pane`, `search_pane`, `write_pane` | covered-final | `final/11-flow-watch-mcp.txt`, `final/04-mcp-write-approval.txt` |
| scripting.md#mcp | Tools `ask_human`, `list_actions`, `run_action` exist with their descriptions | covered-earlier | `cli/16-mcp.txt` (tools/list) |
| scripting.md#mcp | First write raises "*source* wants to write into this pane" with Allow for this agent session and Deny; `approval_pending` until decided | covered-final | `final/04-mcp-write-approval.txt`, `final/10-mcp-approval-bar.jpg`, `final/12-mcp-write-allowed.jpg` |
| scripting.md#mcp | The command palette offers Allow and Deny | covered-earlier | `cli/30-palette-allow.jpg`, `cli/30-palette-deny.jpg` |
| scripting.md#mcp | An approval ends when either agent is relaunched | covered-final | `final/04-mcp-write-approval.txt`, `final/11-mcp-approval-after-relaunch.jpg` |
| scripting.md#mcp | An approval ends when the host restarts | covered-earlier | `cli/30-followup.txt` |
| scripting.md#mcp | A request nobody answers is refused after 120 s | covered-earlier | `cli/17-workspace-mismatch.txt` (host.log "expired unanswered") |
| scripting.md#mcp | With no window open, the tool says that no window can approve the write | covered-final | final/50-quit-keep-relaunch.txt; earlier: `cli/23-windowless.txt` shows the pre-fix failure only (fixed `f7808aa5`, test only) |
| scripting.md#mcp | An agent cannot write into its own pane | covered-earlier | `cli/17-workspace-mismatch.txt` |
| scripting.md#mcp | A write stays in the caller's workspace unless `PANEFLOW_MCP_SCOPE=all` and `ipc.orchestration` | covered-earlier | `cli/16-mcp.txt`, `cli/30-followup.txt` |
| scripting.md#mcp | Prefix `[Paneflow: message from <name>, surface <number>]` | covered-final | `final/04-mcp-write-approval.txt` |
| scripting.md#mcp | Control characters stripped except newline and tab, 16 KiB cap, 1 write per second with a burst of 3, refuses a pane waiting for a decision | covered-earlier | `cli/30-followup.txt`, `cli/17-workspace-mismatch.txt` |
| scripting.md#mcp | Each request and decision logged at info without the text | covered-final | `final/04-mcp-write-approval.txt` (host.log) |
| scripting.md#mcp | With `submit`, the result reports `started` and `state` | covered-final | `final/04-mcp-write-approval.txt` |
| scripting.md#mcp | Only the Paneflow window grants an approval | covered-final | `final/02-principals.txt` (B: foreign `approval.decide` refused) |
| scripting.md#mcp | Annotations: read tools and `list_actions` read-only; `write_pane`, `run_action` destructive; `ask_human` not read-only, not destructive | covered-earlier | `cli/16-mcp.txt` |
| scripting.md#mcp | `ask_human` shows the question in the pane, `options`/`multiple`, waits 20 s, `answered` or `pending` with `prompt_id`; terminal statuses | covered-final | final/50-mcp-actions.txt (answered path; pending and the error statuses not observed); earlier: none (listed by tools/list only) |
| scripting.md#mcp | `list_actions` returns `name`, `title`, `description`, args schema | covered-final | final/50-mcp-actions.txt |
| scripting.md#mcp | `run_action` refuses an action not marked for agents, waits 20 s, `done` or `running` with `run_id` | covered-final | final/50-mcp-actions.txt |
| scripting.md#mcp | Questions and runs share a burst of 3 then 1 per second, `rate_limited` past it | covered-final | final/50-mcp-actions.txt |
| scripting.md#mcp | `mcp install`, `mcp status`, `mcp uninstall`; covers Claude Code, Codex, Gemini CLI, opencode without clobbering | covered-final | final/50-integrations-hooks-skills.txt; `cli/16-mcp.txt`, `cli/20-hooks.txt` |
| scripting.md#hooks | `integrations list`, `integrations install claude-code`, `integrations install codex`, `hooks status` | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` |
| scripting.md#hooks | Installing also registers the `paneflow` MCP server | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` |
| scripting.md#hooks | **Paneflow hooks** toggle in Settings > Agents installs the hooks | covered-final | final/52-agent-config-dirs.txt, final/52-hooks-toggle.jpg (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| scripting.md#hooks | Codex asks once to trust the new hooks with `/hooks` | unavailable | Codex not signed in: none |
| scripting.md#hooks | The hooks exit immediately outside a Paneflow pane | covered-final | final/50-integrations-hooks-skills.txt |
| scripting.md#hooks | `hooks setup` installs every detected runtime that has an installer | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` |
| scripting.md#hooks | Hook metadata (session id, prompt, last result) reaches `ps`, `status`, `watch`, `sessions` | covered-earlier | `cli/21-lifecycle.txt`, `cli/21-watch-all.jsonl`, `cli/30-followup.txt` |
| scripting.md#hooks | The worker outlives the window; `status`, `send --submit`, `wait --idle` read it with no window | covered-earlier | `cli/23-windowless.txt` |
| scripting.md#hooks | `sessions` takes `--json` and `--follow` | covered-earlier | `cli/22-worker.txt`, `cli/22-sessions-follow.txt` |

### scripting/reference.md

| page | claim | verdict | evidence |
|---|---|---|---|
| reference.md#cli-verbs | Unknown verbs exit 2 | covered-final | `final/01-verbs-send-up-wait.txt` |
| reference.md#cli-verbs | `ls`, `read`, `search`, `ps`, `status`, `new`, `send`, `key`, `wait`, `up` | covered-final | `final/01-verbs-send-up-wait.txt` |
| reference.md#cli-verbs | `select <index>` | covered-final | `final/07-worktrees.txt` |
| reference.md#cli-verbs | `split <h or v>` | covered-earlier | `cli/03-selectors.txt`, `cli/17-workspace-mismatch.txt` |
| reference.md#cli-verbs | `focus <target>` focuses a pane and its workspace | covered-earlier | `cli/27-send-extras.txt` |
| reference.md#cli-verbs | `watch`, `flow run` | covered-final | `final/11-flow-watch-mcp.txt` |
| reference.md#cli-verbs | `sessions [--json] [--follow]` | covered-earlier | `cli/22-worker.txt`, `cli/22-sessions-follow.txt` |
| reference.md#cli-verbs | Aliases `list_panes`, `read_pane`, `search_pane` | covered-earlier | `cli/04-read-search.txt` |
| reference.md#selectors | Numeric id and name | covered-final | `final/01-verbs-send-up-wait.txt` |
| reference.md#selectors | `cmdline:` matches the executable basename on macOS; `cwd:` | covered-earlier | `cli/03-selectors.txt` |
| reference.md#selectors | `cmdline:` matches the full argv on Linux | unavailable | Linux-only: none |
| reference.md#selectors | No match or several matches exit 3 | covered-final | final/50-sandbox.txt; `cli/03-selectors.txt`, `cli/19-sandbox.txt` |
| reference.md#selectors | `send --broadcast`, `wait --any`, `wait --all` accept several matches | covered-earlier | `cli/27-send-extras.txt`, `cli/07-wait.txt` |
| reference.md#exit-codes | 1 runtime failure (gate refused, unreachable) | covered-final | `final/01-verbs-send-up-wait.txt`, `final/05-grant-prompt.txt` |
| reference.md#exit-codes | 4 on `wait` timeout | covered-final | `final/01-verbs-send-up-wait.txt` |
| reference.md#exit-codes | 4 on a flow ready timeout | covered-final | final/50-flows.txt; earlier: `cli/15-flow.txt` timeout runs failed on the pre-`7ad62b3f` race with exit 1 |
| reference.md#write-gates | `send`, `send --submit`, `key` need `ipc.scripting` or `ai_unrestricted` | covered-final | `final/01-verbs-send-up-wait.txt` |
| reference.md#write-gates | `send/key/flow run --scope all` from a pane need `ipc.orchestration` | covered-final | final/50-grants.txt; `cli/18-grants.txt`, `cli/scripts/inpane-scope.out` |
| reference.md#write-gates | Host `session.input` from a control client needs `ipc.scripting` | covered-final | `final/03-session-input-journal.txt` |
| reference.md#write-gates | Host `session.create` from a control client: orchestration with command, prompt or env, scripting otherwise | covered-final | `final/51-scope-grants.txt` (5): control client: plain create needs ipc.scripting, command or env needs ipc.orchestration (opened by scripting, which implies it) (fd627b91) |
| reference.md#write-gates | Flow step with `submit = true` needs the scripting capability from `system.capabilities` | covered-final | final/50-flows.txt; `cli/15-flow.txt` |
| reference.md#write-gates | `key` rejects `enter`, `ctrl-m`, `ctrl-j` | covered-earlier | `cli/02-gates.txt` |
| reference.md#write-gates | A `surface.send_text` payload is capped at 64 KiB | covered-earlier | `cli/02-gates.txt` |
| reference.md#config-keys | `submit_paste_delay_ms` (default 70) | covered-final | final/50-config-keys.txt |
| reference.md#config-keys | `terminal.env` injected into new terminals | covered-earlier | `cli/26-config-keys.txt` |
| reference.md#read-fields | `text`, `lines`, `total_lines`, `eof`, `output_generation` | covered-final | final/50-flows.txt; `cli/04-read-search.txt`, `cli/15-flow-race-probe.txt` |
| reference.md#read-fields | `lines` default 200, clamped 1 to 4000; out-of-range offset is invalid params | covered-earlier | `cli/04-read-search.txt` |
| reference.md#read-fields | `fenced` defaults to `ai_injection_fence`; `--raw` sends `fenced: false` | covered-earlier | `cli/04-read-search.txt`, `cli/26-config-keys.txt` |
| reference.md#agent-state | `ps --json` returns `{"agents":[...]}` with the listed fields | covered-earlier | `cli/06-fake-agent.txt`, `cli/21-lifecycle.txt`, `cli/30-followup.txt` |
| reference.md#agent-state | `state` values including `unknown_running`; `reason` `no_hook` | covered-earlier | `cli/21-lifecycle.txt` |
| reference.md#agent-state | Empty fleet `{"agents":[]}` exit 0; a pane with no agent returns idle | covered-earlier | `cli/06-fake-agent.txt`, `cli/03-selectors.txt` |
| reference.md#workspace-spec | Top-level `name`, `space`, `layout`, `port_base`, `[[panes]]` | covered-earlier | `cli/14-up.txt`, `cli/scripts/full.toml` |
| reference.md#workspace-spec | Pane `cwd` must exist; `agent` and `command` exclusive; `prompt` never submitted; `focus`; `env` with `${port_offset}`; `name` | covered-earlier | `cli/14-up.txt` |
| reference.md#workspace-spec | Pane `worktree`, `from`, `copy_env`, `setup`, `setup_timeout_secs`, `worktree_teardown` | covered-earlier | `cli/14-up.txt`, `cli/scripts/wt-clean.toml`, `worktrees/19-up-teardown.txt` |
| reference.md#workspace-spec | Unknown keys are errors; paths validated before panes are created | covered-earlier | `cli/14-up.txt` (unknownkey.toml, badcwd.toml) |
| reference.md#flow-spec | `id`, `needs`, `pane`, `ready` | covered-final | `final/11-flow-watch-mcp.txt` |
| reference.md#flow-spec | `foreach`, `send`, `capture` (1 to 500 lines), `submit`, `${item}`, `${var}`, `${var.<item>}` | fail | final/50-flows.txt: F-flow-1 (a foreach pane's prompt is often never typed) |
| reference.md#flow-spec | Validation: cycles, undefined captures | covered-final | final/50-flows.txt; `final/11-flow-watch-mcp.txt` (cycle), `cli/15-flow.txt` (unknown variable) |
| reference.md#flow-spec | Validation: unknown keys, missing dependencies, invalid regexes, pane budget, missing timeouts | covered-final | final/50-flows.txt |
| reference.md#flow-spec | `Ctrl-C` stops the loop and leaves created panes | covered-final | final/50-flows.txt |
| reference.md#jsonrpc | Linux endpoint and its fallbacks | unavailable | Linux-only: none |
| reference.md#jsonrpc | macOS default endpoint `$TMPDIR/paneflow/paneflow.sock` | covered-earlier | `cli/25-jsonrpc.txt` (socket listed, not connected) |
| reference.md#jsonrpc | macOS fallback `~/Library/Caches/run` when `TMPDIR` is unset; `XDG_RUNTIME_DIR` ignored | unavailable | the ~/Library/Caches/run fallback applies only to the default home (the installed app's ~/.paneflow, out of bounds); checked instead in `final/51-jsonrpc.txt`: with TMPDIR unset an isolated home falls back to /tmp, and XDG_RUNTIME_DIR is ignored on macOS (fd627b91) |
| reference.md#jsonrpc | Windows endpoint `\\.\pipe\paneflow`, one request per pipe connection | unavailable | Windows-only: none |
| reference.md#jsonrpc | Isolated home listens on `paneflow-ipc-<16 hex>.sock` in `$TMPDIR` | covered-final | `final/02-principals.txt` |
| reference.md#jsonrpc | A foreign `PANEFLOW_SOCKET_PATH` is ignored unless `PANEFLOW_ALLOW_SOCKET_OVERRIDE=1` | covered-final | `final/51-jsonrpc.txt`: a foreign PANEFLOW_SOCKET_PATH is ignored (call answered by this home's window); with PANEFLOW_ALLOW_SOCKET_OVERRIDE=1 it is used (-32030, no window there) (fd627b91) |
| reference.md#jsonrpc | Newline-delimited JSON-RPC 2.0; several requests on one connection answered in order | covered-earlier | `cli/25-jsonrpc.txt` |
| reference.md#jsonrpc | The connection closes after 30 s without a request | covered-final | `final/51-jsonrpc.txt`: the window endpoint closed after 30.0 s idle, with and without a prior request (fd627b91) |
| reference.md#jsonrpc | `events.subscribe` turns the connection into the stream | covered-earlier | `cli/25-jsonrpc.txt` |
| reference.md#jsonrpc | Connection caps and bounded queues return `-32000` or `dropped` | covered-final | final/50-dropped-busy.txt; `cli/41-dropped-busy.txt`; report row: request id fixed (`c7bba5a2`) |
| reference.md#jsonrpc | The `nc -U` capabilities probe | covered-earlier | `cli/25-jsonrpc.txt` |
| reference.md#methods | `system.ping`, `system.capabilities`, `system.identify` | covered-final | `final/01-verbs-send-up-wait.txt` |
| reference.md#methods | `workspace.list`, `workspace.current`, `workspace.create` (`space`), `workspace.select`, `workspace.close`, `workspace.up` | covered-final | final/50-spaces-config.txt; `final/01-verbs-send-up-wait.txt`, `cli/13-spaces.txt` |
| reference.md#methods | `workspace.restore_layout` applies a layout tree, command and prompt run | covered-final | final/50-restore-layout.txt; `cli/43-restore-layout.txt`; report row: fixed (`dace0435`), test only after |
| reference.md#methods | `surface.list` fields including `space` | covered-final | final/50-spaces-config.txt; `cli/13-spaces.txt`, `cli/03-selectors.txt` |
| reference.md#methods | `surface.read`, `surface.search` (case-insensitive), `surface.status` | covered-earlier | `cli/04-read-search.txt`, `cli/08-send-refusal.txt` |
| reference.md#methods | `surface.rename` renames or clears a name | covered-earlier | `cli/03-selectors.txt` (rename only, clear not shown) |
| reference.md#methods | `surface.focus` | covered-earlier | `cli/27-send-extras.txt`, `worktrees/26-keep-limit.txt` |
| reference.md#methods | `surface.send_text` and `surface.send_keystroke` gated | covered-final | `final/01-verbs-send-up-wait.txt`, `final/05-grant-prompt.txt` |
| reference.md#methods | `surface.split` reply fields; with `worktree`, `split: false`, `new_tab: true`, `tab_id`, `notices` | covered-final | `final/07-worktrees.txt`, `worktrees/15-worktreeinclude.txt` |
| reference.md#methods | `surface.split` worktree `setup` needs the orchestration gate | covered-earlier | `worktrees/17-snapshot-setup.txt`, `cli/14-up.txt` |
| reference.md#methods | `tmux.compat` own panes, `{stdout, stderr, exit}`, `send-keys` under `ipc.scripting` | fail | final/50-tmux-send-keys-scripting.txt: send-keys Enter or C-m is always refused, so a team can type into a teammate but never submit; `final/08-tmux-compat.txt` (send-keys with scripting closed); `cli/42-tmux-compat.txt` (typed once true) |
| reference.md#methods | `fleet.list` read-only snapshot | covered-final | final/50-spaces-config.txt; `cli/13-spaces.txt` |
| reference.md#methods | `ai.*` methods are no longer accepted on the app socket | covered-earlier | `cli/25-jsonrpc.txt` |
| reference.md#methods | Error codes `-32602`, `-32601`, `-32001`, `-32000` | covered-final | final/50-dropped-busy.txt; `final/02-principals.txt`, `final/03-session-input-journal.txt`; `cli/41-dropped-busy.txt` for `-32000` |
| reference.md#methods | Legacy `{"error":"..."}` inside `result` | covered-final | `final/51-jsonrpc.txt`: eight invalid calls all answer JSON-RPC error envelopes; the window promotes a legacy {"error"} result to -32603 (jsonrpc.rs promote_response), so the legacy shape no longer reaches a window client; client advice holds (fd627b91) |
| reference.md#events | First frame `subscribed`; `surface_changed`; `heartbeat` | covered-final | `final/11-flow-watch-mcp.txt` |
| reference.md#events | `ai.session_start`, `ai.prompt_submit`, `ai.tool_use`, `ai.notification`, `ai.stop`, `ai.exit`, `ai.session_end` | covered-earlier | `cli/21-watch-all.jsonl` |
| reference.md#events | `dropped` when a subscriber lags; resync with `ps` or `status` | covered-final | final/50-dropped-busy.txt; `cli/41-dropped-busy.txt` |
| reference.md#mcp-bridge | `list_panes` fields; `read_pane`, `search_pane` fenced as untrusted | covered-final | `final/11-flow-watch-mcp.txt` |
| reference.md#mcp-bridge | `write_pane` `approval_pending`, then delivery, `started` and `state` with `submit` | covered-final | `final/04-mcp-write-approval.txt` |
| reference.md#mcp-bridge | `ask_human`, `list_actions`, `run_action` behavior | covered-final | final/50-mcp-actions.txt; earlier: none (see scripting.md#mcp rows) |
| reference.md#hooks | `paneflow-ai-hook` sends one event to the host and exits 0 even when Paneflow is stopped | covered-final | final/50-stop-everything.txt; earlier: `cli/30-followup.txt` (fired with Paneflow running only) |
| reference.md#hooks | `integrations install claude-code` (or `claude`) and `codex`, registering the MCP server | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` (`claude` alias not run) |
| reference.md#hooks | `integrations remove` keeps a user handler in the same hook group | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` |
| reference.md#hooks | `hooks uninstall` removes integrations and the MCP entry; `hooks status` one line per runtime, exit 0 | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` |
| reference.md#hooks | Claude hooks in `~/.claude/settings.json`, Codex in `~/.codex/hooks.json` (throwaway HOME) | covered-final | final/50-integrations-hooks-skills.txt; `cli/20-hooks.txt` |
| reference.md#hooks | `$CLAUDE_CONFIG_DIR/settings.json` and `$CODEX_HOME/hooks.json` overrides | covered-final | final/50-integrations-hooks-skills.txt; earlier: `cli/20-hooks.txt` ran with both unset |
| reference.md#worker | `serve start` starts or adopts the worker; `serve status` prints pid, protocol, home, sessions, capabilities | covered-final | `final/06-host-serve-status.txt`; `cli/22-worker.txt` (start) |
| reference.md#worker | `serve stop [--drain-ms 5000]` leaves terminals untouched | covered-earlier | `cli/29-shutdown.txt` (default drain only) |
| reference.md#worker | The worker rebuilds session state from disk when it restarts | covered-earlier | `cli/22-worker.txt`, `cli/30-followup.txt` |
| reference.md#worker | `sessions --follow` reconnects when the worker restarts | covered-final | `final/51-jsonrpc.txt`: sessions --follow printed 'the worker stream dropped (the worker is stopping); reconnecting' and a second bootstrap with resumed=true after serve stop / serve start (fd627b91) |
| reference.md#worker | `sessions ack <id>...` lowers `unread` | covered-earlier | `cli/22-worker.txt` |

### conductor.md

| page | claim | verdict | evidence |
|---|---|---|---|
| conductor.md#TL;DR | ps --json, status --json, read --lines, send without Enter, watch, wait are the conductor verbs | covered-final | final/01-verbs-send-up-wait.txt, final/11-flow-watch-mcp.txt |
| conductor.md#Install the conductor skill | integrations install claude-code writes skills/paneflow-conductor/SKILL.md under ~/.claude with the Paneflow version in its frontmatter | covered-final | final/50-integrations-hooks-skills.txt; cli/20-hooks.txt (paneflow-version 0.20.0) |
| conductor.md#Install the conductor skill | an edited installed copy is kept and reported; delete and install again restores the shipped copy | covered-final | final/50-integrations-hooks-skills.txt; cli/20-hooks.txt ("kept your modified paneflow-conductor skill") |
| conductor.md#Install the conductor skill | integrations remove claude-code removes the skill only when unmodified | covered-final | final/50-integrations-hooks-skills.txt; cli/20-hooks.txt |
| conductor.md#Install the conductor skill | skill install under CLAUDE_CONFIG_DIR when set | covered-final | final/52-agent-config-dirs.txt: the space's 'Add hooks' wrote skills/paneflow-conductor/SKILL.md under its CLAUDE_CONFIG_DIR (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| conductor.md#Install the conductor skill | the Paneflow hooks toggle in Settings > Agents installs the skill and hooks | covered-final | final/52-agent-config-dirs.txt, final/52-hooks-toggle.jpg (skill and hooks written, removed when off) (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| conductor.md#Install the conductor skill | bunx skills add <conductor URL> installs for every detected agent; -a codex installs for one runtime | covered-final | final/50-integrations-hooks-skills.txt; cli/47-skills-add.txt |
| conductor.md#Install the conductor skill | npx, pnpm dlx, yarn dlx variants of skills add | unavailable | final/50-integrations-hooks-skills.txt: npx, pnpm and yarn are not installed on this Mac |
| conductor.md#How do I start a conductor session | PANEFLOW_SOCKET_PATH injected in a pane; outside Paneflow the CLI reaches the instance by discovery or the variable | covered-final | final/02-principals.txt (pane and external terminal), cli/09-principals.txt |
| conductor.md#How do I start a conductor session | ps lists agents across workspaces; ls --human lists panes of the active workspace | covered-final | final/01-verbs-send-up-wait.txt; ls scope fix 2ae87dab in 113-fix-verification.txt |
| conductor.md#How do I read the fleet | hooked true for a tracked agent | covered-earlier | cli/21-lifecycle.txt, cli/30-followup.txt |
| conductor.md#How do I read the fleet | unhooked process appears as unknown_running with reason no_hook | covered-earlier | cli/21-lifecycle.txt |
| conductor.md#How do I read the fleet | state comes from OSC 7501; sessions row has activity_source declared | covered-final | final/01-verbs-send-up-wait.txt (activity_source declared), cli/06-fake-agent.txt |
| conductor.md#How do I read the fleet | integrations install codex reports hooks for Codex | covered-final | final/50-integrations-hooks-skills.txt; cli/20-hooks.txt (Codex not signed in, install only) |
| conductor.md#How do I read the fleet | without a window, status keeps the pane id and the window fields; ps lists worker agents with workspace index and pane name | covered-earlier | cli/23-windowless.txt; fix 4cecdd7f |
| conductor.md#How do I read the fleet | paneflow sessions reads the worker directly | covered-earlier | cli/22-worker.txt, cli/22-sessions-follow.txt |
| conductor.md#How do I read the fleet | paneflow up names panes for stable selectors | covered-final | final/01-verbs-send-up-wait.txt (fleet.toml, agentA, sh1) |
| conductor.md#How do I dispatch work safely | send stages text without Enter by default | covered-final | final/01-verbs-send-up-wait.txt |
| conductor.md#How do I dispatch work safely | --submit needs ipc.scripting true or AI free access; refused otherwise | covered-final | final/01-verbs-send-up-wait.txt (refused, then allowed after ipc.scripting), cli/02-gates.txt, cli/26-config-keys.txt (ai_unrestricted) |
| conductor.md#How do I dispatch work safely | --report-file appends report instructions; refused with --broadcast | covered-earlier | cli/27-send-extras.txt |
| conductor.md#How do I wait for results | watch --surface backend --type ai.stop streams NDJSON lifecycle events | covered-earlier | cli/21-watch-stop-only.jsonl; final/11-flow-watch-mcp.txt (watch --surface) |
| conductor.md#How do I wait for results | watch emits surface_changed | covered-final | final/11-flow-watch-mcp.txt |
| conductor.md#How do I wait for results | wait --idle --pattern returns on either signal | covered-earlier | cli/07-wait.txt |
| conductor.md#How do I wait for results | wait --pattern sentinel such as DONE: or REPORT_DONE | covered-final | final/01-verbs-send-up-wait.txt, cli/07-wait.txt, cli/28-blog.txt |
| conductor.md#How do I wait for results | watch works on Windows | unavailable | Windows-only claim: report: Windows and Linux not covered |
| conductor.md#How do I handle untrusted peer output | read wraps text in untrusted_terminal_output by default | covered-final | final/01-verbs-send-up-wait.txt, final/11-flow-watch-mcp.txt |
| conductor.md#How do I handle untrusted peer output | ai_injection_fence false drops the default fence | covered-earlier | cli/26-config-keys.txt |
| conductor.md#How do I recover full reports | --report-file then status --json confirms the pane state | covered-earlier | cli/27-send-extras.txt |

### conductor/reference.md

| page | claim | verdict | evidence |
|---|---|---|---|
| conductor/reference.md#CLI verbs | ps, status, watch, read, send, wait, up, flow run, ls, search, focus, key exist with the listed methods | covered-final | final/01-verbs-send-up-wait.txt, final/11-flow-watch-mcp.txt; focus in cli/27-send-extras.txt |
| conductor/reference.md#CLI verbs | reads allowed by default; send --submit, key and submitting flow steps need the scripting gate or AI free access | covered-final | final/01-verbs-send-up-wait.txt, final/11-flow-watch-mcp.txt (flow refused, then run under ipc.scripting) |
| conductor/reference.md#Selectors | numeric id and name selectors | covered-final | final/01-verbs-send-up-wait.txt |
| conductor/reference.md#Selectors | cmdline:<substr> and cwd:<path> selectors | covered-earlier | cli/03-selectors.txt, cli/07-wait.txt |
| conductor/reference.md#Selectors | no match or several matches exits 3, except broadcast and wait --any/--all | covered-earlier | cli/03-selectors.txt, cli/07-wait.txt (exit 3 without --any), cli/27-send-extras.txt |
| conductor/reference.md#fleet.list | agents array fields pid, tool, state, hooked, reason, surface_id, surface_name, workspace, active_tool_name, message, last_result, waiting_ms, idle_ms | covered-final | final/01-verbs-send-up-wait.txt |
| conductor/reference.md#fleet.list | pid null for scan-only detection | covered-earlier | cli/21-lifecycle.txt |
| conductor/reference.md#fleet.list | empty fleet is {"agents":[]} with exit 0 | covered-earlier | cli/06-fake-agent.txt |
| conductor/reference.md#surface.status | status fields incl. output_generation; pane with no agent returns idle | covered-final | final/01-verbs-send-up-wait.txt, cli/06-fake-agent.txt |
| conductor/reference.md#surface.read | fenced by default; --raw for trusted scripts; JSON text, lines, total_lines, eof, output_generation | covered-final | final/04-mcp-write-approval.txt (--raw), final/01-verbs-send-up-wait.txt; cli/04-read-search.txt (eof) |
| conductor/reference.md#events.subscribe and watch | ai.session_start, prompt_submit, tool_use, notification, stop, exit, session_end events | covered-earlier | cli/21-watch-all.jsonl, cli/21-watch-notif-surface.jsonl |
| conductor/reference.md#events.subscribe and watch | subscribed and heartbeat control frames | covered-final | final/11-flow-watch-mcp.txt, cli/25-jsonrpc.txt |
| conductor/reference.md#events.subscribe and watch | dropped frame for a lagging subscriber | covered-final | final/50-dropped-busy.txt; cli/41-dropped-busy.txt |
| conductor/reference.md#wait | --pattern polls every 500 ms over the latest 500 lines and only text after the wait starts counts | covered-final | final/01-verbs-send-up-wait.txt (agentB sentinel already on screen not matched); wording 9e99fa53 |
| conductor/reference.md#wait | idle plus pattern reply carries idle and matched | covered-earlier | cli/07-wait.txt |
| conductor/reference.md#wait | --any and --all in pattern mode | covered-earlier | cli/07-wait.txt |
| conductor/reference.md#wait | timeout exits 4 | covered-final | final/01-verbs-send-up-wait.txt, cli/07-wait.txt |
| conductor/reference.md#Config keys | ai_unrestricted default false allows submit when true | covered-earlier | cli/26-config-keys.txt, cli/02-gates.txt |
| conductor/reference.md#Config keys | ai_injection_fence default true | covered-final | final/01-verbs-send-up-wait.txt (fence on a default config) |
| conductor/reference.md#Exit codes | 0 success, 1 runtime failure, 2 usage, 3 target, 4 wait timeout | covered-final | final/50-spaces-config.txt; final/01-verbs-send-up-wait.txt (exit 2 unknown verb, 3, 4), cli/13-spaces.txt (exit 1) |

### configuration.md

| page | claim | verdict | evidence |
|---|---|---|---|
| configuration.md#Where is paneflow.json | ~/.paneflow/paneflow.json on macOS | covered-final | final/50-quit-keep-relaunch.txt; report: installed app home; 83-malformed-json.jpg |
| configuration.md#Where is paneflow.json | Windows path %USERPROFILE%\.paneflow | unavailable | Windows-only: report: Windows not covered |
| configuration.md#Where is paneflow.json | PANEFLOW_HOME moves the whole directory | covered-final | final/01-verbs-send-up-wait.txt header (~/.paneflow-dev-final), final/07-worktrees.txt (worktrees under the home) |
| configuration.md#Where is paneflow.json | config at ~/Library/Application Support/paneflow or ~/.config/paneflow is copied into ~/.paneflow on first launch | unavailable | migration copies ~/Library/Application Support/paneflow or ~/.config/paneflow, the user's real legacy config, into the new home; not exercised to keep the user's files untouched (fd627b91) |
| configuration.md#Where is paneflow.json | a non-default home runs its own host, worker and paneflow-ipc-<fingerprint>.sock endpoint | covered-final | final/02-principals.txt (endpoints), final/06-host-serve-status.txt |
| configuration.md#Where is paneflow.json | missing file boots with defaults | covered-final | final/01-verbs-send-up-wait.txt (fresh home, default gates closed) |
| configuration.md#Editor autocomplete | $schema URL gives editor autocomplete; schema tracks the accepted keys | covered-earlier | report row: schema describes every key, fixed a65127ce (schema test) |
| configuration.md#How does Paneflow handle unknown keys | unknown top-level keys ignored at runtime | covered-earlier | cli/26-config-keys.txt (agent_stall_threshold_secs ignored, rest applied) |
| configuration.md#How does Paneflow handle unknown keys | schema rejects unknown keys (additionalProperties false) | covered-earlier | report row: schema test a65127ce |
| configuration.md#Spaces | spaces entries with id, name, color; hand edits apply without restart | covered-final | final/50-spaces-config.txt; 79-spaces-sidebar.jpg, 80-space-work.jpg, 81-space-color.jpg, cli/13-spaces.txt |
| configuration.md#Spaces | up to 9 spaces next to Default | covered-final | final/50-spaces-config.txt, final/50-settings-pages.txt |
| configuration.md#Spaces | Settings > Spaces records the image as avatar | unavailable | the space image is picked through the system Open panel, which neither CGEvent nor System Events keystrokes could drive in this session; Cancel closed it (`final/51-space-env.txt`) (fd627b91) |
| configuration.md#Spaces | space env set on every terminal with precedence terminal.env < space env < profile env < surface env | covered-final | final/50-spaces-config.txt (terminal env below the space env; profile and surface env not combined); earlier: cli/26-config-keys.txt covers terminal.env only |
| configuration.md#Spaces | Paneflow git, gh, clone, tab-naming processes get the fixed subset of the space env | covered-final | final/51-worktrees.txt (the snapshot commit made by Paneflow's git carries the space's GIT_AUTHOR_NAME; that non-subset variables stay out was not observable) (fd627b91) |
| configuration.md#Spaces | env change applies to terminals opened after it; rc exports override | covered-final | `final/51-space-env.txt`: an env change applied only to terminals opened after it; a shell whose rc exports the variable shows the rc value (fd627b91) |

### configuration/schema.md

| page | claim | verdict | evidence |
|---|---|---|---|
| configuration/schema.md#intro | invalid JSON on hot reload keeps the previous config | covered-final | final/50-quit-keep-relaunch.txt; 83-malformed-json.jpg |
| configuration/schema.md#intro | invalid file at startup reads as defaults and logs a warning | covered-final | final/50-quit-keep-relaunch.txt |
| configuration/schema.md#Top-level keys | $schemaVersion unknown version warns but loads | covered-final | final/50-config-keys.txt |
| configuration/schema.md#Top-level keys | default_shell order configured, $SHELL, /bin/sh | covered-earlier | default only: 04-terminal-render.jpg (zsh from $SHELL); a configured default_shell is uncovered |
| configuration/schema.md#Top-level keys | theme presets and aliases, case-insensitive, hot reload | covered-final | final/50-themes.txt; final/10-theme.txt; 23..27-*.jpg |
| configuration/schema.md#Top-level keys | theme_mode light, dark, system | covered-final | final/10-theme.txt; 28..30-system-*.jpg |
| configuration/schema.md#Top-level keys | font_family, font_fallbacks, font_weight, line_height, cell_width hot reload | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (font_size, line_height, font_family hot reload; font_fallbacks and cell_width not set) (fd627b91) |
| configuration/schema.md#Top-level keys | font_size range 8 to 32, invalid falls back to 13 | covered-final | final/50-config-keys.txt; earlier: 60-font-bigger.jpg covers Cmd+= only |
| configuration/schema.md#Top-level keys | unfocused_pane_opacity dims unfocused panes | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (fd627b91) |
| configuration/schema.md#Top-level keys | reduce_motion | unavailable | reduce_motion: the Settings switch was offscreen for a headless press and both frame captures were covered by another app's window, so no before/after frames (fd627b91) |
| configuration/schema.md#Top-level keys | comfortable_density | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (density has no effect under classic; cards vs 1 px dividers under rounded) (fd627b91) |
| configuration/schema.md#Top-level keys | interface_style classic and rounded, set from Settings | covered-final | write only: 121-ax-prompt-palette.txt (dropdown writes rounded); the rendered layout is uncovered |
| configuration/schema.md#Top-level keys | workspace_icon styles | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (six styles) (fd627b91) |
| configuration/schema.md#Top-level keys | app_icon shows in the Dock on macOS while running | covered-final | final/51-restart-settings.txt, final/51-app-icon-dock.jpg (Dock tile changes at once; Settings picker not opened) (fd627b91) |
| configuration/schema.md#Top-level keys | sidebar_show flags (branch, diffstat, pr, indent_guide, agents, status, shimmer, agent_*) | covered-final | `final/51-sidebar-show-keys.jpg`, `final/51-sidebar-branch-diffstat-indent.jpg`, `final/51-shimmer-*.jpg`: branch, diffstat, indent_guide, agents, status, shimmer, agent_logo, agent_workspace, agent_shimmer all hot reload as documented; pr not exercised (no GitHub remote in the fixture) (fd627b91) |
| configuration/schema.md#Top-level keys | sidebar_group_by folder and status | covered-final | `final/51-sidebar-group-by-status.jpg`, `final/51-agents-section.txt`: sidebar_group_by status groups tabs by status, hides Agents, adds a New tab button that asks for the workspace; Group by > Folder from the menu writes folder (fd627b91) |
| configuration/schema.md#Top-level keys | worktrees object hot reload | covered-earlier | worktrees/16-worktrees-dir-setting.txt |
| configuration/schema.md#Top-level keys | window_decorations client and server, startup only | covered-final | final/51-restart-settings.txt (window_decorations is startup only; on macOS server looks identical to client, observation) (fd627b91) |
| configuration/schema.md#Top-level keys | background_style themed and blended, legacy resolution, unknown resolves to themed | covered-earlier | 31..53-*.jpg, 44-invalid-style-both.jpg, 51-upgrade-macos-chrome-material.jpg, 52-upgrade-no-keys.jpg |
| configuration/schema.md#Top-level keys | window_backdrop opaque, off, transparent resolve to Themed on macOS; PANEFLOW_WINDOW_BACKDROP overrides one launch | fail | C16: final/51-restart2.txt, final/51-window-backdrop.jpg, final/51-backdrop-env-opaque.jpg (config opaque/off/transparent resolve to Themed on macOS as documented; PANEFLOW_WINDOW_BACKDROP is ignored on macOS) (fd627b91) |
| configuration/schema.md#Top-level keys | macos_chrome_material true resolves to Blended while background_style is absent | covered-earlier | 51-upgrade-macos-chrome-material.jpg |
| configuration/schema.md#Top-level keys | windows_* and linux_* material keys | unavailable | Windows and Linux only: ignored on macOS per the schema |
| configuration/schema.md#Top-level keys | option_as_meta false on macOS by default; true makes Option send Meta | fail | C10: final/51-config-hot-reload.txt, final/51-option-as-meta.jpg (default false types ∫; true sends ESC b AND ∫) (fd627b91) |
| configuration/schema.md#Top-level keys | shell_integration OSC 7 and OSC 133 | covered-earlier | default on: 64-prompt-jump-fixed.jpg, split in the pane cwd (report row); false is uncovered |
| configuration/schema.md#Top-level keys | automation.tab_auto_naming names tabs through the agent CLI | unavailable | tab_auto_naming asks the agent CLI to name the tab; the stand-in cannot answer and the real CLI is off limits (fd627b91) |
| configuration/schema.md#Top-level keys | submit_paste_delay_ms floor 10 to 5000 | covered-final | final/50-config-keys.txt; earlier: default send --submit only (final/01) |
| configuration/schema.md#Top-level keys | external_editor auto, system, named editors | unavailable | external_editor opens the user's own editor app outside the dev sandbox (fd627b91) |
| configuration/schema.md#Top-level keys | shortcuts map keystrokes to actions | covered-final | final/50-config-keys.txt; 92-custom-shortcuts.jpg, 93-none-no-split.jpg, 94-default-restored.jpg |
| configuration/schema.md#Top-level keys | claude_code_bypass_permissions adds --permission-mode bypassPermissions | covered-final | final/50-quit-keep-relaunch.txt |
| configuration/schema.md#Top-level keys | ipc scripting and orchestration gates, next request after save | covered-final | final/01-verbs-send-up-wait.txt, final/03-session-input-journal.txt, final/11-flow-watch-mcp.txt |
| configuration/schema.md#Top-level keys | ai_unrestricted opens both gates for every caller | covered-earlier | cli/26-config-keys.txt |
| configuration/schema.md#Top-level keys | ai_injection_fence per read call | covered-earlier | cli/26-config-keys.txt |
| configuration/schema.md#Top-level keys | on_quit ask opens the quit dialog; keep and stop | covered-earlier | ask); keep written from Settings covered-final: 46-quit-dialog.jpg, 47-relaunch-reconnected.jpg; final/40-mouse-pass.txt (on_quit keep written); keep and stop effects uncovered |
| configuration/schema.md#Top-level keys | agent_prompts native and paneflow | covered-final | final/52-agent-config-dirs.txt, final/52-agent-prompts.jpg (paneflow: Paneflow prompt answers PermissionRequest and AskUserQuestion through the hook; native: no prompt) (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| configuration/schema.md#Top-level keys | telemetry consent; PANEFLOW_NO_TELEMETRY=1 overrides | covered-final | final/51-restart2.txt (enabled=true logs the opted-in warning; with PANEFLOW_NO_TELEMETRY=1 nothing) (fd627b91) |
| configuration/schema.md#Agent launcher buttons | *_button_visible null autodetects, false hides, true forces | covered-final | final/50-quit-keep-relaunch.txt (false hides; null and true not compared); earlier: 01-first-launch.jpg shows autodetect only |
| configuration/schema.md#Agent launcher buttons | Windows shows only five runtimes | unavailable | Windows-only: report: Windows not covered |
| configuration/schema.md#terminal | terminal.ligatures, integrated_glyphs, color_emoji | covered-earlier | defaults only: 04-terminal-render.jpg (emoji, CJK); toggles uncovered |
| configuration/schema.md#terminal | terminal.cursor_color, cursor_shape, cursor_blink | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (cursor_color hot, underline + blink off on a new terminal) (fd627b91) |
| configuration/schema.md#terminal | terminal.scrollbar overlay | covered-final | final/51-sidebar-scroll.txt, final/51-scrollbar-multiplier.jpg (fd627b91) |
| configuration/schema.md#terminal | terminal.scrollback_lines 100 to 100000 | covered-final | final/50-config-keys.txt (the cap is page granular) |
| configuration/schema.md#terminal | terminal.osc52_clipboard copy and off; reads always denied | covered-final | final/50-config-keys.txt (off) |
| configuration/schema.md#terminal | terminal.open_links_in dock or external_editor, Shift picks the other | unavailable | terminal.open_links_in external_editor and Shift+click launch the user's external code editor outside the dev sandbox; not exercised by rule (fd627b91) |
| configuration/schema.md#terminal | terminal.xt_checksum_report and xt_checksum_extension (out of range falls back to 0 with a warning) | covered-final | final/50-config-keys.txt |
| configuration/schema.md#terminal | terminal.env injected verbatim, per-surface env wins, reserved keys ignored | covered-earlier | injection only: cli/26-config-keys.txt; precedence and reserved keys uncovered |
| configuration/schema.md#terminal | terminal.scroll_multiplier | covered-final | final/51-sidebar-scroll.txt, final/51-scrollbar-multiplier.jpg (fd627b91) |
| configuration/schema.md#terminal | terminal.minimum_contrast APCA | covered-final | final/51-config-hot-reload.txt, final/51-config-hot-reload.jpg (Auto, 0 off, 90, 120 capped to 90, invalid -> Auto, hot reload) (fd627b91) |
| configuration/schema.md#editor | editor.minimap, scrollbar, word_wrap, markdown_rendered, csv_table, semantic_tokens | covered-final | final/51-editor-dock.txt, final/51-markdown-config.txt (word_wrap, markdown_rendered, csv_table written and honored; minimap and scrollbar listed in Editor Controls) (fd627b91) |
| configuration/schema.md#agent_panel | notify_when_agent_waiting PrimaryScreen, AllScreens, Never; also governs OSC 9 and OSC 777 | covered-final | write only: final/40-mouse-pass.txt (dropdown writes PrimaryScreen); the notification itself is uncovered |
| configuration/schema.md#agents | agents.restore_conversations types the resume command when the host session did not survive | unavailable | restore_conversations and resume type the agent's resume command after the host session is lost; that needs a real agent session to resume, and real agent binaries are off limits (fd627b91) |
| configuration/schema.md#agents | agents.update_methods | unavailable | agents.update_methods: the Update method combobox could not be opened headlessly and the Update buttons would run the real agent updaters (fd627b91) |
| configuration/schema.md#agents | agents.default_accounts passes CLAUDE_CONFIG_DIR, CODEX_HOME, GROK_HOME to new panes | covered-final | final/50-config-keys.txt (claude-code) |
| configuration/schema.md#agents | agents.claude_preassign_session_id adds --session-id | covered-final | final/50-quit-keep-relaunch.txt |
| configuration/schema.md#usage | usage.plan_limits true installs the status line relay; own statusLine never changed | covered-final | final/52-agent-config-dirs.txt (1825a6fc, throwaway HOME and space config dirs; the real ~/.claude files checked unchanged) |
| configuration/schema.md#usage | usage.plan_limits false removes the relay | covered-final | final/50-integrations-hooks-skills.txt |
| configuration/schema.md#usage | usage.sidebar_agents filters the capsule; empty hides it | covered-final | final/51-restart-settings.txt, final/51-usage-capsule.jpg (fd627b91) |
| configuration/schema.md#worktrees | ownership marker in .git/worktrees/<name>, git status clean | covered-final | final/07-worktrees.txt, worktrees/10-new-branch-split.txt |
| configuration/schema.md#worktrees | empty branch name gives a detached <base>-<sha7>; Create branch here keeps changes | covered-earlier | worktrees/77-detached-worktree.jpg, worktrees/78-create-branch-here.jpg |
| configuration/schema.md#worktrees | .worktreeinclude rules; default copies .env* and AGENTS.override.md; never overwrites | covered-earlier | worktrees/15-worktreeinclude.txt |
| configuration/schema.md#worktrees | snapshot under refs/paneflow/snapshots, Restore, Delete, clean leaves none | covered-earlier | worktrees/24-snapshot-contents.txt, worktrees/57-snapshot-restore.txt, worktrees/59-snapshot-delete.txt |
| configuration/schema.md#worktrees | worktrees.dir with ~ and <repo>-<hash> layout | covered-final | final/07-worktrees.txt, worktrees/16-worktrees-dir-setting.txt |
| configuration/schema.md#worktrees | worktrees.auto_remove and keep_limit | covered-earlier | worktrees/26-keep-limit.txt, worktrees/27-auto-remove-off.txt |
| configuration/schema.md#worktrees | worktrees.for_new_branches false switches the checkout; refused while an agent works there | covered-earlier | switch); refusal uncovered: worktrees/79-worktree-toggle-off.jpg, worktrees/82-mouse-flows.txt |
| configuration/schema.md#ipc | malformed ipc value logged with its key and closes both gates | covered-earlier | cli/11-config-gates.txt |
| configuration/schema.md#ipc | scripting implies orchestration; orchestration alone launches commands and --scope all | covered-final | final/50-grants.txt; cli/11-config-gates.txt, cli/14-up.txt, cli/18-grants.txt |
| configuration/schema.md#ipc | window and background process always pass the gates | covered-final | final/02-principals.txt |
| configuration/schema.md#language_servers | server per root, built-in rust-analyzer and zls, trust banner Start and Not now, trust file, Forget permissions | covered-final | final/51-restart-settings.txt, final/51-language-server-banner.jpg (banner Start / Not now, Not now asks again after restart, trust file written, Forget empties it; rust-analyzer itself exited twice on this fixture and was left stopped as documented) (fd627b91) |
| configuration/schema.md#language_servers | entries replace built-in fields; malformed entry skipped with a log line | covered-final | final/51-restart-settings.txt (malformed bogus-ls skipped with a WARN line; built-in rust-analyzer entry with only args kept its other fields) (fd627b91) |
| configuration/schema.md#agent_profiles | profiles show in the pane palette with env and args; bad entries skipped with a warning | covered-final | final/50-quit-keep-relaunch.txt |
| configuration/schema.md#agent_profiles | Settings > Agents New preset and Add account write the list | covered-final | final/51-restart-settings.txt (New preset writes agent_profiles; Add account not pressed) (fd627b91) |
| configuration/schema.md#spaces | Cmd+1..9 and the sidebar cover the active space only; Default implicit | covered-earlier | 79-spaces-sidebar.jpg, 80-space-work.jpg |
| configuration/schema.md#spaces | spaces.*.color gradient; unknown color draws no color | covered-earlier | 81-space-color.jpg |
| configuration/schema.md#spaces | malformed, duplicate, reserved or tenth entries skipped with a warning | covered-final | final/50-spaces-config.txt |
| configuration/schema.md#spaces | removing a space moves its workspaces to Default | covered-final | final/50-spaces-config.txt |
| configuration/schema.md#spaces | avatar file name pattern, bad value ignored | covered-final | final/50-spaces-config.txt |
| configuration/schema.md#spaces | env reserved keys (PANEFLOW_, TERM, DYLD_*) ignored; CLAUDE_CONFIG_DIR etc. absolute and created mode 0700 | covered-final | final/50-spaces-config.txt |
| configuration/schema.md#commands | commands entries and workspace templates (name, cwd, layout_preset, color, layout, surfaces with command, prompt, cwd, env, focus) | fail | C11 + C12: final/51-layouts-templates.txt (template keys cwd, env, command, focus, layout_preset work; a simple commands[].command entry never reaches the palette; workspace.name does not create that workspace) (fd627b91) |
| configuration/schema.md#telemetry | telemetry.enabled null, true, false | covered-final | final/51-restart2.txt (null: off, no prompt; true: opted in; false via live reload: nothing further) (fd627b91) |

### troubleshooting.md

| page | claim | verdict | evidence |
|---|---|---|---|
| troubleshooting.md#Launch and rendering | Linux Vulkan, Wayland, XWayland checks | unavailable | Linux-only: report: Linux not covered |
| troubleshooting.md#Launch and rendering | Windows NoSupportedDeviceFound and DirectX 11 | unavailable | Windows-only: report: Windows not covered |
| troubleshooting.md#Launch and rendering | macOS needs macOS 13 or later on Apple Silicon | covered-earlier | report environment (macOS 27.2.1, M4); Intel unavailable |
| troubleshooting.md#Why is my paneflow.json not loading | python3 -m json.tool flags a syntax error | covered-final | final/50-troubleshooting.txt; earlier: none (trivial; no transcript) |
| troubleshooting.md#Why is my paneflow.json not loading | invalid edit while running keeps the last valid config | covered-final | final/50-quit-keep-relaunch.txt; 83-malformed-json.jpg |
| troubleshooting.md#Why is my paneflow.json not loading | invalid configuration at startup falls back to defaults | covered-final | final/50-quit-keep-relaunch.txt |
| troubleshooting.md#Why is my paneflow.json not loading | window_decorations and window_backdrop need a restart | covered-final | final/51-restart-settings.txt (window_decorations is startup only; on macOS server looks identical to client, observation) (fd627b91) |
| troubleshooting.md#Why are my shortcuts not working | shortcut entry maps to an action; none disables | covered-final | final/50-config-keys.txt; 92-custom-shortcuts.jpg, 93-none-no-split.jpg |
| troubleshooting.md#Why are my shortcuts not working | unknown action skipped with a warning | covered-final | final/50-config-keys.txt, final/50-troubleshooting.txt |
| troubleshooting.md#Why are my shortcuts not working | Terminal, Search, Markdown, Diff actions have their own contexts | covered-earlier | 90-dock-cmd-g.jpg, 101-dock-cmd-j-fixed.jpg (context fix 5c85556c) |
| troubleshooting.md#Why is my theme not hot-reloading | One Dark and PaneFlow Light resolve; a saved edit applies without restart | covered-final | final/50-themes.txt; final/10-theme.txt; 26-one-dark-alias.jpg |
| troubleshooting.md#Why is paneflow not in my PATH | macOS app install does not add paneflow to PATH | covered-earlier | 100-dmg-provenance.txt |
| troubleshooting.md#Why is paneflow not in my PATH | Linux and Windows PATH steps | unavailable | Linux and Windows only: report: not covered |
| troubleshooting.md#Why does macOS say Apple cannot check this app | the official DMG is signed and notarized | covered-earlier | 100-dmg-provenance.txt |
| troubleshooting.md#Why does Windows SmartScreen block the MSI | signature Valid, publisher Strivex | unavailable | Windows-only: report: Windows not covered |
| troubleshooting.md#Collect diagnostics | Help > System Info > Copy gives a diagnostics block | covered-final | final/50-troubleshooting.txt; earlier: report lists the menu bar (PaneFlow, Edit, Window) only; src-app/src/app/macos_menu.rs has Help > System Info |
| troubleshooting.md#Collect diagnostics | RUST_LOG=info paneflow logs to the terminal | covered-earlier | cli/app-window-run1.log, cli/serve.log |
| troubleshooting.md#Collect diagnostics | /Applications/PaneFlow.app/Contents/MacOS/paneflow is the DMG binary path | covered-earlier | 100-dmg-provenance.txt |
| troubleshooting.md#Collect diagnostics | Windows RUST_LOG and winver | unavailable | Windows-only: report: Windows not covered |

### blog/agent-status-osc-7501.md

| page | claim | verdict | evidence |
|---|---|---|---|
| blog/agent-status-osc-7501.md#One Claude Code turn | OSC 7501 working, blocked, done map to spinner, Need you, Done | covered-earlier | 72-osc7501-blocked.jpg, cli/06-fake-agent.txt, 67-waiting-agent-sidebar.jpg |
| blog/agent-status-osc-7501.md#The general shape | a known agent silent for 3 s shows No status | covered-final | `final/51-agents-section.txt`: a stand-in agent that emits no OSC 7501 shows 'starting', then 'No status' after about 3 s; `final/51-agents-groups.jpg` (fd627b91) |
| blog/agent-status-osc-7501.md#What about agents that say nothing | send --submit reports no_signal for a silent agent | covered-final | final/01-verbs-send-up-wait.txt, cli/02-gates.txt |
| blog/agent-status-osc-7501.md#Identity is matched | fx counts as the agent only while the pane title starts with fx v | covered-final | final/51-usage-fx.txt (fd627b91) |
