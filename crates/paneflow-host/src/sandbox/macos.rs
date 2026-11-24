use std::path::{Path, PathBuf};

use super::{SandboxPlan, Wrapped};

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

const PREFLIGHT_PROGRAM: &str = "/usr/bin/true";

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
    let mut fixed: Vec<PathBuf> = Vec::new();
    let anchors = plan
        .shared_endpoints
        .iter()
        .chain(std::iter::once(&plan.home));
    for form in anchors.flat_map(|anchor| spellings(anchor)) {
        for path in form.ancestors().filter(|path| path.parent().is_some()) {
            if !fixed.iter().any(|known| known == path) {
                fixed.push(path.to_path_buf());
            }
        }
    }
    for path in fixed {
        rules.push(format!("(deny file-write* (literal {}))", quoted(&path)));
    }
    for operation in ["signal", "process-info*"] {
        rules.push(format!("(deny {operation})"));
        rules.push(format!("(allow {operation} (target same-sandbox))"));
    }
    rules.join("\n")
}

fn preflight(sandbox_exec: &Path, profile: &str) -> Result<(), String> {
    let output = std::process::Command::new(sandbox_exec)
        .args(["-p", profile, PREFLIGHT_PROGRAM])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("{} cannot run: {error}", sandbox_exec.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(format!(
        "sandbox-exec refused the sandbox profile ({}): {}",
        output.status,
        if detail.is_empty() {
            "no error was printed"
        } else {
            detail.as_str()
        }
    ))
}

pub fn wrap(plan: &SandboxPlan, shell: &str, args: &[String]) -> Result<Wrapped, String> {
    wrap_with(plan, Path::new(SANDBOX_EXEC), shell, args)
}

fn wrap_with(
    plan: &SandboxPlan,
    sandbox_exec: &Path,
    shell: &str,
    args: &[String],
) -> Result<Wrapped, String> {
    if !sandbox_exec.is_file() {
        return Err(format!(
            "the sandbox needs {}, which is missing",
            sandbox_exec.display()
        ));
    }
    let profile = profile(plan);
    preflight(sandbox_exec, &profile)?;
    std::fs::create_dir_all(&plan.session_data_dir).map_err(|error| {
        format!(
            "cannot prepare {} for the sandbox: {error}",
            plan.session_data_dir.display()
        )
    })?;
    let mut wrapped = vec!["-p".to_string(), profile, shell.to_string()];
    wrapped.extend(args.iter().cloned());
    Ok(Wrapped {
        program: sandbox_exec.display().to_string(),
        args: wrapped,
        env: Vec::new(),
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
        assert!(profile.contains("(deny signal)\n(allow signal (target same-sandbox))"));
        assert!(
            profile.contains("(deny process-info*)\n(allow process-info* (target same-sandbox))")
        );
        assert!(profile.starts_with("(version 1)\n(allow default)"));
    }

    fn plan_in(home: &Path) -> SandboxPlan {
        SandboxPlan::new(home, "s-1", home, [home.join("host.sock")])
    }

    #[test]
    fn the_profile_fixes_each_shared_socket_and_its_directories_in_place() {
        let plan = SandboxPlan::new(
            Path::new("/Users/u/.paneflow-dev"),
            "s-1",
            Path::new("/Users/u/project"),
            [PathBuf::from("/var/folders/x/T/paneflow-host-1.sock")],
        );
        let profile = profile(&plan);
        for path in [
            "/var/folders/x/T/paneflow-host-1.sock",
            "/var/folders/x/T",
            "/var/folders/x",
            "/var",
            "/Users/u/.paneflow-dev",
            "/Users/u",
        ] {
            assert!(
                profile.contains(&format!("(deny file-write* (literal \"{path}\"))")),
                "{path}: {profile}"
            );
        }
        assert!(!profile.contains("(literal \"/\")"), "{profile}");
    }

    fn sandboxed(profile: &str, script: &str) -> std::process::Output {
        std::process::Command::new(SANDBOX_EXEC)
            .args(["-p", profile, "/bin/sh", "-c", script])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    #[test]
    fn a_pane_cannot_move_a_shared_socket_or_its_directory_to_reach_it_under_another_name() {
        let scratch = tempfile::Builder::new()
            .prefix("pfm")
            .tempdir_in("/tmp")
            .unwrap();
        let root = std::fs::canonicalize(scratch.path()).unwrap();
        let runtime = root.join("u").join("T");
        std::fs::create_dir_all(&runtime).unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let socket = runtime.join("host.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let plan = SandboxPlan::new(&home, "s-1", &root, [socket.clone()]);
        let profile = profile(&plan);
        let user_dir = runtime.parent().unwrap();
        for (source, target) in [
            (socket.clone(), runtime.join("moved.sock")),
            (runtime.clone(), user_dir.join("T2")),
            (user_dir.to_path_buf(), root.join("u2")),
            (home.clone(), root.join("home2")),
        ] {
            let output = sandboxed(
                &profile,
                &format!("mv '{}' '{}'", source.display(), target.display()),
            );
            assert!(!output.status.success(), "{source:?} moved: {output:?}");
            assert!(source.exists(), "{source:?}");
        }
        let output = sandboxed(&profile, &format!("rm -f '{}'", socket.display()));
        assert!(socket.exists(), "the socket was removed: {output:?}");
        let scratch_file = runtime.join("scratch");
        let output = sandboxed(
            &profile,
            &format!(
                "touch '{0}' && mv '{0}' '{0}2' && mkdir '{1}' && rmdir '{1}'",
                scratch_file.display(),
                runtime.join("d").display()
            ),
        );
        assert!(
            output.status.success(),
            "the runtime directory stays writable: {output:?}"
        );
    }

    #[test]
    fn a_missing_sandbox_exec_fails_the_launch_naming_it() {
        let home = tempfile::tempdir().unwrap();
        let missing = home.path().join("sandbox-exec");
        let error = wrap_with(&plan_in(home.path()), &missing, "/bin/sh", &[]).unwrap_err();
        assert!(error.contains("which is missing"), "{error}");
        assert!(error.contains(&missing.display().to_string()), "{error}");
    }

    #[test]
    fn a_profile_sandbox_exec_cannot_parse_fails_the_preflight_with_its_error() {
        let error = preflight(
            Path::new(SANDBOX_EXEC),
            "(version 1)(allow bogus-operation)",
        )
        .unwrap_err();
        assert!(
            error.starts_with("sandbox-exec refused the sandbox profile"),
            "{error}"
        );
        assert!(error.contains("bogus-operation"), "{error}");
    }

    #[test]
    fn the_generated_profile_passes_the_preflight() {
        let home = tempfile::tempdir().unwrap();
        let wrapped = wrap(&plan_in(home.path()), "/bin/sh", &["-l".into()]).unwrap();
        assert_eq!(wrapped.program, SANDBOX_EXEC);
        assert_eq!(wrapped.args[0], "-p");
        assert_eq!(&wrapped.args[2..], ["/bin/sh", "-l"]);
    }

    #[test]
    fn a_quote_in_a_path_cannot_close_the_profile_string() {
        assert_eq!(quoted(Path::new("/a\"b\\c")), "\"/a\\\"b\\\\c\"");
    }
}
