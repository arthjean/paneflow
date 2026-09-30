use std::ffi::OsString;
use std::path::PathBuf;

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

pub fn absolute_env_dir(name: &str, value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value.filter(|value| !value.is_empty())?);
    if path.is_absolute() {
        return Some(path);
    }
    log::warn!(
        "paneflow: ignoring {name}={}: it must be an absolute path",
        path.display()
    );
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePaths {
    pub config_dir: PathBuf,
    pub global_config: PathBuf,
}

impl ClaudePaths {
    pub fn from_env(
        lookup: impl Fn(&str) -> Option<OsString>,
        home: Option<PathBuf>,
    ) -> Option<Self> {
        match absolute_env_dir("CLAUDE_CONFIG_DIR", lookup("CLAUDE_CONFIG_DIR")) {
            Some(config_dir) => Some(Self {
                global_config: config_dir.join(".claude.json"),
                config_dir,
            }),
            None => home.map(|home| Self {
                config_dir: home.join(".claude"),
                global_config: home.join(".claude.json"),
            }),
        }
    }

    pub fn current() -> Option<Self> {
        Self::from_env(|name| std::env::var_os(name), home_dir())
    }

    pub fn settings(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn projects(&self) -> PathBuf {
        self.config_dir.join("projects")
    }
}

pub fn claude_config_dir() -> Option<PathBuf> {
    ClaudePaths::current().map(|paths| paths.config_dir)
}

pub fn codex_home_from(value: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    absolute_env_dir("CODEX_HOME", value).or_else(|| home.map(|home| home.join(".codex")))
}

pub fn codex_home() -> Option<PathBuf> {
    codex_home_from(std::env::var_os("CODEX_HOME"), home_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn claude_paths_follow_claude_config_dir_for_every_file() {
        let root = std::env::temp_dir().join("claude-profile");
        let raw = root.to_string_lossy().into_owned();
        let paths = ClaudePaths::from_env(
            env(&[("CLAUDE_CONFIG_DIR", raw.as_str())]),
            Some(PathBuf::from("/home/u")),
        )
        .unwrap();
        assert_eq!(paths.config_dir, root);
        assert_eq!(paths.global_config, root.join(".claude.json"));
        assert_eq!(paths.settings(), root.join("settings.json"));
        assert_eq!(paths.projects(), root.join("projects"));
    }

    #[test]
    fn claude_paths_default_to_the_home_layout() {
        let home = PathBuf::from("/home/u");
        for value in [
            &[][..],
            &[("CLAUDE_CONFIG_DIR", "")],
            &[("CLAUDE_CONFIG_DIR", "rel/dir")],
        ] {
            let paths = ClaudePaths::from_env(env(value), Some(home.clone())).unwrap();
            assert_eq!(paths.config_dir, home.join(".claude"), "{value:?}");
            assert_eq!(paths.global_config, home.join(".claude.json"), "{value:?}");
        }
    }

    #[test]
    fn relative_agent_dirs_are_rejected() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            codex_home_from(Some(OsString::from("codex")), home.clone()),
            Some(PathBuf::from("/home/u/.codex"))
        );
        assert_eq!(
            absolute_env_dir("OPENCODE_CONFIG_DIR", Some(OsString::from("./oc"))),
            None
        );
        let absolute = std::env::temp_dir().join("codex-home");
        assert_eq!(
            codex_home_from(Some(absolute.clone().into_os_string()), home),
            Some(absolute)
        );
    }
}
