use crate::Result;
use crate::engine::DisplayTerminal;

const MAX_SCROLLBACK_LINES: usize = 4_000;
const MAX_SCROLLBACK_CHARS: usize = 400_000;

impl DisplayTerminal {
    pub fn extract_scrollback(&self) -> Result<Option<String>> {
        let history_rows = self.scrollback_rows()?;
        if history_rows == 0 {
            return Ok(None);
        }
        let start = history_rows.saturating_sub(MAX_SCROLLBACK_LINES);
        let mut lines: Vec<String> = self
            .grid_lines(Some(start..history_rows))?
            .into_iter()
            .map(|line| line.text.trim_end().to_owned())
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        if lines.is_empty() {
            return Ok(None);
        }
        let mut text = lines.join("\n");
        cap_complete_lines(&mut text, MAX_SCROLLBACK_CHARS);
        Ok((!text.is_empty()).then_some(text))
    }

    pub fn restore_scrollback(&mut self, text: &str) -> Result<()> {
        let mut lines: Vec<String> = bounded_recent_text(text, MAX_SCROLLBACK_CHARS)
            .split('\n')
            .rev()
            .take(MAX_SCROLLBACK_LINES)
            .map(sanitize_scrollback_line)
            .collect();
        lines.reverse();
        let mut sanitized = lines.join("\n");
        cap_complete_lines(&mut sanitized, MAX_SCROLLBACK_CHARS);
        self.feed(b"\x1b[0m")?;
        for line in sanitized.split('\n') {
            self.feed(line.as_bytes())?;
            self.feed(b"\r\n")?;
        }
        Ok(())
    }
}

fn sanitize_scrollback_line(line: &str) -> String {
    line.chars()
        .filter(|&character| {
            character == '\t'
                || (!character.is_control() && !('\u{80}'..='\u{9f}').contains(&character))
        })
        .collect()
}

fn cap_complete_lines(text: &mut String, cap: usize) {
    if text.len() <= cap {
        return;
    }
    let mut start = text.len() - cap;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 && text.as_bytes()[start - 1] == b'\n' {
        text.drain(..start);
        return;
    }
    if let Some(newline) = text[start..].find('\n') {
        start += newline + 1;
    } else {
        text.clear();
        return;
    }
    text.drain(..start);
}

fn bounded_recent_text(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut start = text.len() - cap;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 && text.as_bytes()[start - 1] == b'\n' {
        return &text[start..];
    }
    text[start..]
        .find('\n')
        .map_or("", |newline| &text[start + newline + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_sanitizer_removes_escape_and_c1_controls() {
        assert_eq!(
            sanitize_scrollback_line("safe\u{1b}]0;spoof\u{7}\u{9b}31m\tend"),
            "safe]0;spoof31m\tend"
        );
    }

    #[test]
    fn cap_keeps_valid_utf8_and_complete_recent_lines() {
        let mut text = "old\n中中\nrecent".to_owned();
        cap_complete_lines(&mut text, 11);
        assert_eq!(text, "recent");
    }

    #[test]
    fn oversized_single_line_is_not_restored_partially() {
        assert_eq!(bounded_recent_text("0123456789", 5), "");
        assert_eq!(bounded_recent_text("old\nrecent", 6), "recent");
    }
}
