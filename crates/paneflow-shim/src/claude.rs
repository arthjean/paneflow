use std::ffi::OsString;
use std::hash::{BuildHasher, Hasher};

const SESSION_ID_FLAG: &str = "--session-id";

const SUBCOMMANDS: &[&str] = &[
    "agents",
    "attach",
    "auth",
    "auto-mode",
    "config",
    "doctor",
    "gateway",
    "import",
    "install",
    "kill",
    "logs",
    "mcp",
    "migrate-installer",
    "plugin",
    "plugins",
    "project",
    "respawn",
    "rm",
    "setup-token",
    "stop",
    "ultrareview",
    "update",
    "upgrade",
];

const SESSION_FLAGS: &[&str] = &[
    "-r",
    "--resume",
    "-c",
    "--continue",
    "--session-id",
    "-p",
    "--print",
    "--fork-session",
    "--from-pr",
    "--teleport",
    "--cloud",
    "-h",
    "--help",
    "-v",
    "--version",
];

const SESSION_SHORT_FLAGS: &[char] = &['r', 'c', 'p', 'h', 'v'];

pub(crate) fn starts_fresh_session(args: &[OsString]) -> bool {
    args.iter().all(|arg| {
        let Some(text) = arg.to_str() else {
            return false;
        };
        if SUBCOMMANDS.contains(&text) {
            return false;
        }
        let flag = text.split_once('=').map_or(text, |(flag, _)| flag);
        if SESSION_FLAGS.contains(&flag) {
            return false;
        }
        let clustered_short = text
            .strip_prefix('-')
            .filter(|rest| !rest.starts_with('-') && rest.len() > 1);
        !clustered_short
            .is_some_and(|letters| letters.chars().any(|c| SESSION_SHORT_FLAGS.contains(&c)))
    })
}

pub(crate) fn with_session_id(args: Vec<OsString>, session_id: &str) -> Vec<OsString> {
    let mut preassigned = Vec::with_capacity(args.len() + 2);
    preassigned.push(OsString::from(SESSION_ID_FLAG));
    preassigned.push(OsString::from(session_id));
    preassigned.extend(args);
    preassigned
}

pub(crate) fn new_session_id() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut bytes = [0u8; 16];
    for (index, half) in bytes.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(seed);
        hasher.write_u32(std::process::id());
        hasher.write_usize(index);
        half.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        id.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        id.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_plain_interactive_launch_gets_a_session_id_in_front() {
        for launch in [
            &[][..],
            &["fix the doctor page"][..],
            &["--model", "opus"][..],
            &["--permission-mode", "bypassPermissions"][..],
            &["-n", "review", "--add-dir", "../lib"][..],
        ] {
            let argv = args(launch);
            assert!(starts_fresh_session(&argv), "{launch:?}");
            let rewritten = with_session_id(argv.clone(), "id");
            assert_eq!(rewritten[..2], args(&["--session-id", "id"])[..]);
            assert_eq!(rewritten[2..], argv[..]);
        }
    }

    #[test]
    fn resumes_prints_subcommands_and_explicit_ids_are_left_untouched() {
        for launch in [
            &["--session-id", "X"][..],
            &["--session-id=X"][..],
            &["mcp", "list"][..],
            &["update"][..],
            &["-r"][..],
            &["--resume", "abc"][..],
            &["--resume=abc"][..],
            &["-c"][..],
            &["--continue"][..],
            &["-p", "hello"][..],
            &["--print"][..],
            &["--resume", "abc", "--fork-session"][..],
            &["--fork-session"][..],
            &["-pc"][..],
            &["--version"][..],
            &["--teleport"][..],
        ] {
            assert!(!starts_fresh_session(&args(launch)), "{launch:?}");
        }
    }

    #[test]
    fn a_generated_session_id_is_a_lowercase_uuid_v4() {
        let first = new_session_id();
        let second = new_session_id();
        assert_ne!(first, second);
        for id in [&first, &second] {
            assert_eq!(id.len(), 36, "{id}");
            let groups = id.split('-').map(str::len).collect::<Vec<_>>();
            assert_eq!(groups, vec![8, 4, 4, 4, 12], "{id}");
            assert!(id
                .chars()
                .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert_eq!(id.as_bytes()[14], b'4', "{id}");
            assert!(
                matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
                "{id}"
            );
        }
    }
}
