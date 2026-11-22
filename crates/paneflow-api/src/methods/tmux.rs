use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{NO_FALLBACK, WINDOW};
use crate::descriptor::{Audience, Gate, Reply};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompatParams {
    #[schemars(description = "Team name, 1 to 64 bytes")]
    pub team: String,
    #[schemars(description = "The calling pane's surface")]
    pub surface_id: u64,
    #[schemars(description = "The tmux command line, at most 256 arguments and 256 KiB")]
    pub argv: Vec<String>,
    pub scope_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
}

method! {
    Compat {
        name: "tmux.compat",
        summary: "Run a tmux command for an agent team, as the paneflow tmux shim does.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Async,
        capabilities: true,
        params: CompatParams,
        result: Outcome,
        examples: [({"team": "review", "surface_id": 7, "argv": ["display-message", "-p", "#{pane_id}"]} => {
            "stdout": "%0\n",
            "stderr": "",
            "exit": 0
        })],
    }
}
