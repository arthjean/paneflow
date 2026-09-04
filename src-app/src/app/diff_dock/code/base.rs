use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AsyncApp, Context, WeakEntity};

use crate::diff::{
    HeadFile, MAX_DIFF_FILE_BYTES, classify_git_bytes, head_sha, show_head_file,
    try_worktree_toplevel,
};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) enum Base {
    #[default]
    None,
    Untracked,
    Text {
        text: Arc<str>,
        head_sha: String,
    },
}

impl Base {
    pub(crate) fn text(&self) -> Option<&Arc<str>> {
        match self {
            Self::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    pub(crate) fn head_sha(&self) -> Option<&str> {
        match self {
            Self::Text { head_sha, .. } => Some(head_sha),
            _ => None,
        }
    }
}

pub(crate) fn load_base_blocking(path: &Path) -> Base {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Base::None;
    };
    let toplevel = match try_worktree_toplevel(parent) {
        Ok(Some(toplevel)) => toplevel,
        Ok(None) => {
            log::debug!("editor base: {} is outside a git worktree", path.display());
            return Base::None;
        }
        Err(err) => {
            log::warn!("editor base: {}: {err}", path.display());
            return Base::None;
        }
    };
    let Some(rel_path) = relative_git_path(&toplevel, path) else {
        log::debug!(
            "editor base: {} is not under its worktree {}",
            path.display(),
            toplevel.display()
        );
        return Base::None;
    };
    let Some(sha) = head_sha(&toplevel) else {
        log::debug!("editor base: {} has no HEAD commit yet", toplevel.display());
        return Base::Untracked;
    };
    match show_head_file(&toplevel, &rel_path) {
        Ok(HeadFile::Missing) => Base::Untracked,
        Ok(HeadFile::Content(bytes)) => {
            if bytes.len() as u64 > MAX_DIFF_FILE_BYTES {
                log::debug!(
                    "editor base: {rel_path}: HEAD content is {} bytes, past the {} byte cap",
                    bytes.len(),
                    MAX_DIFF_FILE_BYTES
                );
                return Base::None;
            }
            let (text, binary) = classify_git_bytes(bytes);
            if binary {
                log::debug!("editor base: {rel_path}: HEAD content is binary");
                return Base::None;
            }
            Base::Text {
                text: text.into(),
                head_sha: sha,
            }
        }
        Err(err) => {
            log::warn!("editor base: {}: {err}", path.display());
            Base::None
        }
    }
}

fn relative_git_path(toplevel: &Path, path: &Path) -> Option<String> {
    let relative = match path.strip_prefix(toplevel) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => {
            let canonical_top = std::fs::canonicalize(toplevel).ok()?;
            let canonical_path = std::fs::canonicalize(path).ok()?;
            canonical_path
                .strip_prefix(&canonical_top)
                .ok()?
                .to_path_buf()
        }
    };
    git_path_string(&relative)
}

fn git_path_string(relative: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

pub(crate) fn spawn_base_load<V, F>(path: PathBuf, generation: u64, cx: &mut Context<V>, apply: F)
where
    V: 'static,
    F: FnOnce(&mut V, u64, Base, &mut Context<V>) + 'static,
{
    cx.spawn(async move |this: WeakEntity<V>, cx: &mut AsyncApp| {
        let base = smol::unblock(move || load_base_blocking(&path)).await;
        cx.update(|cx| {
            let _ = this.update(cx, |view: &mut V, cx: &mut Context<V>| {
                apply(view, generation, base, cx);
            });
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(cwd: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn commit(root: &Path, message: &str) -> bool {
        git(root, &["add", "-A"])
            && git(
                root,
                &[
                    "-c",
                    "user.email=paneflow@example.com",
                    "-c",
                    "user.name=Paneflow",
                    "commit",
                    "-q",
                    "-m",
                    message,
                ],
            )
    }

    fn repo() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().expect("tempdir");
        if !git(dir.path(), &["init", "-q"]) {
            return None;
        }
        assert!(git(dir.path(), &["config", "core.autocrlf", "false"]));
        Some(dir)
    }

    #[test]
    fn a_committed_then_modified_file_loads_its_committed_text_and_head_sha() {
        let Some(dir) = repo() else { return };
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        let file = root.join("src").join("main.rs");
        std::fs::write(&file, "fn main() {}\n").expect("seed");
        assert!(commit(root, "init"));
        std::fs::write(&file, "fn main() { edited(); }\n").expect("modify");

        let base = load_base_blocking(&file);
        let Base::Text { text, head_sha } = base else {
            panic!("expected a tracked base, got {base:?}");
        };
        assert_eq!(&*text, "fn main() {}\n");
        assert_eq!(head_sha.len(), 40);
        assert_eq!(super::head_sha(root).as_deref(), Some(head_sha.as_str()));
    }

    #[test]
    fn crlf_committed_content_is_normalized_like_the_document() {
        let Some(dir) = repo() else { return };
        let root = dir.path();
        let file = root.join("notes.txt");
        std::fs::write(&file, "one\r\ntwo\r\n").expect("seed");
        assert!(commit(root, "init"));
        assert_eq!(
            load_base_blocking(&file)
                .text()
                .map(|text| text.to_string()),
            Some("one\ntwo\n".to_string())
        );
    }

    #[test]
    fn an_untracked_file_and_a_file_missing_from_head_are_untracked() {
        let Some(dir) = repo() else { return };
        let root = dir.path();
        std::fs::write(root.join("tracked.txt"), "x\n").expect("seed");
        assert!(commit(root, "init"));
        let fresh = root.join("fresh.txt");
        std::fs::write(&fresh, "new\n").expect("write");
        assert_eq!(load_base_blocking(&fresh), Base::Untracked);

        let staged = root.join("staged.txt");
        std::fs::write(&staged, "staged\n").expect("write");
        assert!(git(root, &["add", "staged.txt"]));
        assert_eq!(load_base_blocking(&staged), Base::Untracked);
    }

    #[test]
    fn a_repository_without_a_commit_yields_untracked() {
        let Some(dir) = repo() else { return };
        let file = dir.path().join("first.txt");
        std::fs::write(&file, "x\n").expect("write");
        assert_eq!(load_base_blocking(&file), Base::Untracked);
    }

    #[test]
    fn a_path_outside_a_repository_has_no_base() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("loose.txt");
        std::fs::write(&file, "x\n").expect("write");
        if try_worktree_toplevel(dir.path()).is_ok_and(|top| top.is_some()) {
            return;
        }
        assert_eq!(load_base_blocking(&file), Base::None);
        assert_eq!(
            load_base_blocking(Path::new("relative-only.txt")),
            Base::None
        );
    }

    #[test]
    fn binary_and_oversized_head_content_have_no_base() {
        let Some(dir) = repo() else { return };
        let root = dir.path();
        let blob = root.join("blob.bin");
        std::fs::write(&blob, [b'a', 0, b'b', b'\n']).expect("seed");
        let huge = root.join("huge.txt");
        std::fs::write(&huge, vec![b'x'; MAX_DIFF_FILE_BYTES as usize + 1]).expect("seed");
        assert!(commit(root, "init"));
        assert_eq!(load_base_blocking(&blob), Base::None);
        assert_eq!(load_base_blocking(&huge), Base::None);
    }

    #[test]
    fn git_paths_use_forward_slashes_and_refuse_escapes() {
        assert_eq!(
            git_path_string(Path::new("src").join("app").join("x.rs").as_path()),
            Some("src/app/x.rs".to_string())
        );
        assert_eq!(git_path_string(Path::new("")), None);
        assert_eq!(
            git_path_string(Path::new("..").join("x.rs").as_path()),
            None
        );
    }
}
