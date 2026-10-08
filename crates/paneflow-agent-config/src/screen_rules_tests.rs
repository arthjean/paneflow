#![allow(clippy::panic, reason = "a corpus failure names the offending fixture")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::runtime_catalog::SCREEN_RULE_SOURCES;
use crate::screen_rules::{
    apply_overrides, evaluate, parse_base_rules, parse_rule_file, Region, RuleEntry, RuleOrigin,
    ScreenInput, ScreenRule, ScreenState, MAX_PATTERNS_PER_RULE, MAX_REGEX_BYTES,
    MAX_RULES_PER_SOURCE,
};

const CORPUS_RUNTIMES: &[&str] = &[
    "claude-code",
    "codex",
    "opencode",
    "gemini",
    "pi",
    "hermes",
    "fx",
];

const BLESS_ENV: &str = "PANEFLOW_SCREEN_CORPUS_BLESS";

fn builtin(slug: &str) -> Vec<ScreenRule> {
    let (_, text) = SCREEN_RULE_SOURCES
        .iter()
        .find(|(held, _)| *held == slug)
        .unwrap_or_else(|| panic!("no builtin screen rules for {slug}"));
    parse_base_rules(text, RuleOrigin::Builtin).expect("builtin rules parse")
}

fn verdict(rules: &[ScreenRule], screen: &str) -> Option<ScreenState> {
    evaluate(
        rules,
        &ScreenInput {
            screen,
            ..ScreenInput::default()
        },
    )
    .state(rules)
}

fn rules(text: &str) -> Vec<ScreenRule> {
    parse_base_rules(text, RuleOrigin::Builtin).expect("rules parse")
}

#[test]
fn the_highest_priority_match_wins_and_ties_keep_the_file_order() {
    let rules = rules(
        r#"
engine = 2

[[rules]]
id = "idle-prompt"
state = "idle"
priority = 10
any = ['❯']

[[rules]]
id = "working-a"
state = "working"
priority = 20
any = ['busy']

[[rules]]
id = "blocked-b"
state = "blocked"
priority = 20
any = ['busy']
"#,
    );
    let evaluation = evaluate(
        &rules,
        &ScreenInput {
            screen: "busy\n❯",
            ..ScreenInput::default()
        },
    );
    assert_eq!(evaluation.matched, vec![true, true, true]);
    assert_eq!(evaluation.winner, Some(1));
    assert_eq!(evaluation.state(&rules), Some(ScreenState::Working));
    assert_eq!(verdict(&rules, "❯"), Some(ScreenState::Idle));
    assert_eq!(verdict(&rules, "nothing"), None);
}

#[test]
fn regions_select_the_last_and_first_non_blank_lines_or_the_title() {
    let rules = rules(
        r#"
engine = 2

[[rules]]
id = "tail"
state = "working"
region = "last:2"
any = ['(?m)^marker$']

[[rules]]
id = "head"
state = "idle"
region = "first:1"
all = ['^top']

[[rules]]
id = "titled"
state = "blocked"
region = "title"
any = ['^fx · ']
not = ['json']
"#,
    );
    let screen = "top line\n\n\nmarker\n   \nbottom\n\n";
    let evaluation = evaluate(
        &rules,
        &ScreenInput {
            screen,
            title: Some("fx · fix the build · kimi"),
            progress: None,
            program_status: None,
        },
    );
    assert_eq!(evaluation.matched, vec![true, true, true]);
    let deep = "marker\nmiddle\nbottom";
    let evaluation = evaluate(
        &rules,
        &ScreenInput {
            screen: deep,
            title: Some("fx data.json"),
            progress: None,
            program_status: None,
        },
    );
    assert_eq!(evaluation.matched, vec![false, false, false]);
    assert_eq!(rules[0].region, Region::Last(2));
}

#[test]
fn a_progress_condition_reads_the_osc_progress_state() {
    let rules = rules(
        r#"
engine = 2

[[rules]]
id = "spinning"
state = "working"
progress = ["indeterminate", "set"]
"#,
    );
    let at = |progress| {
        evaluate(
            &rules,
            &ScreenInput {
                screen: "",
                title: None,
                progress,
                program_status: None,
            },
        )
        .state(&rules)
    };
    assert_eq!(at(Some("indeterminate")), Some(ScreenState::Working));
    assert_eq!(at(Some("pause")), None);
    assert_eq!(at(None), None);
}

#[test]
fn a_broken_source_is_rejected_whole_with_the_faulty_line() {
    let invalid_regex =
        "engine = 2\n\n[[rules]]\nid = \"broken\"\nstate = \"idle\"\nany = ['(unclosed']\n";
    let error = parse_rule_file(invalid_regex, RuleOrigin::Local).unwrap_err();
    assert_eq!(error.line, Some(6), "{error}");
    assert!(error.to_string().contains("invalid regex"), "{error}");

    let unknown_state =
        "engine = 2\n\n[[rules]]\nid = \"odd\"\nstate = \"sleeping\"\nany = ['x']\n";
    let error = parse_rule_file(unknown_state, RuleOrigin::Local).unwrap_err();
    assert_eq!(error.line, Some(5), "{error}");
    assert!(error.to_string().contains("unknown state"), "{error}");

    let wrong_engine = "engine = 3\n";
    let error = parse_rule_file(wrong_engine, RuleOrigin::Remote(4)).unwrap_err();
    assert_eq!(error.line, Some(1), "{error}");
    assert!(error.to_string().contains("engine 3"), "{error}");

    let unknown_field =
        "engine = 2\n\n[[rules]]\nid = \"x\"\nstate = \"idle\"\nany = ['x']\ncolour = 1\n";
    let error = parse_rule_file(unknown_field, RuleOrigin::Local).unwrap_err();
    assert_eq!(error.line, Some(7), "{error}");

    let duplicate = "engine = 2\n[[rules]]\nid = \"x\"\nstate = \"idle\"\nany = ['x']\n[[rules]]\nid = \"x\"\nstate = \"idle\"\nany = ['y']\n";
    let error = parse_rule_file(duplicate, RuleOrigin::Local).unwrap_err();
    assert_eq!(error.line, Some(7), "{error}");
}

#[test]
fn a_regex_beyond_one_mebibyte_of_program_is_refused() {
    let oversized = format!(
        "engine = 2\n[[rules]]\nid = \"huge\"\nstate = \"idle\"\nany = ['\\w{{{}}}']\n",
        MAX_REGEX_BYTES / 64
    );
    let error = parse_rule_file(&oversized, RuleOrigin::Local).unwrap_err();
    assert!(error.to_string().contains("invalid regex"), "{error}");
    assert_eq!(error.line, Some(5));
}

#[test]
fn a_source_with_too_many_rules_or_patterns_is_refused() {
    let mut many = String::from("engine = 2\n");
    for index in 0..=MAX_RULES_PER_SOURCE {
        many.push_str(&format!(
            "[[rules]]\nid = \"rule-{index}\"\nstate = \"idle\"\nany = ['x']\n"
        ));
    }
    let error = parse_rule_file(&many, RuleOrigin::Remote(1)).unwrap_err();
    assert!(error.to_string().contains("at most 256 rules"), "{error}");
    assert_eq!(error.line, Some(2 + 4 * MAX_RULES_PER_SOURCE + 1));

    let patterns = vec!["'x'"; MAX_PATTERNS_PER_RULE + 1].join(", ");
    let wide =
        format!("engine = 2\n[[rules]]\nid = \"wide\"\nstate = \"idle\"\nany = [{patterns}]\n");
    let error = parse_rule_file(&wide, RuleOrigin::Local).unwrap_err();
    assert!(
        error.to_string().contains("more than 64 patterns"),
        "{error}"
    );
    let fits = vec!["'x'"; MAX_PATTERNS_PER_RULE].join(", ");
    assert!(parse_rule_file(
        &format!("engine = 2\n[[rules]]\nid = \"wide\"\nstate = \"idle\"\nany = [{fits}]\n"),
        RuleOrigin::Local
    )
    .is_ok());
}

#[test]
fn a_rule_needs_a_positive_condition_and_a_blocker_must_block() {
    let only_not = "engine = 2\n[[rules]]\nid = \"x\"\nstate = \"idle\"\nnot = ['x']\n";
    assert!(parse_rule_file(only_not, RuleOrigin::Local).is_err());
    let working_blocker =
        "engine = 2\n[[rules]]\nid = \"x\"\nstate = \"working\"\nany = ['x']\nvisible_blocker = true\n";
    let error = parse_rule_file(working_blocker, RuleOrigin::Local).unwrap_err();
    assert!(error.to_string().contains("must be blocked"), "{error}");
}

#[test]
fn local_overrides_replace_disable_and_add_rules_by_id() {
    let base = builtin("claude-code");
    let local = parse_rule_file(
        r#"
engine = 2

[[rules]]
id = "idle-prompt"
state = "idle"
priority = 10
region = "last:15"
any = ['(?m)^\s*>']

[[rules]]
id = "working-spinner"
disabled = true

[[rules]]
id = "thinking-word"
state = "working"
priority = 25
any = ['(?i)thinking']
"#,
        RuleOrigin::Local,
    )
    .expect("local rules");
    assert!(matches!(local[1], RuleEntry::Disabled(_)));
    let merged = apply_overrides(&base, local);
    let ids: Vec<&str> = merged.iter().map(|rule| rule.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "approval-menu",
            "menu-navigate-select",
            "menu-qualified-selector",
            "menu-confirm-cancel",
            "idle-prompt",
            "thinking-word"
        ]
    );
    let idle = merged
        .iter()
        .find(|rule| rule.id == "idle-prompt")
        .expect("idle");
    assert_eq!(idle.origin, RuleOrigin::Local);
    assert_eq!(verdict(&merged, "> "), Some(ScreenState::Idle));
    assert_eq!(verdict(&merged, "esc to interrupt"), None);
    assert_eq!(
        verdict(&merged, "Thinking hard"),
        Some(ScreenState::Working)
    );
}

#[test]
fn every_builtin_source_parses_with_the_runtime_parser() {
    assert!(!SCREEN_RULE_SOURCES.is_empty());
    for (slug, text) in SCREEN_RULE_SOURCES {
        let rules = parse_base_rules(text, RuleOrigin::Builtin)
            .unwrap_or_else(|error| panic!("{slug}: {error}"));
        assert!(!rules.is_empty(), "{slug}");
    }
}

fn claude_menu(screen: &str) -> bool {
    let rules = builtin("claude-code");
    evaluate(
        &rules,
        &ScreenInput {
            screen,
            ..ScreenInput::default()
        },
    )
    .visible_blocker
    .is_some()
}

fn corpus_screen(slug: &str, name: &str) -> String {
    let path = runtimes_root()
        .join(slug)
        .join("fixtures")
        .join("screens")
        .join(name);
    let capture = Capture::read(&path);
    capture.screen
}

#[test]
fn the_approval_menu_blocker_needs_nearby_selected_choices() {
    let claude = corpus_screen("claude-code", "blocked-approval-menu.txt");
    assert!(claude_menu(&claude));
    assert!(claude_menu(
        &claude.replace("Tab to amend", "Tab to\n amend")
    ));
    assert!(claude_menu(
        &claude.replace("❯ 1.", "  1.").replace("  3.", "❯ 3.")
    ));
    assert!(claude_menu(&corpus_screen(
        "codex",
        "blocked-approval-menu.txt"
    )));
    assert!(!claude_menu("Working… Esc to cancel · Tab to amend"));
    assert!(!claude_menu(&claude.replace('❯', " ")));
    assert!(!claude_menu(
        &claude.replace("  2.", "  x.").replace("  3.", "  x.")
    ));
    assert!(!claude_menu(&claude.replace(
        "Esc to cancel",
        &format!("{}Esc to cancel", "output\n".repeat(13))
    )));
}

#[test]
fn navigation_select_and_confirm_cancel_footers_are_blockers() {
    assert!(claude_menu(
        "❯ 1. Switch to the yearly plan\n  2. Keep the one-time license\n\n\
         Enter to select · ↑/↓ to navigate · Esc to cancel"
    ));
    assert!(claude_menu(
        "Use arrow keys to move.\nPress return to confirm your choice."
    ));
    assert!(claude_menu(
        "  1. Yes, proceed\n2. No, and tell Codex what to do differently (esc)\n\n\
         Press enter to confirm or esc to cancel"
    ));
    assert!(claude_menu(
        "  1. Keep working\n  2. Stop\n↑/↓ to select · Enter to confirm · Esc to cancel"
    ));
}

#[test]
fn prose_hints_and_passive_footers_are_not_blockers() {
    assert!(!claude_menu("Working… press esc to cancel"));
    assert!(!claude_menu(
        "Please select the files you want to keep and let me know."
    ));
    assert!(!claude_menu("Use ↑/↓ to navigate the log output."));
    assert!(!claude_menu(""));
    let mut far = String::from("the footer said \"↑/↓ to navigate\" in the transcript\n");
    for index in 0..30 {
        far.push_str(&format!("transcript line {index}\n"));
    }
    far.push_str("$ ");
    assert!(!claude_menu(&far));
    assert!(!claude_menu(
        "⏺ Working…\n\n⏺ main   ↑/↓ to select · Enter to view  ◯ Explore  Audit resume pipeline"
    ));
    assert!(!claude_menu(
        "  ◯ main           ↑/↓ to select · Enter to\n                   view\n\
         \u{23fa} general-purpose 55m 46s · ↓ 348.3k"
    ));
    assert!(!claude_menu("  ⏺ main           ↑/↓ to select · Enter to"));
    assert!(!claude_menu(
        "The arrows are now first: ← ↑ ↓ →\n(compiling)\n(compiling)\n(compiling)\n\
         Press Enter to submit your prompt."
    ));
}

struct Capture {
    state: String,
    title: Option<String>,
    progress: Option<String>,
    screen: String,
}

impl Capture {
    fn read(path: &Path) -> Self {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
            .replace("\r\n", "\n");
        let (header, screen) = text
            .split_once("\n---\n")
            .unwrap_or_else(|| panic!("{}: no header separator", path.display()));
        let mut lines = header.lines();
        assert_eq!(
            lines.next(),
            Some("paneflow-screen-capture: 1"),
            "{}",
            path.display()
        );
        let fields: BTreeMap<&str, &str> = lines.filter_map(|line| line.split_once(": ")).collect();
        for required in [
            "state", "runtime", "cols", "rows", "cli", "paneflow", "captured",
        ] {
            assert!(
                fields.contains_key(required),
                "{}: missing {required}",
                path.display()
            );
        }
        Self {
            state: fields["state"].to_string(),
            title: fields.get("title").map(|title| (*title).to_string()),
            progress: fields
                .get("progress")
                .map(|progress| (*progress).to_string()),
            screen: screen.to_string(),
        }
    }
}

fn runtimes_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("runtimes")
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("bench")
        .join("screen-corpus-baseline.json")
}

fn corpus_report() -> Value {
    let mut slugs: Vec<String> = CORPUS_RUNTIMES
        .iter()
        .map(|slug| slug.to_string())
        .collect();
    for entry in std::fs::read_dir(runtimes_root()).expect("runtimes") {
        let path = entry.expect("runtime entry").path();
        let slug = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if path.join("fixtures").join("screens").is_dir() && !slugs.contains(&slug) {
            slugs.push(slug);
        }
    }
    let mut runtimes = serde_json::Map::new();
    for slug in slugs {
        let rules = SCREEN_RULE_SOURCES
            .iter()
            .find(|(held, _)| *held == slug)
            .map(|(_, text)| parse_base_rules(text, RuleOrigin::Builtin).expect("rules"))
            .unwrap_or_default();
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(runtimes_root().join(&slug).join("fixtures").join("screens"))
                .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
                .unwrap_or_default();
        files.sort();
        let mut verdicts = serde_json::Map::new();
        let mut correct = 0u64;
        let mut seen_states = Vec::new();
        for file in &files {
            let capture = Capture::read(file);
            let state = evaluate(
                &rules,
                &ScreenInput {
                    screen: &capture.screen,
                    title: capture.title.as_deref(),
                    progress: capture.progress.as_deref(),
                    program_status: None,
                },
            )
            .state(&rules)
            .map_or("none", ScreenState::as_str);
            if state == capture.state {
                correct += 1;
            }
            seen_states.push(capture.state.clone());
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            verdicts.insert(name, json!(state));
        }
        let captures = files.len() as u64;
        let accuracy = (captures > 0)
            .then(|| (correct as f64 / captures as f64 * 10_000.0).round() / 10_000.0);
        let missing: Vec<&str> = ["working", "idle", "blocked"]
            .into_iter()
            .filter(|state| !seen_states.iter().any(|seen| seen == state))
            .collect();
        runtimes.insert(
            slug,
            json!({
                "accuracy": accuracy,
                "captures": captures,
                "correct": correct,
                "missing_states": missing,
                "verdicts": verdicts,
            }),
        );
    }
    json!({ "runtimes": runtimes })
}

#[test]
fn the_screen_corpus_classifies_as_its_recorded_baseline() {
    let report = corpus_report();
    let path = baseline_path();
    if std::env::var_os(BLESS_ENV).is_some() {
        let mut text = serde_json::to_string_pretty(&report).expect("serialize");
        text.push('\n');
        std::fs::write(&path, text).expect("write baseline");
        return;
    }
    let baseline: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("baseline file"))
            .expect("baseline JSON");
    assert_eq!(
        report, baseline,
        "the corpus no longer classifies as {}; rerun with {BLESS_ENV}=1 once the change is intended",
        path.display()
    );
}

#[test]
fn a_pane_showing_the_cli_help_is_neither_working_nor_blocked() {
    for slug in ["opencode", "pi", "hermes"] {
        let path = runtimes_root()
            .join(slug)
            .join("fixtures")
            .join("help-screen.txt");
        let screen = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let state = verdict(&builtin(slug), &screen);
        assert!(
            !matches!(state, Some(ScreenState::Working | ScreenState::Blocked)),
            "{slug}: the --help screen classifies as {state:?}"
        );
    }
}

const PRE_ENGINE_PATTERNS: &[(&str, &[&str], &[&str])] = &[
    ("claude-code", &["… (", "esc to interrupt"], &["❯"]),
    ("codex", &["esc to interrupt", "• Working"], &["›"]),
    (
        "gemini",
        &["esc to cancel"],
        &["Type your message", "> Type your message"],
    ),
];

fn pre_engine_verdict(screen: &str, working: &[&str], idle: &[&str]) -> Option<ScreenState> {
    let bottom: Vec<&str> = screen
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(15)
        .collect();
    if bottom.iter().any(|line| {
        let lowered = line.to_lowercase();
        working
            .iter()
            .any(|marker| lowered.contains(&marker.to_lowercase()))
    }) {
        return Some(ScreenState::Working);
    }
    bottom
        .iter()
        .any(|line| {
            idle.iter()
                .any(|marker| line.trim_start().starts_with(marker))
        })
        .then_some(ScreenState::Idle)
}

#[test]
fn the_converted_working_and_idle_rules_classify_as_the_pre_engine_patterns() {
    let mut window = vec!["old: Thinking… (9s)".to_string()];
    window.extend((0..20).map(|index| format!("line {index}")));
    let window = window.join("\n");
    let claude_menu_screen = corpus_screen("claude-code", "blocked-approval-menu.txt");
    let screens: &[(&str, &str)] = &[
        ("claude-code", "✽ Levitating… (1m 52s · ↓ 5.1k tokens)\n  ⎿  Tip: Use /btw\n────\n❯\n────\n  ⏵⏵ auto mode on"),
        ("claude-code", "◇ Double checking (5s · esc to interrupt)\n── Voice input ──\n❯"),
        ("claude-code", "✻ Brewed for 3s · done 10:05 AM\n   97533 tokens\n────\n❯\n────\n  ⏵⏵ auto mode on"),
        ("claude-code", ""),
        ("claude-code", "just some shell output\n$ "),
        ("claude-code", &window),
        ("codex", "────\n⠁      ⠄\n› Ask Codex to do anything\n  gpt-6 xhigh · ~/dev/paneflow"),
        ("codex", "• Working (12s • Esc to interrupt)\n› "),
        ("gemini", "  > Type your message or @path/to/file"),
        ("gemini", "⠏ Thinking (esc to cancel, 3s)\n> Type your message"),
        ("claude-code", "✻ Levitating… (1m 52s · ↓ 5.1k tokens)\nesc to interrupt"),
        ("claude-code", "❯"),
        ("claude-code", &claude_menu_screen),
        ("codex", "• Working (12s • Esc to interrupt)"),
        ("codex", "› Ask Codex to do anything"),
        ("codex", "Shell command\n\n  rm -rf build\n  Remove the build directory\n\n  1. Yes, proceed\n  2. No, and tell Codex what to do differently (esc)\n\nPress enter to confirm or esc to cancel"),
        ("gemini", "Working (esc to cancel)"),
        ("gemini", "Type your message"),
    ];
    let mut compared = 0;
    for (slug, working, idle) in PRE_ENGINE_PATTERNS {
        let steady: Vec<ScreenRule> = builtin(slug)
            .into_iter()
            .filter(|rule| rule.state != ScreenState::Blocked)
            .collect();
        for (_, screen) in screens.iter().filter(|(held, _)| held == slug) {
            assert_eq!(
                verdict(&steady, screen),
                pre_engine_verdict(screen, working, idle),
                "{slug}:\n{screen}"
            );
            compared += 1;
        }
    }
    assert_eq!(compared, screens.len());
}

#[test]
#[ignore = "timing bench, run with --release --ignored"]
fn twenty_rules_on_a_200_by_60_viewport_evaluate_within_a_millisecond_p95() {
    let mut text = String::from("engine = 2\n");
    for index in 0..20 {
        text.push_str(&format!(
            "[[rules]]\nid = \"rule-{index}\"\nstate = \"working\"\npriority = {index}\nregion = \"{region}\"\nany = ['(?i)marker-{index} \\(', '(?m)^\\s*❯ {index}']\nnot = ['(?i)to view']\n",
            region = if index % 2 == 0 { "all" } else { "last:15" }
        ));
    }
    let rules = rules(&text);
    let line = "x".repeat(199);
    let screen = (0..60)
        .map(|row| format!("{row:>2}{}", &line[2..]))
        .collect::<Vec<_>>()
        .join("\n");
    let mut samples = Vec::with_capacity(2_000);
    for _ in 0..2_000 {
        let started = std::time::Instant::now();
        let evaluation = evaluate(
            &rules,
            &ScreenInput {
                screen: &screen,
                ..ScreenInput::default()
            },
        );
        samples.push(started.elapsed());
        assert!(evaluation.winner.is_none());
    }
    samples.sort();
    let p95 = samples[samples.len() * 95 / 100];
    println!("screen rule evaluation: 20 rules, 200x60, p95 = {p95:?}");
    assert!(p95 <= std::time::Duration::from_millis(1), "p95 = {p95:?}");
}

#[test]
fn a_declared_program_status_decides_before_any_text_rule() {
    let rules = rules(
        r#"
engine = 2

[[rules]]
id = "blocked-on-busy"
state = "blocked"
priority = 20
any = ['busy']
visible_blocker = true
"#,
    );
    for declared in [
        ScreenState::Idle,
        ScreenState::Working,
        ScreenState::Blocked,
    ] {
        let evaluation = evaluate(
            &rules,
            &ScreenInput {
                screen: "busy",
                program_status: Some(declared),
                ..ScreenInput::default()
            },
        );
        assert_eq!(evaluation.state(&rules), Some(declared));
        assert_eq!(evaluation.winner, None);
        assert_eq!(evaluation.visible_blocker, None);
    }

    let fallback = evaluate(
        &rules,
        &ScreenInput {
            screen: "busy",
            ..ScreenInput::default()
        },
    );
    assert_eq!(fallback.state(&rules), Some(ScreenState::Blocked));
    assert_eq!(fallback.visible_blocker, Some(0));
}
