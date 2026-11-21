use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::schema::SessionState;

pub const MAX_SESSION_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum SessionWriteError {
    #[error("Could not create the session directory")]
    Directory(#[source] io::Error),
    #[error("Could not encode the session")]
    Encode(#[source] serde_json::Error),
    #[error("The session exceeds the size that Paneflow can restore")]
    TooLarge,
    #[error("Could not write the new session")]
    Stage(#[source] io::Error),
    #[error("Could not stage the session for recovery")]
    Promote(#[source] io::Error),
    #[error("Could not replace {}. The new session is preserved at {}", target.display(), recovery.display())]
    Replace {
        target: PathBuf,
        recovery: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl SessionWriteError {
    pub fn cause(&self) -> String {
        let kind = match self {
            Self::Directory(error)
            | Self::Stage(error)
            | Self::Promote(error)
            | Self::Replace { source: error, .. } => format!("{:?}", error.kind()),
            Self::Encode(_) => "encode".to_string(),
            Self::TooLarge => "too_large".to_string(),
        };
        let step = match self {
            Self::Directory(_) => "directory",
            Self::Encode(_) => "encode",
            Self::TooLarge => "size",
            Self::Stage(_) => "stage",
            Self::Promote(_) => "promote",
            Self::Replace { .. } => "replace",
        };
        format!("{step}:{kind}")
    }
}

pub fn pending_session_path(path: &Path) -> PathBuf {
    paneflow_home::resolve_write_target(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .with_extension("json.pending")
}

pub fn retry_session_io<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        match operation() {
            Err(error)
                if attempt < 3
                    && matches!(
                        error.kind(),
                        io::ErrorKind::PermissionDenied
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

pub fn write_session_file(path: &Path, state: &SessionState) -> Result<(), SessionWriteError> {
    if let Some(parent) = path.parent() {
        paneflow_home::create_private_dir_all(parent).map_err(SessionWriteError::Directory)?;
    }
    let json = serde_json::to_vec_pretty(state).map_err(SessionWriteError::Encode)?;
    if json.len() as u64 > MAX_SESSION_FILE_BYTES {
        return Err(SessionWriteError::TooLarge);
    }
    let mut staged = paneflow_home::stage_write(path, &json).map_err(SessionWriteError::Stage)?;
    let target = staged.target().to_path_buf();
    let recovery = pending_session_path(&target);
    retry_session_io(|| staged.promote(&recovery)).map_err(SessionWriteError::Promote)?;
    retry_session_io(|| std::fs::rename(&recovery, &target)).map_err(|source| {
        SessionWriteError::Replace {
            target,
            recovery,
            source,
        }
    })
}
