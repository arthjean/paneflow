use std::path::Path;

const HOOK_PROGRAM: &str = "paneflow-ai-hook";

pub fn display_hook_program(path: &Path) -> String {
    let rendered = path.display().to_string();
    #[cfg(windows)]
    {
        rendered.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        rendered
    }
}

pub fn shell_program_path(path: &Path) -> String {
    let rendered = display_hook_program(path);
    if rendered.chars().any(|character| {
        character.is_whitespace()
            || matches!(character, '\'' | '"' | '\\' | '$' | '`' | ';' | '&' | '|')
    }) {
        format!("'{}'", rendered.replace('\'', "'\\''"))
    } else {
        rendered
    }
}

pub fn render_hook_command(path: &Path, event: &str) -> String {
    #[cfg(windows)]
    {
        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& {} {event}\"",
            powershell_single_quoted(&display_hook_program(path))
        )
    }
    #[cfg(not(windows))]
    {
        format!("{} {event}", shell_program_path(path))
    }
}

#[cfg(windows)]
fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn is_paneflow_hook_command(command: &str) -> bool {
    paneflow_hook_program_token(command).is_some()
}

pub fn paneflow_hook_program_token(command: &str) -> Option<String> {
    if let Some(program) = command_program_token(command) {
        if is_paneflow_hook_program(&program) {
            return Some(program);
        }
    }
    powershell_invoked_program(command).filter(|program| is_paneflow_hook_program(program))
}

fn is_paneflow_hook_program(program: &str) -> bool {
    let basename = program.rsplit(['/', '\\']).next().unwrap_or(program);
    basename == HOOK_PROGRAM || basename == "paneflow-ai-hook.exe"
}

fn powershell_invoked_program(command: &str) -> Option<String> {
    let (_, script) = command.split_once("-Command")?;
    let (_, invocation) = script.split_once('&')?;
    let invocation = invocation.trim_start();
    let Some(quoted) = invocation.strip_prefix('\'') else {
        return command_program_token(invocation);
    };

    let mut output = String::new();
    let mut characters = quoted.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\'' {
            output.push(character);
            continue;
        }
        if characters.peek() == Some(&'\'') {
            let _ = characters.next();
            output.push('\'');
            continue;
        }
        return (!output.is_empty()).then_some(output);
    }
    None
}

pub fn command_program_token(command: &str) -> Option<String> {
    let mut output = String::new();
    let mut characters = command.trim_start().chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(character) = characters.next() {
        match quote {
            None => {
                if character.is_whitespace() {
                    break;
                }
                match character {
                    '\'' | '"' => quote = Some(character),
                    '\\' if characters.peek() == Some(&'\'') => {
                        let _ = characters.next();
                        output.push('\'');
                    }
                    _ => output.push(character),
                }
            }
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    output.push(character);
                }
            }
            Some('"') => {
                if character == '"' {
                    quote = None;
                } else if character == '\\' {
                    match characters.peek().copied() {
                        Some(next @ ('"' | '\\' | '$' | '`')) => {
                            let _ = characters.next();
                            output.push(next);
                        }
                        _ => output.push(character),
                    }
                } else {
                    output.push(character);
                }
            }
            _ => {}
        }
    }

    (!output.is_empty()).then_some(output)
}

pub fn sh_command_word(program: &str) -> String {
    let needs_quotes = program.starts_with('~')
        || program.chars().any(|character| {
            character.is_whitespace()
                || matches!(
                    character,
                    '\'' | '"'
                        | '$'
                        | '`'
                        | ';'
                        | '&'
                        | '|'
                        | '<'
                        | '>'
                        | '('
                        | ')'
                        | '*'
                        | '?'
                        | '['
                        | ']'
                        | '#'
                        | '{'
                        | '}'
                        | '!'
                )
        });
    if needs_quotes {
        format!("'{}'", program.replace('\'', "'\\''"))
    } else {
        program.to_string()
    }
}

pub fn cmd_command_word(program: &str) -> String {
    let needs_quotes = program.chars().any(|character| {
        character.is_whitespace()
            || matches!(
                character,
                '&' | '|' | '<' | '>' | '^' | '(' | ')' | '%' | '!' | ',' | ';' | '=' | '"'
            )
    });
    if needs_quotes {
        format!("\"{}\"", program.replace('"', ""))
    } else {
        program.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_reporter_paths_keep_their_exact_command_word() {
        for program in [
            "/home/u/.paneflow/bin/paneflow-ai-hook.sh",
            r"C:\Users\jdupont\.paneflow\bin\paneflow-ai-hook.exe",
            "/opt/paneflow-1.2+dev@x/bin/hook",
        ] {
            assert_eq!(sh_command_word(program), program);
            assert_eq!(cmd_command_word(program), program);
        }
    }

    #[test]
    fn a_windows_profile_with_a_space_is_quoted_for_both_shells() {
        let program = r"C:\Users\Jean Dupont\.paneflow\bin\paneflow-ai-hook.exe";
        let cmd = format!("{} SessionStart", cmd_command_word(program));
        assert_eq!(
            cmd,
            r#""C:\Users\Jean Dupont\.paneflow\bin\paneflow-ai-hook.exe" SessionStart"#
        );
        let sh = format!("{} SessionStart", sh_command_word(program));
        assert_eq!(
            sh,
            r"'C:\Users\Jean Dupont\.paneflow\bin\paneflow-ai-hook.exe' SessionStart"
        );
        assert_eq!(command_program_token(&cmd).as_deref(), Some(program));
        assert_eq!(command_program_token(&sh).as_deref(), Some(program));
        assert!(is_paneflow_hook_command(&cmd));
    }

    #[test]
    fn shell_specials_are_quoted_and_round_trip() {
        let program = "/home/o'neil/(work)/paneflow-ai-hook";
        let word = sh_command_word(program);
        assert_eq!(word, r"'/home/o'\''neil/(work)/paneflow-ai-hook'");
        assert_eq!(command_program_token(&word).as_deref(), Some(program));
        assert_eq!(
            cmd_command_word("C:\\a&b\\hook.exe"),
            "\"C:\\a&b\\hook.exe\""
        );
    }
}
