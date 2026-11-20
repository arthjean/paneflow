# Layout gesture checklist

Since the epic "The layout becomes plain data with stable ids" (EP-002 of
`tasks/prd-lua-scripting.md`), every layout change goes through
`paneflow_layout::Model::apply`. The tests in `src-app/src/app/layout_pins/`
and `crates/paneflow-layout/tests/` drive these changes through their real
entry points with display-only terminals and a fake session host. They do not
cover pointer drags, a real detached window or real paint. This checklist covers
that part: run it by hand on a debug build in its own home
(`scripts/dev.sh --tag layout`), on Linux, macOS and Windows.

Each line gives the gesture and the result it must give. A difference from
0.20.0 is a regression, unless the line says the behavior changed on purpose.

## Split

- [ ] `split_vertically` on a focused pane: a new pane opens to its right, takes
  half of its width and gets focus.
- [ ] `split_horizontally` on a focused pane: a new pane opens below it, takes
  half of its height and gets focus.
- [ ] Split in a tab that already holds the pane limit: nothing opens and a
  toast names the limit.
- [ ] Split while a pane is zoomed: refused, the zoom stays.
- [ ] `paneflow surface split` (IPC) aimed at a pane: the new pane opens next to
  it and focus stays where it was.
- [ ] IPC split aimed at a surface that is not the shown tab of its pane: the
  pane splits. Changed on purpose: 0.20.0 answered `Surface not found`.
- [ ] Drop an agent session from the sessions sidebar on a pane edge: a pane
  opens on that edge and resumes the session.

## Close

- [ ] `close_pane`: the pane closes, its neighbor grows over the space, focus
  goes to the previous sibling, and the session is held for undo.
- [ ] Close the last pane of a tab: the tab keeps one fresh shell.
- [ ] Close a zoomed pane: zoom ends and the first pane takes focus.
- [ ] The close button of a pane header: the session stops, no undo record.
- [ ] The close button of a surface tab in a pane: that surface stops, its
  neighbor is shown.
- [ ] `close_tab`, then choose to keep the sessions: each pane's surfaces are
  held and the next tab is shown.
- [ ] `close_tab`, then choose to stop them: the sessions stop and nothing is
  held.
- [ ] `close_workspace`: its sessions stop and the next workspace is shown.
- [ ] `paneflow workspace close` on the last workspace: refused.

## Undo

- [ ] `undo_close_pane` within the hold: the pane comes back next to the first
  pane with its live session.
- [ ] `undo_close_pane` after the hold expired: a fresh shell opens instead.
- [ ] Close six panes, then undo six times: only five come back, the oldest
  record was evicted and its session stopped.
- [ ] Undo a closed tab: the tab comes back with its layout.
- [ ] Undo a pane whose session is already shown in another view: a fresh shell
  opens and a toast says so.

## Drag

- [ ] Drag a pane header onto another pane's center: the two swap.
- [ ] Drag a pane header onto each of the four edges of another pane: it moves
  to that edge.
- [ ] Drag a surface tab onto another pane's center: it joins that pane and is
  shown.
- [ ] Drag a surface tab onto an edge: a new pane opens on that edge with it.
- [ ] Drag a surface tab onto a full pane: refused with the tab limit toast.
- [ ] Drag a divider and release it: the panes keep the new sizes, and only the
  release is saved (one `session.json` write).
- [ ] Drag two nested dividers one after the other: each keeps its own sizes.
- [ ] Start a divider drag, then detach a pane with its shortcut before
  releasing: the drag is cancelled and the sizes go back.
- [ ] Drag a tab in the tab strip: the tabs reorder.
- [ ] Drag a tab onto another workspace in the sidebar: it moves there.

## Zoom

- [ ] `toggle_zoom`: the focused pane fills the tab, and again restores the
  layout with its sizes.
- [ ] Switch to another tab and back: the zoom is per tab and comes back.
- [ ] `paneflow surface focus` on a pane hidden by the zoom: zoom ends and that
  pane takes focus.
- [ ] Focus the zoomed pane itself through IPC: the zoom stays.

## Presets

- [ ] `layout_even_horizontal`, `layout_even_vertical`, `layout_main_vertical`
  and `layout_tiled` on a tab of four panes: each gives its arrangement and keeps
  every session.
- [ ] A preset on a zoomed tab: zoom ends first.
- [ ] `split_equalize`: every split of the tab gets equal sizes.

## Detach and reattach

- [ ] Detach a pane: it opens in its own window and leaves a gap-free layout in
  the tab.
- [ ] Detach the zoomed pane: zoom ends.
- [ ] Reattach from the detached window: the pane returns to its tab, which is
  selected, and takes focus.
- [ ] Close the detached window with the window manager: the pane is shown in
  its tab again with its sizes.
- [ ] Close the pane of a detached window: the window closes.
- [ ] Quit with a detached pane and restart: the window reopens at its bounds
  around the same pane.

## Rename and reorder

- [ ] Rename a tab from its context menu: the title holds, and an agent title
  no longer overrides it.
- [ ] `paneflow surface rename`: the surface title changes and is saved.
- [ ] Reorder workspaces in the sidebar: the order holds after a restart.
- [ ] Switch workspaces and spaces with the sidebar and the shortcuts: the
  focused pane of each comes back.

## Restart

- [ ] Quit and restart: every workspace, tab, layout, size, title and detached
  window comes back.
- [ ] `paneflow surface list` before and after the restart: every surviving
  terminal keeps its `surface_id`, and `echo $PANEFLOW_SURFACE_ID` in it prints
  the same value.
- [ ] After the first restart that follows the update from 0.20.0: a terminal
  the host still runs keeps the `PANEFLOW_SURFACE_ID` it was spawned with.
