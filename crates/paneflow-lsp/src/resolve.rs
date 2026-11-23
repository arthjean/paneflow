use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    NotFound(String),
    RelativePath(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(command) => write!(f, "language server command {command:?} was not found"),
            Self::RelativePath(command) => write!(
                f,
                "language server command {command:?} is a relative path; use a bare name found on PATH or an absolute path"
            ),
        }
    }
}

pub fn resolve_command(command: &str) -> Result<PathBuf, ResolveError> {
    resolve_command_in(command, std::env::var_os("PATH"))
}

pub fn resolve_command_in(
    command: &str,
    search_path: Option<OsString>,
) -> Result<PathBuf, ResolveError> {
    let candidate = Path::new(command);
    if candidate.is_absolute() {
        return if candidate.is_file() {
            Ok(candidate.to_path_buf())
        } else {
            Err(ResolveError::NotFound(command.to_string()))
        };
    }
    if has_separator(command) {
        let error = ResolveError::RelativePath(command.to_string());
        log::warn!("{error}");
        return Err(error);
    }
    let absolute_entries = search_path.and_then(|path| {
        let entries: Vec<PathBuf> = std::env::split_paths(&path)
            .filter(|entry| entry.is_absolute())
            .collect();
        std::env::join_paths(entries).ok()
    });
    let cwd = std::env::temp_dir();
    which::which_in(command, absolute_entries, cwd)
        .map_err(|_| ResolveError::NotFound(command.to_string()))
}

pub fn has_separator(command: &str) -> bool {
    command.contains('/') || (cfg!(windows) && command.contains('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        });
        std::fs::write(&path, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn a_bare_name_is_searched_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let server = executable(dir.path(), "fake-ls");
        let found = resolve_command_in("fake-ls", Some(dir.path().as_os_str().to_owned())).unwrap();
        assert_eq!(
            found.canonicalize().unwrap(),
            server.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_command_in("other-ls", Some(dir.path().as_os_str().to_owned())),
            Err(ResolveError::NotFound("other-ls".to_string()))
        );
    }

    #[test]
    fn an_absolute_path_is_used_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let server = executable(dir.path(), "abs-ls");
        let command = server.to_str().unwrap();
        assert_eq!(resolve_command_in(command, None), Ok(server.clone()));
        let missing = dir.path().join("missing-ls");
        assert!(matches!(
            resolve_command_in(missing.to_str().unwrap(), None),
            Err(ResolveError::NotFound(_))
        ));
    }

    fn below_root(path: &Path) -> impl Iterator<Item = std::path::Component<'_>> {
        path.components().filter(|part| {
            !matches!(
                part,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        })
    }

    fn relative_to_working_directory(target: &Path) -> PathBuf {
        let cwd = std::env::current_dir().unwrap();
        let mut relative: PathBuf = below_root(&cwd).map(|_| "..").collect();
        relative.extend(below_root(target));
        relative
    }

    #[test]
    fn a_relative_path_entry_never_resolves_against_the_working_directory() {
        let planted = tempfile::tempdir().unwrap();
        executable(planted.path(), "planted-ls");
        let relative = relative_to_working_directory(planted.path());
        assert!(!relative.is_absolute());
        let search_path = std::env::join_paths([PathBuf::new(), relative]).unwrap();
        assert_eq!(
            resolve_command_in("planted-ls", Some(search_path)),
            Err(ResolveError::NotFound("planted-ls".to_string()))
        );
        assert!(
            resolve_command_in("planted-ls", Some(planted.path().as_os_str().to_owned())).is_ok()
        );
    }

    #[test]
    fn a_relative_path_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        executable(dir.path(), "rel-ls");
        assert_eq!(
            resolve_command_in("./rel-ls", Some(dir.path().as_os_str().to_owned())),
            Err(ResolveError::RelativePath("./rel-ls".to_string()))
        );
        assert_eq!(
            resolve_command_in("bin/rel-ls", None),
            Err(ResolveError::RelativePath("bin/rel-ls".to_string()))
        );
    }
}
