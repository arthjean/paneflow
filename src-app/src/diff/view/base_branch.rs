use gpui::{Context, Window};

use super::DiffView;

pub(super) fn matching_indices(branches_lc: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..branches_lc.len()).collect();
    }
    branches_lc
        .iter()
        .enumerate()
        .filter_map(|(index, branch)| branch.contains(query).then_some(index))
        .collect()
}

pub(super) fn first_matching_index(branches_lc: &[String], query: &str) -> Option<usize> {
    if query.is_empty() && !branches_lc.is_empty() {
        return Some(0);
    }
    branches_lc.iter().position(|branch| branch.contains(query))
}

impl DiffView {
    pub(super) fn resolve_and_set_base(&mut self, raw: String, cx: &mut Context<Self>) {
        let raw = raw.trim().to_string();
        if raw.is_empty() {
            return;
        }
        let Some(probe_dir) = self.columns.first().map(|column| column.path.clone()) else {
            return;
        };
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

    pub(super) fn toggle_base_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.base_picker_open = !self.base_picker_open;
        if self.base_picker_open {
            self.base_filter.update(cx, |input, cx| {
                input.clear(cx);
            });
            let focus_handle = self.base_filter.read(cx).focus_handle.clone();
            window.focus(&focus_handle, cx);
        } else {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    pub(super) fn close_base_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.base_picker_open {
            self.base_picker_open = false;
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    pub(super) fn set_base(&mut self, base: String, cx: &mut Context<Self>) {
        if base == self.base_ref {
            self.base_picker_open = false;
            cx.notify();
            return;
        }
        self.base_ref = base;
        self.base_picker_open = false;
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
