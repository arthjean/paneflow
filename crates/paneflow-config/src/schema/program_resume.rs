use serde_json::Value;

use super::{PaneFlowConfig, TerminalConfig};

pub const PROGRAM_RESUME_KEY: &str = "terminal.program_resume";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProgramResumeMode {
    #[default]
    Ask,
    Auto,
    Off,
}

impl ProgramResumeMode {
    pub const ACCEPTED: &'static str = "ask, auto, off";

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "ask" => Some(Self::Ask),
            "auto" => Some(Self::Auto),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }

    pub fn advertises(self) -> bool {
        self != Self::Off
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramResumeSettings {
    pub mode: ProgramResumeMode,
    pub trusted_programs: Vec<String>,
    errors: Vec<String>,
}

pub fn is_bare_program_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
        && !(name.len() >= 2 && name.as_bytes()[1] == b':')
}

impl ProgramResumeSettings {
    pub fn from_value(value: Option<&Value>) -> Self {
        let mut settings = Self::default();
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return settings;
        };
        let Some(fields) = value.as_object() else {
            settings.errors.push(format!(
                "invalid config key {PROGRAM_RESUME_KEY}: expected an object; programs are offered before they resume"
            ));
            return settings;
        };
        match fields.get("mode") {
            None | Some(Value::Null) => {}
            Some(Value::String(raw)) if ProgramResumeMode::parse(raw).is_some() => {
                settings.mode = ProgramResumeMode::parse(raw).unwrap_or_default();
            }
            Some(other) => settings.errors.push(format!(
                "invalid config key {PROGRAM_RESUME_KEY}.mode: {other} is not one of {}; using ask",
                ProgramResumeMode::ACCEPTED
            )),
        }
        match fields.get("trusted_programs") {
            None | Some(Value::Null) => {}
            Some(Value::Array(entries)) => {
                for entry in entries {
                    match entry.as_str() {
                        Some(name) if is_bare_program_name(name) => {
                            if !settings.trusted_programs.iter().any(|kept| kept == name) {
                                settings.trusted_programs.push(name.to_string());
                            }
                        }
                        Some(name) => settings.errors.push(format!(
                            "invalid config key {PROGRAM_RESUME_KEY}.trusted_programs: {name:?} is not a bare program name; ignoring it"
                        )),
                        None => settings.errors.push(format!(
                            "invalid config key {PROGRAM_RESUME_KEY}.trusted_programs: {entry} is not a program name; ignoring it"
                        )),
                    }
                }
            }
            Some(_) => settings.errors.push(format!(
                "invalid config key {PROGRAM_RESUME_KEY}.trusted_programs: expected a list of program names; trusting none"
            )),
        }
        settings
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    pub fn trusts(&self, argv: &[String]) -> bool {
        argv.first().is_some_and(|program| {
            is_bare_program_name(program)
                && self.trusted_programs.iter().any(|name| name == program)
        })
    }
}

impl TerminalConfig {
    pub fn program_resume_settings(&self) -> ProgramResumeSettings {
        ProgramResumeSettings::from_value(self.program_resume.as_ref())
    }
}

impl PaneFlowConfig {
    pub fn program_resume_settings(&self) -> ProgramResumeSettings {
        self.terminal
            .as_ref()
            .map(TerminalConfig::program_resume_settings)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn load(json: &str) -> ProgramResumeSettings {
        crate::loader::try_parse_and_validate(json)
            .expect("config loads")
            .program_resume_settings()
    }

    #[test]
    fn no_setting_asks_and_trusts_nothing() {
        let settings = load("{}");
        assert_eq!(settings.mode, ProgramResumeMode::Ask);
        assert!(settings.trusted_programs.is_empty());
        assert!(settings.errors().is_empty());
        assert_eq!(
            load(r#"{"terminal": {}}"#),
            ProgramResumeSettings::default()
        );
    }

    #[test]
    fn each_mode_resolves_from_the_terminal_block() {
        for (raw, mode) in [
            ("ask", ProgramResumeMode::Ask),
            ("auto", ProgramResumeMode::Auto),
            ("off", ProgramResumeMode::Off),
        ] {
            let settings = load(&format!(
                r#"{{"terminal": {{"program_resume": {{"mode": "{raw}"}}}}}}"#
            ));
            assert_eq!(settings.mode, mode, "{raw}");
            assert!(settings.errors().is_empty(), "{raw}");
        }
    }

    #[tracing_test::traced_test]
    #[test]
    fn an_unknown_mode_is_reported_naming_the_key_and_falls_back_to_ask() {
        let config = crate::loader::try_parse_and_validate(
            r#"{"theme": "One Dark", "terminal": {"program_resume": {"mode": "sometimes"}}}"#,
        )
        .expect("the rest of the config still loads");
        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        let settings = config.program_resume_settings();
        assert_eq!(settings.mode, ProgramResumeMode::Ask);
        assert_eq!(
            settings.errors(),
            ["invalid config key terminal.program_resume.mode: \"sometimes\" is not one of ask, auto, off; using ask"]
        );
        assert!(logs_contain(
            "invalid config key terminal.program_resume.mode: \"sometimes\""
        ));
    }

    #[tracing_test::traced_test]
    #[test]
    fn an_absolute_trusted_program_is_reported_and_the_bare_names_are_kept() {
        let settings = load(
            r#"{"terminal": {"program_resume": {"trusted_programs": ["nvim", "/usr/bin/htop"]}}}"#,
        );
        assert_eq!(settings.trusted_programs, ["nvim"]);
        assert_eq!(
            settings.errors(),
            ["invalid config key terminal.program_resume.trusted_programs: \"/usr/bin/htop\" is not a bare program name; ignoring it"]
        );
        assert!(logs_contain("\"/usr/bin/htop\" is not a bare program name"));
    }

    #[test]
    fn a_malformed_block_trusts_nothing_and_asks() {
        for value in [
            json!(true),
            json!({"trusted_programs": "nvim"}),
            json!({"mode": 3}),
        ] {
            let settings = ProgramResumeSettings::from_value(Some(&value));
            assert_eq!(settings.mode, ProgramResumeMode::Ask, "{value}");
            assert!(settings.trusted_programs.is_empty(), "{value}");
            assert_eq!(settings.errors().len(), 1, "{value}");
        }
    }

    #[test]
    fn only_a_bare_argv0_matches_a_trusted_name() {
        let settings = ProgramResumeSettings::from_value(Some(&json!({
            "trusted_programs": ["nvim"]
        })));
        let argv = |items: &[&str]| {
            items
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>()
        };
        assert!(settings.trusts(&argv(&["nvim", "-S", "session.vim"])));
        assert!(!settings.trusts(&argv(&["/tmp/x/nvim"])));
        assert!(!settings.trusts(&argv(&["htop"])));
        assert!(!settings.trusts(&[]));
        for name in ["", ".", "..", "a/b", "a\\b", "C:nvim", "nv\u{7}im"] {
            assert!(!is_bare_program_name(name), "{name:?}");
        }
    }
}
