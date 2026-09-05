use gpui::Context;

use super::DiffView;

pub(crate) fn matching_indices(branches_lc: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..branches_lc.len()).collect();
    }
    branches_lc
        .iter()
        .enumerate()
        .filter_map(|(index, branch)| branch.contains(query).then_some(index))
        .collect()
}

pub(crate) fn first_matching_index(branches_lc: &[String], query: &str) -> Option<usize> {
    if query.is_empty() && !branches_lc.is_empty() {
        return Some(0);
    }
    branches_lc.iter().position(|branch| branch.contains(query))
}

impl DiffView {
    pub fn resolve_and_set_base(&mut self, raw: String, cx: &mut Context<Self>) {
        let raw = raw.trim().to_string();
        if raw.is_empty() {
            return;
        }
        let probe_dir = self.column.path.clone();
        cx.spawn(async move |this, cx| {
            let candidate = raw.clone();
            let exists =
                smol::unblock(move || super::super::git::ref_exists(&probe_dir, &candidate)).await;
            if !exists {
                log::debug!("diff: base '{raw}' did not resolve to a ref; ignored");
                return;
            }
            let _ = cx.update(|cx| this.update(cx, |view: &mut Self, cx| view.set_base(raw, cx)));
        })
        .detach();
    }

    pub fn set_base(&mut self, base: String, cx: &mut Context<Self>) {
        if base == self.base_ref {
            cx.notify();
            return;
        }
        self.base_ref = base;
        self.start_loading(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_filter_preserves_display_order() {
        let branches = vec![
            "main".to_string(),
            "feature/auth".to_string(),
            "feature/api".to_string(),
        ];
        assert_eq!(matching_indices(&branches, "feature"), vec![1, 2]);
        assert_eq!(first_matching_index(&branches, "api"), Some(2));
    }
}
