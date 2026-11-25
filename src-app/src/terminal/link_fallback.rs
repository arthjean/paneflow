use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use futures::future::{Either, select};
use gpui::{App, BackgroundExecutor, Global};

use super::element::{loose_code_paths, loose_code_zone, match_loose_path};
use super::project_files::{ProjectFiles, walk_project_files};
use super::types::{HyperlinkZone, Line};

pub(crate) const LOOSE_LINK_DEADLINE: Duration = Duration::from_millis(200);
const INDEX_LIFETIME: Duration = Duration::from_secs(30);
const RECHECK_AFTER: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(crate) struct WorkspaceRoots(HashMap<u64, PathBuf>);

impl Global for WorkspaceRoots {}

pub(crate) fn sync_workspace_roots(roots: impl Iterator<Item = (u64, PathBuf)>, cx: &mut App) {
    let roots: HashMap<u64, PathBuf> = roots
        .filter(|(_, root)| !root.as_os_str().is_empty())
        .collect();
    if cx
        .try_global::<WorkspaceRoots>()
        .is_some_and(|known| known.0 == roots)
    {
        return;
    }
    cx.set_global(WorkspaceRoots(roots));
}

pub(crate) fn workspace_root(workspace_id: u64, cx: &App) -> Option<PathBuf> {
    cx.try_global::<WorkspaceRoots>()?
        .0
        .get(&workspace_id)
        .cloned()
}

#[derive(Default)]
struct IndexState {
    files: Option<(Arc<ProjectFiles>, Instant)>,
    walking: bool,
}

#[derive(Clone, Default)]
pub(crate) struct LinkIndex(Arc<Mutex<IndexState>>);

#[derive(Default)]
struct LinkIndexes(HashMap<PathBuf, LinkIndex>);

impl Global for LinkIndexes {}

pub(crate) fn link_index(root: Option<&Path>, cx: &mut App) -> LinkIndex {
    let Some(root) = root else {
        return LinkIndex::default();
    };
    let known = cx.try_global::<WorkspaceRoots>();
    let live =
        |candidate: &PathBuf| known.is_none_or(|roots| roots.0.values().any(|r| r == candidate));
    if cx
        .try_global::<LinkIndexes>()
        .is_some_and(|indexes| indexes.0.keys().any(|kept| !live(kept)))
    {
        let kept: HashMap<PathBuf, LinkIndex> = cx
            .global::<LinkIndexes>()
            .0
            .iter()
            .filter(|(kept, _)| live(kept))
            .map(|(kept, index)| (kept.clone(), index.clone()))
            .collect();
        cx.set_global(LinkIndexes(kept));
    }
    cx.default_global::<LinkIndexes>()
        .0
        .entry(root.to_path_buf())
        .or_default()
        .clone()
}

impl LinkIndex {
    fn known(&self, root: &Path) -> Option<(Arc<ProjectFiles>, bool)> {
        let state = self.0.lock().ok()?;
        let (files, built) = state.files.as_ref()?;
        (files.root == root).then(|| (files.clone(), built.elapsed() < INDEX_LIFETIME))
    }

    fn built_at(&self, files: &Arc<ProjectFiles>) -> Option<Instant> {
        let state = self.0.lock().ok()?;
        let (kept, built) = state.files.as_ref()?;
        Arc::ptr_eq(kept, files).then_some(*built)
    }

    fn begin_walk(&self) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        if state.walking {
            return false;
        }
        state.walking = true;
        true
    }

    fn store(&self, files: Option<Arc<ProjectFiles>>) {
        if let Ok(mut state) = self.0.lock() {
            state.walking = false;
            if let Some(files) = files {
                state.files = Some((files, Instant::now()));
            }
        }
    }

    fn walk(&self, root: &Path, executor: &BackgroundExecutor) -> oneshot::Receiver<()> {
        let (done, walked) = oneshot::channel();
        let index = self.clone();
        let walk_root = root.to_path_buf();
        executor
            .spawn(async move {
                let files = walk_project_files(&walk_root, &AtomicBool::new(false)).map(Arc::new);
                index.store(files);
                let _ = done.send(());
            })
            .detach();
        walked
    }

    async fn files(&self, root: &Path, executor: &BackgroundExecutor) -> Option<Arc<ProjectFiles>> {
        let known = self.known(root);
        if let Some((files, true)) = known {
            return Some(files);
        }
        let walked = self.begin_walk().then(|| self.walk(root, executor));
        if let Some((files, _)) = known {
            return Some(files);
        }
        if !walk_within_deadline(walked, executor).await {
            return None;
        }
        self.known(root).map(|(files, _)| files)
    }

    async fn walked_after(
        &self,
        root: &Path,
        since: Instant,
        executor: &BackgroundExecutor,
    ) -> Option<Arc<ProjectFiles>> {
        let walked = self.begin_walk().then(|| self.walk(root, executor));
        if !walk_within_deadline(walked, executor).await {
            return None;
        }
        let (files, _) = self.known(root)?;
        self.built_at(&files)
            .is_some_and(|built| built > since)
            .then_some(files)
    }
}

async fn walk_within_deadline(
    walked: Option<oneshot::Receiver<()>>,
    executor: &BackgroundExecutor,
) -> bool {
    let deadline = Box::pin(executor.timer(LOOSE_LINK_DEADLINE));
    match walked {
        Some(walked) => matches!(select(walked, deadline).await, Either::Left(_)),
        None => {
            deadline.await;
            true
        }
    }
}

pub(crate) struct LooseLinkScope {
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) root: Option<PathBuf>,
    pub(crate) index: LinkIndex,
}

pub(crate) struct LooseLinks {
    pub(crate) zones: Vec<HyperlinkZone>,
    pub(crate) settled: bool,
}

pub(crate) async fn loose_link_zones(
    line_text: &str,
    line: Line,
    char_to_column: &[usize],
    scope: LooseLinkScope,
    executor: BackgroundExecutor,
) -> LooseLinks {
    let candidates = loose_code_paths(line_text, scope.cwd.as_deref());
    if candidates.is_empty() {
        return LooseLinks {
            zones: Vec::new(),
            settled: true,
        };
    }
    let mut zones = Vec::new();
    let mut unresolved = Vec::new();
    for candidate in candidates {
        match match_loose_path(&candidate.path, scope.root.as_deref(), None) {
            Some(found) => zones.extend(loose_code_zone(
                line_text,
                line,
                char_to_column,
                &candidate,
                &found,
            )),
            None => unresolved.push(candidate),
        }
    }
    let Some(root) = scope.root.as_deref().filter(|_| !unresolved.is_empty()) else {
        return LooseLinks {
            zones,
            settled: true,
        };
    };
    let Some(files) = scope.index.files(root, &executor).await else {
        return LooseLinks {
            zones,
            settled: false,
        };
    };
    let mut missing = Vec::new();
    for candidate in unresolved {
        let project = Some((files.root.as_path(), files.paths.as_slice()));
        match match_loose_path(&candidate.path, None, project) {
            Some(found) => zones.extend(loose_code_zone(
                line_text,
                line,
                char_to_column,
                &candidate,
                &found,
            )),
            None => missing.push(candidate),
        }
    }
    if missing.is_empty() {
        return LooseLinks {
            zones,
            settled: true,
        };
    }
    let Some(built) = scope
        .index
        .built_at(&files)
        .filter(|built| built.elapsed() >= RECHECK_AFTER)
    else {
        return LooseLinks {
            zones,
            settled: false,
        };
    };
    let Some(files) = scope.index.walked_after(root, built, &executor).await else {
        return LooseLinks {
            zones,
            settled: false,
        };
    };
    for candidate in missing {
        let project = Some((files.root.as_path(), files.paths.as_slice()));
        if let Some(found) = match_loose_path(&candidate.path, None, project) {
            zones.extend(loose_code_zone(
                line_text,
                line,
                char_to_column,
                &candidate,
                &found,
            ));
        }
    }
    LooseLinks {
        zones,
        settled: true,
    }
}

pub(crate) fn link_is_ambiguous(link: &HyperlinkZone) -> bool {
    !Path::new(&link.uri).is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ascii_map(text: &str) -> Vec<usize> {
        (0..text.chars().count()).collect()
    }

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = crate::runtime_paths::strip_verbatim_prefix(
            dir.path().canonicalize().expect("canonical root"),
        );
        for file in [
            "src/app/mod.rs",
            "src/terminal/mod.rs",
            "src/terminal/view.rs",
            "crates/core/lib.rs",
            "crates/one/util/helpers.rs",
            "crates/two/util/helpers.rs",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, "fn main() {}\n").expect("write");
        }
        std::fs::create_dir_all(root.join("elsewhere")).expect("mkdir");
        (dir, root)
    }

    async fn links(cx: &mut gpui::TestAppContext, text: &str, root: &Path) -> LooseLinks {
        let scope = LooseLinkScope {
            cwd: Some(root.join("elsewhere")),
            root: Some(root.to_path_buf()),
            index: LinkIndex::default(),
        };
        let executor = cx.executor();
        let map = ascii_map(text);
        let text = text.to_owned();
        loose_link_zones(&text, Line(0), &map, scope, executor).await
    }

    async fn zones(cx: &mut gpui::TestAppContext, text: &str, root: &Path) -> Vec<HyperlinkZone> {
        links(cx, text, root).await.zones
    }

    #[gpui::test]
    async fn a_path_missing_from_the_cwd_is_tried_from_the_workspace_root(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, root) = project();
        let found = zones(cx, "see src/terminal/view.rs:12:3 now", &root).await;
        assert_eq!(found.len(), 1);
        assert_eq!(
            PathBuf::from(&found[0].uri),
            root.join("src/terminal/view.rs")
        );
        assert_eq!((found[0].line, found[0].col), (Some(12), Some(3)));
        assert_eq!((found[0].start.column.0, found[0].end.column.0), (4, 28));
        assert!(!link_is_ambiguous(&found[0]));
    }

    #[gpui::test]
    async fn a_truncated_path_opens_the_only_project_file_that_ends_with_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, root) = project();
        let found = zones(cx, "terminal/view.rs:7", &root).await;
        assert_eq!(found.len(), 1);
        assert_eq!(
            PathBuf::from(&found[0].uri),
            root.join("src/terminal/view.rs")
        );
        assert_eq!(found[0].line, Some(7));
    }

    #[gpui::test]
    async fn several_candidates_keep_the_typed_path_for_the_file_picker(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, root) = project();
        let found = zones(cx, "open util/helpers.rs:3", &root).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].uri, "util/helpers.rs");
        assert_eq!(found[0].line, Some(3));
        assert!(link_is_ambiguous(&found[0]));
    }

    #[gpui::test]
    async fn twin_files_created_after_the_index_was_walked_still_open_the_file_picker(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, root) = project();
        let index = LinkIndex::default();
        let executor = cx.executor();
        let walked = index.files(&root, &executor).await.expect("first walk");
        if let Ok(mut state) = index.0.lock() {
            state.files = Instant::now()
                .checked_sub(INDEX_LIFETIME / 2)
                .map(|built| (walked.clone(), built));
        }
        for file in ["p/q/twin.rs", "r/q/twin.rs"] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, "fn main() {}\n").expect("write");
        }
        let text = "open q/twin.rs:3".to_owned();
        let map = ascii_map(&text);
        let scope = LooseLinkScope {
            cwd: Some(root.clone()),
            root: Some(root.clone()),
            index,
        };
        let found = loose_link_zones(&text, Line(0), &map, scope, executor).await;
        assert!(found.settled);
        assert_eq!(found.zones.len(), 1, "{:?}", found.zones);
        assert_eq!(found.zones[0].uri, "q/twin.rs");
        assert!(link_is_ambiguous(&found.zones[0]));
    }

    #[gpui::test]
    async fn a_path_no_project_file_ends_with_is_not_a_link(cx: &mut gpui::TestAppContext) {
        let (_dir, root) = project();
        let missing = links(cx, "missing/nowhere.rs:3", &root).await;
        assert!(missing.zones.is_empty());
        assert!(
            !missing.settled,
            "a path missing from an index walked moments ago is checked again on the next hover"
        );
        assert!(zones(cx, "../core/lib.rs", &root).await.is_empty());
    }

    #[gpui::test]
    async fn an_index_that_is_still_being_walked_yields_no_link(cx: &mut gpui::TestAppContext) {
        let (_dir, root) = project();
        let index = LinkIndex::default();
        assert!(index.begin_walk());
        let scope = LooseLinkScope {
            cwd: None,
            root: Some(root.clone()),
            index,
        };
        let executor = cx.executor();
        let text = "terminal/view.rs:7".to_owned();
        let map = ascii_map(&text);
        let links = loose_link_zones(&text, Line(0), &map, scope, executor).await;
        assert!(links.zones.is_empty());
        assert!(!links.settled);
    }

    #[gpui::test]
    async fn an_expired_index_still_answers_while_it_is_walked_again(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, root) = project();
        let index = LinkIndex::default();
        let executor = cx.executor();
        let first = index.files(&root, &executor).await.expect("first walk");
        if let Ok(mut state) = index.0.lock() {
            state.files = Instant::now()
                .checked_sub(INDEX_LIFETIME * 2)
                .map(|built| (first.clone(), built));
        }
        assert!(index.begin_walk());
        let again = index.files(&root, &executor).await.expect("expired index");
        assert!(Arc::ptr_eq(&first, &again));
    }

    #[gpui::test]
    fn terminals_of_one_root_share_an_index_and_a_closed_root_drops_its_own(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let w = PathBuf::from("/w");
            let v = PathBuf::from("/v");
            sync_workspace_roots([(1, w.clone()), (2, v.clone())].into_iter(), cx);
            let first = link_index(Some(&w), cx);
            let second = link_index(Some(&w), cx);
            let other = link_index(Some(&v), cx);
            assert!(Arc::ptr_eq(&first.0, &second.0));
            assert!(!Arc::ptr_eq(&first.0, &other.0));
            sync_workspace_roots([(2, v.clone())].into_iter(), cx);
            let _ = link_index(Some(&v), cx);
            assert_eq!(cx.global::<LinkIndexes>().0.len(), 1);
            assert!(!Arc::ptr_eq(&first.0, &link_index(None, cx).0));
        });
    }

    #[gpui::test]
    fn workspace_roots_are_published_once_per_change(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            sync_workspace_roots(
                [(1, PathBuf::from("/w")), (2, PathBuf::new())].into_iter(),
                cx,
            );
            assert_eq!(workspace_root(1, cx), Some(PathBuf::from("/w")));
            assert_eq!(workspace_root(2, cx), None);
            sync_workspace_roots([(1, PathBuf::from("/v"))].into_iter(), cx);
            assert_eq!(workspace_root(1, cx), Some(PathBuf::from("/v")));
        });
    }
}
