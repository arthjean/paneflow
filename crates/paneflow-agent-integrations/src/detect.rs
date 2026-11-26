use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Present,
    Absent,
}

impl Presence {
    #[must_use]
    pub fn from_signals(cli_on_path: bool, config_exists: bool) -> Self {
        if cli_on_path || config_exists {
            Self::Present
        } else {
            Self::Absent
        }
    }

    #[must_use]
    pub fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

#[must_use]
pub fn detect(cli: Option<&str>, config_paths: &[PathBuf]) -> Presence {
    let cli_on_path = cli.is_some_and(|c| which::which(c).is_ok());
    let config_exists = config_paths.iter().any(|p| p.exists());
    Presence::from_signals(cli_on_path, config_exists)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_when_cli_on_path() {
        assert_eq!(Presence::from_signals(true, false), Presence::Present);
    }

    #[test]
    fn present_when_config_exists() {
        assert_eq!(Presence::from_signals(false, true), Presence::Present);
    }

    #[test]
    fn present_when_both() {
        assert_eq!(Presence::from_signals(true, true), Presence::Present);
    }

    #[test]
    fn absent_when_neither() {
        assert_eq!(Presence::from_signals(false, false), Presence::Absent);
        assert!(!Presence::from_signals(false, false).is_present());
    }

    #[test]
    fn detect_uses_config_path_existence() {
        let dir = tempfile::TempDir::new().unwrap();
        let existing = dir.path().join("config.json");
        std::fs::write(&existing, b"{}").unwrap();
        let missing = dir.path().join("nope.json");

        assert_eq!(
            detect(None, std::slice::from_ref(&existing)),
            Presence::Present
        );
        assert_eq!(detect(None, &[missing]), Presence::Absent);
    }

    #[test]
    fn detect_absent_for_unresolvable_cli_and_no_config() {
        let bogus = "paneflow-nonexistent-agent-cli-xyz";
        assert_eq!(detect(Some(bogus), &[]), Presence::Absent);
    }
}
