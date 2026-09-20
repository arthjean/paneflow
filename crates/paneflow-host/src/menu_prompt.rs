const NAV_MARKERS: &[&str] = &["to navigate", "↑/↓", "↑ ↓", "▲/▼", "arrow keys"];

const SELECT_MARKERS: &[&str] = &[
    "to select",
    "to confirm",
    "to choose",
    "enter to",
    "esc to cancel",
    "return to",
];

const CONFIRM_MARKERS: &[&str] = &["enter to confirm", "return to confirm"];

const CANCEL_MARKERS: &[&str] = &["esc to cancel", "escape to cancel"];

const AMEND_MARKERS: &[&str] = &["tab to amend"];

const PASSIVE_MARKERS: &[&str] = &["to view"];

const PASSIVE_SELECTOR_PREFIXES: &[&str] = &["↑/↓ to select"];

const INTERACTIVE_QUALIFIERS: &[&str] = &[
    "to navigate",
    "to confirm",
    "to choose",
    "esc to cancel",
    "escape to cancel",
];

const CHOICE_LOOKBACK_LINES: usize = 12;

pub fn viewport_has_menu_prompt(screen_text: &str) -> bool {
    let lines: Vec<String> = screen_text
        .lines()
        .map(|line| line.to_lowercase())
        .collect();
    for (index, line) in lines.iter().enumerate() {
        let window = match lines.get(index + 1) {
            Some(next) => format!("{line}\n{next}"),
            None => line.clone(),
        };
        let window = window.split_whitespace().collect::<Vec<_>>().join(" ");
        let has_nav = NAV_MARKERS.iter().any(|marker| window.contains(marker));
        let has_select = SELECT_MARKERS.iter().any(|marker| window.contains(marker));
        let has_confirm = CONFIRM_MARKERS.iter().any(|marker| window.contains(marker));
        let has_cancel = CANCEL_MARKERS.iter().any(|marker| window.contains(marker));
        let has_amend = AMEND_MARKERS.iter().any(|marker| window.contains(marker));
        let passive_action = PASSIVE_MARKERS.iter().any(|marker| window.contains(marker));
        let passive_selector_prefix = PASSIVE_SELECTOR_PREFIXES
            .iter()
            .any(|marker| window.contains(marker));
        let interactive_qualifier = INTERACTIVE_QUALIFIERS
            .iter()
            .any(|marker| window.contains(marker));
        let passive = passive_action || (passive_selector_prefix && !interactive_qualifier);
        let approval = has_cancel && has_amend && has_selected_choices(&lines, index);
        if ((has_nav && has_select) || (has_confirm && has_cancel) || approval) && !passive {
            return true;
        }
    }
    false
}

fn has_selected_choices(lines: &[String], footer: usize) -> bool {
    let mut previous = None;
    let mut count = 0;
    let mut selected = false;
    for line in &lines[footer.saturating_sub(CHOICE_LOOKBACK_LINES)..footer] {
        let line = line.trim();
        let cursor = line.starts_with(['❯', '›', '>']);
        let line = line.trim_start_matches(['❯', '›', '>']).trim_start();
        let Some((number, label)) = line.split_once('.') else {
            continue;
        };
        if !label.starts_with(char::is_whitespace) || label.trim().is_empty() {
            continue;
        }
        let Ok(number @ 1..=9) = number.parse::<u8>() else {
            continue;
        };
        if previous != Some(number - 1) {
            count = 0;
            selected = false;
        }
        previous = Some(number);
        count += 1;
        selected |= cursor;
    }
    count >= 2 && selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(slug: &str, name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("runtimes")
            .join(slug)
            .join("fixtures")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read the fixture {}: {error}", path.display()))
    }

    #[test]
    fn the_shipped_approval_fixtures_are_menus() {
        let claude = fixture("claude-code", "approval-menu.txt");
        assert!(viewport_has_menu_prompt(&claude));
        assert!(viewport_has_menu_prompt(
            &claude.replace("Tab to amend", "Tab to\n amend")
        ));
        assert!(viewport_has_menu_prompt(
            &claude.replace("❯ 1.", "  1.").replace("  3.", "❯ 3.")
        ));
        assert!(viewport_has_menu_prompt(&fixture(
            "codex",
            "approval-menu.txt"
        )));
    }

    #[test]
    fn cancel_and_amend_require_nearby_selected_choices() {
        let claude = fixture("claude-code", "approval-menu.txt");
        assert!(!viewport_has_menu_prompt(
            "Working… Esc to cancel · Tab to amend"
        ));
        assert!(!viewport_has_menu_prompt(&claude.replace('❯', " ")));
        assert!(!viewport_has_menu_prompt(
            &claude.replace("  2.", "  x.").replace("  3.", "  x.")
        ));
        assert!(!viewport_has_menu_prompt(&claude.replace(
            "Esc to cancel",
            &format!("{}Esc to cancel", "output\n".repeat(13))
        )));
    }

    #[test]
    fn a_navigation_hint_beside_a_select_hint_is_a_menu() {
        let screen = "❯ 1. Switch to the yearly plan\n  2. Keep the one-time license\n\n\
             Enter to select · ↑/↓ to navigate · Esc to cancel";
        assert!(viewport_has_menu_prompt(screen));
        let arrows = "Use arrow keys to move.\nPress return to confirm your choice.";
        assert!(viewport_has_menu_prompt(arrows));
    }

    #[test]
    fn a_confirm_key_beside_a_cancel_key_is_a_menu_without_any_navigation_hint() {
        let screen = "  1. Yes, proceed\n\
             2. No, and tell Codex what to do differently (esc)\n\n\
             Press enter to confirm or esc to cancel";
        assert!(viewport_has_menu_prompt(screen));
    }

    #[test]
    fn a_lone_cancel_hint_is_not_a_menu() {
        assert!(!viewport_has_menu_prompt("Working… press esc to cancel"));
    }

    #[test]
    fn prose_that_merely_mentions_selecting_or_navigating_is_not_a_menu() {
        assert!(!viewport_has_menu_prompt(
            "Please select the files you want to keep and let me know."
        ));
        assert!(!viewport_has_menu_prompt(
            "Use ↑/↓ to navigate the log output."
        ));
        assert!(!viewport_has_menu_prompt(""));
    }

    #[test]
    fn quoted_navigation_prose_far_above_a_shell_prompt_is_not_a_menu() {
        let mut screen = String::from("the footer said \"↑/↓ to navigate\" in the transcript\n");
        for index in 0..30 {
            screen.push_str(&format!("transcript line {index}\n"));
        }
        screen.push_str("$ ");
        assert!(!viewport_has_menu_prompt(&screen));
    }

    #[test]
    fn the_passive_subagent_footer_never_asks_for_a_choice() {
        let pinned = "⏺ Working…\n\n\
             ⏺ main   ↑/↓ to select · Enter to view  ◯ Explore  Audit resume pipeline";
        assert!(!viewport_has_menu_prompt(pinned));
        let wrapped = "  ◯ main           ↑/↓ to select · Enter to\n\
                   view\n\
             \u{23fa} general-purpose 55m 46s · ↓ 348.3k";
        assert!(!viewport_has_menu_prompt(wrapped));
        let partial = "  ⏺ main           ↑/↓ to select · Enter to";
        assert!(!viewport_has_menu_prompt(partial));
        let qualified = "  1. Keep working\n  2. Stop\n\
             ↑/↓ to select · Enter to confirm · Esc to cancel";
        assert!(viewport_has_menu_prompt(qualified));
    }

    #[test]
    fn hints_rows_apart_are_not_one_footer() {
        let screen = "The arrows are now first: ← ↑ ↓ →\n\
             (compiling)\n(compiling)\n(compiling)\n\
             Press Enter to submit your prompt.";
        assert!(!viewport_has_menu_prompt(screen));
    }
}
