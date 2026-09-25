use super::*;

impl PaneFlowApp {
    pub(crate) fn tab_row_branch(&self, ws: &Workspace, tab: &Tab) -> String {
        match tab.worktree.as_ref() {
            Some(_) => self
                .tab_checkout_git(tab)
                .map(|git| git.branch.clone())
                .unwrap_or_default(),
            None => ws.git_branch.clone(),
        }
    }

    pub(crate) fn tab_pull_request(&self, ws: &Workspace, tab: &Tab) -> Option<PullRequest> {
        if !self.cached_config.sidebar_show.pr_enabled() {
            return None;
        }
        let repo_root = ws.repo_root.as_ref()?;
        let branch = self.tab_row_branch(ws, tab);
        (!branch.is_empty())
            .then(|| self.pull_request_for(repo_root, &branch))
            .flatten()
    }

    pub(super) fn workspace_pull_request(&self, ws: &Workspace) -> Option<PullRequest> {
        ws.tabs()
            .iter()
            .filter_map(|tab| self.tab_pull_request(ws, tab))
            .max_by_key(|pr| (pr.state.rank(), pr.number))
    }
}
