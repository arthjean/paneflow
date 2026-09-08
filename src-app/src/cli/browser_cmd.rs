use clap::Subcommand;
use paneflow_ipc_client::{IpcClient, IpcTransport};
use serde_json::{Value, json};

use super::{CliError, print_json};

#[derive(Subcommand, Debug)]
pub(crate) enum BrowserCommand {
    #[command(about = "List browser pages in the scoped workspace")]
    List,
    #[command(about = "Read one browser page state")]
    State { browser_id: String, generation: u64 },
    #[command(about = "Read the terminal status of an accepted browser operation")]
    Operation {
        operation_id: String,
        #[arg(long, help = "Base64 byte offset for a completed screenshot")]
        offset: Option<usize>,
        #[arg(long, help = "Maximum screenshot chunk bytes")]
        limit: Option<usize>,
    },
    #[command(about = "Read a browser accessibility snapshot")]
    Snapshot { browser_id: String, generation: u64 },
    #[command(about = "Capture the browser viewport")]
    Screenshot { browser_id: String, generation: u64 },
    #[command(about = "Renew a pending browser operation lease")]
    Renew {
        operation_id: String,
        #[arg(long, default_value_t = 30000)]
        extension_ms: u64,
    },
    #[command(about = "Read buffered browser console entries")]
    Console { browser_id: String, generation: u64 },
    #[command(about = "Read buffered browser network entries")]
    Network { browser_id: String, generation: u64 },
    #[command(about = "Navigate a browser page without taking focus")]
    Navigate {
        browser_id: String,
        generation: u64,
        url: String,
    },
    #[command(about = "Go back in browser history")]
    Back { browser_id: String, generation: u64 },
    #[command(about = "Go forward in browser history")]
    Forward { browser_id: String, generation: u64 },
    #[command(about = "Reload a browser page without taking focus")]
    Reload { browser_id: String, generation: u64 },
    #[command(about = "Click a browser viewport coordinate")]
    Click {
        browser_id: String,
        generation: u64,
        x: i32,
        y: i32,
    },
    #[command(about = "Type text into the browser's current human-controlled target")]
    Type {
        browser_id: String,
        generation: u64,
        text: String,
    },
    #[command(about = "Scroll a browser viewport coordinate")]
    Scroll {
        browser_id: String,
        generation: u64,
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
    },
}

pub(crate) fn run(command: BrowserCommand, client: &IpcClient) -> Result<i32, CliError> {
    let (method, params) = match command {
        BrowserCommand::List => ("browser.list", json!({})),
        BrowserCommand::State {
            browser_id,
            generation,
        } => ("browser.state", target(&browser_id, generation)),
        BrowserCommand::Operation {
            operation_id,
            offset,
            limit,
        } => {
            let mut params = json!({"operation_id": operation_id});
            if let Some(offset) = offset {
                params["offset"] = json!(offset);
            }
            if let Some(limit) = limit {
                params["limit"] = json!(limit);
            }
            ("browser.operation", params)
        }
        BrowserCommand::Snapshot {
            browser_id,
            generation,
        } => ("browser.snapshot", target(&browser_id, generation)),
        BrowserCommand::Screenshot {
            browser_id,
            generation,
        } => ("browser.screenshot", target(&browser_id, generation)),
        BrowserCommand::Renew {
            operation_id,
            extension_ms,
        } => (
            "browser.renew",
            json!({"operation_id": operation_id, "extension_ms": extension_ms}),
        ),
        BrowserCommand::Console {
            browser_id,
            generation,
        } => ("browser.console", target(&browser_id, generation)),
        BrowserCommand::Network {
            browser_id,
            generation,
        } => ("browser.network", target(&browser_id, generation)),
        BrowserCommand::Navigate {
            browser_id,
            generation,
            url,
        } => (
            "browser.navigate",
            merge(target(&browser_id, generation), json!({"url": url})),
        ),
        BrowserCommand::Back {
            browser_id,
            generation,
        } => ("browser.back", target(&browser_id, generation)),
        BrowserCommand::Forward {
            browser_id,
            generation,
        } => ("browser.forward", target(&browser_id, generation)),
        BrowserCommand::Reload {
            browser_id,
            generation,
        } => ("browser.reload", target(&browser_id, generation)),
        BrowserCommand::Click {
            browser_id,
            generation,
            x,
            y,
        } => (
            "browser.click",
            merge(target(&browser_id, generation), json!({"x": x, "y": y})),
        ),
        BrowserCommand::Type {
            browser_id,
            generation,
            text,
        } => (
            "browser.type",
            merge(target(&browser_id, generation), json!({"text": text})),
        ),
        BrowserCommand::Scroll {
            browser_id,
            generation,
            x,
            y,
            delta_x,
            delta_y,
        } => (
            "browser.scroll",
            merge(
                target(&browser_id, generation),
                json!({"x": x, "y": y, "delta_x": delta_x, "delta_y": delta_y}),
            ),
        ),
    };
    let result = client.call(method, params).map_err(CliError::runtime)?;
    print_json(&result)?;
    Ok(super::EXIT_OK)
}

fn target(browser_id: &str, generation: u64) -> Value {
    json!({"browser_id": browser_id, "generation": generation})
}

fn merge(mut target: Value, extra: Value) -> Value {
    if let (Some(target), Some(extra)) = (target.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    target
}
