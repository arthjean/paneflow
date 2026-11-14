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

    pub(super) fn workspace_checkout_pull_request(&self, ws: &Workspace) -> Option<PullRequest> {
        if !self.cached_config.sidebar_show.pr_enabled() {
            return None;
        }
        let repo_root = ws.repo_root.as_ref()?;
        (!ws.git_branch.is_empty())
            .then(|| self.pull_request_for(repo_root, &ws.git_branch))
            .flatten()
    }

    pub(super) fn workspace_pull_request(&self, ws: &Workspace) -> Option<PullRequest> {
        let branch_shown = self.cached_config.sidebar_show.branch_enabled();
        ws.tabs()
            .iter()
            .filter(|tab| {
                folded_slot_counts_tab(
                    branch_shown,
                    tab_has_own_checkout(tab.worktree.as_deref(), &ws.worktree_root),
                )
            })
            .filter_map(|tab| self.tab_pull_request(ws, tab))
            .max_by_key(|pr| (pr.state.rank(), pr.number))
    }
}

fn folded_slot_counts_tab(branch_shown: bool, has_own_checkout: bool) -> bool {
    !branch_shown || has_own_checkout
}

#[cfg(test)]
mod tests {
    use super::folded_slot_counts_tab;

    #[test]
    fn a_folded_workspace_leaves_its_own_pull_request_to_its_branch_label() {
        assert!(folded_slot_counts_tab(false, false));
        assert!(folded_slot_counts_tab(false, true));
        assert!(
            !folded_slot_counts_tab(true, false),
            "the branch label already carries the workspace checkout's pull request"
        );
        assert!(folded_slot_counts_tab(true, true));
    }
}
