use std::path::{Path, PathBuf};

use super::{SandboxPlan, Wrapped};

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

fn quoted(path: &Path) -> String {
    let mut out = String::from("\"");
    for character in path.display().to_string().chars() {
        if matches!(character, '"' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

fn spellings(path: &Path) -> Vec<PathBuf> {
    let mut forms = vec![path.to_path_buf()];
    let canonical = match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent).ok().map(|dir| dir.join(name)),
        _ => None,
    };
    if let Some(canonical) = canonical.filter(|canonical| canonical != path) {
        forms.push(canonical);
    }
    forms
}

pub fn profile(plan: &SandboxPlan) -> String {
    let mut rules = vec!["(version 1)".to_string(), "(allow default)".to_string()];
    for endpoint in &plan.shared_endpoints {
        for form in spellings(endpoint) {
            rules.push(format!(
                "(deny network-outbound (remote unix-socket (path-literal {})))",
                quoted(&form)
            ));
        }
    }
    let panes = paneflow_home::panes_dir_in(&plan.home);
    for form in spellings(&panes) {
        rules.push(format!(
            "(deny network-outbound (remote unix-socket (subpath {})))",
            quoted(&form)
        ));
    }
    for form in spellings(&plan.pane_dir) {
        rules.push(format!(
            "(allow network-outbound (remote unix-socket (subpath {})))",
            quoted(&form)
        ));
    }
    for form in spellings(&plan.home) {
        rules.push(format!("(deny file-write* (subpath {}))", quoted(&form)));
    }
    let mut writable = vec![plan.pane_dir.clone(), plan.session_data_dir.clone()];
    if plan.cwd != plan.home && plan.cwd.starts_with(&plan.home) && !plan.cwd.starts_with(&panes) {
        writable.push(plan.cwd.clone());
    }
    for dir in writable {
        for form in spellings(&dir) {
            rules.push(format!("(allow file-write* (subpath {}))", quoted(&form)));
        }
    }
    rules.push("(deny signal (target others))".to_string());
    rules.push("(allow signal (target same-sandbox))".to_string());
    rules.join("\n")
}

pub fn wrap(plan: &SandboxPlan, shell: &str, args: &[String]) -> Result<Wrapped, String> {
    if !Path::new(SANDBOX_EXEC).is_file() {
        return Err(format!(
            "the sandbox needs {SANDBOX_EXEC}, which is missing"
        ));
    }
    std::fs::create_dir_all(&plan.session_data_dir).map_err(|error| {
        format!(
            "cannot prepare {} for the sandbox: {error}",
            plan.session_data_dir.display()
        )
    })?;
    let mut wrapped = vec!["-p".to_string(), profile(plan), shell.to_string()];
    wrapped.extend(args.iter().cloned());
    Ok(Wrapped {
        program: SANDBOX_EXEC.to_string(),
        args: wrapped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_denies_the_shared_sockets_and_other_panes_but_allows_its_own() {
        let home = PathBuf::from("/Users/u/.paneflow-dev");
        let plan = SandboxPlan::new(
            &home,
            "s-1",
            Path::new("/Users/u/project"),
            [PathBuf::from("/var/folders/x/T/paneflow-host-1.sock")],
        );
        let profile = profile(&plan);
        let deny_panes = profile
            .find("(deny network-outbound (remote unix-socket (subpath \"/Users/u/.paneflow-dev/panes\")))")
            .unwrap();
        let allow_own = profile
            .find("(allow network-outbound (remote unix-socket (subpath \"/Users/u/.paneflow-dev/panes/s-1\")))")
            .unwrap();
        assert!(deny_panes < allow_own);
        assert!(profile.contains("(path-literal \"/var/folders/x/T/paneflow-host-1.sock\")"));
        assert!(profile.contains("(deny signal (target others))"));
        assert!(profile.starts_with("(version 1)\n(allow default)"));
    }

    #[test]
    fn a_quote_in_a_path_cannot_close_the_profile_string() {
        assert_eq!(quoted(Path::new("/a\"b\\c")), "\"/a\\\"b\\\\c\"");
    }
}
