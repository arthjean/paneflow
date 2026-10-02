use std::collections::BTreeSet;
use std::fmt;

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use toml::Spanned;

pub const SCREEN_ENGINE: u32 = 2;

pub const MAX_REGEX_BYTES: usize = 1 << 20;

pub const MAX_RULES_PER_SOURCE: usize = 256;

pub const MAX_PATTERNS_PER_RULE: usize = 64;

const MAX_REGION_LINES: usize = 10_000;

const PROGRESS_STATES: &[&str] = &["none", "set", "error", "indeterminate", "pause"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenState {
    Working,
    Idle,
    Blocked,
}

impl ScreenState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "working" => Some(Self::Working),
            "idle" => Some(Self::Idle),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    All,
    Last(usize),
    First(usize),
    Title,
}

impl Region {
    fn parse(value: &str) -> Option<Self> {
        let count = |raw: &str| {
            raw.parse::<usize>()
                .ok()
                .filter(|count| (1..=MAX_REGION_LINES).contains(count))
        };
        match value {
            "all" => Some(Self::All),
            "title" => Some(Self::Title),
            _ => {
                if let Some(raw) = value.strip_prefix("last:") {
                    count(raw).map(Self::Last)
                } else if let Some(raw) = value.strip_prefix("first:") {
                    count(raw).map(Self::First)
                } else {
                    None
                }
            }
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => formatter.write_str("all"),
            Self::Last(count) => write!(formatter, "last:{count}"),
            Self::First(count) => write!(formatter, "first:{count}"),
            Self::Title => formatter.write_str("title"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOrigin {
    Builtin,
    Remote(u64),
    Local,
}

impl fmt::Display for RuleOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => formatter.write_str("builtin"),
            Self::Remote(version) => write!(formatter, "remote v{version}"),
            Self::Local => formatter.write_str("local"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScreenRule {
    pub id: String,
    pub state: ScreenState,
    pub priority: i64,
    pub region: Region,
    pub all: Vec<Regex>,
    pub any: Vec<Regex>,
    pub not: Vec<Regex>,
    pub progress: Vec<String>,
    pub visible_blocker: bool,
    pub origin: RuleOrigin,
}

#[derive(Debug, Clone)]
pub enum RuleEntry {
    Rule(ScreenRule),
    Disabled(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for RuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for RuleError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    engine: Spanned<u32>,
    #[serde(default)]
    rules: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: Spanned<String>,
    #[serde(default)]
    disabled: bool,
    state: Option<Spanned<String>>,
    #[serde(default)]
    priority: i64,
    region: Option<Spanned<String>>,
    #[serde(default)]
    all: Vec<Spanned<String>>,
    #[serde(default)]
    any: Vec<Spanned<String>>,
    #[serde(default)]
    not: Vec<Spanned<String>>,
    #[serde(default)]
    progress: Vec<Spanned<String>>,
    #[serde(default)]
    visible_blocker: bool,
}

pub fn parse_rule_file(text: &str, origin: RuleOrigin) -> Result<Vec<RuleEntry>, RuleError> {
    let raw: RawFile = toml::from_str(text).map_err(|error| RuleError {
        line: error.span().map(|span| line_of(text, span.start)),
        message: error.message().trim().to_string(),
    })?;
    if *raw.engine.get_ref() != SCREEN_ENGINE {
        return Err(RuleError {
            line: Some(line_of(text, raw.engine.span().start)),
            message: format!(
                "engine {} is not supported, this build reads engine {SCREEN_ENGINE}",
                raw.engine.get_ref()
            ),
        });
    }
    if let Some(extra) = raw.rules.get(MAX_RULES_PER_SOURCE) {
        return Err(RuleError {
            line: Some(line_of(text, extra.id.span().start)),
            message: format!(
                "a source holds at most {MAX_RULES_PER_SOURCE} rules, this one holds {}",
                raw.rules.len()
            ),
        });
    }
    let mut ids = BTreeSet::new();
    let mut entries = Vec::with_capacity(raw.rules.len());
    for rule in raw.rules {
        let at = |spanned_start: usize| Some(line_of(text, spanned_start));
        let id = rule.id.get_ref().trim().to_string();
        if !is_rule_id(&id) {
            return Err(RuleError {
                line: at(rule.id.span().start),
                message: format!(
                    "rule id {id:?} must be lowercase ASCII letters, digits and dashes"
                ),
            });
        }
        if !ids.insert(id.clone()) {
            return Err(RuleError {
                line: at(rule.id.span().start),
                message: format!("rule id {id:?} is declared twice"),
            });
        }
        if rule.disabled {
            entries.push(RuleEntry::Disabled(id));
            continue;
        }
        let state = rule.state.as_ref().ok_or_else(|| RuleError {
            line: at(rule.id.span().start),
            message: format!("rule {id:?} has no state (working, idle or blocked)"),
        })?;
        let state_value = ScreenState::parse(state.get_ref()).ok_or_else(|| RuleError {
            line: at(state.span().start),
            message: format!(
                "rule {id:?} has the unknown state {:?} (working, idle or blocked)",
                state.get_ref()
            ),
        })?;
        let region = match rule.region.as_ref() {
            None => Region::All,
            Some(region) => Region::parse(region.get_ref()).ok_or_else(|| RuleError {
                line: at(region.span().start),
                message: format!(
                    "rule {id:?} has the unknown region {:?} (all, title, last:N or first:N)",
                    region.get_ref()
                ),
            })?,
        };
        for progress in &rule.progress {
            if !PROGRESS_STATES.contains(&progress.get_ref().as_str()) {
                return Err(RuleError {
                    line: at(progress.span().start),
                    message: format!(
                        "rule {id:?} has the unknown progress {:?} ({})",
                        progress.get_ref(),
                        PROGRESS_STATES.join(", ")
                    ),
                });
            }
        }
        if rule.all.is_empty() && rule.any.is_empty() && rule.progress.is_empty() {
            return Err(RuleError {
                line: at(rule.id.span().start),
                message: format!("rule {id:?} needs an `all`, `any` or `progress` condition"),
            });
        }
        if rule.visible_blocker && state_value != ScreenState::Blocked {
            return Err(RuleError {
                line: at(rule.id.span().start),
                message: format!("rule {id:?} is a visible_blocker, so its state must be blocked"),
            });
        }
        entries.push(RuleEntry::Rule(ScreenRule {
            state: state_value,
            priority: rule.priority,
            region,
            all: compile_all(text, &id, &rule.all)?,
            any: compile_all(text, &id, &rule.any)?,
            not: compile_all(text, &id, &rule.not)?,
            progress: rule
                .progress
                .iter()
                .map(|progress| progress.get_ref().clone())
                .collect(),
            visible_blocker: rule.visible_blocker,
            origin,
            id,
        }));
    }
    Ok(entries)
}

pub fn parse_base_rules(text: &str, origin: RuleOrigin) -> Result<Vec<ScreenRule>, RuleError> {
    Ok(parse_rule_file(text, origin)?
        .into_iter()
        .filter_map(|entry| match entry {
            RuleEntry::Rule(rule) => Some(rule),
            RuleEntry::Disabled(_) => None,
        })
        .collect())
}

pub fn apply_overrides(base: &[ScreenRule], overrides: Vec<RuleEntry>) -> Vec<ScreenRule> {
    let mut rules = base.to_vec();
    for entry in overrides {
        match entry {
            RuleEntry::Disabled(id) => rules.retain(|rule| rule.id != id),
            RuleEntry::Rule(rule) => match rules.iter_mut().find(|held| held.id == rule.id) {
                Some(held) => *held = rule,
                None => rules.push(rule),
            },
        }
    }
    rules
}

fn compile_all(
    text: &str,
    id: &str,
    patterns: &[Spanned<String>],
) -> Result<Vec<Regex>, RuleError> {
    if let Some(extra) = patterns.get(MAX_PATTERNS_PER_RULE) {
        return Err(RuleError {
            line: Some(line_of(text, extra.span().start)),
            message: format!(
                "rule {id:?} lists more than {MAX_PATTERNS_PER_RULE} patterns in one condition"
            ),
        });
    }
    patterns
        .iter()
        .map(|pattern| {
            RegexBuilder::new(pattern.get_ref())
                .size_limit(MAX_REGEX_BYTES)
                .dfa_size_limit(MAX_REGEX_BYTES)
                .build()
                .map_err(|error| RuleError {
                    line: Some(line_of(text, pattern.span().start)),
                    message: format!("rule {id:?} has an invalid regex: {error}"),
                })
        })
        .collect()
}

fn is_rule_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn line_of(text: &str, offset: usize) -> usize {
    text.get(..offset.min(text.len()))
        .map_or(0, |prefix| prefix.matches('\n').count())
        + 1
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenInput<'a> {
    pub screen: &'a str,
    pub title: Option<&'a str>,
    pub progress: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub matched: Vec<bool>,
    pub winner: Option<usize>,
    pub visible_blocker: Option<usize>,
}

impl Evaluation {
    pub fn state(&self, rules: &[ScreenRule]) -> Option<ScreenState> {
        self.winner
            .and_then(|index| rules.get(index))
            .map(|rule| rule.state)
    }
}

pub fn evaluate(rules: &[ScreenRule], input: &ScreenInput<'_>) -> Evaluation {
    let lines: Vec<&str> = input
        .screen
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let mut regions: Vec<(Region, String)> = Vec::new();
    let mut matched = Vec::with_capacity(rules.len());
    let mut winner: Option<usize> = None;
    let mut visible_blocker = None;
    for (index, rule) in rules.iter().enumerate() {
        let position = match regions
            .iter()
            .position(|(region, _)| *region == rule.region)
        {
            Some(position) => position,
            None => {
                regions.push((rule.region, region_text(rule.region, &lines, input.title)));
                regions.len() - 1
            }
        };
        let text = regions[position].1.as_str();
        let progress = input.progress.unwrap_or("none");
        let hit = (rule.progress.is_empty() || rule.progress.iter().any(|held| held == progress))
            && rule.all.iter().all(|pattern| pattern.is_match(text))
            && (rule.any.is_empty() || rule.any.iter().any(|pattern| pattern.is_match(text)))
            && !rule.not.iter().any(|pattern| pattern.is_match(text));
        matched.push(hit);
        if !hit {
            continue;
        }
        if rule.visible_blocker && visible_blocker.is_none() {
            visible_blocker = Some(index);
        }
        if winner.is_none_or(|held: usize| rule.priority > rules[held].priority) {
            winner = Some(index);
        }
    }
    Evaluation {
        matched,
        winner,
        visible_blocker,
    }
}

fn region_text(region: Region, lines: &[&str], title: Option<&str>) -> String {
    match region {
        Region::All => lines.join("\n"),
        Region::Last(count) => lines[lines.len().saturating_sub(count)..].join("\n"),
        Region::First(count) => lines[..count.min(lines.len())].join("\n"),
        Region::Title => title.unwrap_or_default().to_string(),
    }
}
