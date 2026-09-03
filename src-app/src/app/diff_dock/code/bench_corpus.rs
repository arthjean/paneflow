use std::ops::Range;

pub(crate) const EDITOR_CORPUS_SEED: u64 = 0x4544_4954_4f52_5f31;

pub(crate) const HIGHLIGHTED_RUST_BYTES: usize = 295_000;

pub(crate) const LARGE_RUST_BYTES: usize = 3_700_000;

pub(crate) const RELOAD_RUST_BYTES: usize = 2_000_000;

pub(crate) const MINIFIED_JSON_CHARS: usize = 10_000;

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn pick(&mut self, choices: usize) -> usize {
        (self.next_u64() % choices.max(1) as u64) as usize
    }
}

const NAMES: [&str; 8] = [
    "node", "frame", "cursor", "buffer", "anchor", "region", "token", "layer",
];

const VERBS: [&str; 6] = ["resolve", "measure", "collect", "flush", "adopt", "clamp"];

fn truncate_at_line(mut text: String, target: usize) -> String {
    if text.len() <= target {
        return text;
    }
    let cut = text[..target]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(target);
    text.truncate(cut);
    text
}

fn push_rust_item(out: &mut String, rng: &mut Lcg, index: usize) {
    let name = NAMES[rng.pick(NAMES.len())];
    let verb = VERBS[rng.pick(VERBS.len())];
    match rng.pick(6) {
        0 => {
            out.push_str(&format!(
                "use crate::{name}_{index}::{{Handle, Slot, SlotKind, SlotRange}};\n\n"
            ));
            out.push_str("#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]\n");
            out.push_str(&format!("pub struct {name}{index} {{\n"));
            out.push_str("    pub identifier: usize,\n");
            out.push_str("    pub label: std::borrow::Cow<'static, str>,\n");
            out.push_str("    pub occupied_slots: Vec<SlotRange>,\n");
            out.push_str("    pub parent_handle: Option<Handle>,\n");
            out.push_str("    pub last_measured_width: Option<usize>,\n");
            out.push_str("}\n\n");
        }
        1 => {
            out.push_str(&format!("impl {name}{index} {{\n"));
            out.push_str("    pub fn with_identifier(identifier: usize) -> Self {\n");
            out.push_str("        Self {\n");
            out.push_str("            identifier,\n");
            out.push_str(&format!(
                "            label: std::borrow::Cow::Borrowed(\"{name}-{index}\"),\n"
            ));
            out.push_str("            occupied_slots: Vec::with_capacity(8),\n");
            out.push_str("            parent_handle: None,\n");
            out.push_str("            last_measured_width: None,\n");
            out.push_str("        }\n");
            out.push_str("    }\n\n");
            out.push_str(&format!(
                "    pub fn {verb}(&self, rows: usize, columns: usize) -> Option<usize> {{\n"
            ));
            out.push_str(
                "        let occupied = self.occupied_slots.iter().filter(|slot| slot.len() > 0);\n",
            );
            out.push_str("        let resolved = occupied.map(SlotRange::len).sum::<usize>();\n");
            out.push_str("        if resolved > rows.saturating_mul(columns) {\n");
            out.push_str(
                "            return Some(resolved.saturating_sub(rows).min(columns * 4));\n",
            );
            out.push_str("        }\n");
            out.push_str("        None\n");
            out.push_str("    }\n");
            out.push_str("}\n\n");
        }
        2 => {
            out.push_str(&format!(
                "pub fn {verb}_{index}(values: &[usize], threshold: usize) -> usize {{\n"
            ));
            out.push_str("    let mut total = 0usize;\n");
            out.push_str("    for (position, value) in values.iter().copied().enumerate() {\n");
            out.push_str(&format!(
                "        total = total.saturating_add(value.wrapping_mul({}));\n",
                index % 7 + 1
            ));
            out.push_str("        if total > threshold.saturating_mul(position.max(1)) {\n");
            out.push_str(
                "            log::debug!(\"threshold crossed at {position}: {total}\");\n",
            );
            out.push_str("            break;\n");
            out.push_str("        }\n");
            out.push_str("    }\n");
            out.push_str("    total\n");
            out.push_str("}\n\n");
        }
        3 => {
            out.push_str(&format!("pub enum {name}Kind{index} {{\n"));
            out.push_str("    Idle,\n");
            out.push_str("    Pending { since: std::time::Instant },\n");
            out.push_str("    Ready { rows: usize, columns: usize, dirty: bool },\n");
            out.push_str("}\n\n");
            out.push_str(&format!("impl {name}Kind{index} {{\n"));
            out.push_str("    pub fn rank(&self) -> u8 {\n");
            out.push_str("        match self {\n");
            out.push_str("            Self::Idle => 0,\n");
            out.push_str("            Self::Pending { .. } => 1,\n");
            out.push_str("            Self::Ready { dirty: true, .. } => 2,\n");
            out.push_str("            Self::Ready { .. } => 3,\n");
            out.push_str("        }\n");
            out.push_str("    }\n");
            out.push_str("}\n\n");
        }
        4 => {
            out.push_str("/// Documented helper kept next to the data it walks, so the query\n");
            out.push_str("/// stays close to the rows it colors on the render thread.\n");
            out.push_str(&format!(
                "pub fn {verb}_{name}_{index}(input: &str, limit: usize) -> Option<usize> {{\n"
            ));
            out.push_str("    let trimmed = input.trim_start_matches(char::is_whitespace);\n");
            out.push_str(&format!(
                "    if trimmed.starts_with(\"{name}\") && trimmed.len() <= limit {{\n"
            ));
            out.push_str("        return Some(trimmed.len().saturating_sub(limit / 2));\n");
            out.push_str("    }\n");
            out.push_str("    trimmed.find(':').map(|position| position + limit)\n");
            out.push_str("}\n\n");
        }
        _ => {
            let upper = name.to_uppercase();
            out.push_str(&format!(
                "pub const {upper}_LIMIT_{index}: usize = {};\n",
                index % 512 + 16
            ));
            out.push_str(&format!(
                "static {upper}_TABLE_{index}: [&str; 4] = [\"alpha\", \"beta\", \"gamma\", \"delta\"];\n\n"
            ));
            out.push_str(&format!(
                "pub fn {verb}_table_{index}(row: usize) -> &'static str {{\n"
            ));
            out.push_str(&format!(
                "    {upper}_TABLE_{index}[row % {upper}_TABLE_{index}.len()]\n"
            ));
            out.push_str("}\n\n");
        }
    }
}

pub(crate) fn rust_source(target_bytes: usize) -> String {
    let mut rng = Lcg::new(EDITOR_CORPUS_SEED);
    let mut out = String::with_capacity(target_bytes + 4_096);
    let mut index = 0usize;
    while out.len() < target_bytes {
        push_rust_item(&mut out, &mut rng, index);
        index += 1;
    }
    truncate_at_line(out, target_bytes)
}

pub(crate) fn markdown_source(target_bytes: usize) -> String {
    let mut rng = Lcg::new(EDITOR_CORPUS_SEED ^ 0x00ff);
    let mut out = String::with_capacity(target_bytes + 4_096);
    let mut index = 0usize;
    while out.len() < target_bytes {
        let name = NAMES[rng.pick(NAMES.len())];
        out.push_str(&format!("## Section {index}: the {name} pass\n\n"));
        out.push_str(&format!(
            "The `{name}_{index}` helper returns `Option<usize>` and never blocks the\n"
        ));
        out.push_str("render thread. See `docs/user/scripting.md` for the wiring.\n\n");
        out.push_str("```rust\n");
        out.push_str(&format!("fn {name}_{index}(rows: usize) -> usize {{\n"));
        out.push_str("    rows.saturating_sub(1)\n");
        out.push_str("}\n");
        out.push_str("```\n\n");
        out.push_str(&format!("- `{name}` is stable across frames\n"));
        out.push_str("- inline `code` mixes with *emphasis* and **strong**\n\n");
        index += 1;
    }
    truncate_at_line(out, target_bytes)
}

pub(crate) fn minified_json_line(target_chars: usize) -> String {
    assert!(target_chars >= 64, "the minified json corpus needs room");
    let mut rng = Lcg::new(EDITOR_CORPUS_SEED ^ 0xff00);
    let mut out = String::with_capacity(target_chars + 64);
    out.push('{');
    let mut index = 0usize;
    while out.len() < target_chars {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!("\"{index:x}\":"));
        match rng.pick(4) {
            0 => out.push_str(&rng.pick(100).to_string()),
            1 => out.push_str(&format!("\"{}\"", rng.pick(100))),
            2 => out.push_str(if rng.pick(2) == 0 { "1" } else { "0" }),
            _ => out.push_str(&format!("[{}]", rng.pick(10))),
        }
        index += 1;
    }
    let overhead = ",\"pad\":\"\"".len();
    while out.len() + overhead + 1 > target_chars {
        let cut = out.rfind(',').unwrap_or(1);
        out.truncate(cut);
    }
    let filler = target_chars - 1 - overhead - out.len();
    out.push_str(",\"pad\":\"");
    out.push_str(&"0".repeat(filler));
    out.push('"');
    out.push('}');
    out.push('\n');
    out
}

pub(crate) fn json_token_ranges(line: &str, wanted: usize) -> Vec<Range<usize>> {
    let body = line.trim_end_matches('\n');
    let mut ranges = Vec::with_capacity(wanted);
    ranges.push(0..body.len());
    let mut start = 0usize;
    for (index, byte) in body.as_bytes().iter().enumerate() {
        if !matches!(byte, b'{' | b'}' | b'[' | b']' | b',' | b':') {
            continue;
        }
        if index > start {
            ranges.push(start..index);
        }
        ranges.push(index..index + 1);
        start = index + 1;
        if ranges.len() >= wanted {
            break;
        }
    }
    if start < body.len() && ranges.len() < wanted {
        ranges.push(start..body.len());
    }
    ranges.truncate(wanted);
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpora_are_deterministic_and_sized() {
        for target in [4_096usize, HIGHLIGHTED_RUST_BYTES] {
            let first = rust_source(target);
            assert_eq!(first, rust_source(target), "the rust corpus must be stable");
            assert!(first.len() <= target, "{} > {target}", first.len());
            assert!(first.len() + 512 > target, "{} is too short", first.len());
            assert!(first.ends_with('\n'));
        }
        let markdown = markdown_source(64_000);
        assert_eq!(markdown, markdown_source(64_000));
        assert!(markdown.contains("```rust"), "markdown carries fenced code");
        assert!(markdown.contains('`'), "markdown carries inline code");
        let json = minified_json_line(MINIFIED_JSON_CHARS);
        assert_eq!(json, minified_json_line(MINIFIED_JSON_CHARS));
        assert_eq!(json.lines().count(), 1, "the json corpus is a single line");
        assert_eq!(json.trim_end().len(), MINIFIED_JSON_CHARS);
    }

    #[test]
    fn the_json_corpus_stays_valid_at_every_size_the_bench_uses() {
        for target in [64usize, 2_048, MINIFIED_JSON_CHARS] {
            let line = minified_json_line(target);
            assert_eq!(line.trim_end().len(), target, "target {target}");
            assert!(line.starts_with('{'), "target {target}");
            assert!(line.trim_end().ends_with("\"}"), "target {target}");
        }
    }

    #[test]
    fn the_large_corpus_has_the_line_shape_the_bench_claims() {
        let source = rust_source(LARGE_RUST_BYTES);
        let lines = source.lines().count();
        assert!(
            (95_000..130_000).contains(&lines),
            "the 3.7 MB corpus must sit near 110 000 lines, got {lines}"
        );
    }

    #[test]
    fn json_token_ranges_cover_the_line_and_stay_in_bounds() {
        let line = minified_json_line(MINIFIED_JSON_CHARS);
        let ranges = json_token_ranges(&line, 3_750);
        assert_eq!(ranges.len(), 3_750);
        assert_eq!(ranges[0], 0..MINIFIED_JSON_CHARS);
        for range in &ranges {
            assert!(range.start < range.end);
            assert!(range.end <= MINIFIED_JSON_CHARS);
        }
    }
}
