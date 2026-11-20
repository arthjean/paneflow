use gpui::{Keystroke, OperatingSystem};

use super::apply::{canonical_keystroke, host_os};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChordFit {
    Exact,
    Within,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Held {
    control: bool,
    alt: bool,
    shift: bool,
    platform: bool,
}

impl Held {
    fn of(keystroke: &Keystroke) -> Self {
        Self {
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
            platform: keystroke.modifiers.platform,
        }
    }

    fn within(self, other: Self) -> bool {
        (!self.control || other.control)
            && (!self.alt || other.alt)
            && (!self.shift || other.shift)
            && (!self.platform || other.platform)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChordPattern {
    held: Held,
    command: bool,
    key: Option<String>,
}

enum Modifier {
    Control,
    Alt,
    Shift,
    Command,
}

fn modifier(name: &str) -> Option<Modifier> {
    match name {
        "ctrl" | "control" | "ctl" | "\u{2303}" => Some(Modifier::Control),
        "alt" | "option" | "opt" | "\u{2325}" => Some(Modifier::Alt),
        "shift" | "\u{21e7}" => Some(Modifier::Shift),
        "cmd" | "command" | "super" | "win" | "windows" | "meta" | "secondary" | "\u{2318}" => {
            Some(Modifier::Command)
        }
        _ => None,
    }
}

fn key_name(name: &str) -> Option<String> {
    let canonical = match name {
        "" => return None,
        "esc" => "escape",
        "return" | "\u{21b5}" | "\u{23ce}" | "\u{21a9}" => "enter",
        "del" | "\u{2326}" => "delete",
        "bksp" | "\u{232b}" => "backspace",
        "pgup" | "page up" => "pageup",
        "pgdn" | "page down" => "pagedown",
        "arrowup" | "\u{2191}" => "up",
        "arrowdown" | "\u{2193}" => "down",
        "arrowleft" | "\u{2190}" => "left",
        "arrowright" | "\u{2192}" => "right",
        "\u{21e5}" => "tab",
        "ins" => "insert",
        "plus" => "+",
        "minus" => "-",
        "equal" | "equals" => "=",
        other => other,
    };
    Some(canonical.to_string())
}

fn split_parts(text: &str) -> Option<(Vec<&str>, &str)> {
    let separator = if text.contains('+') { '+' } else { '-' };
    let doubled = format!("{separator}{separator}");
    let (head, key) = match text.strip_suffix(doubled.as_str()) {
        Some(head) => (head, &text[head.len() + 1..]),
        None => match text.rsplit_once(separator) {
            Some((head, key)) => (head, key),
            None => return None,
        },
    };
    let parts = head
        .split(separator)
        .filter(|part| !part.is_empty())
        .collect();
    Some((parts, key))
}

impl ChordPattern {
    pub fn parse(token: &str) -> Option<Self> {
        let lowered = token.trim().to_lowercase();
        let mut pattern = Self::default();
        let mut rest = lowered.as_str();
        let mut symbols = false;
        while let Some(first) = rest.chars().next() {
            let Some(held) = modifier(&first.to_string()) else {
                break;
            };
            pattern.hold(held);
            symbols = true;
            rest = &rest[first.len_utf8()..];
        }
        let rest = if symbols {
            rest.strip_prefix('+').unwrap_or(rest)
        } else {
            rest
        };

        let key = match split_parts(rest) {
            Some((parts, key)) if !parts.is_empty() || symbols => {
                for part in parts {
                    pattern.hold(modifier(part)?);
                }
                key
            }
            _ if symbols => rest,
            _ => return None,
        };
        match modifier(key) {
            Some(held) => pattern.hold(held),
            None => pattern.key = key_name(key),
        }
        pattern.holds_any().then_some(pattern)
    }

    fn hold(&mut self, modifier: Modifier) {
        match modifier {
            Modifier::Control => self.held.control = true,
            Modifier::Alt => self.held.alt = true,
            Modifier::Shift => self.held.shift = true,
            Modifier::Command => self.command = true,
        }
    }

    fn holds_any(&self) -> bool {
        self.command || self.held != Held::default()
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    fn readings(&self, os: OperatingSystem) -> Vec<Held> {
        if !self.command {
            return vec![self.held];
        }
        let platform = Held {
            platform: true,
            ..self.held
        };
        if os == OperatingSystem::Mac {
            vec![platform]
        } else {
            vec![
                Held {
                    control: true,
                    ..self.held
                },
                platform,
            ]
        }
    }

    pub fn fit_on(&self, keystroke: &Keystroke, os: OperatingSystem) -> Option<ChordFit> {
        if self
            .key
            .as_deref()
            .is_some_and(|key| !keystroke.key.eq_ignore_ascii_case(key))
        {
            return None;
        }
        let held = Held::of(keystroke);
        self.readings(os)
            .into_iter()
            .filter_map(|reading| {
                if reading == held {
                    Some(ChordFit::Exact)
                } else if reading.within(held) {
                    Some(ChordFit::Within)
                } else {
                    None
                }
            })
            .min()
    }

    pub fn fit_raw(&self, raw: &str) -> Option<ChordFit> {
        canonical_keystroke(raw).and_then(|keystroke| self.fit_on(&keystroke, host_os()))
    }

    pub fn raw(&self) -> Option<String> {
        let key = self.key.as_deref()?;
        let mut parts = Vec::new();
        if self.held.control {
            parts.push("ctrl");
        }
        if self.held.alt {
            parts.push("alt");
        }
        if self.held.shift {
            parts.push("shift");
        }
        if self.command {
            parts.push("secondary");
        }
        parts.push(key);
        Some(parts.join("-"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(source: &str, os: OperatingSystem) -> Keystroke {
        Keystroke::parse_with_os(source, os).expect("a valid keystroke")
    }

    fn fit(query: &str, binding: &str, os: OperatingSystem) -> Option<ChordFit> {
        ChordPattern::parse(query)
            .expect("a chord")
            .fit_on(&keystroke(binding, os), os)
    }

    #[test]
    fn plain_words_are_not_chords() {
        for word in ["split", "shift", "f11", "c++", "tab", "", "-", "zoom-in"] {
            assert_eq!(ChordPattern::parse(word), None, "{word}");
        }
    }

    #[test]
    fn modifier_order_and_case_do_not_matter() {
        let linux = OperatingSystem::Linux;
        assert_eq!(
            fit("ctrl+shift+d", "ctrl-shift-d", linux),
            Some(ChordFit::Exact)
        );
        assert_eq!(
            fit("Shift+Ctrl+D", "ctrl-shift-d", linux),
            Some(ChordFit::Exact)
        );
        assert_eq!(
            fit("control-shift-d", "ctrl-shift-d", linux),
            Some(ChordFit::Exact)
        );
    }

    #[test]
    fn the_displayed_form_parses_back_on_every_platform() {
        let mac = OperatingSystem::Mac;
        assert_eq!(
            fit("\u{2318}\u{21e7}D", "cmd-shift-d", mac),
            Some(ChordFit::Exact)
        );
        assert_eq!(
            fit("\u{2325}PageUp", "alt-pageup", mac),
            Some(ChordFit::Exact)
        );
        let linux = OperatingSystem::Linux;
        assert_eq!(
            fit("Ctrl+Alt+PageDown", "ctrl-alt-pagedown", linux),
            Some(ChordFit::Exact)
        );
        assert_eq!(fit("Super+L", "super-l", linux), Some(ChordFit::Exact));
    }

    #[test]
    fn command_means_the_platform_key_on_mac_and_either_key_elsewhere() {
        assert_eq!(
            fit("cmd+k", "cmd-k", OperatingSystem::Mac),
            Some(ChordFit::Exact)
        );
        assert_eq!(fit("cmd+k", "ctrl-k", OperatingSystem::Mac), None);
        assert_eq!(
            fit("cmd+k", "ctrl-k", OperatingSystem::Linux),
            Some(ChordFit::Exact)
        );
        assert_eq!(
            fit("cmd+k", "super-k", OperatingSystem::Windows),
            Some(ChordFit::Exact)
        );
    }

    #[test]
    fn fewer_modifiers_match_within_a_larger_chord() {
        let linux = OperatingSystem::Linux;
        assert_eq!(fit("ctrl+d", "ctrl-shift-d", linux), Some(ChordFit::Within));
        assert_eq!(fit("ctrl+shift+d", "ctrl-d", linux), None);
        assert_eq!(fit("ctrl+d", "ctrl-e", linux), None);
    }

    #[test]
    fn modifiers_alone_find_every_chord_that_holds_them() {
        let linux = OperatingSystem::Linux;
        let pattern = ChordPattern::parse("ctrl+alt").expect("a chord");
        assert!(!pattern.has_key());
        assert_eq!(fit("ctrl+alt", "ctrl-alt-1", linux), Some(ChordFit::Exact));
        assert_eq!(fit("ctrl+alt+", "ctrl-alt-1", linux), Some(ChordFit::Exact));
        assert_eq!(fit("ctrl+", "ctrl-alt-1", linux), Some(ChordFit::Within));
        assert_eq!(fit("ctrl+alt", "ctrl-1", linux), None);
    }

    #[test]
    fn symbol_keys_survive_their_separator() {
        let linux = OperatingSystem::Linux;
        assert_eq!(fit("ctrl+-", "ctrl--", linux), Some(ChordFit::Exact));
        assert_eq!(fit("ctrl--", "ctrl--", linux), Some(ChordFit::Exact));
        assert_eq!(
            fit("ctrl+shift+=", "ctrl-shift-=", linux),
            Some(ChordFit::Exact)
        );
        assert_eq!(fit("ctrl+minus", "ctrl--", linux), Some(ChordFit::Exact));
    }

    #[test]
    fn key_aliases_reach_the_names_gpui_uses() {
        let linux = OperatingSystem::Linux;
        assert_eq!(
            fit("ctrl+pgdn", "ctrl-pagedown", linux),
            Some(ChordFit::Exact)
        );
        assert_eq!(
            fit("alt+\u{2190}", "alt-left", linux),
            Some(ChordFit::Exact)
        );
        assert_eq!(
            fit("shift+esc", "shift-escape", linux),
            Some(ChordFit::Exact)
        );
    }

    #[test]
    fn a_complete_pattern_writes_back_as_a_raw_chord() {
        let pattern = ChordPattern::parse("Shift+Cmd+K").expect("a chord");
        assert_eq!(pattern.raw().as_deref(), Some("shift-secondary-k"));
        assert_eq!(
            ChordPattern::parse("ctrl+alt").expect("a chord").raw(),
            None
        );
    }
}
