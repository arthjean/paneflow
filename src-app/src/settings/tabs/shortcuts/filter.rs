use std::ops::Range;

use crate::app::command_palette::matcher::{Rank, match_entry};
use crate::keybindings::{self, ChordFit, ChordPattern, ShortcutEntry, ShortcutGroup};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShortcutFilter {
    text: String,
    chords: Vec<ChordPattern>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct HitScore {
    chord: ChordFit,
    text: Rank,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutHit {
    pub(crate) index: usize,
    pub(crate) score: HitScore,
    pub(crate) label: Vec<Range<usize>>,
    pub(crate) chords: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutSection {
    pub(crate) group: ShortcutGroup,
    pub(crate) hits: Vec<ShortcutHit>,
}

const UNASSIGNED_TERMS: &str = "unassigned none";

fn raw_chords(entry: &ShortcutEntry) -> impl Iterator<Item = &str> {
    entry
        .raw_key
        .as_deref()
        .into_iter()
        .chain(entry.extra_raw_keys.iter().map(String::as_str))
}

fn key_text(entry: &ShortcutEntry) -> String {
    if entry.raw_key.is_none() {
        return UNASSIGNED_TERMS.to_string();
    }
    let shown = std::iter::once(entry.key.clone())
        .chain(
            entry
                .extra_raw_keys
                .iter()
                .map(|raw| keybindings::format_keystroke(raw)),
        )
        .collect::<Vec<_>>()
        .join(" ");
    format!("{shown} {}", entry.search_key).to_lowercase()
}

fn keywords(entry: &ShortcutEntry) -> String {
    format!(
        "{} {} {}",
        entry.group.label(),
        entry.action_name,
        entry.action_name.replace('_', " ")
    )
}

impl ShortcutFilter {
    pub(crate) fn parse(query: &str) -> Self {
        let mut text = Vec::new();
        let mut chords = Vec::new();
        for token in query.split_whitespace() {
            match ChordPattern::parse(token) {
                Some(chord) => chords.push(chord),
                None => text.push(token),
            }
        }
        Self {
            text: text.join(" ").to_lowercase(),
            chords,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty() && self.chords.is_empty()
    }

    pub(crate) fn sole_chord(&self) -> Option<&ChordPattern> {
        match self.chords.as_slice() {
            [chord] if self.text.is_empty() && chord.has_key() => Some(chord),
            _ => None,
        }
    }

    pub(crate) fn words(&self) -> impl Iterator<Item = &str> {
        self.text.split_whitespace()
    }

    pub(crate) fn hit(&self, index: usize, entry: &ShortcutEntry) -> Option<ShortcutHit> {
        let mut chord = ChordFit::Exact;
        let mut matched_chords = Vec::new();
        for pattern in &self.chords {
            let best = raw_chords(entry)
                .enumerate()
                .filter_map(|(position, raw)| pattern.fit_raw(raw).map(|fit| (fit, position)))
                .min()?;
            chord = chord.max(best.0);
            matched_chords.push(best.1);
        }
        matched_chords.sort_unstable();
        matched_chords.dedup();

        let (text, label) = if self.text.is_empty() {
            (Rank::LabelWordStart, Vec::new())
        } else {
            let matched = match_entry(
                &entry.description,
                Some(&key_text(entry)),
                &keywords(entry),
                &self.text,
            )?;
            (matched.rank, matched.highlights)
        };

        Some(ShortcutHit {
            index,
            score: HitScore { chord, text },
            label,
            chords: matched_chords,
        })
    }

    pub(crate) fn sections(&self, entries: &[ShortcutEntry]) -> Vec<ShortcutSection> {
        let mut sections: Vec<ShortcutSection> = ShortcutGroup::ALL
            .iter()
            .map(|group| ShortcutSection {
                group: *group,
                hits: Vec::new(),
            })
            .collect();
        for (index, entry) in entries.iter().enumerate() {
            let Some(hit) = self.hit(index, entry) else {
                continue;
            };
            if let Some(section) = sections
                .iter_mut()
                .find(|section| section.group == entry.group)
            {
                section.hits.push(hit);
            }
        }
        sections.retain(|section| !section.hits.is_empty());
        if !self.is_empty() {
            for section in &mut sections {
                section.hits.sort_by_key(|hit| hit.score);
            }
            sections.sort_by_key(|section| section.hits.first().map(|hit| hit.score));
        }
        sections
    }
}

pub(crate) fn word_ranges(text: &str, words: &[&str]) -> Vec<Range<usize>> {
    let lowered = text.to_ascii_lowercase();
    let mut ranges: Vec<Range<usize>> = words
        .iter()
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            lowered
                .match_indices(*word)
                .map(|(start, matched)| start..start + matched.len())
                .collect::<Vec<_>>()
        })
        .collect();
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(description: &str, raw: Option<&str>, group: ShortcutGroup) -> ShortcutEntry {
        ShortcutEntry {
            key: raw.map_or_else(|| "Unassigned".to_string(), keybindings::format_keystroke),
            raw_key: raw.map(str::to_string),
            extra_raw_keys: Vec::new(),
            customized: false,
            description: description.to_string(),
            action_name: Box::leak(
                description
                    .to_lowercase()
                    .replace(' ', "_")
                    .into_boxed_str(),
            ),
            group,
            search_key: raw.unwrap_or_default().replace('-', "+"),
        }
    }

    fn catalog() -> Vec<ShortcutEntry> {
        vec![
            entry(
                "Split horizontal",
                Some("ctrl-shift-d"),
                ShortcutGroup::Panes,
            ),
            entry("Toggle zoom", Some("ctrl-shift-z"), ShortcutGroup::Panes),
            entry("New tab", Some("ctrl-shift-t"), ShortcutGroup::Tabs),
            entry("Focus split below", Some("alt-down"), ShortcutGroup::Panes),
            entry("Open settings", None, ShortcutGroup::Application),
        ]
    }

    fn found(query: &str) -> Vec<String> {
        let entries = catalog();
        ShortcutFilter::parse(query)
            .sections(&entries)
            .into_iter()
            .flat_map(|section| section.hits)
            .map(|hit| entries[hit.index].description.clone())
            .collect()
    }

    #[test]
    fn an_empty_query_keeps_the_registry_order() {
        assert_eq!(
            found("  "),
            [
                "Split horizontal",
                "Toggle zoom",
                "Focus split below",
                "New tab",
                "Open settings"
            ]
        );
    }

    #[test]
    fn a_word_start_match_ranks_before_a_subsequence() {
        let by_label_then_section = ["Split horizontal", "Focus split below", "Toggle zoom"];
        assert_eq!(found("split"), by_label_then_section);
        assert_eq!(found("spl"), by_label_then_section);
        assert_eq!(
            found("sph").first().map(String::as_str),
            Some("Split horizontal")
        );
    }

    #[test]
    fn a_chord_finds_its_owner_whatever_the_modifier_order() {
        assert_eq!(found("shift+ctrl+t"), ["New tab"]);
        assert_eq!(found("Ctrl+Shift+T"), ["New tab"]);
    }

    #[test]
    fn modifiers_alone_list_every_chord_that_holds_them() {
        assert_eq!(
            found("ctrl+shift"),
            ["Split horizontal", "Toggle zoom", "New tab"]
        );
    }

    #[test]
    fn words_and_chords_narrow_together() {
        assert_eq!(found("zoom ctrl+shift"), ["Toggle zoom"]);
        assert!(found("split ctrl+shift+t").is_empty());
    }

    #[test]
    fn the_section_name_the_action_id_and_unassigned_are_searchable() {
        assert_eq!(found("tabs"), ["New tab"]);
        assert!(found("toggle_zoom").contains(&"Toggle zoom".to_string()));
        assert_eq!(found("unassigned"), ["Open settings"]);
    }

    #[test]
    fn the_matched_chord_and_label_are_reported() {
        let entries = catalog();
        let hit = ShortcutFilter::parse("tab ctrl+shift+t")
            .hit(2, &entries[2])
            .expect("a hit");
        assert_eq!(hit.label, vec![4..7]);
        assert_eq!(hit.chords, vec![0]);
    }

    #[test]
    fn only_a_lone_complete_chord_can_be_free() {
        assert!(ShortcutFilter::parse("ctrl+shift+q").sole_chord().is_some());
        assert!(ShortcutFilter::parse("ctrl+shift").sole_chord().is_none());
        assert!(ShortcutFilter::parse("tab ctrl+q").sole_chord().is_none());
    }

    #[test]
    fn word_ranges_merge_overlaps() {
        assert_eq!(word_ranges("Panes & splits", &["pan", "anes"]), vec![0..5]);
        assert!(word_ranges("Tabs", &[]).is_empty());
    }
}
