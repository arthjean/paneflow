use crate::schema::{validate_layout, PaneFlowConfig};
use serde_json::{Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

pub const APP_SUBDIR: &str = if cfg!(debug_assertions) {
    "paneflow-dev"
} else {
    "paneflow"
};

const MAX_CONFIG_SIZE_BYTES: u64 = 1 << 20;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("config path is not a regular file: {path}")]
    NotRegularFile { path: PathBuf },
    #[error("config file {path} is {actual} bytes, over the {maximum}-byte cap")]
    TooLarge {
        path: PathBuf,
        actual: u64,
        maximum: u64,
    },
    #[error("invalid config document: {0}")]
    ParseError(#[from] serde_json::Error),
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(APP_SUBDIR).join("paneflow.json"))
}

pub fn session_path() -> Option<PathBuf> {
    let filename = if cfg!(debug_assertions) {
        "session-dev.json"
    } else {
        "session.json"
    };
    dirs::cache_dir().map(|dir| dir.join(APP_SUBDIR).join(filename))
}

pub fn load_config() -> PaneFlowConfig {
    let Some(path) = config_path() else {
        warn!("could not determine config directory; using defaults");
        return PaneFlowConfig::default();
    };

    load_config_from_path(&path)
}

pub fn read_config_string(path: &Path) -> Result<Option<String>, ConfigError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::IoError {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let metadata = file.metadata().map_err(|source| ConfigError::IoError {
        path: path.to_path_buf(),
        source,
    })?;
    match metadata {
        meta if !meta.file_type().is_file() => Err(ConfigError::NotRegularFile {
            path: path.to_path_buf(),
        }),
        meta if meta.len() > MAX_CONFIG_SIZE_BYTES => Err(ConfigError::TooLarge {
            path: path.to_path_buf(),
            actual: meta.len(),
            maximum: MAX_CONFIG_SIZE_BYTES,
        }),
        _ => {
            let mut contents = String::new();
            match file
                .take(MAX_CONFIG_SIZE_BYTES + 1)
                .read_to_string(&mut contents)
            {
                Ok(_) if contents.len() as u64 <= MAX_CONFIG_SIZE_BYTES => Ok(Some(contents)),
                Ok(_) => Err(ConfigError::TooLarge {
                    path: path.to_path_buf(),
                    actual: contents.len() as u64,
                    maximum: MAX_CONFIG_SIZE_BYTES,
                }),
                Err(source) => Err(ConfigError::IoError {
                    path: path.to_path_buf(),
                    source,
                }),
            }
        }
    }
}

pub fn load_config_from_path(path: &std::path::Path) -> PaneFlowConfig {
    match read_config_string(path) {
        Ok(Some(contents)) => parse_and_validate_with_path(&contents, path),
        Ok(None) => PaneFlowConfig::default(),
        Err(error) => {
            warn!("{error}; using defaults");
            PaneFlowConfig::default()
        }
    }
}

pub fn parse_and_validate(json: &str) -> PaneFlowConfig {
    parse_and_validate_with_path(json, Path::new("<config>"))
}

pub fn parse_and_validate_with_path(json: &str, path: &Path) -> PaneFlowConfig {
    try_parse_and_validate(json).unwrap_or_else(|e| {
        warn!("invalid config {}: {e}; using defaults", path.display());
        PaneFlowConfig::default()
    })
}

pub fn try_parse_and_validate(json: &str) -> Result<PaneFlowConfig, ConfigError> {
    let root: Map<String, Value> = serde_json::from_str(json)?;
    match root.get("$schemaVersion") {
        Some(Value::String(version)) if version != "1.0.0" => {
            warn!("config schema version `{version}` is not recognized; loading leniently");
        }
        Some(Value::String(_)) | None => {}
        Some(_) => warn!("config schema version is not a string; ignoring it"),
    }

    let mut config: PaneFlowConfig = serde_json::from_value(Value::Object(root))?;

    for cmd in &mut config.commands {
        if let Some(ws) = cmd.workspace_mut() {
            if let Some(ref mut layout) = ws.layout {
                validate_layout(layout);
            }
        }
    }

    Ok(config)
}
#[cfg(test)]
#[path = "loader_tests/core.rs"]
mod core_tests;
#[cfg(test)]
#[path = "loader_tests/session.rs"]
mod session_tests;
#[cfg(test)]
#[path = "loader_tests/settings.rs"]
mod settings_tests;
