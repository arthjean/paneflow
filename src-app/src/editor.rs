use std::path::{Path, PathBuf};
use std::process::Command;

use paneflow_process::spawn_detached;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorKind {
    VsCodeLike,
    Zed,
    Sublime,
    VimFamily,
    Helix,
    Emacs,
    Unknown,
}

impl EditorKind {
    fn from_binary_name(name: &str) -> Self {
        let base = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name)
            .to_ascii_lowercase();
        match base.as_str() {
            "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => Self::VsCodeLike,
            "zed" | "zed-preview" | "zed-nightly" => Self::Zed,
            "subl" | "sublime_text" => Self::Sublime,
            "nvim" | "vim" | "vi" | "nvim-qt" | "gvim" | "mvim" => Self::VimFamily,
            "hx" | "helix" => Self::Helix,
            "emacs" | "emacsclient" => Self::Emacs,
            _ => Self::Unknown,
        }
    }

    fn argv_for(self, path: &Path, line: Option<u32>, col: Option<u32>) -> Vec<String> {
        let path_str = path.to_string_lossy().into_owned();
        match self {
            Self::VsCodeLike => {
                let mut args = vec!["-g".to_string()];
                args.push(format_path_line_col(&path_str, line, col));
                args
            }
            Self::Zed | Self::Sublime | Self::Helix => {
                vec![format_path_line_col(&path_str, line, col)]
            }
            Self::VimFamily => {
                let mut args = Vec::new();
                if let Some(l) = line {
                    args.push(format!("+{l}"));
                }
                args.push(path_str);
                args
            }
            Self::Emacs => {
                let mut args = Vec::new();
                if let Some(l) = line {
                    let token = match col {
                        Some(c) => format!("+{l}:{c}"),
                        None => format!("+{l}"),
                    };
                    args.push(token);
                }
                args.push(path_str);
                args
            }
            Self::Unknown => vec![path_str],
        }
    }
}

fn format_path_line_col(path: &str, line: Option<u32>, col: Option<u32>) -> String {
    match (line, col) {
        (Some(l), Some(c)) => format!("{path}:{l}:{c}"),
        (Some(l), None) => format!("{path}:{l}"),
        (None, _) => path.to_string(),
    }
}

fn parse_env_editor(value: &str) -> Option<(String, Vec<String>)> {
    let mut parts = split_editor_command_line(value).into_iter();
    let bin = parts.next()?;
    if bin.is_empty() {
        return None;
    }
    Some((bin, parts.collect()))
}

fn split_editor_command_line(value: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    let mut quote: Option<char> = None;
    let mut token_started = false;

    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' if matches!(chars.peek(), Some('"') | Some('\\')) => {
                    current.push(chars.next().expect("peeked char exists"));
                }
                _ => current.push(ch),
            },
            Some(_) => unreachable!("only single and double quotes are set"),
            None if ch.is_whitespace() => {
                if token_started {
                    args.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            None if matches!(ch, '\'' | '"') => {
                quote = Some(ch);
                token_started = true;
            }
            None => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if token_started {
        args.push(current);
    }
    args
}

fn resolve_editor_command(command: &str) -> PathBuf {
    let path = Path::new(command);
    if path.is_absolute() || path.components().count() > 1 {
        PathBuf::from(command)
    } else {
        crate::app::workspace_ops::resolve_editor_binary(command)
    }
}

const FALLBACK_PROBES: &[&str] = &[
    "code",
    "cursor",
    "zed",
    "subl",
    "code-insiders",
    "windsurf",
    "hx",
    "nvim",
    "vim",
    "emacs",
];

pub fn open_at_location(path: &Path, line: Option<u32>, col: Option<u32>) -> bool {
    for var in &["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(var)
            && let Some((bin, extra_args)) = parse_env_editor(&value)
        {
            let kind = EditorKind::from_binary_name(&bin);
            let mut args = extra_args;
            args.extend(kind.argv_for(path, line, col));
            let resolved = resolve_editor_command(&bin);
            if try_spawn(&resolved.to_string_lossy(), &args) {
                return true;
            }
            log::warn!("editor: ${var}={value:?} failed to spawn - falling through");
        }
    }

    for probe in FALLBACK_PROBES {
        let found = resolve_editor_command(probe);
        if found == PathBuf::from(probe) {
            continue;
        }
        let kind = EditorKind::from_binary_name(probe);
        let args = kind.argv_for(path, line, col);
        if try_spawn(&found.to_string_lossy(), &args) {
            return true;
        }
    }

    log::warn!(
        "editor: no $VISUAL/$EDITOR and none of {:?} on PATH - falling back to OS handler",
        FALLBACK_PROBES
    );
    open::that(path).is_ok()
}

fn try_spawn(bin: &str, args: &[String]) -> bool {
    match spawn_detached(Command::new(bin).args(args)) {
        Ok(()) => {
            log::info!("editor: spawned {bin} {args:?}");
            true
        }
        Err(e) => {
            log::warn!("editor: spawn {bin} {args:?} failed: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn editor_kind_recognises_vscode_family() {
        assert_eq!(EditorKind::from_binary_name("code"), EditorKind::VsCodeLike);
        assert_eq!(
            EditorKind::from_binary_name("cursor"),
            EditorKind::VsCodeLike
        );
        assert_eq!(
            EditorKind::from_binary_name("/usr/bin/code"),
            EditorKind::VsCodeLike
        );
        assert_eq!(
            EditorKind::from_binary_name("code.cmd"),
            EditorKind::VsCodeLike
        );
    }

    #[test]
    fn editor_kind_recognises_zed_vim_helix_emacs() {
        assert_eq!(EditorKind::from_binary_name("zed"), EditorKind::Zed);
        assert_eq!(EditorKind::from_binary_name("nvim"), EditorKind::VimFamily);
        assert_eq!(EditorKind::from_binary_name("vim"), EditorKind::VimFamily);
        assert_eq!(EditorKind::from_binary_name("hx"), EditorKind::Helix);
        assert_eq!(EditorKind::from_binary_name("emacs"), EditorKind::Emacs);
        assert_eq!(
            EditorKind::from_binary_name("emacsclient"),
            EditorKind::Emacs
        );
    }

    #[test]
    fn editor_kind_unknown_falls_back() {
        assert_eq!(
            EditorKind::from_binary_name("my-weird-editor"),
            EditorKind::Unknown
        );
        assert_eq!(EditorKind::from_binary_name(""), EditorKind::Unknown);
    }

    #[test]
    fn argv_vscode_uses_g_flag() {
        let args = EditorKind::VsCodeLike.argv_for(p("/tmp/x.rs"), Some(42), Some(7));
        assert_eq!(args, vec!["-g".to_string(), "/tmp/x.rs:42:7".to_string()]);
    }

    #[test]
    fn argv_vim_uses_plus_line_no_col() {
        let args = EditorKind::VimFamily.argv_for(p("/tmp/x.rs"), Some(42), Some(7));
        assert_eq!(args, vec!["+42".to_string(), "/tmp/x.rs".to_string()]);
    }

    #[test]
    fn argv_emacs_uses_plus_line_col() {
        let args = EditorKind::Emacs.argv_for(p("/tmp/x.rs"), Some(42), Some(7));
        assert_eq!(args, vec!["+42:7".to_string(), "/tmp/x.rs".to_string()]);
    }

    #[test]
    fn argv_zed_bare_path_colon_line() {
        let args = EditorKind::Zed.argv_for(p("/tmp/x.rs"), Some(42), None);
        assert_eq!(args, vec!["/tmp/x.rs:42".to_string()]);
    }

    #[test]
    fn argv_unknown_drops_location() {
        let args = EditorKind::Unknown.argv_for(p("/tmp/x.rs"), Some(42), Some(7));
        assert_eq!(args, vec!["/tmp/x.rs".to_string()]);
    }

    #[test]
    fn argv_no_line_no_col_just_path() {
        let args = EditorKind::VsCodeLike.argv_for(p("/tmp/x.rs"), None, None);
        assert_eq!(args, vec!["-g".to_string(), "/tmp/x.rs".to_string()]);
    }

    #[test]
    fn parse_env_editor_splits_binary_and_flags() {
        let (bin, args) = parse_env_editor("code --wait").unwrap();
        assert_eq!(bin, "code");
        assert_eq!(args, vec!["--wait".to_string()]);
    }

    #[test]
    fn parse_env_editor_preserves_quoted_windows_binary() {
        let (bin, args) =
            parse_env_editor(r#""C:\Program Files\Microsoft VS Code\bin\code.cmd" --wait"#)
                .unwrap();
        assert_eq!(bin, r"C:\Program Files\Microsoft VS Code\bin\code.cmd");
        assert_eq!(args, vec!["--wait".to_string()]);
    }

    #[test]
    fn parse_env_editor_preserves_quoted_flag_value() {
        let (bin, args) = parse_env_editor(r#"code --profile "Arthur Dev""#).unwrap();
        assert_eq!(bin, "code");
        assert_eq!(
            args,
            vec!["--profile".to_string(), "Arthur Dev".to_string()]
        );
    }

    #[test]
    fn parse_env_editor_empty_is_none() {
        assert!(parse_env_editor("").is_none());
        assert!(parse_env_editor("   ").is_none());
    }

    #[test]
    fn parse_env_editor_only_binary() {
        let (bin, args) = parse_env_editor("nvim").unwrap();
        assert_eq!(bin, "nvim");
        assert!(args.is_empty());
    }

    #[test]
    fn format_path_line_col_combinations() {
        assert_eq!(format_path_line_col("x.rs", None, None), "x.rs");
        assert_eq!(format_path_line_col("x.rs", Some(1), None), "x.rs:1");
        assert_eq!(format_path_line_col("x.rs", Some(1), Some(2)), "x.rs:1:2");
        assert_eq!(format_path_line_col("x.rs", None, Some(7)), "x.rs");
    }
}
