use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

pub const PANES_DIR_NAME: &str = "panes";

pub const PANE_HOST_ENDPOINT_ENV: &str = "PANEFLOW_PANE_HOST_ENDPOINT";
pub const PANE_SERVE_ENDPOINT_ENV: &str = "PANEFLOW_PANE_SERVE_ENDPOINT";
pub const PANE_WINDOW_ENDPOINT_ENV: &str = "PANEFLOW_PANE_WINDOW_ENDPOINT";

pub const PANE_ENDPOINT_ENV: [&str; 3] = [
    PANE_HOST_ENDPOINT_ENV,
    PANE_SERVE_ENDPOINT_ENV,
    PANE_WINDOW_ENDPOINT_ENV,
];

pub const SANDBOX_SID_PREFIX: &str = "S-1-9-1347";

pub const SANDBOX_SHARED_SID: &str = "S-1-9-1347-0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneServer {
    Host,
    Worker,
    Window,
}

impl PaneServer {
    pub const ALL: [Self; 3] = [Self::Host, Self::Worker, Self::Window];

    pub const fn env(self) -> &'static str {
        match self {
            Self::Host => PANE_HOST_ENDPOINT_ENV,
            Self::Worker => PANE_SERVE_ENDPOINT_ENV,
            Self::Window => PANE_WINDOW_ENDPOINT_ENV,
        }
    }

    pub const fn stem(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Worker => "serve",
            Self::Window => "window",
        }
    }
}

pub fn panes_dir_in(home: &Path) -> PathBuf {
    home.join(PANES_DIR_NAME)
}

pub fn pane_dir_in(home: &Path, session: &str) -> PathBuf {
    panes_dir_in(home).join(session)
}

pub fn is_pane_session_name(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 64
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(unix)]
pub fn pane_endpoint_path(home: &Path, session: &str, server: PaneServer) -> PathBuf {
    pane_dir_in(home, session).join(format!("{}.sock", server.stem()))
}

#[cfg(windows)]
pub fn pane_endpoint_path(home: &Path, session: &str, server: PaneServer) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\{}{session}-{}",
        pane_pipe_prefix(home),
        server.stem()
    ))
}

#[cfg(windows)]
fn pane_pipe_prefix(home: &Path) -> String {
    format!("paneflow-pane-{}-", crate::home_fingerprint(home))
}

pub fn pane_endpoint_session(home: &Path, server: PaneServer, path: &Path) -> Option<String> {
    let session = endpoint_session(home, server, path)?;
    crate::same_endpoint(&pane_endpoint_path(home, &session, server), path).then_some(session)
}

#[cfg(unix)]
fn endpoint_session(home: &Path, server: PaneServer, path: &Path) -> Option<String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return None;
    }
    let rest = path.strip_prefix(panes_dir_in(home)).ok()?;
    let mut parts = rest.components();
    let (Some(Component::Normal(session)), Some(Component::Normal(file)), None) =
        (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let session = session.to_str()?;
    (is_pane_session_name(session) && file == OsStr::new(&format!("{}.sock", server.stem())))
        .then(|| session.to_string())
}

#[cfg(windows)]
fn endpoint_session(home: &Path, server: PaneServer, path: &Path) -> Option<String> {
    let mut components = path.components();
    let (Some(Component::Prefix(_)), Some(Component::RootDir), Some(Component::Normal(name)), None) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) else {
        return None;
    };
    let name = name.to_str()?;
    let prefix = pane_pipe_prefix(home);
    let session = name
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(&prefix))
        .and_then(|_| name.get(prefix.len()..))?
        .strip_suffix(&format!("-{}", server.stem()))?;
    is_pane_session_name(session).then(|| session.to_string())
}

pub fn pane_endpoint_from(home: &Path, server: PaneServer, raw: Option<&OsStr>) -> Option<PathBuf> {
    let path = PathBuf::from(raw.filter(|raw| !raw.is_empty())?);
    pane_endpoint_session(home, server, &path)?;
    Some(path)
}

pub fn pane_endpoint_for_current_home(server: PaneServer) -> Option<PathBuf> {
    let home = crate::paneflow_home()?;
    pane_endpoint_from(&home, server, std::env::var_os(server.env()).as_deref())
}

pub fn pane_sandbox_sid(session: &str) -> String {
    let digest = crate::home_fingerprint(Path::new(session));
    let high = u32::from_str_radix(&digest[..8], 16).unwrap_or_default();
    let low = u32::from_str_radix(&digest[8..], 16).unwrap_or_default();
    format!("{SANDBOX_SID_PREFIX}-1-{high}-{low}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\Users\someone\.paneflow-dev"
        } else {
            "/home/someone/.paneflow-dev"
        })
    }

    const SESSION: &str = "0b5f3c1e-7d1a-4f0e-9a4b-2c8d6e1f3a5b";

    #[test]
    fn a_pane_endpoint_resolves_back_to_its_session() {
        for server in PaneServer::ALL {
            let path = pane_endpoint_path(&home(), SESSION, server);
            assert_eq!(
                pane_endpoint_session(&home(), server, &path).as_deref(),
                Some(SESSION),
                "{server:?}"
            );
            assert_eq!(
                pane_endpoint_from(&home(), server, Some(path.as_os_str())),
                Some(path)
            );
        }
    }

    #[test]
    fn an_endpoint_of_another_server_or_home_is_not_honored() {
        let host = pane_endpoint_path(&home(), SESSION, PaneServer::Host);
        assert_eq!(
            pane_endpoint_from(&home(), PaneServer::Worker, Some(host.as_os_str())),
            None
        );
        let other = PathBuf::from(if cfg!(windows) {
            r"C:\Users\someone\.paneflow-other"
        } else {
            "/home/someone/.paneflow-other"
        });
        assert_eq!(
            pane_endpoint_from(&other, PaneServer::Host, Some(host.as_os_str())),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_unix_pane_endpoint_must_lie_under_the_home_panes_directory() {
        let honored = |raw: &str| pane_endpoint_from(&home(), PaneServer::Host, Some(raw.as_ref()));
        assert!(
            honored(&format!(
                "/home/someone/.paneflow-dev/panes/{SESSION}/host.sock"
            ))
            .is_some()
        );
        for refused in [
            "/run/user/1000/paneflow-host-0123.sock".to_string(),
            format!("/home/someone/.paneflow-dev/panes/{SESSION}/../x/host.sock"),
            format!("/home/someone/.paneflow-dev/panes/{SESSION}/sub/host.sock"),
            "/home/someone/.paneflow-dev/panes/host.sock".to_string(),
            format!("panes/{SESSION}/host.sock"),
            "/home/someone/.paneflow-dev/panes/a b/host.sock".to_string(),
            String::new(),
        ] {
            assert_eq!(honored(&refused), None, "{refused}");
        }
        assert_eq!(pane_endpoint_from(&home(), PaneServer::Host, None), None);
    }

    #[test]
    fn a_session_name_is_one_plain_token() {
        assert!(is_pane_session_name(SESSION));
        for refused in ["", "..", "a/b", r"a\b", "a b", &"x".repeat(65)] {
            assert!(!is_pane_session_name(refused), "{refused}");
        }
    }

    #[test]
    fn each_session_has_its_own_sandbox_sid_under_the_paneflow_authority() {
        let sid = pane_sandbox_sid(SESSION);
        assert!(
            sid.starts_with(&format!("{SANDBOX_SID_PREFIX}-1-")),
            "{sid}"
        );
        assert_eq!(sid, pane_sandbox_sid(SESSION));
        assert_ne!(sid, pane_sandbox_sid("another-session"));
        assert_ne!(sid, SANDBOX_SHARED_SID);
    }
}
