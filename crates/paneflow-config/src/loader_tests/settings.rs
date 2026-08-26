use super::*;
use crate::schema::*;

#[test]
fn test_telemetry_missing_block() {
    // No `telemetry` key at all → outer None (never asked).
    let config = parse_and_validate(r#"{"default_shell": "/bin/sh"}"#);
    assert!(config.telemetry.is_none());
}

#[test]
fn test_telemetry_enabled_null_and_empty() {
    // Both `{"enabled": null}` and `{}` must parse to the same state:
    // block present, enabled unresolved. Both forms are expected in
    // the wild (users editing by hand vs. the modal writing `{}` before
    // the user clicks).
    let via_null = parse_and_validate(r#"{"telemetry": {"enabled": null}}"#);
    let via_empty = parse_and_validate(r#"{"telemetry": {}}"#);

    assert_eq!(via_null.telemetry, Some(TelemetryConfig { enabled: None }));
    assert_eq!(via_empty.telemetry, Some(TelemetryConfig { enabled: None }));
    assert_eq!(via_null.telemetry, via_empty.telemetry);
}

#[test]
fn test_telemetry_enabled_true() {
    let config = parse_and_validate(r#"{"telemetry": {"enabled": true}}"#);
    assert_eq!(
        config.telemetry,
        Some(TelemetryConfig {
            enabled: Some(true)
        })
    );

    // Round-trip: re-serialize then re-parse - the consent answer
    // must survive without loss so the modal never re-prompts.
    let json = serde_json::to_string(&config).unwrap();
    let reparsed = parse_and_validate(&json);
    assert_eq!(reparsed.telemetry, config.telemetry);
}

#[test]
fn test_telemetry_enabled_false() {
    let config = parse_and_validate(r#"{"telemetry": {"enabled": false}}"#);
    assert_eq!(
        config.telemetry,
        Some(TelemetryConfig {
            enabled: Some(false)
        })
    );

    let json = serde_json::to_string(&config).unwrap();
    let reparsed = parse_and_validate(&json);
    assert_eq!(reparsed.telemetry, config.telemetry);
}

// ─── Terminal config - ligatures (US-008) ─────────────────────────────
//
// Behavior contract:
//   - block missing                   → terminal = None    (default off)
//   - {"terminal": {}}                → terminal = Some(TerminalConfig { ligatures: None })
//   - {"terminal": {"ligatures": null}} → terminal = Some(TerminalConfig { ligatures: None })
//   - {"terminal": {"ligatures": true}}  → ligatures opt-in
//   - {"terminal": {"ligatures": false}} → explicit opt-out (same as default)

#[test]
fn test_terminal_block_missing_defaults_off() {
    let config = parse_and_validate(r#"{"default_shell": "/bin/sh"}"#);
    assert!(config.terminal.is_none());
}

#[test]
fn test_terminal_ligatures_default_when_block_empty() {
    let from_empty = parse_and_validate(r#"{"terminal": {}}"#);
    let from_null = parse_and_validate(r#"{"terminal": {"ligatures": null}}"#);
    assert_eq!(
        from_empty.terminal,
        Some(TerminalConfig {
            backend: Default::default(),
            ligatures: None,
            integrated_glyphs: None,
            color_emoji: None,
            cursor_color: None,
            scrollback_lines: None,
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        })
    );
    assert_eq!(
        from_null.terminal,
        Some(TerminalConfig {
            backend: Default::default(),
            ligatures: None,
            integrated_glyphs: None,
            color_emoji: None,
            cursor_color: None,
            scrollback_lines: None,
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        })
    );
}

#[test]
fn test_terminal_ligatures_true() {
    let config = parse_and_validate(r#"{"terminal": {"ligatures": true}}"#);
    assert_eq!(
        config.terminal,
        Some(TerminalConfig {
            backend: Default::default(),
            ligatures: Some(true),
            integrated_glyphs: None,
            color_emoji: None,
            cursor_color: None,
            scrollback_lines: None,
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        })
    );

    // Survive a serialize → parse round-trip so the user's opt-in
    // isn't dropped if Paneflow rewrites the config file.
    let json = serde_json::to_string(&config).unwrap();
    let reparsed = parse_and_validate(&json);
    assert_eq!(reparsed.terminal, config.terminal);
}

#[test]
fn test_terminal_ligatures_false() {
    let config = parse_and_validate(r#"{"terminal": {"ligatures": false}}"#);
    assert_eq!(
        config.terminal,
        Some(TerminalConfig {
            backend: Default::default(),
            ligatures: Some(false),
            integrated_glyphs: None,
            color_emoji: None,
            cursor_color: None,
            scrollback_lines: None,
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        })
    );
}

#[test]
fn test_terminal_integrated_glyphs_default_on_and_false_opt_out() {
    let absent = parse_and_validate(r#"{"terminal": {}}"#);
    assert!(
        absent
            .terminal
            .as_ref()
            .expect("terminal block present")
            .resolved_integrated_glyphs(),
        "absent terminal.integrated_glyphs resolves to enabled"
    );

    let disabled = parse_and_validate(r#"{"terminal": {"integrated_glyphs": false}}"#);
    assert_eq!(
        disabled.terminal,
        Some(TerminalConfig {
            backend: Default::default(),
            ligatures: None,
            integrated_glyphs: Some(false),
            color_emoji: None,
            cursor_color: None,
            scrollback_lines: None,
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        })
    );
    assert!(
        !disabled
            .terminal
            .as_ref()
            .expect("terminal block present")
            .resolved_integrated_glyphs(),
        "explicit false disables integrated glyphs"
    );
}

#[test]
fn test_terminal_color_emoji_default_on_and_false_opt_out() {
    let absent = parse_and_validate(r#"{"terminal": {}}"#);
    assert!(
        absent
            .terminal
            .as_ref()
            .expect("terminal block present")
            .resolved_color_emoji(),
        "absent terminal.color_emoji resolves to enabled"
    );

    let disabled = parse_and_validate(r#"{"terminal": {"color_emoji": false}}"#);
    assert_eq!(
        disabled.terminal,
        Some(TerminalConfig {
            backend: Default::default(),
            ligatures: None,
            integrated_glyphs: None,
            color_emoji: Some(false),
            cursor_color: None,
            scrollback_lines: None,
            cursor_shape: None,
            cursor_blink: None,
            env: None,
            scroll_multiplier: None,
        })
    );
    assert!(
        !disabled
            .terminal
            .as_ref()
            .expect("terminal block present")
            .resolved_color_emoji(),
        "explicit false disables color emoji"
    );
}

#[test]
fn test_terminal_scrollback_lines_resolves_to_default_when_absent() {
    let config = parse_and_validate(r#"{"terminal": {}}"#);
    let tc = config.terminal.expect("terminal block present");
    assert_eq!(
        tc.resolved_scrollback_lines(),
        TerminalConfig::DEFAULT_SCROLLBACK_LINES
    );
}

#[test]
fn test_terminal_scrollback_lines_clamps_out_of_range() {
    let tc = TerminalConfig {
        backend: Default::default(),
        ligatures: None,
        integrated_glyphs: None,
        color_emoji: None,
        cursor_color: None,
        scrollback_lines: Some(50), // below MIN_SCROLLBACK_LINES
        cursor_shape: None,
        cursor_blink: None,
        env: None,
        scroll_multiplier: None,
    };
    assert_eq!(
        tc.resolved_scrollback_lines(),
        TerminalConfig::MIN_SCROLLBACK_LINES
    );
    let tc = TerminalConfig {
        backend: Default::default(),
        ligatures: None,
        integrated_glyphs: None,
        color_emoji: None,
        cursor_color: None,
        scrollback_lines: Some(20_000_000), // way above MAX
        cursor_shape: None,
        cursor_blink: None,
        env: None,
        scroll_multiplier: None,
    };
    assert_eq!(
        tc.resolved_scrollback_lines(),
        TerminalConfig::MAX_SCROLLBACK_LINES
    );
}

// US-014: global terminal.env round-trips through parse + serialize.
#[test]
fn test_terminal_env_round_trip() {
    let config = parse_and_validate(
        r#"{"terminal": {"env": {"RUST_LOG": "debug", "ANTHROPIC_API_KEY": "sk-x"}}}"#,
    );
    let env = config
        .terminal
        .as_ref()
        .and_then(|t| t.env.as_ref())
        .expect("terminal.env must parse");
    assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("debug"));
    assert_eq!(
        env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-x")
    );

    // Survive a serialize → parse round-trip.
    let json = serde_json::to_string(&config).unwrap();
    let reparsed = parse_and_validate(&json);
    assert_eq!(reparsed.terminal, config.terminal);
}

// US-014: an absent env block resolves to None (no injection).
#[test]
fn test_terminal_env_absent_is_none() {
    let config = parse_and_validate(r#"{"terminal": {}}"#);
    assert!(
        config
            .terminal
            .expect("terminal block present")
            .env
            .is_none(),
        "US-014: absent terminal.env must be None"
    );
}

// US-022: scroll_multiplier resolver - default, clamp, in-range, round-trip.
#[test]
fn test_scroll_multiplier_resolver_default_and_clamp() {
    assert_eq!(
        TerminalConfig::default().resolved_scroll_multiplier(),
        1.0,
        "absent → default 1.0"
    );
    assert_eq!(
        TerminalConfig {
            scroll_multiplier: Some(0.01),
            ..Default::default()
        }
        .resolved_scroll_multiplier(),
        TerminalConfig::MIN_SCROLL_MULTIPLIER,
        "below min → clamped"
    );
    assert_eq!(
        TerminalConfig {
            scroll_multiplier: Some(99.0),
            ..Default::default()
        }
        .resolved_scroll_multiplier(),
        TerminalConfig::MAX_SCROLL_MULTIPLIER,
        "above max → clamped"
    );
    assert_eq!(
        TerminalConfig {
            scroll_multiplier: Some(2.5),
            ..Default::default()
        }
        .resolved_scroll_multiplier(),
        2.5,
        "in range → unchanged"
    );
}

#[test]
fn test_scroll_multiplier_serde_roundtrip() {
    let config = parse_and_validate(r#"{"terminal": {"scroll_multiplier": 3.0}}"#);
    let tc = config.terminal.expect("terminal block present");
    assert_eq!(tc.scroll_multiplier, Some(3.0));
    assert_eq!(tc.resolved_scroll_multiplier(), 3.0);

    let absent = parse_and_validate(r#"{"terminal": {}}"#);
    let tc = absent.terminal.expect("terminal block present");
    assert!(tc.scroll_multiplier.is_none());
    assert_eq!(tc.resolved_scroll_multiplier(), 1.0);
}

#[test]
fn test_terminal_ligatures_wrong_type_falls_back_to_defaults() {
    // A typo in one terminal field must not discard siblings or the whole
    // config. The bad bool resolves as absent, while valid neighbours stay.
    let config = parse_and_validate(
        r#"{"theme": "One Dark", "terminal": {"ligatures": "yes", "color_emoji": false}}"#,
    );
    assert_eq!(config.theme.as_deref(), Some("One Dark"));
    let terminal = config.terminal.expect("terminal block survives");
    assert_eq!(terminal.ligatures, None);
    assert_eq!(terminal.color_emoji, Some(false));
}

// SessionState carries the top-level UI `mode`. The tests below cover
// round-trip with mixed state, backward compatibility with a session.json
// that predates the field, and AppMode serialisation.
