---@meta paneflow

---@class paneflow
paneflow = {}

---@class paneflow.ctx
---@field surface_id integer? The surface the script, handler or timer belongs to
---@field pane_id integer? The pane that holds it
---@field tab_id integer? The tab that holds it
---@field workspace_id integer? The layout workspace that holds it
---@field origin "cli"|"timer"|"event"|string What started this code

---@class paneflow.timer
local timer = {}

---Stop the timer; returns whether it was active.
---@return boolean
function timer:cancel() end

---@return boolean
function timer:active() end

---@class paneflow.exec.result
---@field code integer? The exit code, nil when a signal ended the program
---@field signal integer? The signal that ended the program, on Unix
---@field stdout string? Captured output, nil when stdout = false
---@field stderr string? Captured errors, nil when stderr = false
---@field timed_out boolean True when the timeout stopped the program
---@field ok boolean True when the code is 0 and the program did not time out

---@class paneflow.exec.options
---@field cwd string? Directory to run in
---@field env table<string, string|false>? Variables to set, false removes one
---@field clear_env boolean? Start from an empty environment
---@field stdin string|boolean? Text to write to stdin, or false for no stdin
---@field timeout number? Seconds before the process tree is terminated
---@field stdout boolean|fun(chunk: string)? false discards, a function receives output as it arrives
---@field stderr boolean|fun(chunk: string)? false discards, a function receives output as it arrives
---@field text boolean? Decode output as text and turn CRLF into LF, true by default
---@field detach boolean? Keep the program running when the scope that started it ends; its output is discarded unless a function receives it

---@class paneflow.exec.handle
---@field pid integer
local handle = {}

---Wait for the program to end; nil and "timeout" when seconds pass first.
---@param seconds number?
---@return paneflow.exec.result?
---@return string? message
function handle:wait(seconds) end

---@param data string
---@return boolean? ok
---@return string? message
function handle:write(data) end

---@return boolean
function handle:close_stdin() end

---@param signal string|integer? TERM by default
---@return boolean
function handle:kill(signal) end

---@return boolean
function handle:running() end

---@class paneflow.event
---@field type string
---@field seq integer
---@field session string?
---@field [string] any

---@class paneflow.layout.node
---@field type "pane"|"split"

---@class paneflow.json
paneflow.json = {}

---Encode a value; a table with keys 1 to n becomes an array, any other table an object.
---@param value any
---@param pretty boolean?
---@return string? json
---@return string? message
function paneflow.json.encode(value, pretty) end

---Decode JSON; null becomes nil and integers stay integers.
---@param text string
---@return any value
---@return string? message
function paneflow.json.decode(text) end

---@class paneflow.layout
paneflow.layout = {}

---@param spec { surfaces: table[]?, command: string?, cwd: string?, name: string?, env: table<string, string>?, focus: boolean? }?
---@return paneflow.layout.node
function paneflow.layout.pane(spec) end

---Stack two nodes, the first taking ratio of the height.
---@param ratio number
---@param first paneflow.layout.node
---@param second paneflow.layout.node
---@return paneflow.layout.node
function paneflow.layout.horizontal(ratio, first, second) end

---Put two nodes side by side, the first taking ratio of the width.
---@param ratio number
---@param first paneflow.layout.node
---@param second paneflow.layout.node
---@return paneflow.layout.node
function paneflow.layout.vertical(ratio, first, second) end

---The key=value pairs or the --args JSON given to paneflow do.
---@type table<string, any>
paneflow.args = {}

---@type paneflow.ctx
paneflow.ctx = { origin = "cli" }

---Call any method by its full name.
---@param method string
---@param params table?
---@return table? result
---@return string? message
---@return integer? code
function paneflow.call(method, params) end

---Call fn(ctx, ev) for every matching event; at the top level only. A name may be a family such as terminal.*.
---@param event string
---@param fn fun(ctx: paneflow.ctx, ev: paneflow.event)
---@return boolean
function paneflow.on(event, fn) end

---Wait for the next matching event since the script started; nil and "timeout" when seconds pass first.
---@param event string
---@param filter table|fun(ev: paneflow.event, ctx: paneflow.ctx): boolean|nil
---@param timeout number?
---@return paneflow.event? ev
---@return paneflow.ctx|string ctx_or_message
function paneflow.wait(event, filter, timeout) end

---@param seconds number
function paneflow.sleep(seconds) end

---End the script at once with this exit status.
---@param status integer?
function paneflow.stop(status) end

---@param seconds number
---@param fn fun(ctx: paneflow.ctx)
---@return paneflow.timer
function paneflow.after(seconds, fn) end

---@param seconds number
---@param fn fun(ctx: paneflow.ctx)
---@return paneflow.timer
function paneflow.every(seconds, fn) end

---@class paneflow.exec
---Run a program from an argument list, never through a shell.
---@overload fun(argv: string[], opts: paneflow.exec.options?): paneflow.exec.result?, string?
paneflow.exec = {}

---Start a program and return a handle at once.
---@param argv string[]
---@param opts paneflow.exec.options?
---@return paneflow.exec.handle? handle
---@return string? message
function paneflow.exec.start(argv, opts) end

---@param level "debug"|"info"|"warn"|"error"|any
---@param message any?
function paneflow.log(level, message) end

---@class paneflow.action.ctx: paneflow.ctx
---@field origin "key"|"palette"|"cli"|"api"|"script"|"agent"|"timer"|"event"|string What started the run
---@field client_id string? The client that asked for the run

---@class paneflow.action.spec
---@field name string Lowercase letters, digits and underscores
---@field title string Shown in the command palette
---@field description string?
---@field category string? Palette group, Actions by default
---@field keywords string[]? Extra words the palette filter matches
---@field args table<string, string|table>? Argument types such as "string" or "number?", or JSON Schema tables
---@field agents boolean? Whether agents may run the action, false by default
---@field run fun(ctx: paneflow.action.ctx, args: table): any

---Declare an action while init.lua loads; call paneflow.action.invoke or paneflow.invoke to run one.
---@class paneflow.action
---@overload fun(spec: paneflow.action.spec): boolean?
paneflow.action = {}

---Run an action with arguments inside an action or a script; nil and a message when it fails.
---@param name string
---@param args table?
---@return any value
---@return string? message
function paneflow.invoke(name, args) end

---Bind a key to an action name while init.lua loads; Settings shortcuts win over it.
---@param key string A key in the syntax of paneflow.json shortcuts, such as "ctrl-alt-d"
---@param action string A Lua action or a built-in action name
---@param args table? Arguments for a Lua action
---@return boolean?
function paneflow.bind(key, action, args) end

---Remove this file's bindings of a key and drop the window's default for it.
---@param key string
---@return boolean
function paneflow.unbind(key) end

---@class paneflow.binding
---@field key string
---@field action string
---@field args table?
---@field source string

---@return paneflow.binding[]
function paneflow.bindings() end

---@class paneflow.client
paneflow.client = {}

---Queue a built-in window action; inside an action it runs once run returns without error.
---@param name string A name from paneflow.client.list_actions
---@return boolean
function paneflow.client.queue(name) end

---@class paneflow.ui
paneflow.ui = {}

---@class paneflow.ui.item
---@field id string Returned when the item is chosen
---@field title string? The label shown and filtered on, the id by default
---@field subtitle string?
---@field icon "document"|"folder"|"branch"|"terminal"|"code"|"server"|"database"|"package"|"globe"|"link"|"person"|"tag"|"clock"|"star"|"gear"|"plus"|nil
---@field detail string? Text shown at the end of the row
---@field selected boolean? Starts ticked when multiple is true

---@class paneflow.ui.input.spec
---@field title string
---@field placeholder string?
---@field value string? Text the field starts with
---@field confirm string? Label of the confirming row
---@field timeout number? Seconds before the prompt ends with "timeout", 300 by default, at most 3600

---@class paneflow.ui.pick.spec: paneflow.ui.input.spec
---@field items (string|paneflow.ui.item)[] At most 2000; a string is its own id and title
---@field multiple boolean? Tick several items; the answer is the list of ids in the order of items
---@field allow_other boolean? Accept typed text that matches no item; the answer is { text = ... }

---Ask the user for a line of text in every window that can show it; the first answer wins and the action's 30 s limit pauses while it waits.
---Returns nil and "cancelled", "timeout", "no_client", "unsupported", "busy" or "failed" (with a detail) when nobody answers.
---@param spec paneflow.ui.input.spec
---@return string? text
---@return string? reason
---@return string? detail
function paneflow.ui.input(spec) end

---Ask the user to choose among items; the answer is an id, a list of ids with multiple, or { text = ... } with allow_other.
---@param spec paneflow.ui.pick.spec
---@return string|string[]|{ text: string }|nil answer
---@return string? reason
---@return string? detail
function paneflow.ui.pick(spec) end

---@class paneflow.action.invoke.params
---@field args? table The action's arguments, checked against its declared schema
---@field ctx? table
---@field name string

---@class paneflow.action.invoke.result
---@field result? any

---Run an action of init.lua with arguments and return what it returned. Arguments are checked against its schema first; a run is stopped after 30 seconds.
---@param params paneflow.action.invoke.params
---@return paneflow.action.invoke.result? result
---@return string? message
---@return integer? code
paneflow.action.invoke = function(params) end

---@class paneflow.action.list.params

---@class paneflow.action.list.result
---@field actions table[]

---List the actions init.lua declared, with their titles, argument schemas and sources.
---@param params paneflow.action.list.params?
---@return paneflow.action.list.result? result
---@return string? message
---@return integer? code
paneflow.action.list = function(params) end

---@class paneflow.agent
paneflow.agent = {}

---@class paneflow.agent.acknowledge.params
---@field session? string One session, when sessions is absent
---@field sessions? string[]

---@class paneflow.agent.acknowledge.result
---@field acknowledged string[]

---Clear the unread mark of sessions and publish the lowered rows.
---@param params paneflow.agent.acknowledge.params?
---@return paneflow.agent.acknowledge.result? result
---@return string? message
---@return integer? code
paneflow.agent.acknowledge = function(params) end

---@class paneflow.agent.activity_log.params
---@field limit? integer Entries to return, 50 by default

---@class paneflow.agent.activity_log.result
---@field entries any[]

---Read the most recent agent activity entries.
---@param params paneflow.agent.activity_log.params?
---@return paneflow.agent.activity_log.result? result
---@return string? message
---@return integer? code
paneflow.agent.activity_log = function(params) end

---@class paneflow.agent.follow.params

---@class paneflow.agent.follow.result
---@field capabilities string[]
---@field core_connected boolean
---@field following? boolean
---@field sessions any[]
---@field usage any
---@field worker table

---Stream the session projection: a snapshot header, then one frame per reduced transition.
---@param params paneflow.agent.follow.params?
---@return paneflow.agent.follow.result? result
---@return string? message
---@return integer? code
paneflow.agent.follow = function(params) end

---@class paneflow.agent.snapshot.params

---@class paneflow.agent.snapshot.result
---@field capabilities string[]
---@field core_connected boolean
---@field following? boolean
---@field sessions any[]
---@field usage any
---@field worker table

---Read every session's reduced agent row and the usage frame.
---@param params paneflow.agent.snapshot.params?
---@return paneflow.agent.snapshot.result? result
---@return string? message
---@return integer? code
paneflow.agent.snapshot = function(params) end

---@class paneflow.client.dispatch_action.params
---@field name string A name from client.list_actions, such as toggle_zoom

---@class paneflow.client.dispatch_action.result
---@field dispatched boolean
---@field name string

---Dispatch a built-in action in the window as the command palette does, and reply once it is dispatched.
---@param params paneflow.client.dispatch_action.params
---@return paneflow.client.dispatch_action.result? result
---@return string? message
---@return integer? code
paneflow.client.dispatch_action = function(params) end

---@class paneflow.client.list_actions.params

---@class paneflow.client.list_actions.result
---@field actions table[]

---List the window's built-in actions: the names key bindings and client.dispatch_action accept.
---@param params paneflow.client.list_actions.params?
---@return paneflow.client.list_actions.result? result
---@return string? message
---@return integer? code
paneflow.client.list_actions = function(params) end

---@class paneflow.config
paneflow.config = {}

---@class paneflow.config.reload.params

---@class paneflow.config.reload.result
---@field actions integer
---@field bindings integer
---@field found boolean
---@field handler_errors integer
---@field handlers integer
---@field loaded_at_ms? integer
---@field ok boolean
---@field path string
---@field problems table[]
---@field timer_errors integer
---@field timers integer

---Load init.lua again now and return the status; a failed load keeps the previous configuration active.
---@param params paneflow.config.reload.params?
---@return paneflow.config.reload.result? result
---@return string? message
---@return integer? code
paneflow.config.reload = function(params) end

---@class paneflow.config.status.params

---@class paneflow.config.status.result
---@field actions integer
---@field bindings integer
---@field found boolean
---@field handler_errors integer
---@field handlers integer
---@field loaded_at_ms? integer
---@field ok boolean
---@field path string
---@field problems table[]
---@field timer_errors integer
---@field timers integer

---Read the state of init.lua: its path, whether it was found, when it loaded, what it declared and its load problems.
---@param params paneflow.config.status.params?
---@return paneflow.config.status.result? result
---@return string? message
---@return integer? code
paneflow.config.status = function(params) end

---@class paneflow.events
paneflow.events = {}

---@class paneflow.events.follow.params
---@field epoch? string The epoch since belongs to; a different epoch is refused with an evicted error
---@field since? integer Replay the events after this seq first; refused with an evicted error when they left the ring
---@field types? string[] Only send these types: exact names or a family such as terminal.*
---@field workspace_id? integer Only send events about this layout workspace id

---@class paneflow.events.follow.result
---@field epoch string
---@field seq integer
---@field type "following"

---Stream the worker's numbered event log: a following header, then every event with consecutive seq values, keepalive frames when quiet, and an end frame when the worker closes the stream. A since that left the ring, or an epoch from another worker start, is refused with -32040 and data {reason: evicted, seq, epoch}.
---@param params paneflow.events.follow.params?
---@return paneflow.events.follow.result? result
---@return string? message
---@return integer? code
paneflow.events.follow = function(params) end

---@class paneflow.events.publish.params
---@field events table[] Events in the order their ops were applied

---@class paneflow.events.publish.result
---@field seqs integer[]

---Append layout events to the worker's log, in order. Any event outside the layout family refuses the whole batch with -32602.
---@param params paneflow.events.publish.params
---@return paneflow.events.publish.result? result
---@return string? message
---@return integer? code
paneflow.events.publish = function(params) end

---@class paneflow.events.subscribe.params
---@field surfaces? integer[] Only stream events about these surfaces
---@field types? string[] Only stream these event types: surface_changed and the ai.* lifecycle types

---@class paneflow.events.subscribe.result
---@field id integer
---@field type "subscribed"

---Stream surface and agent events as JSON lines: a subscribed frame, then events, heartbeat and dropped frames.
---@param params paneflow.events.subscribe.params?
---@return paneflow.events.subscribe.result? result
---@return string? message
---@return integer? code
paneflow.events.subscribe = function(params) end

---@class paneflow.fleet
paneflow.fleet = {}

---@class paneflow.fleet.list.params

---@class paneflow.fleet.list.result
---@field agents table[]

---List the agents running in every workspace.
---@param params paneflow.fleet.list.params?
---@return paneflow.fleet.list.result? result
---@return string? message
---@return integer? code
paneflow.fleet.list = function(params) end

---@class paneflow.keymap
paneflow.keymap = {}

---@class paneflow.keymap.list.params

---@class paneflow.keymap.list.result
---@field bindings table[]
---@field unbound table[]

---List the key bindings init.lua declared and the keys it unbound, for the window to apply.
---@param params paneflow.keymap.list.params?
---@return paneflow.keymap.list.result? result
---@return string? message
---@return integer? code
paneflow.keymap.list = function(params) end

---@class paneflow.pane
paneflow.pane = {}

---@class paneflow.pane.write.params
---@field scope? "workspace"|"all"
---@field session string The session to write into
---@field source_session string The calling pane's session (PANEFLOW_SESSION_ID)
---@field submit? boolean
---@field text string Text to relay, prefixed with a provenance line

---@class paneflow.pane.write.result
---@field bytes? integer
---@field message? string
---@field request? string
---@field status string
---@field submit_mode? string
---@field submitted? boolean

---Relay a message from one agent's pane into another's, once a human allows the pair.
---@param params paneflow.pane.write.params
---@return paneflow.pane.write.result? result
---@return string? message
---@return integer? code
paneflow.pane.write = function(params) end

---@class paneflow.prompt
paneflow.prompt = {}

---@class paneflow.prompt.follow.params
---@field kinds ("input"|"pick")[] The prompt kinds this client can show

---@class paneflow.prompt.follow.result
---@field kinds ("input"|"pick")[]
---@field pending integer
---@field type "following"

---Stream the prompts this client can show: a following header, a requested frame for each pending prompt oldest first, then requested frames (id, spec, origin, remaining time) and ended frames (id, reason), keepalive frames when quiet.
---@param params paneflow.prompt.follow.params
---@return paneflow.prompt.follow.result? result
---@return string? message
---@return integer? code
paneflow.prompt.follow = function(params) end

---@class paneflow.prompt.reply.params
---@field answer? string|string[]|table The text, the id, the list of ids or {text}, as the prompt's kind takes
---@field cancel? boolean Dismiss the prompt; the asker receives cancelled
---@field id integer

---@class paneflow.prompt.reply.result
---@field id integer
---@field reason "answered"|"cancelled"|"timeout"|"no_client"|"failed"

---Answer or cancel a pending prompt. The first reply wins and every follower receives ended; a later one is refused with -32060 and data {reason: not_pending}.
---@param params paneflow.prompt.reply.params
---@return paneflow.prompt.reply.result? result
---@return string? message
---@return integer? code
paneflow.prompt.reply = function(params) end

---@class paneflow.prompt.request.params
---@field allow_other? boolean Accept typed text that matches no item; the answer is {text}
---@field confirm? string Label of the confirming row
---@field items? (string|table)[] The choices of a pick prompt, at most 2000: an id string or an item
---@field kind "input"|"pick" input asks for a line of text, pick for a choice among items
---@field multiple? boolean Let the user tick several items; the answer is the list of ids
---@field origin? table Who asks, shown under the title
---@field placeholder? string
---@field timeout? number Seconds before the prompt ends with timeout: 300 by default, at most 3600
---@field title string
---@field value? string Text the field starts with
---@field wait_ms? integer Milliseconds to wait for the answer before returning pending with the id, 0 by default, at most 25000

---@class paneflow.prompt.request.result
---@field answer? string|string[]|table
---@field detail? string
---@field id? integer
---@field status "answered"|"pending"|"cancelled"|"timeout"|"no_client"|"unsupported"|"busy"|"failed"

---Ask the user through every attached client that can show the prompt; the first answer wins. Returns {status: answered, answer} within wait_ms, {status: pending, id} when wait_ms passes first, or the reason it ended: cancelled, timeout, no_client, unsupported, busy or failed with a detail.
---@param params paneflow.prompt.request.params
---@return paneflow.prompt.request.result? result
---@return string? message
---@return integer? code
paneflow.prompt.request = function(params) end

---@class paneflow.prompt.wait.params
---@field id integer
---@field wait_ms? integer Milliseconds to wait, 0 by default, at most 25000

---@class paneflow.prompt.wait.result
---@field answer? string|string[]|table
---@field detail? string
---@field id? integer
---@field status "answered"|"pending"|"cancelled"|"timeout"|"no_client"|"unsupported"|"busy"|"failed"

---Keep waiting on a pending prompt by id, with the same outcomes as prompt.request.
---@param params paneflow.prompt.wait.params
---@return paneflow.prompt.wait.result? result
---@return string? message
---@return integer? code
paneflow.prompt.wait = function(params) end

---@class paneflow.surface
paneflow.surface = {}

---@class paneflow.surface.focus.params
---@field surface_id integer

---@class paneflow.surface.focus.result
---@field focused boolean
---@field scope string
---@field surface_id integer
---@field workspace integer

---Reveal a surface and give it the keyboard focus.
---@param params paneflow.surface.focus.params
---@return paneflow.surface.focus.result? result
---@return string? message
---@return integer? code
paneflow.surface.focus = function(params) end

---@class paneflow.surface.list.params
---@field scope_session? string List the workspace that holds this session
---@field workspace_id? integer List this workspace instead of the shown one

---@class paneflow.surface.list.result
---@field pane_count integer
---@field scope_workspace_id? integer
---@field surfaces table[]
---@field workspace integer

---List the surfaces of the shown workspace, or of one workspace, with their identity.
---@param params paneflow.surface.list.params?
---@return paneflow.surface.list.result? result
---@return string? message
---@return integer? code
paneflow.surface.list = function(params) end

---@class paneflow.surface.read.params
---@field fenced? boolean Wrap the text in an untrusted-output fence; the ai_injection_fence setting decides when absent
---@field lines? integer Trailing lines to return, clamped to 1..4000
---@field name? string Pick the surface by its unique name instead of its id
---@field offset? integer Lines to skip back from the end of the buffer
---@field scope_session? string
---@field surface_id? integer The surface to read; the first surface of the shown tab when absent
---@field workspace_id? integer

---@class paneflow.surface.read.result
---@field eof boolean
---@field lines integer
---@field output_generation integer
---@field text string
---@field total_lines integer
---@field truncated boolean

---Read a surface's trailing scrollback lines.
---@param params paneflow.surface.read.params?
---@return paneflow.surface.read.result? result
---@return string? message
---@return integer? code
paneflow.surface.read = function(params) end

---@class paneflow.surface.rename.params
---@field name? string The new name; an absent or empty name clears the custom name
---@field new_name? string Alias of name
---@field surface_id? integer

---@class paneflow.surface.rename.result
---@field name? string
---@field renamed boolean

---Give a surface a custom name, or clear it.
---@param params paneflow.surface.rename.params?
---@return paneflow.surface.rename.result? result
---@return string? message
---@return integer? code
paneflow.surface.rename = function(params) end

---@class paneflow.surface.search.params
---@field max_matches? integer Matches to return, clamped to 1..1000
---@field name? string
---@field pattern string Substring or pattern to find, at most 512 bytes
---@field scope_session? string
---@field surface_id? integer
---@field workspace_id? integer

---@class paneflow.surface.search.result
---@field matches table[]
---@field truncated boolean

---Find the scrollback lines of a surface that contain a pattern.
---@param params paneflow.surface.search.params
---@return paneflow.surface.search.result? result
---@return string? message
---@return integer? code
paneflow.surface.search = function(params) end

---@class paneflow.surface.send_keystroke.params
---@field keystroke? string Dash-separated key such as escape, ctrl-c or alt-f; required, and never one that submits a line
---@field scope? "workspace"|"all"
---@field scope_session? string
---@field surface_id? integer

---@class paneflow.surface.send_keystroke.result
---@field sent boolean

---Send a named key such as escape or ctrl-c to a surface.
---@param params paneflow.surface.send_keystroke.params?
---@return paneflow.surface.send_keystroke.result? result
---@return string? message
---@return integer? code
paneflow.surface.send_keystroke = function(params) end

---@class paneflow.surface.send_text.params
---@field force? boolean Write even when the agent waits for a decision; the write is logged
---@field paste? boolean Force or refuse bracketed paste; decided from the target when absent
---@field scope? "workspace"|"all"
---@field scope_session? string The calling pane's session; writes stay in its workspace
---@field submit? boolean Submit with a carriage return after the text
---@field surface_id? integer
---@field text? string Text to write; required unless submit is true. At most 64 KiB

---@class paneflow.surface.send_text.result
---@field agent_target boolean
---@field agent_tool? string
---@field length integer
---@field paste boolean
---@field sent boolean
---@field submit_mode? "deferred_paste_cr"|"inline_cr"
---@field submitted boolean
---@field terminal_bracketed_paste boolean

---Write text into a surface, submitting it only when asked.
---@param params paneflow.surface.send_text.params?
---@return paneflow.surface.send_text.result? result
---@return string? message
---@return integer? code
paneflow.surface.send_text = function(params) end

---@class paneflow.surface.split.params
---@field command? string
---@field cwd? string
---@field direction "horizontal"|"vertical" horizontal stacks the panes, vertical puts them side by side
---@field env? table<string, string>
---@field label? string
---@field managed_worktree? table
---@field name? string Alias of label
---@field profile? "normal"|"agent"|"cached"
---@field prompt? string
---@field surface_id? integer Split the pane holding this surface; the shown tab's first pane when absent

---@class paneflow.surface.split.result
---@field direction string
---@field panes integer
---@field split boolean
---@field surface_id integer

---Split the pane holding a surface and start a terminal in the new pane.
---@param params paneflow.surface.split.params
---@return paneflow.surface.split.result? result
---@return string? message
---@return integer? code
paneflow.surface.split = function(params) end

---@class paneflow.surface.status.params
---@field name? string
---@field surface_id? integer

---@class paneflow.surface.status.result
---@field active_tool_name? string
---@field hooked boolean
---@field idle_ms? integer
---@field last_result? string
---@field message? string
---@field output_generation integer
---@field session string
---@field state? string
---@field state_seq? integer
---@field surface_id integer
---@field tool? string
---@field waiting_ms? integer

---Read a surface's agent state, hook status and output generation.
---@param params paneflow.surface.status.params?
---@return paneflow.surface.status.result? result
---@return string? message
---@return integer? code
paneflow.surface.status = function(params) end

---@class paneflow.system
paneflow.system = {}

---@class paneflow.system.capabilities.params

---@class paneflow.system.capabilities.result
---@field methods string[]
---@field orchestration boolean
---@field scripting boolean

---List the methods this endpoint serves and whether its gates are open.
---@param params paneflow.system.capabilities.params?
---@return paneflow.system.capabilities.result? result
---@return string? message
---@return integer? code
paneflow.system.capabilities = function(params) end

---@class paneflow.system.counters.params

---@class paneflow.system.counters.result
---@field counters table

---Read the app's work counters: renders, git spawns, process spawns.
---@param params paneflow.system.counters.params?
---@return paneflow.system.counters.result? result
---@return string? message
---@return integer? code
paneflow.system.counters = function(params) end

---@class paneflow.system.describe.params
---@field method? string Describe only this method

---@class paneflow.system.describe.result
---@field callable boolean
---@field examples table[]
---@field fallback? string
---@field gate string
---@field gate_note? string
---@field in_capabilities boolean
---@field methods table[]
---@field name string
---@field owner string
---@field params any
---@field reply string
---@field result any
---@field stream boolean
---@field summary string

---Describe every method, or one: owner, gate, params and result schemas, examples.
---@param params paneflow.system.describe.params?
---@return paneflow.system.describe.result? result
---@return string? message
---@return integer? code
paneflow.system.describe = function(params) end

---@class paneflow.system.identify.params

---@class paneflow.system.identify.result
---@field name string
---@field protocol string
---@field version string

---Name the running app, its version and its wire protocol.
---@param params paneflow.system.identify.params?
---@return paneflow.system.identify.result? result
---@return string? message
---@return integer? code
paneflow.system.identify = function(params) end

---@class paneflow.system.ping.params

---@class paneflow.system.ping.result
---@field pong boolean

---Check that the endpoint answers.
---@param params paneflow.system.ping.params?
---@return paneflow.system.ping.result? result
---@return string? message
---@return integer? code
paneflow.system.ping = function(params) end

---@class paneflow.tmux
paneflow.tmux = {}

---@class paneflow.tmux.compat.params
---@field argv string[] The tmux command line, at most 256 arguments and 256 KiB
---@field scope_session? string
---@field surface_id integer The calling pane's surface
---@field team string Team name, 1 to 64 bytes

---@class paneflow.tmux.compat.result
---@field exit integer
---@field stderr string
---@field stdout string

---Run a tmux command for an agent team, as the paneflow tmux shim does.
---@param params paneflow.tmux.compat.params
---@return paneflow.tmux.compat.result? result
---@return string? message
---@return integer? code
paneflow.tmux.compat = function(params) end

---@class paneflow.usage
paneflow.usage = {}

---@class paneflow.usage.import.params
---@field accounts? table[]
---@field force? boolean usage.import only: start again even when an import already ran

---@class paneflow.usage.import.result

---Start importing past activity from the agents' local logs.
---@param params paneflow.usage.import.params?
---@return paneflow.usage.import.result? result
---@return string? message
---@return integer? code
paneflow.usage.import = function(params) end

---@class paneflow.usage.refresh.params
---@field accounts? table[]
---@field force? boolean usage.import only: start again even when an import already ran

---@class paneflow.usage.refresh.result

---Read fresh usage for the running agents and the listed accounts.
---@param params paneflow.usage.refresh.params?
---@return paneflow.usage.refresh.result? result
---@return string? message
---@return integer? code
paneflow.usage.refresh = function(params) end

---@class paneflow.usage.snapshot.params

---@class paneflow.usage.snapshot.result

---Read the usage ledger snapshot.
---@param params paneflow.usage.snapshot.params?
---@return paneflow.usage.snapshot.result? result
---@return string? message
---@return integer? code
paneflow.usage.snapshot = function(params) end

---@class paneflow.worker
paneflow.worker = {}

---@class paneflow.worker.hello.params

---@class paneflow.worker.hello.result
---@field build_id string
---@field capabilities string[]
---@field endpoint string
---@field home string
---@field name string
---@field pid integer
---@field protocol integer
---@field required_core_protocol integer
---@field started_at_ms integer
---@field version string

---Open a worker connection and read the worker's identity and capabilities.
---@param params paneflow.worker.hello.params?
---@return paneflow.worker.hello.result? result
---@return string? message
---@return integer? code
paneflow.worker.hello = function(params) end

---@class paneflow.worker.shutdown.params

---@class paneflow.worker.shutdown.result
---@field pid integer
---@field stopping boolean

---Ask the worker to stop; terminals keep running in the host.
---@param params paneflow.worker.shutdown.params?
---@return paneflow.worker.shutdown.result? result
---@return string? message
---@return integer? code
paneflow.worker.shutdown = function(params) end

---@class paneflow.worker.status.params

---@class paneflow.worker.status.result
---@field capabilities string[]
---@field core_connected boolean
---@field counters any
---@field endpoint string
---@field home string
---@field pid integer
---@field protocol integer
---@field session_count integer
---@field version string

---Read the worker's pid, version, session count and counters.
---@param params paneflow.worker.status.params?
---@return paneflow.worker.status.result? result
---@return string? message
---@return integer? code
paneflow.worker.status = function(params) end

---@class paneflow.workspace
paneflow.workspace = {}

---@class paneflow.workspace.close.params
---@field index integer Zero-based workspace index

---@class paneflow.workspace.close.result
---@field closed integer

---Close the workspace at an index; the last workspace cannot be closed.
---@param params paneflow.workspace.close.params
---@return paneflow.workspace.close.result? result
---@return string? message
---@return integer? code
paneflow.workspace.close = function(params) end

---@class paneflow.workspace.create.params
---@field cwd? string Directory of the first pane; it must exist
---@field layout? table
---@field name? string Workspace title
---@field space? string Space by name or id; the active space when absent

---@class paneflow.workspace.create.result
---@field index integer
---@field panes integer
---@field title string

---Open a workspace with one terminal, or with a layout document.
---@param params paneflow.workspace.create.params?
---@return paneflow.workspace.create.result? result
---@return string? message
---@return integer? code
paneflow.workspace.create = function(params) end

---@class paneflow.workspace.current.params

---@class paneflow.workspace.current.result
---@field cwd string
---@field index integer
---@field layout? table
---@field panes integer
---@field space? table
---@field title string

---Describe the shown workspace with its layout document, or null when none is shown.
---@param params paneflow.workspace.current.params?
---@return paneflow.workspace.current.result? result
---@return string? message
---@return integer? code
paneflow.workspace.current = function(params) end

---@class paneflow.workspace.list.params

---@class paneflow.workspace.list.result
---@field workspaces table[]

---List every workspace with its title, directory, pane count and space.
---@param params paneflow.workspace.list.params?
---@return paneflow.workspace.list.result? result
---@return string? message
---@return integer? code
paneflow.workspace.list = function(params) end

---@class paneflow.workspace.restore_layout.params
---@field layout table Holds between 1 and 32 panes

---@class paneflow.workspace.restore_layout.result
---@field panes integer
---@field restored boolean

---Conform the shown tab to a layout document, reusing its panes.
---@param params paneflow.workspace.restore_layout.params
---@return paneflow.workspace.restore_layout.result? result
---@return string? message
---@return integer? code
paneflow.workspace.restore_layout = function(params) end

---@class paneflow.workspace.select.params
---@field index integer Zero-based workspace index

---@class paneflow.workspace.select.result
---@field selected integer

---Show the workspace at an index.
---@param params paneflow.workspace.select.params
---@return paneflow.workspace.select.result? result
---@return string? message
---@return integer? code
paneflow.workspace.select = function(params) end

---@class paneflow.workspace.up.params
---@field layout? string Layout preset
---@field name? string Workspace title
---@field panes table[]
---@field space? string

---@class paneflow.workspace.up.result
---@field index integer
---@field labels string[]
---@field panes integer
---@field surface_ids integer[]
---@field title string

---Open a workspace from pane specs, each with its own directory, command or prompt.
---@param params paneflow.workspace.up.params
---@return paneflow.workspace.up.result? result
---@return string? message
---@return integer? code
paneflow.workspace.up = function(params) end
