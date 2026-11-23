use clap::Subcommand;
use paneflow_api::grants::{GrantOrigin, GrantSet, PaneGrant};
use paneflow_api::methods::grant::Revoked;
use serde_json::{Value, json};

use super::router::{self, Endpoints};
use super::{CliError, EXIT_OK};

#[derive(Subcommand, Debug)]
pub enum GrantsCommand {
    #[command(
        about = "List the grants panes hold: id, holder session, rights, scope, expiry and origin",
        long_about = "List the grants panes hold, as the worker stores them. From a pane, only the grants that pane holds or delegated are listed.\n\nExamples:\n  paneflow grants ls\n  paneflow grants ls --json"
    )]
    Ls {
        #[arg(long, help = "Print the grant.list result as JSON")]
        json: bool,
    },
    #[command(
        about = "Revoke a grant and every grant delegated from it",
        long_about = "Revoke a grant and every grant delegated from it. From a pane, only a grant that pane delegated can be revoked.\n\nExamples:\n  paneflow grants revoke g-12-1"
    )]
    Revoke {
        #[arg(
            value_name = "ID",
            help = "The grant id, as paneflow grants ls prints it"
        )]
        id: String,
        #[arg(long, help = "Print the grant.revoke result as JSON")]
        json: bool,
    },
}

fn call_worker(method: &str, params: Value) -> Result<Value, CliError> {
    let catalog = paneflow_api::catalog().map_err(|error| CliError::runtime(error.to_string()))?;
    let endpoints = Endpoints::from_environment();
    let endpoint = router::route(&catalog, &endpoints, method)?;
    router::send(&catalog, &endpoint, method, params)
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value)
        .map_err(|error| CliError::runtime(format!("the worker sent an unexpected reply: {error}")))
}

fn origin_label(origin: &GrantOrigin) -> String {
    match origin {
        GrantOrigin::Prompt => "prompt".to_string(),
        GrantOrigin::Profile { profile } => format!("profile {profile}"),
        GrantOrigin::Delegation => "delegation".to_string(),
    }
}

pub(super) fn grant_line(grant: &PaneGrant, now_ms: u64) -> String {
    let rights = grant
        .rights
        .iter()
        .map(|right| right.label())
        .collect::<Vec<_>>()
        .join(",");
    let expiry = grant.expires_at_ms.map_or_else(
        || "no expiry".to_string(),
        |expiry| format!("{}s left", expiry.saturating_sub(now_ms) / 1000),
    );
    let parent = grant
        .delegated_from
        .as_deref()
        .map(|parent| format!(" from {parent}"))
        .unwrap_or_default();
    format!(
        "{}  session {}  {rights}  {}  {expiry}  {}{parent}",
        grant.id,
        grant.holder.session,
        grant.scope.label(),
        origin_label(&grant.origin)
    )
}

pub(super) fn run(command: GrantsCommand) -> Result<i32, CliError> {
    match command {
        GrantsCommand::Ls { json } => {
            let value = call_worker("grant.list", json!({}))?;
            if json {
                super::print_json(&value)?;
                return Ok(EXIT_OK);
            }
            let set: GrantSet = decode(value)?;
            if set.grants.is_empty() {
                println!("no grant is held (revision {})", set.revision);
            }
            let now = paneflow_host::manifest::now_ms();
            for grant in &set.grants {
                println!("{}", grant_line(grant, now));
            }
            Ok(EXIT_OK)
        }
        GrantsCommand::Revoke { id, json } => {
            let value = call_worker("grant.revoke", json!({"id": id}))?;
            if json {
                super::print_json(&value)?;
                return Ok(EXIT_OK);
            }
            let revoked: Revoked = decode(value)?;
            println!(
                "revoked {} (revision {})",
                revoked.revoked.join(", "),
                revoked.revision
            );
            Ok(EXIT_OK)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::grants::{GrantHolder, GrantScope, Right};

    #[test]
    fn a_grant_line_names_id_holder_rights_scope_expiry_origin_and_parent() {
        let grant = PaneGrant {
            id: "g-2-1".into(),
            holder: GrantHolder {
                session: "s-1".into(),
                generation: 1,
                workspace: None,
            },
            rights: vec![Right::Write, Right::Spawn],
            scope: GrantScope::Workspace,
            expires_at_ms: Some(61_000),
            delegated_from: Some("g-1-1".into()),
            origin: GrantOrigin::Delegation,
            granted_at_ms: 0,
        };
        assert_eq!(
            grant_line(&grant, 1_000),
            "g-2-1  session s-1  write,spawn  workspace  60s left  delegation from g-1-1"
        );
    }
}
