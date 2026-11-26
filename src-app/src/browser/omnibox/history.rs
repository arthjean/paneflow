use std::ops::Range;

use serde::{Deserialize, Serialize};

pub(crate) const MAX_ENTRIES: usize = 3000;
const TYPED_WEIGHT: f64 = 20.;
const HALF_LIFE_DAYS: f64 = 30.;
const DAY_SECONDS: f64 = 86_400.;
const HOST_PREFIX_QUALITY: f64 = 3.;
const WORD_START_QUALITY: f64 = 2.;
const ROOT_SHARE_FOR_INLINE: f64 = 0.5;
const INLINE_MIN_VISITS: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Visit {
    pub(crate) url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    pub(crate) visits: u32,
    #[serde(default)]
    pub(crate) typed: u32,
    pub(crate) last: u64,
}

impl Visit {
    fn frecency(&self, now: u64) -> f64 {
        let age_days = now.saturating_sub(self.last) as f64 / DAY_SECONDS;
        let weight = f64::from(self.typed) * TYPED_WEIGHT + f64::from(self.visits);
        weight * 0.5_f64.powf(age_days / HALF_LIFE_DAYS)
    }

    fn inlinable(&self) -> bool {
        self.typed > 0 || self.visits >= INLINE_MIN_VISITS
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct History {
    #[serde(default)]
    entries: Vec<Visit>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Match {
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    pub(crate) shown: String,
    pub(crate) score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inline {
    pub(crate) url: String,
    pub(crate) text: String,
}

fn without_web_prefix(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(scheme, rest)| {
        if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            rest
        } else {
            url
        }
    });
    rest.strip_prefix("www.").unwrap_or(rest)
}

pub(crate) fn shown_url(url: &str) -> String {
    let rest = without_web_prefix(url);
    rest.strip_suffix('/').unwrap_or(rest).to_string()
}

fn host_of(shown: &str) -> &str {
    shown.split(['/', '?', '#']).next().unwrap_or(shown)
}

pub(crate) fn terms(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(|term| {
            let lower = term.to_lowercase();
            shown_url(&lower)
        })
        .filter(|term| !term.is_empty())
        .collect()
}

fn is_word_start(text: &str, at: usize) -> bool {
    at == 0
        || text[..at]
            .chars()
            .next_back()
            .is_some_and(|before| !before.is_alphanumeric())
}

pub(crate) fn word_starts(text: &str, term: &str) -> Vec<Range<usize>> {
    let lower = text.to_lowercase();
    if lower.len() != text.len() || term.is_empty() {
        return Vec::new();
    }
    lower
        .match_indices(term)
        .filter(|(at, _)| is_word_start(&lower, *at))
        .map(|(at, found)| at..at + found.len())
        .collect()
}

pub(crate) fn highlights(text: &str, terms: &[String]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = terms
        .iter()
        .flat_map(|term| word_starts(text, term))
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

fn quality(shown: &str, title: &str, terms: &[String]) -> Option<f64> {
    let host = host_of(shown);
    let mut total = 0.;
    for (index, term) in terms.iter().enumerate() {
        let best = if index == 0 && host.starts_with(term.as_str()) {
            HOST_PREFIX_QUALITY
        } else if !word_starts(shown, term).is_empty() || !word_starts(title, term).is_empty() {
            WORD_START_QUALITY
        } else {
            return None;
        };
        total += best;
    }
    Some(total / terms.len() as f64)
}

impl History {
    pub(crate) fn record(&mut self, url: &str, title: Option<&str>, typed: bool, now: u64) {
        let title = title
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string);
        match self.entries.iter_mut().find(|visit| visit.url == url) {
            Some(visit) => {
                visit.visits = visit.visits.saturating_add(1);
                visit.typed = visit.typed.saturating_add(u32::from(typed));
                visit.last = now.max(visit.last);
                if title.is_some() {
                    visit.title = title;
                }
            }
            None => self.entries.push(Visit {
                url: url.to_string(),
                title,
                visits: 1,
                typed: u32::from(typed),
                last: now,
            }),
        }
        if self.entries.len() > MAX_ENTRIES {
            self.prune(now);
        }
    }

    pub(crate) fn retitle(&mut self, url: &str, title: &str) -> bool {
        let title = title.trim();
        match self.entries.iter_mut().find(|visit| visit.url == url) {
            Some(visit) if !title.is_empty() && visit.title.as_deref() != Some(title) => {
                visit.title = Some(title.to_string());
                true
            }
            _ => false,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn kept_since(&self, since: u64) -> usize {
        self.entries
            .iter()
            .filter(|visit| visit.last >= since)
            .count()
    }

    pub(crate) fn clear_since(&mut self, since: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|visit| visit.last < since);
        self.entries.len() != before
    }

    pub(crate) fn remove(&mut self, url: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|visit| visit.url != url);
        self.entries.len() != before
    }

    fn prune(&mut self, now: u64) {
        self.entries
            .sort_by(|a, b| b.frecency(now).total_cmp(&a.frecency(now)));
        self.entries.truncate(MAX_ENTRIES);
    }

    pub(crate) fn matches(&self, input: &str, now: u64, limit: usize) -> Vec<Match> {
        let terms = terms(input);
        if terms.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<Match> = self
            .entries
            .iter()
            .filter_map(|visit| {
                let shown = shown_url(&visit.url);
                let title = visit.title.as_deref().unwrap_or_default();
                let quality = quality(&shown, title, &terms)?;
                Some(Match {
                    url: visit.url.clone(),
                    title: visit.title.clone(),
                    score: quality * (1. + visit.frecency(now)).ln_1p(),
                    shown,
                })
            })
            .collect();
        found.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.shown.cmp(&b.shown))
        });
        found.truncate(limit);
        found
    }

    pub(crate) fn inline(&self, typed: &str, now: u64) -> Option<Inline> {
        if typed.chars().any(char::is_whitespace) {
            return None;
        }
        let lower = typed.to_lowercase();
        if lower.len() != typed.len() {
            return None;
        }
        let prefix = without_web_prefix(&lower);
        if prefix.is_empty() {
            return None;
        }
        let candidates: Vec<(&Visit, String)> = self
            .entries
            .iter()
            .filter(|visit| visit.inlinable())
            .map(|visit| (visit, shown_url(&visit.url)))
            .filter(|(_, shown)| {
                shown.len() > prefix.len() && shown.to_lowercase().starts_with(prefix)
            })
            .collect();
        let (best, best_shown) = candidates
            .iter()
            .max_by(|a, b| a.0.frecency(now).total_cmp(&b.0.frecency(now)))?;
        let host = host_of(best_shown);
        let root = (!prefix.contains('/') && host.len() > prefix.len())
            .then(|| {
                candidates.iter().find(|(visit, shown)| {
                    shown == host
                        && visit.frecency(now) >= best.frecency(now) * ROOT_SHARE_FOR_INLINE
                })
            })
            .flatten()
            .map_or((*best, best_shown.as_str()), |(visit, shown)| {
                (*visit, shown.as_str())
            });
        let (visit, shown) = root;
        Some(Inline {
            url: visit.url.clone(),
            text: format!("{typed}{}", &shown[prefix.len()..]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const DAY: u64 = 86_400;

    fn history(visits: &[(&str, &str, u32, u32, u64)]) -> History {
        History {
            entries: visits
                .iter()
                .map(|(url, title, visits, typed, age_days)| Visit {
                    url: (*url).to_string(),
                    title: Some((*title).to_string()),
                    visits: *visits,
                    typed: *typed,
                    last: NOW - age_days * DAY,
                })
                .collect(),
        }
    }

    #[test]
    fn a_url_is_shown_without_its_web_scheme_www_and_trailing_slash() {
        assert_eq!(shown_url("https://www.vercel.com/"), "vercel.com");
        assert_eq!(
            shown_url("http://localhost:5173/app/"),
            "localhost:5173/app"
        );
        assert_eq!(shown_url("about:blank"), "about:blank");
    }

    #[test]
    fn every_term_must_start_a_word_of_the_address_or_the_title() {
        let history = history(&[
            ("https://vercel.com/strive-x", "StriveX - Vercel", 3, 1, 1),
            (
                "https://vercel.com/blog/introducing-vercel-board",
                "Introducing Vercel Board",
                1,
                0,
                3,
            ),
            ("https://github.com/vercel/next.js", "Next.js", 1, 0, 2),
        ]);
        let urls = |input: &str| -> Vec<String> {
            history
                .matches(input, NOW, 8)
                .into_iter()
                .map(|found| found.shown)
                .collect()
        };
        assert_eq!(urls("vercel")[0], "vercel.com/strive-x");
        assert_eq!(urls("vercel").len(), 3);
        assert_eq!(
            urls("vercel board"),
            ["vercel.com/blog/introducing-vercel-board"]
        );
        assert!(
            urls("ercel").is_empty(),
            "a match inside a word does not count"
        );
        assert_eq!(urls("next"), ["github.com/vercel/next.js"]);
    }

    #[test]
    fn typed_and_recent_pages_rank_above_old_ones() {
        let history = history(&[
            ("https://docs.rs/gpui", "gpui - Docs.rs", 30, 0, 120),
            ("https://docs.rs/serde", "serde - Docs.rs", 2, 2, 1),
        ]);
        let found = history.matches("docs", NOW, 8);
        assert_eq!(found[0].shown, "docs.rs/serde");
    }

    #[test]
    fn inline_completion_follows_the_typed_prefix_and_prefers_the_site_root() {
        let history = history(&[
            ("https://vercel.com/strive-x", "StriveX", 6, 2, 1),
            ("https://vercel.com/", "Vercel", 4, 1, 2),
            ("https://www.verge.com/", "The Verge", 1, 0, 1),
        ]);
        assert_eq!(
            history.inline("verc", NOW),
            Some(Inline {
                url: "https://vercel.com/".to_string(),
                text: "vercel.com".to_string()
            })
        );
        assert_eq!(
            history
                .inline("vercel.com/s", NOW)
                .map(|inline| inline.text),
            Some("vercel.com/strive-x".to_string())
        );
        assert_eq!(
            history.inline("Verc", NOW).map(|inline| inline.text),
            Some("Vercel.com".to_string())
        );
        assert_eq!(
            history.inline("verge", NOW),
            None,
            "seen once and never typed"
        );
        assert_eq!(history.inline("vercel board", NOW), None);
        assert_eq!(
            history.inline("vercel.com", NOW).map(|inline| inline.text),
            Some("vercel.com/strive-x".to_string())
        );
    }

    #[test]
    fn recording_counts_visits_and_typed_entries_and_keeps_the_newest_title() {
        let mut history = History::default();
        history.record("https://vercel.com/", None, true, NOW);
        history.record("https://vercel.com/", Some("Vercel"), false, NOW + 10);
        assert!(history.retitle("https://vercel.com/", "Vercel: Build and deploy"));
        assert!(!history.retitle("https://vercel.com/", "Vercel: Build and deploy"));
        let visit = &history.entries[0];
        assert_eq!((visit.visits, visit.typed, visit.last), (2, 1, NOW + 10));
        assert_eq!(visit.title.as_deref(), Some("Vercel: Build and deploy"));
        assert!(history.remove("https://vercel.com/"));
        assert!(history.entries.is_empty());
    }

    #[test]
    fn clearing_a_time_range_forgets_the_pages_last_visited_in_it() {
        let mut history = History::default();
        history.record("https://old.example/", None, false, NOW - 3 * DAY);
        history.record("https://recent.example/", None, false, NOW - 60);
        assert_eq!((history.len(), history.kept_since(NOW - DAY)), (2, 1));
        assert!(history.clear_since(NOW - DAY));
        assert_eq!(history.len(), 1);
        assert!(!history.clear_since(NOW - DAY));
        assert!(history.clear_since(0));
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn the_history_keeps_its_most_relevant_entries_past_the_cap() {
        let mut history = History::default();
        history.record("https://kept.example/", None, true, NOW);
        for index in 0..MAX_ENTRIES {
            history.record(
                &format!("https://old{index}.example/"),
                None,
                false,
                NOW - 400 * DAY,
            );
        }
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert!(
            history
                .entries
                .iter()
                .any(|visit| visit.url == "https://kept.example/")
        );
    }

    #[test]
    fn matched_words_are_highlighted_as_merged_byte_ranges() {
        let terms = terms("vercel boa");
        assert_eq!(
            highlights("Introducing Vercel Board", &terms),
            [12..18, 19..22]
        );
        assert_eq!(
            highlights("vercel.com/strive-x", &terms),
            [Range { start: 0, end: 6 }]
        );
    }
}
