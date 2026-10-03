#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Caller,
    Pane(u32),
    Window,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TmuxCommand {
    Display {
        target: Target,
        format: Option<String>,
    },
    Accepted,
    ListPanes {
        format: String,
    },
    ListWindows {
        format: String,
    },
    Split {
        target: Target,
        side_by_side: bool,
        print: Option<String>,
        cwd: Option<String>,
        command: Option<String>,
    },
    SelectPane {
        target: Target,
        title: Option<String>,
    },
    Respawn {
        target: Target,
        cwd: Option<String>,
        command: String,
    },
    Kill {
        target: Target,
    },
    SendKeys {
        target: Target,
        literal: bool,
        keys: Vec<String>,
    },
    Unsupported(String),
}

pub(crate) const VERSION_LINE: &str = "tmux 3.5a";
const DEFAULT_PANE_FORMAT: &str = "#{session_name}:#{window_index}.#{pane_index}";
const DEFAULT_LIST_PANES_FORMAT: &str = "#{pane_index}: #{pane_id}";
const DEFAULT_LIST_WINDOWS_FORMAT: &str = "#{window_index}: #{window_name}";

struct Parsed {
    values: Vec<(char, String)>,
    flags: Vec<char>,
    positional: Vec<String>,
}

impl Parsed {
    fn value(&self, flag: char) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(name, _)| *name == flag)
            .map(|(_, value)| value.as_str())
    }

    fn has(&self, flag: char) -> bool {
        self.flags.contains(&flag)
    }

    fn target(&self) -> Target {
        self.value('t').map_or(Target::Caller, parse_target)
    }

    fn command(&self) -> Option<String> {
        let joined = self.positional.join(" ");
        (!joined.trim().is_empty()).then_some(joined)
    }
}

fn parse_flags(args: &[String], value_flags: &str) -> Result<Parsed, String> {
    let mut parsed = Parsed {
        values: Vec::new(),
        flags: Vec::new(),
        positional: Vec::new(),
    };
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if arg == "--" {
            parsed.positional.extend(args[index..].iter().cloned());
            break;
        }
        let Some(cluster) = arg.strip_prefix('-').filter(|rest| !rest.is_empty()) else {
            parsed.positional.push(arg.clone());
            parsed.positional.extend(args[index..].iter().cloned());
            break;
        };
        for (offset, flag) in cluster.char_indices() {
            if value_flags.contains(flag) {
                let inline = &cluster[offset + flag.len_utf8()..];
                let value = if inline.is_empty() {
                    let Some(next) = args.get(index) else {
                        return Err(format!("-{flag} requires a value"));
                    };
                    index += 1;
                    next.clone()
                } else {
                    inline.to_string()
                };
                parsed.values.push((flag, value));
                break;
            }
            parsed.flags.push(flag);
        }
    }
    Ok(parsed)
}

pub(crate) fn parse_target(raw: &str) -> Target {
    let pane = raw.rsplit('.').next().unwrap_or(raw);
    if let Some(id) = pane.strip_prefix('%') {
        return id
            .parse()
            .map_or_else(|_| Target::Unknown(raw.to_string()), Target::Pane);
    }
    if raw.is_empty() || raw.starts_with('@') || raw.starts_with('$') || !raw.contains('%') {
        return Target::Window;
    }
    Target::Unknown(raw.to_string())
}

pub(crate) fn strip_global_flags(argv: &[String]) -> &[String] {
    let mut rest = argv;
    while let Some(first) = rest.first() {
        match first.as_str() {
            "-S" | "-L" | "-f" => rest = rest.get(2..).unwrap_or(&[]),
            "-2" | "-u" | "-q" => rest = &rest[1..],
            _ => break,
        }
    }
    rest
}

pub(crate) fn is_version_request(argv: &[String]) -> bool {
    matches!(
        strip_global_flags(argv).first().map(String::as_str),
        Some("-V" | "-v")
    )
}

pub(crate) fn parse(argv: &[String]) -> Result<TmuxCommand, String> {
    let argv = strip_global_flags(argv);
    let Some((verb, args)) = argv.split_first() else {
        return Err("missing command".to_string());
    };
    let command = match verb.as_str() {
        "display-message" | "display" => {
            let parsed = parse_flags(args, "tcF")?;
            let format = parsed
                .value('F')
                .map(str::to_string)
                .or_else(|| parsed.command());
            TmuxCommand::Display {
                target: parsed.target(),
                format: parsed.has('p').then(|| format.unwrap_or_default()),
            }
        }
        "show-environment"
        | "showenv"
        | "show-options"
        | "show"
        | "show-window-options"
        | "showw"
        | "set-option"
        | "set"
        | "set-window-option"
        | "setw"
        | "select-layout"
        | "selectl"
        | "resize-pane"
        | "resizep"
        | "has-session"
        | "has" => {
            parse_flags(args, "tFtxyl")?;
            TmuxCommand::Accepted
        }
        "list-panes" | "lsp" => {
            let parsed = parse_flags(args, "tF")?;
            TmuxCommand::ListPanes {
                format: parsed
                    .value('F')
                    .unwrap_or(DEFAULT_LIST_PANES_FORMAT)
                    .to_string(),
            }
        }
        "list-windows" | "lsw" => {
            let parsed = parse_flags(args, "tF")?;
            TmuxCommand::ListWindows {
                format: parsed
                    .value('F')
                    .unwrap_or(DEFAULT_LIST_WINDOWS_FORMAT)
                    .to_string(),
            }
        }
        "split-window" | "splitw" | "new-window" | "neww" | "new-session" | "new" => {
            let parsed = parse_flags(args, "tlcFnsexye")?;
            TmuxCommand::Split {
                target: parsed.target(),
                side_by_side: parsed.has('h'),
                print: parsed
                    .has('P')
                    .then(|| parsed.value('F').unwrap_or(DEFAULT_PANE_FORMAT).to_string()),
                cwd: parsed.value('c').map(str::to_string),
                command: parsed.command(),
            }
        }
        "select-pane" | "selectp" => {
            let parsed = parse_flags(args, "tT")?;
            TmuxCommand::SelectPane {
                target: parsed.target(),
                title: parsed.value('T').map(str::to_string),
            }
        }
        "respawn-pane" | "respawnp" => {
            let parsed = parse_flags(args, "tce")?;
            if !parsed.has('k') {
                return Err("respawn-pane needs -k in a Paneflow team".to_string());
            }
            TmuxCommand::Respawn {
                target: parsed.target(),
                cwd: parsed.value('c').map(str::to_string),
                command: parsed
                    .command()
                    .ok_or_else(|| "respawn-pane needs a command".to_string())?,
            }
        }
        "kill-pane" | "killp" => TmuxCommand::Kill {
            target: parse_flags(args, "t")?.target(),
        },
        "send-keys" | "send" => {
            let parsed = parse_flags(args, "tN")?;
            TmuxCommand::SendKeys {
                target: parsed.target(),
                literal: parsed.has('l'),
                keys: parsed.positional,
            }
        }
        other => TmuxCommand::Unsupported(other.to_string()),
    };
    Ok(command)
}

pub(crate) fn unsupported_verb_message(verb: &str) -> String {
    format!("paneflow tmux-compat: unsupported verb: {verb}")
}

pub(crate) struct PaneContext<'a> {
    pub(crate) local: u32,
    pub(crate) title: &'a str,
    pub(crate) active: bool,
}

pub(crate) fn expand_format(format: &str, pane: &PaneContext<'_>) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(start) = rest.find('#') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(stripped) = after.strip_prefix('#') {
            out.push('#');
            rest = stripped;
        } else if let Some(body) = after.strip_prefix('{')
            && let Some(end) = body.find('}')
        {
            out.push_str(&format_variable(&body[..end], pane));
            rest = &body[end + 1..];
        } else if let Some(short) = after.chars().next() {
            out.push_str(&format_short(short, pane));
            rest = &after[short.len_utf8()..];
        } else {
            out.push('#');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn format_short(short: char, pane: &PaneContext<'_>) -> String {
    let name = match short {
        'D' => "pane_id",
        'P' => "pane_index",
        'T' => "pane_title",
        'I' => "window_index",
        'W' => "window_name",
        'S' => "session_name",
        _ => return format!("#{short}"),
    };
    format_variable(name, pane)
}

fn format_variable(name: &str, pane: &PaneContext<'_>) -> String {
    match name {
        "pane_id" => format!("%{}", pane.local),
        "pane_index" => pane.local.to_string(),
        "pane_title" => pane.title.to_string(),
        "pane_active" => u8::from(pane.active).to_string(),
        "window_id" => "@0".to_string(),
        "window_index" | "session_attached" => "0".to_string(),
        "window_name" | "session_name" => "paneflow".to_string(),
        "window_active" => "1".to_string(),
        "session_id" => "$0".to_string(),
        _ => String::new(),
    }
}

pub(crate) fn keys_to_text(keys: &[String], literal: bool) -> String {
    if literal {
        return keys.concat();
    }
    keys.iter().map(|key| key_text(key)).collect()
}

fn key_text(key: &str) -> String {
    match key {
        "Enter" | "C-m" | "KPEnter" => "\r".to_string(),
        "Escape" | "Esc" => "\u{1b}".to_string(),
        "Tab" | "C-i" => "\t".to_string(),
        "Space" => " ".to_string(),
        "BSpace" => "\u{7f}".to_string(),
        _ => match key.strip_prefix("C-").map(str::as_bytes) {
            Some([letter]) if letter.is_ascii_alphabetic() => {
                char::from(letter.to_ascii_lowercase() - b'a' + 1).to_string()
            }
            _ => key.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn every_form_in_the_linux_inventory_parses_to_a_supported_command() {
        let inventory = include_str!("../../tests/fixtures/claude-teams-tmux-linux.log");
        let mut forms = 0;
        for line in inventory.lines() {
            let Some(rest) = line.strip_prefix("  argv: ") else {
                continue;
            };
            if rest.trim_start().starts_with("argv:") || rest.contains("exit:") {
                continue;
            }
            let args: Vec<String> = rest.split(' ').map(|word| word.replace('\\', "")).collect();
            let command = parse(&args).unwrap_or_else(|error| panic!("{rest}: {error}"));
            assert!(
                !matches!(command, TmuxCommand::Unsupported(_)),
                "{rest} must be supported"
            );
            forms += 1;
        }
        assert!(forms >= 30, "the fixture lists every observed call");
    }

    #[test]
    fn the_first_teammate_split_parses_its_placement_and_placeholder() {
        let command = parse(&argv(
            "-S /tmp/sock split-window -d -t %0 -h -l 70% -P -F #{pane_id} -- cat",
        ))
        .expect("parses");
        assert_eq!(
            command,
            TmuxCommand::Split {
                target: Target::Pane(0),
                side_by_side: true,
                print: Some("#{pane_id}".to_string()),
                cwd: None,
                command: Some("cat".to_string()),
            }
        );
    }

    #[test]
    fn respawn_keeps_the_whole_shell_command_after_the_separator() {
        let command = parse(&argv(
            "-S /tmp/sock respawn-pane -k -t %1 -- cd /w && env A=1 claude --agent-id x@t",
        ))
        .expect("parses");
        assert_eq!(
            command,
            TmuxCommand::Respawn {
                target: Target::Pane(1),
                cwd: None,
                command: "cd /w && env A=1 claude --agent-id x@t".to_string(),
            }
        );
    }

    #[test]
    fn an_unknown_verb_is_reported_as_unsupported() {
        assert_eq!(
            parse(&argv("attach-session -t x")).expect("parses"),
            TmuxCommand::Unsupported("attach-session".to_string())
        );
        assert_eq!(
            unsupported_verb_message("attach-session"),
            "paneflow tmux-compat: unsupported verb: attach-session"
        );
    }

    #[test]
    fn display_message_prints_only_with_p() {
        assert_eq!(
            parse(&argv("display-message -t %0 -p #{window_id}")).expect("parses"),
            TmuxCommand::Display {
                target: Target::Pane(0),
                format: Some("#{window_id}".to_string())
            }
        );
        assert_eq!(
            parse(&argv("display-message hello")).expect("parses"),
            TmuxCommand::Display {
                target: Target::Caller,
                format: None
            }
        );
    }

    #[test]
    fn version_requests_skip_global_flags() {
        assert!(is_version_request(&argv("-V")));
        assert!(is_version_request(&argv("-L sock -V")));
        assert!(!is_version_request(&argv("list-panes")));
    }

    #[test]
    fn formats_expand_known_variables_and_drop_unknown_ones() {
        let pane = PaneContext {
            local: 3,
            title: "teammate-2",
            active: false,
        };
        assert_eq!(expand_format("#{pane_id}", &pane), "%3");
        assert_eq!(
            expand_format("#{window_id} #{pane_title} #D ## #{client_termtype}", &pane),
            "@0 teammate-2 %3 # "
        );
    }

    #[test]
    fn targets_map_panes_windows_and_sessions() {
        assert_eq!(parse_target("%4"), Target::Pane(4));
        assert_eq!(parse_target("@0"), Target::Window);
        assert_eq!(parse_target("claude-swarm:swarm-view"), Target::Window);
        assert_eq!(parse_target("claude-swarm:0.%2"), Target::Pane(2));
        assert_eq!(parse_target("%x"), Target::Unknown("%x".to_string()));
    }

    #[test]
    fn send_keys_translates_key_names_unless_literal() {
        let keys = argv("echo Space hi Enter C-c");
        assert_eq!(keys_to_text(&keys, false), "echo hi\r\u{3}");
        assert_eq!(keys_to_text(&argv("Enter"), true), "Enter");
    }

    #[test]
    fn a_missing_flag_value_is_an_error() {
        assert!(parse(&argv("split-window -t")).is_err());
        assert!(parse(&argv("respawn-pane -t %1 -- claude")).is_err());
    }
}
