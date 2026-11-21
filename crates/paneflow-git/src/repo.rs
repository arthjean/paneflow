use std::path::PathBuf;

pub fn read_capped(path: &std::path::Path, limit: u64) -> std::io::Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut content = String::new();
    file.take(limit).read_to_string(&mut content)?;
    Ok(content)
}

pub fn find_git_dir(cwd: &str) -> Option<std::path::PathBuf> {
    let mut search_dir = std::path::Path::new(cwd);
    let git_path = loop {
        let candidate = search_dir.join(".git");
        if candidate.exists() {
            break candidate;
        }
        search_dir = search_dir.parent()?;
    };

    if git_path.is_file() {
        let content = read_capped(&git_path, 512).ok()?;
        let gitdir = content.trim().strip_prefix("gitdir: ")?.to_owned();
        let gitdir_path = if std::path::Path::new(&gitdir).is_absolute() {
            std::path::PathBuf::from(&gitdir)
        } else {
            git_path
                .parent()
                .unwrap_or(std::path::Path::new(cwd))
                .join(&gitdir)
        };
        Some(gitdir_path)
    } else if git_path.is_dir() {
        Some(git_path)
    } else {
        None
    }
}

pub fn canonicalize_or(path: &std::path::Path) -> std::path::PathBuf {
    strip_verbatim_prefix(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

pub fn normalize_lexically(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut stack: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                _ => stack.push(comp),
            },
            other => stack.push(other),
        }
    }
    let mut out = std::path::PathBuf::new();
    for comp in stack {
        out.push(comp.as_os_str());
    }
    out
}

pub fn resolve_main_git_dir(git_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let commondir_file = git_dir.join("commondir");
    let main_git_dir = if commondir_file.is_file() {
        let content = read_capped(&commondir_file, 512).ok()?;
        let rel = content.trim();
        if rel.is_empty() {
            return None;
        }
        let p = std::path::Path::new(rel);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            normalize_lexically(&git_dir.join(p))
        }
    } else {
        git_dir.to_path_buf()
    };
    Some(canonicalize_or(&main_git_dir))
}

pub fn resolve_repo_root(git_dir: &std::path::Path) -> (Option<std::path::PathBuf>, bool) {
    let is_worktree = git_dir.join("commondir").is_file();
    let repo_root = resolve_main_git_dir(git_dir)
        .and_then(|main_git| main_git.parent().map(|p| p.to_path_buf()));
    (repo_root, is_worktree)
}

pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let stripped = path.to_str().and_then(|s| {
        s.strip_prefix(r"\\?\UNC\")
            .map(|rest| PathBuf::from(format!(r"\\{rest}")))
            .or_else(|| s.strip_prefix(r"\\?\").map(PathBuf::from))
    });
    stripped.unwrap_or(path)
}
