use std::fmt::Write as _;

use serde_json::Value;

struct Fill {
    row: u64,
    first: u64,
    last: u64,
    bg: String,
}

fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let value = u32::from_str_radix(hex, 16).unwrap_or(0);
    ((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

fn sgr(style: &str, bg: Option<&str>) -> String {
    let mut codes = vec!["0".to_string()];
    for part in style
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part {
            "b" => codes.push("1".into()),
            "u" => codes.push("4".into()),
            "d" => codes.push("2".into()),
            other => {
                if let Some(index) = other.strip_prefix("p=").and_then(|n| n.parse::<u8>().ok()) {
                    let code = if index < 8 {
                        30 + index
                    } else {
                        90 + index - 8
                    };
                    codes.push(code.to_string());
                } else if let Some(hex) = other.strip_prefix("fg=") {
                    let (r, g, b) = hex_rgb(hex);
                    codes.push(format!("38;2;{r};{g};{b}"));
                }
            }
        }
    }
    if let Some(hex) = bg {
        let (r, g, b) = hex_rgb(hex);
        codes.push(format!("48;2;{r};{g};{b}"));
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// Turns one pane of a terminals document into the VT stream that paints it.
///
/// A document maps pane names to `{cursor, fills, segments}`: `fills` are
/// `[row, first, last, "rrggbb"]` background runs and `segments` are
/// `[row, col, text, style]`, where style lists `b`, `u`, `d` (SGR 2),
/// `fg=rrggbb` and `p=N` (palette index).
pub(crate) fn stream(document: &str, name: &str) -> Vec<u8> {
    let doc: Value = serde_json::from_str(document)
        .unwrap_or_else(|error| panic!("terminals document: {error}"));
    let pane = &doc[name];
    assert!(pane.is_object(), "no pane {name} in the terminals document");
    let fills: Vec<Fill> = pane["fills"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|fill| Fill {
            row: fill[0].as_u64().unwrap_or(0),
            first: fill[1].as_u64().unwrap_or(0),
            last: fill[2].as_u64().unwrap_or(0),
            bg: fill[3].as_str().unwrap_or("000000").to_string(),
        })
        .collect();
    let bg_at = |row: u64, col: u64| {
        fills
            .iter()
            .rev()
            .find(|fill| fill.row == row && (fill.first..=fill.last).contains(&col))
            .map(|fill| fill.bg.as_str())
    };
    let mut out = String::from("\x1b[?25l\x1b[2J\x1b[H");
    for fill in &fills {
        let _ = write!(out, "\x1b[{};{}H", fill.row + 1, fill.first + 1);
        out.push_str(&sgr("", Some(&fill.bg)));
        out.push_str(&" ".repeat((fill.last - fill.first + 1) as usize));
    }
    for segment in pane["segments"].as_array().into_iter().flatten() {
        let row = segment[0].as_u64().unwrap_or(0);
        let col = segment[1].as_u64().unwrap_or(0);
        let text = segment[2].as_str().unwrap_or("");
        let style = segment[3].as_str().unwrap_or("");
        let _ = write!(out, "\x1b[{};{}H", row + 1, col + 1);
        let mut current: Option<Option<&str>> = None;
        for (offset, ch) in text.chars().enumerate() {
            let bg = bg_at(row, col + offset as u64);
            if current != Some(bg) {
                out.push_str(&sgr(style, bg));
                current = Some(bg);
            }
            out.push(ch);
        }
    }
    out.push_str("\x1b[0m");
    if let Some(cursor) = pane["cursor"].as_array() {
        let row = cursor[0].as_u64().unwrap_or(0);
        let col = cursor[1].as_u64().unwrap_or(0);
        let _ = write!(out, "\x1b[{};{}H\x1b[6 q\x1b[?25h", row + 1, col + 1);
    }
    out.into_bytes()
}
