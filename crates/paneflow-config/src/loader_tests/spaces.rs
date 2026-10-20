use super::*;
use crate::schema::*;

const WORK: &str = "550e8400-e29b-41d4-a716-446655440001";
const HOME: &str = "550e8400-e29b-41d4-a716-446655440002";

fn space_entry(index: usize) -> serde_json::Value {
    serde_json::json!({
        "id": format!("550e8400-e29b-41d4-a716-{index:012}"),
        "name": format!("Space {index}"),
    })
}

#[test]
fn spaces_load_with_trimmed_names_and_known_colors() {
    let config = parse_and_validate(&format!(
        r#"{{"spaces": [
            {{"id": "{WORK}", "name": "  Work ", "color": "blue"}},
            {{"id": "{HOME}", "name": "Home", "color": "purple"}}
        ]}}"#
    ));
    assert_eq!(
        config.spaces,
        vec![
            SpaceConfig {
                id: SpaceId::parse(WORK).unwrap(),
                name: "Work".to_string(),
                color: Some(SpaceColor::Blue),
            },
            SpaceConfig {
                id: SpaceId::parse(HOME).unwrap(),
                name: "Home".to_string(),
                color: None,
            },
        ],
        "an unknown color clears the color and keeps the space"
    );
    assert_eq!(
        config
            .space(&SpaceId::parse(HOME).unwrap())
            .map(|space| space.name.as_str()),
        Some("Home")
    );
}

#[test]
#[tracing_test::traced_test]
fn invalid_space_entries_are_skipped_one_warning_each() {
    let long = "x".repeat(MAX_SPACE_NAME_CHARS + 1);
    let mut entries = vec![
        serde_json::json!({"name": "No id"}),
        serde_json::json!({"id": "not-a-uuid", "name": "Bad id"}),
        serde_json::json!({"id": WORK, "name": "Work"}),
        serde_json::json!({"id": HOME, "name": "   "}),
        serde_json::json!({"id": HOME, "name": long}),
        serde_json::json!({"id": HOME, "name": "WORK"}),
        serde_json::json!({"id": HOME, "name": "default"}),
        serde_json::json!({"id": WORK, "name": "Same id"}),
    ];
    entries.extend((1..=MAX_SPACES).map(space_entry));
    let raw = serde_json::json!({"theme": "One Dark", "spaces": entries}).to_string();

    let config = parse_and_validate(&raw);

    assert_eq!(config.theme.as_deref(), Some("One Dark"));
    let names: Vec<_> = config.spaces.iter().map(|s| s.name.as_str()).collect();
    let mut expected = vec!["Work".to_string()];
    expected.extend((1..MAX_SPACES).map(|index| format!("Space {index}")));
    assert_eq!(names, expected, "the first nine valid entries win");
    logs_assert(|lines: &[&str]| {
        let skipped = lines
            .iter()
            .filter(|line| line.contains("skipping space entry"))
            .count();
        if skipped == 8 {
            Ok(())
        } else {
            Err(format!("expected 8 skipped-space warnings, got {skipped}"))
        }
    });
    for index in [0, 1, 3, 4, 5, 6, 7, 16] {
        assert!(
            logs_contain(&format!("skipping space entry at index {index}:")),
            "entry {index} must be reported"
        );
    }
}

#[test]
#[tracing_test::traced_test]
fn a_spaces_key_that_is_not_an_array_loads_the_rest_with_one_warning() {
    let config = parse_and_validate(r#"{"theme": "One Dark", "spaces": {"id": "x"}}"#);
    assert!(config.spaces.is_empty());
    assert_eq!(config.theme.as_deref(), Some("One Dark"));
    logs_assert(|lines: &[&str]| {
        match lines
            .iter()
            .filter(|line| line.contains("`spaces`"))
            .count()
        {
            1 => Ok(()),
            n => Err(format!("expected one warning about spaces, got {n}")),
        }
    });
}

#[test]
fn spaces_round_trip_and_are_omitted_when_empty() {
    let config = parse_and_validate(&format!(
        r#"{{"spaces": [{{"id": "{WORK}", "name": "Work", "color": "cyan"}}]}}"#
    ));
    let written = serde_json::to_value(&config).unwrap();
    assert_eq!(
        written["spaces"],
        serde_json::json!([{"id": WORK, "name": "Work", "color": "cyan"}])
    );
    assert_eq!(
        try_parse_and_validate(&written.to_string()).unwrap(),
        config
    );
    let empty = serde_json::to_value(PaneFlowConfig::default()).unwrap();
    assert!(empty.get("spaces").is_none());
}

#[test]
fn space_names_are_validated_like_the_loader() {
    assert_eq!(
        validate_space_name(" Work ", ["Home"]),
        Ok("Work".to_string())
    );
    assert_eq!(
        validate_space_name("", ["Home"]),
        Err(SpaceNameError::Required)
    );
    assert_eq!(
        validate_space_name("home", ["Home"]),
        Err(SpaceNameError::AlreadyUsed)
    );
    assert_eq!(
        validate_space_name("DEFAULT", []),
        Err(SpaceNameError::Reserved)
    );
    assert_eq!(
        validate_space_name(&"é".repeat(MAX_SPACE_NAME_CHARS), []),
        Ok("é".repeat(MAX_SPACE_NAME_CHARS))
    );
    assert_eq!(
        validate_space_name(&"é".repeat(MAX_SPACE_NAME_CHARS + 1), []),
        Err(SpaceNameError::TooLong)
    );
}
