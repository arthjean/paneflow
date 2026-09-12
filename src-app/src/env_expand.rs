use std::path::Path;

pub(crate) fn expand_env_value<F>(
    value: &str,
    home: Option<&Path>,
    lookup: F,
) -> Result<String, String>
where
    F: Fn(&str) -> Option<String>,
{
    expand_placeholders(&expand_leading_home(value, home), &lookup)
}

pub(crate) fn expand_with_process_env(value: &str, home: Option<&Path>) -> Result<String, String> {
    expand_env_value(value, home, |name| std::env::var(name).ok())
}

fn expand_leading_home(value: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return value.to_string();
    };
    if value == "~" {
        return home.display().to_string();
    }
    match value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        Some(rest) => home.join(rest).display().to_string(),
        None => value.to_string(),
    }
}

fn expand_placeholders<F>(value: &str, lookup: &F) -> Result<String, String>
where
    F: Fn(&str) -> Option<String>,
{
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        let next = value[index..]
            .find(['$', '%'])
            .map_or(bytes.len(), |offset| index + offset);
        if next > index {
            out.push_str(&value[index..next]);
            index = next;
            continue;
        }
        match bytes[index] {
            b'$' if bytes.get(index + 1) == Some(&b'{') => match braced_name(value, index + 2) {
                Some((name, end)) => {
                    out.push_str(&resolve(name, lookup)?);
                    index = end;
                }
                None => {
                    out.push('$');
                    index += 1;
                }
            },
            b'$' => {
                let name = identifier_at(value, index + 1);
                if name.is_empty() {
                    out.push('$');
                    index += 1;
                } else {
                    out.push_str(&resolve(name, lookup)?);
                    index += 1 + name.len();
                }
            }
            _ => {
                let name = identifier_at(value, index + 1);
                let end = index + 1 + name.len();
                if !name.is_empty() && bytes.get(end) == Some(&b'%') {
                    out.push_str(&resolve(name, lookup)?);
                    index = end + 1;
                } else {
                    out.push('%');
                    index += 1;
                }
            }
        }
    }
    Ok(out)
}

fn braced_name(value: &str, start: usize) -> Option<(&str, usize)> {
    let rest = value.get(start..)?;
    let close = rest.find('}')?;
    let name = &rest[..close];
    is_placeholder_name(name).then_some((name, start + close + 1))
}

fn identifier_at(value: &str, start: usize) -> &str {
    let rest = value.get(start..).unwrap_or_default();
    let len = rest
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    let candidate = &rest[..len];
    if is_placeholder_name(candidate) {
        candidate
    } else {
        ""
    }
}

fn is_placeholder_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn resolve<F>(name: &str, lookup: &F) -> Result<String, String>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(name).ok_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "USERPROFILE" => Some("C:\\Users\\u".to_string()),
            "HOME" => Some("/home/u".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    fn expand(value: &str) -> Result<String, String> {
        expand_env_value(value, Some(Path::new("/home/u")), lookup)
    }

    #[test]
    fn leading_tilde_expands_only_at_the_start() {
        let home = Path::new("/home/u");
        assert_eq!(
            expand_env_value("~/.claude-perso", Some(home), lookup).unwrap(),
            home.join(".claude-perso").display().to_string()
        );
        assert_eq!(
            expand_env_value("~", Some(home), lookup).unwrap(),
            "/home/u"
        );
        assert_eq!(
            expand_env_value("~user/x", Some(home), lookup).unwrap(),
            "~user/x"
        );
        assert_eq!(
            expand_env_value("/abs/~/x", Some(home), lookup).unwrap(),
            "/abs/~/x"
        );
        assert_eq!(expand_env_value("~/x", None, lookup).unwrap(), "~/x");
    }

    #[test]
    fn every_supported_syntax_expands_against_the_lookup() {
        assert_eq!(expand("$HOME/x").unwrap(), "/home/u/x");
        assert_eq!(expand("${HOME}/x").unwrap(), "/home/u/x");
        assert_eq!(
            expand("%USERPROFILE%\\.claude-perso").unwrap(),
            "C:\\Users\\u\\.claude-perso"
        );
        assert_eq!(expand("$HOME:${HOME}").unwrap(), "/home/u:/home/u");
        assert_eq!(expand("$EMPTY").unwrap(), "");
    }

    #[test]
    fn an_unknown_name_is_reported_instead_of_expanding_to_nothing() {
        assert_eq!(expand("$NOPE/x"), Err("NOPE".to_string()));
        assert_eq!(expand("${NOPE}"), Err("NOPE".to_string()));
        assert_eq!(expand("%NOPE%"), Err("NOPE".to_string()));
    }

    #[test]
    fn a_marker_that_names_nothing_stays_literal() {
        assert_eq!(expand("100%").unwrap(), "100%");
        assert_eq!(expand("50% off").unwrap(), "50% off");
        assert_eq!(expand("$ ").unwrap(), "$ ");
        assert_eq!(expand("$1").unwrap(), "$1");
        assert_eq!(expand("${}").unwrap(), "${}");
        assert_eq!(expand("${not a name}").unwrap(), "${not a name}");
        assert_eq!(expand("a$").unwrap(), "a$");
        assert_eq!(expand("%%").unwrap(), "%%");
        assert_eq!(expand("100% of $HOME").unwrap(), "100% of /home/u");
    }

    #[test]
    fn expansion_is_not_applied_to_what_a_lookup_returned() {
        let expanded = expand_env_value("$TILDE/x", Some(Path::new("/home/u")), |name| {
            (name == "TILDE").then(|| "~".to_string())
        })
        .unwrap();
        assert_eq!(expanded, "~/x");
    }
}
