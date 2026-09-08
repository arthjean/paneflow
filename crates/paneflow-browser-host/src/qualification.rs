use serde::{Deserialize, Serialize};

pub fn sandbox_disabled(argument: &str) -> bool {
    let Some(switch) = argument
        .strip_prefix("--")
        .or_else(|| argument.strip_prefix('-'))
    else {
        return false;
    };
    let name = switch.split('=').next().unwrap_or_default();
    [
        "no-sandbox",
        "disable-gpu-sandbox",
        "disable-setuid-sandbox",
    ]
    .iter()
    .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureState {
    state: FixtureReadiness,
    width: u32,
    height: u32,
    scale: f64,
    visibility: FixtureVisibility,
    #[serde(default)]
    clicks: u64,
    #[serde(default)]
    keys: u64,
    #[serde(default)]
    wheel: u64,
    #[serde(default)]
    pointerdowns: u64,
    #[serde(default)]
    pointermoves: u64,
    #[serde(default)]
    pointerups: u64,
    #[serde(default)]
    last_pointer: Option<[i32; 2]>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FixtureReadiness {
    Ready,
    Failed,
    Missing,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FixtureVisibility {
    Visible,
    Hidden,
}

pub fn fixture_report(message: &[u16]) -> Option<Result<FixtureState, ()>> {
    if message.len() > 1024 {
        return Some(Err(()));
    }
    let Ok(text) = String::from_utf16(message) else {
        return Some(Err(()));
    };
    text.strip_prefix("PANEFLOW_FIXTURE:")
        .map(|body| serde_json::from_str(body).map_err(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_switch_names_include_cef_prefix_value_and_case_variants() {
        for argument in [
            "--no-sandbox",
            "--no-sandbox=1",
            "-no-sandbox",
            "--NO-SANDBOX",
            "--disable-gpu-sandbox=true",
            "-disable-setuid-sandbox=0",
        ] {
            assert!(sandbox_disabled(argument));
        }
        for argument in ["no-sandbox", "--enable-sandbox", "--title=no-sandbox"] {
            assert!(!sandbox_disabled(argument));
        }
    }

    #[test]
    fn page_reports_are_bounded_before_conversion_and_have_a_fixed_schema() {
        let valid = r#"PANEFLOW_FIXTURE:{"state":"ready","width":1920,"height":1080,"scale":1,"visibility":"visible"}"#;
        assert!(matches!(
            fixture_report(&valid.encode_utf16().collect::<Vec<_>>()),
            Some(Ok(_))
        ));
        for text in [
            "PANEFLOW_FIXTURE:[]",
            "PANEFLOW_FIXTURE:{\"state\":\"unbounded\"}",
            &"x".repeat(1025),
        ] {
            assert!(matches!(
                fixture_report(&text.encode_utf16().collect::<Vec<_>>()),
                Some(Err(()))
            ));
        }
        assert!(
            fixture_report(&"ordinary console text".encode_utf16().collect::<Vec<_>>()).is_none()
        );
    }
}
