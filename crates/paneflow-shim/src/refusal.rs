use std::io::{IsTerminal, Write};
use std::path::{Component, Path};
use std::process::ExitCode;

const COMMAND_NOT_FOUND: u8 = 127;
const RECORD_ID: &str = "paneflow-shim";
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) struct Refusal<'a> {
    pub(crate) tool: &'a str,
    pub(crate) in_pane: bool,
    pub(crate) agents_above: u32,
}

impl Refusal<'_> {
    pub(crate) fn exit(&self, summary: &str, detail: &str) -> ExitCode {
        let mut stderr = std::io::stderr().lock();
        if declares(self.in_pane, stderr.is_terminal(), self.agents_above) {
            let _ = stderr.write_all(declaration(self.tool, summary).as_bytes());
        }
        let _ = writeln!(stderr, "paneflow-shim: {detail}");
        ExitCode::from(COMMAND_NOT_FOUND)
    }
}

pub(crate) fn declares(in_pane: bool, on_terminal: bool, agents_above: u32) -> bool {
    in_pane && on_terminal && agents_above == 0
}

pub(crate) fn declaration(tool: &str, summary: &str) -> String {
    format!(
        "\x1b]7501;state=error:id={RECORD_ID}:app={tool}:msg={}\x1b\\",
        base64(summary.as_bytes())
    )
}

pub(crate) fn launcher_label(launcher: &Path) -> String {
    let mut names: Vec<String> = launcher
        .components()
        .rev()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .take(2)
        .collect();
    names.reverse();
    names.join("/")
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk
            .iter()
            .chain(std::iter::repeat(&0))
            .take(3)
            .fold(0_u32, |group, byte| group << 8 | u32::from(*byte));
        for position in 0..4 {
            if position > chunk.len() {
                encoded.push('=');
                continue;
            }
            let index = (group >> (18 - 6 * position)) & 0x3f;
            encoded.push(
                BASE64_ALPHABET
                    .get(index as usize)
                    .map_or('=', |c| char::from(*c)),
            );
        }
    }
    encoded
}
