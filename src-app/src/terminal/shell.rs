pub(crate) use paneflow_launch::shell::clear_then;
#[cfg(windows)]
pub(crate) use paneflow_launch::shell::find_windows_git_bash;

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn the_bash_hook_percent_encodes_every_byte_of_the_working_directory() {
        use paneflow_terminal_ghostty as ghostty;

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a%20b").join("\u{e9}t\u{e9} x");
        std::fs::create_dir_all(&dir).unwrap();
        let rcfile = root.path().join("bashrc");
        std::fs::write(&rcfile, paneflow_launch::shell::BASH_OSC7).unwrap();

        let output = std::process::Command::new("bash")
            .args([
                "--norc",
                "--noprofile",
                "-c",
                "source \"$1\"; cd \"$2\" && __paneflow_osc7",
                "paneflow-osc7-test",
            ])
            .arg(&rcfile)
            .arg(&dir)
            .env("HOME", root.path())
            .env_remove("PANEFLOW_BIN_DIR")
            .output()
            .unwrap();
        let report = String::from_utf8(output.stdout).unwrap();
        assert!(
            report.ends_with("/a%2520b/%C3%A9t%C3%A9%20x\u{7}"),
            "{report:?}"
        );

        let mut terminal = ghostty::DisplayTerminal::new(
            ghostty::WindowSize::new(80, 24, 8, 16).unwrap(),
            100,
            ghostty::TerminalAppearance::default(),
        )
        .unwrap();
        terminal.feed(report.as_bytes()).unwrap();
        let reported: Vec<String> = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                ghostty::BackendEvent::WorkingDirectory(cwd) => Some(cwd),
                _ => None,
            })
            .collect();
        assert_eq!(reported, vec![dir.display().to_string()]);
    }

    fn prompt_mark(script: &str, function: &str) -> Vec<u8> {
        let body = &script[script.find(function).expect("the prompt hook exists")..];
        let start = body.find("printf '").expect("the hook prints its mark") + "printf '".len();
        let end = start + body[start..].find('\'').expect("the mark is quoted");
        body[start..end]
            .replace("\\e", "\x1b")
            .replace("\\a", "\x07")
            .into_bytes()
    }

    fn screen_rows(terminal: &mut paneflow_terminal_ghostty::DisplayTerminal) -> Vec<String> {
        let content = terminal.snapshot().expect("snapshot");
        (0..content.rows)
            .map(|row| {
                content.cells[row * content.cols..(row + 1) * content.cols]
                    .iter()
                    .map(|cell| cell.character)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn each_shell_declares_how_much_of_its_prompt_it_redraws() {
        assert_eq!(
            prompt_mark(
                paneflow_launch::shell::ZSH_OSC7,
                "__paneflow_osc133_precmd()"
            ),
            b"\x1b]133;A;redraw=1\x07"
        );
        assert_eq!(
            prompt_mark(
                paneflow_launch::shell::BASH_OSC7,
                "__paneflow_osc133_precmd()"
            ),
            b"\x1b]133;A;redraw=last\x07"
        );
        assert_eq!(
            prompt_mark(
                paneflow_launch::shell::FISH_OSC7,
                "__paneflow_osc133_prompt"
            ),
            b"\x1b]133;A;redraw=1\x07"
        );
        assert!(paneflow_launch::shell::ZSH_OSC7.contains(
            "precmd_functions=(${precmd_functions:#__paneflow_osc133_precmd} __paneflow_osc133_precmd)"
        ));
    }

    #[test]
    fn a_full_prompt_redraw_after_a_resize_keeps_the_output_above_the_prompt() {
        use paneflow_terminal_ghostty::{DisplayTerminal, TerminalAppearance, WindowSize};

        let prompt: &[u8] = b"\r\ndune /tmp/claude-1000/fxt\r\n> ";
        for (shell, mark) in [
            (
                "zsh",
                prompt_mark(
                    paneflow_launch::shell::ZSH_OSC7,
                    "__paneflow_osc133_precmd()",
                ),
            ),
            (
                "fish",
                prompt_mark(
                    paneflow_launch::shell::FISH_OSC7,
                    "__paneflow_osc133_prompt",
                ),
            ),
        ] {
            let mut terminal = DisplayTerminal::new(
                WindowSize::new(20, 12, 8, 16).unwrap(),
                100,
                TerminalAppearance::default(),
            )
            .unwrap();
            terminal.feed(b"out1\r\nout2\r\nout3\r\n").unwrap();
            terminal.feed(&mark).unwrap();
            terminal.feed(prompt).unwrap();
            for (cols, rows_above_cursor) in [(60, 3), (20, 2), (60, 3)] {
                terminal
                    .resize(WindowSize::new(cols, 12, 8, 16).unwrap())
                    .unwrap();
                terminal
                    .feed(format!("\r\x1b[{rows_above_cursor}A\x1b[J").as_bytes())
                    .unwrap();
                terminal.feed(prompt).unwrap();
                let rows = screen_rows(&mut terminal);
                assert_eq!(
                    rows[..3],
                    ["out1", "out2", "out3"],
                    "{shell}: the prompt redraw at {cols} columns erased output: {rows:?}"
                );
            }
        }
    }
}
