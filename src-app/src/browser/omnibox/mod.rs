mod history;
mod store;

use std::ops::Range;

use paneflow_config::schema::WebSearch;

pub(crate) use history::{History, Inline, shown_url};
pub(crate) use store::{
    clear_since, forget, history_snapshot, loaded, now, record_visit, retitle, start,
};

use super::address::{self, Address};

pub(crate) const MAX_ROWS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevServer {
    pub(crate) port: u16,
    pub(crate) label: Option<String>,
}

impl DevServer {
    pub(crate) fn url(&self) -> String {
        format!("http://localhost:{}/", self.port)
    }

    fn matches(&self, terms: &[String]) -> bool {
        let port = self.port.to_string();
        let label = self.label.as_deref().unwrap_or_default().to_lowercase();
        let host = format!("localhost:{port}");
        terms.iter().all(|term| {
            host.starts_with(term.as_str())
                || "127.0.0.1".starts_with(term.as_str())
                || port.starts_with(term.as_str())
                || label
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|word| !word.is_empty() && word.starts_with(term.as_str()))
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RowKind {
    Typed,
    Page { title: Option<String> },
    DevServer { label: Option<String> },
    Search { query: String, engine: String },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub(crate) kind: RowKind,
    pub(crate) url: String,
    pub(crate) shown: String,
    pub(crate) title_marks: Vec<Range<usize>>,
    pub(crate) shown_marks: Vec<Range<usize>>,
}

impl Row {
    pub(crate) fn removable(&self) -> bool {
        matches!(self.kind, RowKind::Page { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Suggestions {
    pub(crate) rows: Vec<Row>,
    pub(crate) inline: Option<Inline>,
}

pub(crate) struct Sources<'a> {
    pub(crate) history: &'a History,
    pub(crate) servers: &'a [DevServer],
    pub(crate) search: &'a WebSearch,
    pub(crate) now: u64,
    pub(crate) allow_inline: bool,
}

pub(crate) fn suggest(typed: &str, sources: &Sources<'_>) -> Suggestions {
    let input = typed.trim();
    if input.is_empty() {
        return Suggestions::default();
    }
    let terms = history::terms(input);
    let inline = sources
        .allow_inline
        .then(|| {
            sources
                .history
                .inline(typed, sources.now)
                .or_else(|| server_inline(typed, sources.servers))
        })
        .flatten();
    let mut rows: Vec<Row> = Vec::new();
    let page = |url: &str, title: Option<String>| -> Row {
        let shown = shown_url(url);
        Row {
            title_marks: title
                .as_deref()
                .map(|title| history::highlights(title, &terms))
                .unwrap_or_default(),
            shown_marks: history::highlights(&shown, &terms),
            kind: RowKind::Page { title },
            url: url.to_string(),
            shown,
        }
    };
    let search = Row {
        kind: RowKind::Search {
            query: input.to_string(),
            engine: sources.search.name.clone(),
        },
        url: sources.search.url(input),
        shown: String::new(),
        title_marks: Vec::new(),
        shown_marks: Vec::new(),
    };
    let matches = sources.history.matches(input, sources.now, MAX_ROWS);
    match (&inline, address::resolve(input)) {
        (Some(inline), _) => {
            match sources
                .servers
                .iter()
                .find(|server| server.url() == inline.url)
            {
                Some(server) => rows.push(server_row(server, &terms)),
                None => {
                    let title = matches
                        .iter()
                        .find(|found| found.url == inline.url)
                        .and_then(|found| found.title.clone());
                    rows.push(page(&inline.url, title));
                }
            }
            rows.push(search);
        }
        (None, Address::Go(url)) => {
            let shown = shown_url(&url);
            rows.push(Row {
                kind: RowKind::Typed,
                shown_marks: Vec::new(),
                title_marks: Vec::new(),
                url,
                shown,
            });
            if !has_scheme(input) {
                rows.push(search);
            }
        }
        (None, Address::Refused(_)) => rows.push(search),
    }
    for server in sources
        .servers
        .iter()
        .filter(|server| server.matches(&terms))
    {
        rows.push(server_row(server, &terms));
    }
    for found in matches {
        rows.push(page(&found.url, found.title));
    }
    let mut seen: Vec<String> = Vec::new();
    rows.retain(|row| {
        let key = shown_url(&row.url).to_lowercase();
        let fresh = !seen.contains(&key);
        seen.push(key);
        fresh
    });
    rows.truncate(MAX_ROWS);
    Suggestions { rows, inline }
}

fn server_row(server: &DevServer, terms: &[String]) -> Row {
    let url = server.url();
    let shown = shown_url(&url);
    Row {
        title_marks: server
            .label
            .as_deref()
            .map(|label| history::highlights(label, terms))
            .unwrap_or_default(),
        shown_marks: history::highlights(&shown, terms),
        kind: RowKind::DevServer {
            label: server.label.clone(),
        },
        url,
        shown,
    }
}

fn server_inline(typed: &str, servers: &[DevServer]) -> Option<Inline> {
    let lower = typed.to_lowercase();
    if lower.is_empty() || lower.len() != typed.len() || lower.contains(char::is_whitespace) {
        return None;
    }
    servers.iter().find_map(|server| {
        let host = format!("localhost:{}", server.port);
        (host.len() > lower.len() && host.starts_with(&lower)).then(|| Inline {
            url: server.url(),
            text: format!("{typed}{}", &host[lower.len()..]),
        })
    })
}

fn has_scheme(input: &str) -> bool {
    input.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphabetic())
    }) || input.starts_with("about:")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn duckduckgo() -> WebSearch {
        paneflow_config::schema::BrowserConfig::default().web_search()
    }

    fn visited() -> History {
        let mut history = History::default();
        for _ in 0..3 {
            history.record(
                "https://vercel.com/strive-x",
                Some("StriveX - Vercel"),
                true,
                NOW,
            );
        }
        history.record(
            "https://vercel.com/blog/introducing-vercel-board",
            Some("Introducing Vercel Board"),
            false,
            NOW,
        );
        history
    }

    fn rows(typed: &str, history: &History, servers: &[DevServer]) -> Suggestions {
        suggest(
            typed,
            &Sources {
                history,
                servers,
                search: &duckduckgo(),
                now: NOW,
                allow_inline: true,
            },
        )
    }

    fn kinds(suggestions: &Suggestions) -> Vec<String> {
        suggestions
            .rows
            .iter()
            .map(|row| match &row.kind {
                RowKind::Typed => format!("typed {}", row.shown),
                RowKind::Page { .. } => format!("page {}", row.shown),
                RowKind::DevServer { .. } => format!("server {}", row.shown),
                RowKind::Search { query, .. } => format!("search {query}"),
            })
            .collect()
    }

    #[test]
    fn a_known_prefix_completes_inline_and_opens_the_completed_page_first() {
        let history = visited();
        let suggestions = rows("verc", &history, &[]);
        assert_eq!(
            suggestions
                .inline
                .as_ref()
                .map(|inline| inline.text.as_str()),
            Some("vercel.com/strive-x")
        );
        assert_eq!(
            kinds(&suggestions),
            [
                "page vercel.com/strive-x",
                "search verc",
                "page vercel.com/blog/introducing-vercel-board"
            ]
        );
    }

    #[test]
    fn a_word_that_is_no_address_searches_first_and_lists_matching_pages() {
        let history = visited();
        let suggestions = suggest(
            "vercel",
            &Sources {
                history: &history,
                servers: &[],
                search: &duckduckgo(),
                now: NOW,
                allow_inline: false,
            },
        );
        assert_eq!(
            kinds(&suggestions),
            [
                "search vercel",
                "page vercel.com/strive-x",
                "page vercel.com/blog/introducing-vercel-board",
            ]
        );
        assert_eq!(suggestions.rows[0].url, "https://duckduckgo.com/?q=vercel");
        assert_eq!(
            suggestions.rows[2].title_marks,
            [Range { start: 12, end: 18 }]
        );
    }

    #[test]
    fn an_address_is_opened_as_typed_with_a_search_offered_after_it() {
        let history = History::default();
        let suggestions = rows("example.com/docs", &history, &[]);
        assert_eq!(
            kinds(&suggestions),
            ["typed example.com/docs", "search example.com/docs"]
        );
        assert_eq!(suggestions.rows[0].url, "https://example.com/docs");
        let with_scheme = rows("https://example.com", &history, &[]);
        assert_eq!(kinds(&with_scheme), ["typed example.com"]);
    }

    #[test]
    fn a_running_dev_server_is_offered_by_port_host_or_framework() {
        let history = History::default();
        let servers = [
            DevServer {
                port: 5173,
                label: Some("Vite".to_string()),
            },
            DevServer {
                port: 3000,
                label: None,
            },
        ];
        assert_eq!(
            kinds(&rows("vite", &history, &servers)),
            ["search vite", "server localhost:5173"]
        );
        assert_eq!(
            kinds(&rows("local", &history, &servers)),
            [
                "server localhost:5173",
                "search local",
                "server localhost:3000"
            ]
        );
        assert_eq!(
            rows("local", &history, &servers)
                .inline
                .map(|inline| inline.text),
            Some("localhost:5173".to_string()),
            "a running server completes like a typed site"
        );
        assert_eq!(
            kinds(&rows("3000", &history, &servers)),
            ["search 3000", "server localhost:3000"]
        );
    }

    #[test]
    fn nothing_is_suggested_for_blank_input_and_rows_never_repeat_an_address() {
        let history = visited();
        assert!(rows("  ", &history, &[]).rows.is_empty());
        let suggestions = rows("vercel.com/strive-x", &history, &[]);
        let addresses: Vec<&str> = suggestions
            .rows
            .iter()
            .map(|row| row.shown.as_str())
            .collect();
        assert_eq!(
            addresses
                .iter()
                .filter(|shown| **shown == "vercel.com/strive-x")
                .count(),
            1
        );
    }
}
