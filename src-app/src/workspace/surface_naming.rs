use std::collections::HashMap;

const CWD_SEP: char = '@';

const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "nu",
    "nushell",
    "pwsh",
    "powershell",
    "dash",
    "ksh",
    "tcsh",
    "csh",
    "elvish",
    "xonsh",
];

const FALLBACK: &str = "shell";

pub fn derive_surface_base_name(cmd: Option<&str>, title: Option<&str>) -> String {
    if let Some(name) = cmd.and_then(name_from_command) {
        return name;
    }
    if let Some(name) = title.and_then(name_from_title) {
        return name;
    }
    FALLBACK.to_string()
}

fn name_from_command(cmd: &str) -> Option<String> {
    let tokens = command_tokens(cmd);
    let mut tokens = tokens.iter().map(String::as_str);
    let prog = basename(tokens.next()?);
    if prog.is_empty() {
        return None;
    }
    if SHELLS.contains(&prog.to_ascii_lowercase().as_str()) {
        return Some(FALLBACK.to_string());
    }
    let mut parts = vec![prog.to_string()];
    if let Some(arg) = tokens.find(|t| !t.starts_with('-'))
        && !arg.is_empty()
    {
        parts.push(basename(arg).to_string());
    }
    let slug = slugify(&parts.join("-"));
    (!slug.is_empty()).then_some(slug)
}

fn command_tokens(cmd: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in cmd.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn name_from_title(title: &str) -> Option<String> {
    let first = title.split_whitespace().next()?;
    let slug = slugify(basename(first));
    (!slug.is_empty()).then_some(slug)
}

fn basename(path: &str) -> &str {
    let p = path.trim();
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn resolve_surface_names(entries: &[(Option<String>, String, Option<String>)]) -> Vec<String> {
    let mut auto_base_counts: HashMap<&str, usize> = HashMap::new();
    for (custom, base, _) in entries {
        if custom.is_none() {
            *auto_base_counts.entry(base.as_str()).or_insert(0) += 1;
        }
    }

    let provisional: Vec<String> = entries
        .iter()
        .map(|(custom, base, cwd)| {
            if let Some(c) = custom {
                c.clone()
            } else if auto_base_counts.get(base.as_str()).copied().unwrap_or(0) <= 1 {
                base.clone()
            } else if let Some(q) = cwd.as_deref().map(cwd_basename).filter(|q| !q.is_empty()) {
                format!("{base}{CWD_SEP}{q}")
            } else {
                base.clone()
            }
        })
        .collect();

    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<Option<String>> = vec![None; entries.len()];
    for (i, (custom, _, _)) in entries.iter().enumerate() {
        if custom.is_some() {
            out[i] = Some(claim_unique(&mut taken, &provisional[i]));
        }
    }
    for (i, (custom, _, _)) in entries.iter().enumerate() {
        if custom.is_none() {
            out[i] = Some(claim_unique(&mut taken, &provisional[i]));
        }
    }
    out.into_iter().map(Option::unwrap_or_default).collect()
}

pub(crate) fn claim_unique(taken: &mut std::collections::HashSet<String>, name: &str) -> String {
    if taken.insert(name.to_string()) {
        return name.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{name}-{n}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

fn cwd_basename(cwd: &str) -> String {
    slugify(basename(cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_simple_subcommand() {
        assert_eq!(
            derive_surface_base_name(Some("cargo run"), None),
            "cargo-run"
        );
    }

    #[test]
    fn command_absolute_path_argv0() {
        assert_eq!(
            derive_surface_base_name(Some("/usr/bin/node server.js"), None),
            "node-server.js"
        );
    }

    #[test]
    fn command_quoted_windows_path_argv0() {
        assert_eq!(
            derive_surface_base_name(
                Some(r#""C:\Program Files\nodejs\node.exe" "C:\repo\dev server.js""#),
                None
            ),
            "node.exe-dev-server.js"
        );
    }

    #[test]
    fn command_skips_leading_flags_for_qualifier() {
        assert_eq!(
            derive_surface_base_name(Some("python -m http.server"), None),
            "python-http.server"
        );
    }

    #[test]
    fn idle_shell_maps_to_shell() {
        assert_eq!(
            derive_surface_base_name(Some("/usr/bin/zsh"), None),
            "shell"
        );
        assert_eq!(
            derive_surface_base_name(Some("bash"), Some("~/dev")),
            "shell"
        );
    }

    #[test]
    fn title_used_when_no_command() {
        assert_eq!(
            derive_surface_base_name(None, Some("/home/arthur/dev/paneflow")),
            "paneflow"
        );
        assert_eq!(derive_surface_base_name(None, Some("claude")), "claude");
    }

    #[test]
    fn no_signal_falls_back_to_shell() {
        assert_eq!(derive_surface_base_name(None, None), "shell");
        assert_eq!(derive_surface_base_name(Some("   "), Some("   ")), "shell");
    }

    fn auto(base: &str, cwd: Option<&str>) -> (Option<String>, String, Option<String>) {
        (None, base.to_string(), cwd.map(str::to_string))
    }

    #[test]
    fn unique_bases_pass_through_unchanged() {
        let names = resolve_surface_names(&[
            auto("vite", Some("/a/web")),
            auto("cargo-run", Some("/a/api")),
        ]);
        assert_eq!(names, vec!["vite", "cargo-run"]);
    }

    #[test]
    fn collision_qualified_by_cwd_basename() {
        let names = resolve_surface_names(&[
            auto("cargo-run", Some("/home/a/paneflow")),
            auto("cargo-run", Some("/home/a/web")),
        ]);
        assert_eq!(names, vec!["cargo-run@paneflow", "cargo-run@web"]);
    }

    #[test]
    fn same_base_and_cwd_falls_back_to_ordinal() {
        let names = resolve_surface_names(&[
            auto("cargo-run", Some("/home/a/x")),
            auto("cargo-run", Some("/home/a/x")),
        ]);
        assert_eq!(names, vec!["cargo-run@x", "cargo-run@x-2"]);
    }

    #[test]
    fn collision_without_cwd_uses_ordinal() {
        let names = resolve_surface_names(&[
            auto("shell", None),
            auto("shell", None),
            auto("shell", None),
        ]);
        assert_eq!(names, vec!["shell", "shell-2", "shell-3"]);
    }

    #[test]
    fn custom_name_used_verbatim() {
        let names = resolve_surface_names(&[
            (Some("logs".into()), "cargo-run".into(), Some("/a".into())),
            (None, "vite".into(), Some("/b".into())),
        ]);
        assert_eq!(names, vec!["logs", "vite"]);
    }

    #[test]
    fn custom_wins_auto_yields_on_collision() {
        let names = resolve_surface_names(&[
            (None, "cargo-run".into(), Some("/a".into())),
            (Some("cargo-run".into()), "vite".into(), Some("/b".into())),
        ]);
        assert_eq!(names[1], "cargo-run");
        assert_eq!(names[0], "cargo-run-2");
    }

    #[test]
    fn two_custom_collisions_get_ordinal() {
        let names = resolve_surface_names(&[
            (Some("logs".into()), "x".into(), None),
            (Some("logs".into()), "y".into(), None),
        ]);
        assert_eq!(names, vec!["logs", "logs-2"]);
    }

    #[test]
    fn disambiguation_preserves_input_order_and_arity() {
        let input = vec![
            auto("a", Some("/x")),
            auto("b", None),
            auto("a", Some("/y")),
        ];
        let names = resolve_surface_names(&input);
        assert_eq!(names.len(), input.len());
        assert_eq!(names[1], "b");
        assert_eq!(names[0], "a@x");
        assert_eq!(names[2], "a@y");
    }
}
