use std::path::PathBuf;
use std::sync::Arc;

use clap::Subcommand;
use paneflow_api::methods::config::{ActionList, ConfigProblem, ConfigStatus, Severity};
use paneflow_script::{Level, Options, Runtime, Sink};
use serde_json::{Value, json};

use super::do_cmd::CliBackend;
use super::router::{self, Endpoints};
use super::{CliError, EXIT_OK, EXIT_RUNTIME};

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    #[command(
        about = "Load init.lua in the worker again now and print its status; a failed load keeps the previous configuration"
    )]
    Reload {
        #[arg(long, help = "Print the config.status result as JSON")]
        json: bool,
    },
    #[command(
        about = "Load a configuration file in this process and print its problems with file and line; exits 1 when any is an error"
    )]
    Check {
        #[arg(
            value_name = "PATH",
            help = "The file to check, init.lua of the Paneflow home by default"
        )]
        path: Option<PathBuf>,
    },
}

pub(super) fn problem_line(problem: &ConfigProblem) -> String {
    let severity = match problem.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    match (&problem.file, problem.line) {
        (Some(file), Some(line)) => format!("{file}:{line}: {severity}: {}", problem.message),
        (Some(file), None) => format!("{file}: {severity}: {}", problem.message),
        _ => format!("{severity}: {}", problem.message),
    }
}

fn call_worker(method: &str) -> Result<Value, CliError> {
    let catalog = paneflow_api::catalog().map_err(|error| CliError::runtime(error.to_string()))?;
    let endpoints = Endpoints::from_environment();
    let endpoint = router::route(&catalog, &endpoints, method)?;
    router::send(&catalog, &endpoint, method, json!({}))
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value)
        .map_err(|error| CliError::runtime(format!("the worker sent an unexpected reply: {error}")))
}

fn print_status(status: &ConfigStatus) {
    if !status.found {
        println!("{}: no file; nothing is loaded", status.path);
        return;
    }
    let state = if status.ok {
        "loaded"
    } else {
        "failed to load; the previous configuration stays active"
    };
    println!(
        "{}: {state} ({} actions, {} bindings, {} handlers, {} timers)",
        status.path, status.actions, status.bindings, status.handlers, status.timers
    );
    for problem in &status.problems {
        println!("{}", problem_line(problem));
    }
}

pub(super) fn run(command: ConfigCommand) -> Result<i32, CliError> {
    match command {
        ConfigCommand::Reload { json } => {
            let value = call_worker("config.reload")?;
            if json {
                super::print_json(&value)?;
            } else {
                print_status(&decode::<ConfigStatus>(value.clone())?);
            }
            let status: ConfigStatus = decode(value)?;
            Ok(if status.ok { EXIT_OK } else { EXIT_RUNTIME })
        }
        ConfigCommand::Check { path } => check(path),
    }
}

struct CheckSink;

impl Sink for CheckSink {
    fn log(&self, level: Level, message: &str) {
        eprintln!("[{}] {message}", level.label());
    }

    fn report(&self, message: &str) {
        eprintln!("{message}");
    }
}

fn check(path: Option<PathBuf>) -> Result<i32, CliError> {
    let path = match path {
        Some(path) => path,
        None => paneflow_home::init_lua_path().ok_or_else(|| {
            CliError::runtime("cannot resolve the Paneflow state home; set PANEFLOW_HOME")
        })?,
    };
    let source = std::fs::read(&path)
        .map_err(|error| CliError::runtime(format!("{}: {error}", path.display())))?;
    let root = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut options = Options::new(Arc::new(CheckSink));
    options.config_root = Some(root.clone());
    options.lua_dir = Some(root.join(paneflow_home::LUA_DIR_NAME));
    options.serving = true;
    options.memory_limit = Some(paneflow_serve::config_runtime::MEMORY_LIMIT);
    let catalog = paneflow_api::catalog().map_err(|error| CliError::runtime(error.to_string()))?;
    let backend = Arc::new(CliBackend::new(catalog, Endpoints::from_environment()));
    let runtime = Runtime::new(backend, options).map_err(CliError::runtime)?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().to_string(),
    );
    let report = match runtime.compile(&source, &format!("@{name}")) {
        Ok(chunk) => {
            runtime.attach().map_err(CliError::runtime)?;
            let interrupter = runtime.interrupter();
            let _ = ctrlc::set_handler(move || interrupter.interrupt());
            runtime.load(chunk)
        }
        Err(message) => paneflow_script::LoadReport {
            ok: false,
            problems: vec![paneflow_script::config::problem_from_message(
                Severity::Error,
                &message,
            )],
            error: Some(message),
            ..Default::default()
        },
    };
    for problem in &report.problems {
        println!("{}", problem_line(problem));
    }
    if report.ok && !report.has_errors() {
        println!(
            "{}: ok ({} actions, {} bindings, {} handlers, {} timers)",
            path.display(),
            report.actions.len(),
            report.keymap.bindings.len(),
            report.handlers,
            report.timers
        );
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_RUNTIME)
    }
}

pub(super) fn actions(json_output: bool) -> Result<i32, CliError> {
    let value = call_worker("action.list")?;
    if json_output {
        super::print_json(&value)?;
        return Ok(EXIT_OK);
    }
    let list: ActionList = decode(value)?;
    if list.actions.is_empty() {
        println!("no actions; declare them in init.lua with paneflow.action{{ ... }}");
        return Ok(EXIT_OK);
    }
    let width = list
        .actions
        .iter()
        .map(|action| action.name.len())
        .max()
        .unwrap_or(0);
    for action in &list.actions {
        println!(
            "{:<width$}  {}  [{}]  {}",
            action.name, action.title, action.category, action.source
        );
    }
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_prints_its_file_line_and_severity() {
        let problem = ConfigProblem {
            file: Some("init.lua".into()),
            line: Some(4),
            severity: Severity::Error,
            message: "unexpected symbol".into(),
        };
        assert_eq!(
            problem_line(&problem),
            "init.lua:4: error: unexpected symbol"
        );
    }

    #[test]
    fn check_exits_one_on_an_error_and_zero_on_a_clean_file() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("init.lua");
        std::fs::write(
            &broken,
            "paneflow.action{ name = 'Bad', title = 'x', run = function() end }",
        )
        .unwrap();
        assert_eq!(check(Some(broken.clone())).unwrap(), EXIT_RUNTIME);
        std::fs::write(&broken, "paneflow.action{ name = 'x' title = 'y' }").unwrap();
        assert_eq!(check(Some(broken.clone())).unwrap(), EXIT_RUNTIME);
        std::fs::write(
            &broken,
            "paneflow.action{ name = 'good', title = 'Good', run = function() end }\npaneflow.bind('ctrl-alt-g', 'good')",
        )
        .unwrap();
        assert_eq!(check(Some(broken)).unwrap(), EXIT_OK);
    }
}
