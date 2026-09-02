#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    Row,
    Col,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arrange {
    Leaf(usize),
    Split { axis: Axis, children: Vec<Arrange> },
}

impl Arrange {
    pub fn row(ids: &[usize]) -> Arrange {
        if ids.len() == 1 {
            Arrange::Leaf(ids[0])
        } else {
            Arrange::Split {
                axis: Axis::Row,
                children: ids.iter().map(|&i| Arrange::Leaf(i)).collect(),
            }
        }
    }

    pub fn leaves(&self, out: &mut Vec<usize>) {
        match self {
            Arrange::Leaf(i) => out.push(*i),
            Arrange::Split { children, .. } => {
                for c in children {
                    c.leaves(out);
                }
            }
        }
    }

    fn retain(&mut self, keep: &impl Fn(usize) -> bool) {
        if let Arrange::Split { children, .. } = self {
            let mut i = 0;
            while i < children.len() {
                match &mut children[i] {
                    Arrange::Leaf(id) if !keep(*id) => {
                        children.remove(i);
                    }
                    Arrange::Leaf(_) => i += 1,
                    node @ Arrange::Split { .. } => {
                        node.retain(keep);
                        if node.leaf_count() == 0 {
                            children.remove(i);
                        } else {
                            i += 1;
                        }
                    }
                }
            }
        }
    }

    fn leaf_count(&self) -> usize {
        match self {
            Arrange::Leaf(_) => 1,
            Arrange::Split { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }

    fn normalize(&mut self) {
        if let Arrange::Split { axis, children } = self {
            let axis = *axis;
            for c in children.iter_mut() {
                c.normalize();
            }
            let mut flat: Vec<Arrange> = Vec::with_capacity(children.len());
            for c in children.drain(..) {
                match c {
                    Arrange::Split {
                        axis: ca,
                        children: cc,
                    } if ca == axis => flat.extend(cc),
                    other => flat.push(other),
                }
            }
            *children = flat;
            if children.len() == 1 {
                let only = children.remove(0);
                *self = only;
            }
        }
    }

    pub fn remove(&mut self, id: usize) -> bool {
        let found = self.remove_inner(id);
        if found {
            self.normalize();
        }
        found
    }

    fn remove_inner(&mut self, id: usize) -> bool {
        let Arrange::Split { children, .. } = self else {
            return false;
        };
        let mut found = false;
        let mut i = 0;
        while i < children.len() {
            match &mut children[i] {
                Arrange::Leaf(x) if *x == id => {
                    children.remove(i);
                    found = true;
                }
                node => {
                    if node.remove_inner(id) {
                        found = true;
                        if node.leaf_count() == 0 {
                            children.remove(i);
                            continue;
                        }
                    }
                    i += 1;
                }
            }
        }
        found
    }

    pub fn split(&mut self, target: usize, axis: Axis, new: usize, before: bool) -> bool {
        let found = self.split_inner(target, axis, new, before);
        if found {
            self.normalize();
        }
        found
    }

    fn split_inner(&mut self, target: usize, axis: Axis, new: usize, before: bool) -> bool {
        if let Arrange::Leaf(t) = self {
            if *t == target {
                let pair = if before {
                    vec![Arrange::Leaf(new), Arrange::Leaf(target)]
                } else {
                    vec![Arrange::Leaf(target), Arrange::Leaf(new)]
                };
                *self = Arrange::Split {
                    axis,
                    children: pair,
                };
                return true;
            }
            return false;
        }
        let Arrange::Split {
            axis: self_axis,
            children,
        } = self
        else {
            return false;
        };
        let self_axis = *self_axis;
        if let Some(p) = children
            .iter()
            .position(|c| matches!(c, Arrange::Leaf(t) if *t == target))
        {
            if self_axis == axis {
                let at = if before { p } else { p + 1 };
                children.insert(at, Arrange::Leaf(new));
            } else {
                let pair = if before {
                    vec![Arrange::Leaf(new), Arrange::Leaf(target)]
                } else {
                    vec![Arrange::Leaf(target), Arrange::Leaf(new)]
                };
                children[p] = Arrange::Split {
                    axis,
                    children: pair,
                };
            }
            return true;
        }
        for c in children.iter_mut() {
            if c.split_inner(target, axis, new, before) {
                return true;
            }
        }
        false
    }

    pub fn reconcile(&mut self, visible: &[bool]) {
        let keep = |id: usize| visible.get(id).copied().unwrap_or(false);
        if let Arrange::Leaf(id) = self
            && !keep(*id)
        {
            *self = Arrange::Split {
                axis: Axis::Row,
                children: Vec::new(),
            };
        }
        self.retain(&keep);
        self.normalize();
        let mut present = Vec::new();
        self.leaves(&mut present);
        let missing: Vec<usize> = (0..visible.len())
            .filter(|&i| visible[i] && !present.contains(&i))
            .collect();
        for id in missing {
            self.append_leaf(id);
        }
        self.normalize();
    }

    fn append_leaf(&mut self, id: usize) {
        match self {
            Arrange::Split {
                axis: Axis::Row,
                children,
            } => children.push(Arrange::Leaf(id)),
            other => {
                let prev = std::mem::replace(other, Arrange::Leaf(id));
                *other = Arrange::Split {
                    axis: Axis::Row,
                    children: vec![prev, Arrange::Leaf(id)],
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(a: &Arrange) -> Vec<usize> {
        let mut v = Vec::new();
        a.leaves(&mut v);
        v
    }

    #[test]
    fn row_of_one_is_a_leaf() {
        assert_eq!(Arrange::row(&[2]), Arrange::Leaf(2));
    }

    #[test]
    fn split_same_axis_inserts_sibling() {
        let mut a = Arrange::row(&[0, 1]);
        assert!(a.split(1, Axis::Row, 2, false));
        assert_eq!(leaves(&a), vec![0, 1, 2]);
        assert!(matches!(
            a,
            Arrange::Split {
                axis: Axis::Row,
                ..
            }
        ));
    }

    #[test]
    fn split_cross_axis_nests() {
        let mut a = Arrange::row(&[0, 1]);
        assert!(a.split(0, Axis::Col, 2, false));
        assert_eq!(leaves(&a), vec![0, 2, 1]);
    }

    #[test]
    fn split_before_places_new_first() {
        let mut a = Arrange::Leaf(0);
        assert!(a.split(0, Axis::Row, 1, true));
        assert_eq!(leaves(&a), vec![1, 0]);
    }

    #[test]
    fn remove_collapses_to_leaf() {
        let mut a = Arrange::row(&[0, 1]);
        assert!(a.remove(0));
        assert_eq!(a, Arrange::Leaf(1));
    }

    #[test]
    fn remove_nested_collapses() {
        let mut a = Arrange::row(&[0, 1]);
        a.split(1, Axis::Col, 2, false);
        assert!(a.remove(2));
        assert_eq!(a, Arrange::row(&[0, 1]));
    }

    #[test]
    fn reconcile_prunes_hidden_and_appends_visible() {
        let mut a = Arrange::row(&[0, 1, 2]);
        a.reconcile(&[true, false, true, true]);
        assert_eq!(leaves(&a), vec![0, 2, 3]);
    }

    #[test]
    fn reconcile_rebuilds_when_root_leaf_hidden() {
        let mut a = Arrange::Leaf(0);
        a.reconcile(&[false, true]);
        assert_eq!(leaves(&a), vec![1]);
    }

    #[test]
    fn move_is_remove_then_split() {
        let mut a = Arrange::row(&[0, 1, 2]);
        assert!(a.remove(2));
        assert!(a.split(0, Axis::Col, 2, false));
        assert_eq!(leaves(&a), vec![0, 2, 1]);
    }
}
