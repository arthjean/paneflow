use crate::ids::{PaneId, SplitId};
use crate::model::{Child, Direction, Node, Pane, Split};

pub(crate) fn normalize_ratios(children: &mut [Child]) {
    let sum: f32 = children.iter().map(|child| child.ratio).sum();
    if sum > 0.0 && (sum - 1.0).abs() > f32::EPSILON {
        for child in children {
            child.ratio /= sum;
        }
    }
}

pub const MIN_RATIO: f32 = 0.01;

pub(crate) fn floored_ratios(ratios: &[f32]) -> Vec<f32> {
    let n = ratios.len();
    if n == 0 {
        return Vec::new();
    }
    let floor = MIN_RATIO.min(1.0 / n as f32);
    let mut values: Vec<f32> = ratios
        .iter()
        .map(|ratio| {
            if ratio.is_finite() {
                ratio.max(0.0)
            } else {
                0.0
            }
        })
        .collect();
    let mut pinned = vec![false; n];
    loop {
        let pinned_total = pinned.iter().filter(|pinned| **pinned).count() as f32 * floor;
        let free_total: f32 = values
            .iter()
            .zip(&pinned)
            .filter(|(_, pinned)| !**pinned)
            .map(|(value, _)| *value)
            .sum();
        let budget = 1.0 - pinned_total;
        let free = pinned.iter().filter(|pinned| !**pinned).count();
        let mut changed = false;
        for (value, pinned) in values.iter_mut().zip(pinned.iter_mut()) {
            if *pinned {
                *value = floor;
                continue;
            }
            let scaled = if free_total > 0.0 {
                *value / free_total * budget
            } else {
                budget / free as f32
            };
            if scaled < floor {
                *pinned = true;
                changed = true;
            }
            *value = scaled;
        }
        if !changed {
            break;
        }
    }
    values
}

pub(crate) fn redistribute_equal(children: &mut [Child], removed_ratio: f32) {
    if children.is_empty() {
        return;
    }
    let share = removed_ratio / children.len() as f32;
    for child in children {
        child.ratio += share;
    }
}

fn insert_sibling(children: &mut Vec<Child>, idx: usize, new_pane: Pane) {
    let Some(target) = children.get_mut(idx) else {
        return;
    };
    let half = if target.ratio.is_finite() {
        target.ratio / 2.0
    } else {
        0.5
    };
    target.ratio = half;
    children.insert(
        idx + 1,
        Child {
            node: Node::Pane(new_pane),
            ratio: half,
        },
    );
}

pub(crate) fn new_split(id: SplitId, direction: Direction, first: Node, second: Node) -> Node {
    Node::Split(Split {
        id,
        direction,
        children: vec![
            Child {
                node: first,
                ratio: 0.5,
            },
            Child {
                node: second,
                ratio: 0.5,
            },
        ],
    })
}

pub(crate) fn equal_split(id: SplitId, direction: Direction, panes: Vec<Pane>) -> Option<Node> {
    match panes.len() {
        0 => None,
        1 => panes.into_iter().next().map(Node::Pane),
        n => {
            let ratio = 1.0 / n as f32;
            Some(Node::Split(Split {
                id,
                direction,
                children: panes
                    .into_iter()
                    .map(|pane| Child {
                        node: Node::Pane(pane),
                        ratio,
                    })
                    .collect(),
            }))
        }
    }
}

pub(crate) fn main_vertical(
    main: Pane,
    others: Vec<Pane>,
    mut next_split: impl FnMut() -> SplitId,
) -> Node {
    if others.is_empty() {
        return Node::Pane(main);
    }
    let right_id = next_split();
    let Some(right) = equal_split(right_id, Direction::Horizontal, others) else {
        return Node::Pane(main);
    };
    Node::Split(Split {
        id: next_split(),
        direction: Direction::Vertical,
        children: vec![
            Child {
                node: Node::Pane(main),
                ratio: 0.5,
            },
            Child {
                node: right,
                ratio: 0.5,
            },
        ],
    })
}

pub(crate) fn tiled(panes: Vec<Pane>, mut next_split: impl FnMut() -> SplitId) -> Option<Node> {
    let n = panes.len();
    match n {
        0 => return None,
        1 => return panes.into_iter().next().map(Node::Pane),
        _ => {}
    }
    let mut rows = 1usize;
    let mut cols = 1usize;
    while rows * cols < n {
        if cols <= rows {
            cols += 1;
        } else {
            rows += 1;
        }
    }
    if rows == 1 {
        return equal_split(next_split(), Direction::Vertical, panes);
    }
    let row_ratio = 1.0 / rows as f32;
    let mut remaining = panes.into_iter();
    let mut row_children = Vec::with_capacity(rows);
    for row in 0..rows {
        let in_row = if row < rows - 1 {
            cols
        } else {
            n - cols * (rows - 1)
        };
        let row_panes: Vec<Pane> = remaining.by_ref().take(in_row).collect();
        let row_id = next_split();
        if let Some(node) = equal_split(row_id, Direction::Vertical, row_panes) {
            row_children.push(Child {
                node,
                ratio: row_ratio,
            });
        }
    }
    Some(Node::Split(Split {
        id: next_split(),
        direction: Direction::Horizontal,
        children: row_children,
    }))
}

pub(crate) struct Closed {
    pub(crate) node: Option<Node>,
    pub(crate) closed: bool,
    pub(crate) focus: Option<PaneId>,
    pub(crate) pane: Option<Pane>,
}

impl Node {
    pub fn leaf_count(&self) -> usize {
        match self {
            Node::Pane(_) => 1,
            Node::Split(split) => split.children.iter().map(|c| c.node.leaf_count()).sum(),
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.pane(id).is_some()
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        match self {
            Node::Pane(pane) => (pane.id == id).then_some(pane),
            Node::Split(split) => split.children.iter().find_map(|c| c.node.pane(id)),
        }
    }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match self {
            Node::Pane(pane) => (pane.id == id).then_some(pane),
            Node::Split(split) => split.children.iter_mut().find_map(|c| c.node.pane_mut(id)),
        }
    }

    pub fn panes(&self) -> Vec<&Pane> {
        let mut out = Vec::new();
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            Node::Pane(pane) => out.push(pane),
            Node::Split(split) => {
                for child in &split.children {
                    child.node.collect_panes(out);
                }
            }
        }
    }

    pub fn panes_mut(&mut self) -> Vec<&mut Pane> {
        let mut out = Vec::new();
        self.collect_panes_mut(&mut out);
        out
    }

    fn collect_panes_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Pane>) {
        match self {
            Node::Pane(pane) => out.push(pane),
            Node::Split(split) => {
                for child in &mut split.children {
                    child.node.collect_panes_mut(out);
                }
            }
        }
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.panes().into_iter().map(|pane| pane.id).collect()
    }

    pub fn first_pane(&self) -> Option<&Pane> {
        match self {
            Node::Pane(pane) => Some(pane),
            Node::Split(split) => split.children.first().and_then(|c| c.node.first_pane()),
        }
    }

    pub fn last_pane(&self) -> Option<&Pane> {
        match self {
            Node::Pane(pane) => Some(pane),
            Node::Split(split) => split.children.last().and_then(|c| c.node.last_pane()),
        }
    }

    pub fn split(&self, id: SplitId) -> Option<&Split> {
        match self {
            Node::Pane(_) => None,
            Node::Split(split) if split.id == id => Some(split),
            Node::Split(split) => split.children.iter().find_map(|c| c.node.split(id)),
        }
    }

    pub fn split_mut(&mut self, id: SplitId) -> Option<&mut Split> {
        if matches!(self, Node::Split(split) if split.id == id) {
            return match self {
                Node::Split(split) => Some(split),
                Node::Pane(_) => None,
            };
        }
        match self {
            Node::Pane(_) => None,
            Node::Split(split) => split.children.iter_mut().find_map(|c| c.node.split_mut(id)),
        }
    }

    pub fn split_ids(&self) -> Vec<SplitId> {
        let mut out = Vec::new();
        self.collect_split_ids(&mut out);
        out
    }

    fn collect_split_ids(&self, out: &mut Vec<SplitId>) {
        if let Node::Split(split) = self {
            out.push(split.id);
            for child in &split.children {
                child.node.collect_split_ids(out);
            }
        }
    }

    pub fn into_panes(self) -> Vec<Pane> {
        let mut out = Vec::new();
        self.drain_panes(&mut out);
        out
    }

    fn drain_panes(self, out: &mut Vec<Pane>) {
        match self {
            Node::Pane(pane) => out.push(pane),
            Node::Split(split) => {
                for child in split.children {
                    child.node.drain_panes(out);
                }
            }
        }
    }

    pub(crate) fn split_at_pane(
        &mut self,
        target: PaneId,
        direction: Direction,
        new_pane: Pane,
        next_split: &mut impl FnMut() -> SplitId,
    ) -> Result<(), Pane> {
        match self {
            Node::Pane(pane) if pane.id == target => {
                let placeholder = Node::Split(Split {
                    id: SplitId(0),
                    direction,
                    children: Vec::new(),
                });
                let old = std::mem::replace(self, placeholder);
                *self = new_split(next_split(), direction, old, Node::Pane(new_pane));
                Ok(())
            }
            Node::Pane(_) => Err(new_pane),
            Node::Split(split) => {
                if split.direction == direction
                    && let Some(idx) = split
                        .children
                        .iter()
                        .position(|c| matches!(&c.node, Node::Pane(p) if p.id == target))
                {
                    insert_sibling(&mut split.children, idx, new_pane);
                    return Ok(());
                }
                let mut carried = new_pane;
                for child in &mut split.children {
                    match child
                        .node
                        .split_at_pane(target, direction, carried, next_split)
                    {
                        Ok(()) => return Ok(()),
                        Err(back) => carried = back,
                    }
                }
                Err(carried)
            }
        }
    }

    pub(crate) fn split_first_leaf(
        &mut self,
        direction: Direction,
        new_pane: Pane,
        next_split: &mut impl FnMut() -> SplitId,
    ) {
        match self {
            Node::Pane(_) => {
                let placeholder = Node::Split(Split {
                    id: SplitId(0),
                    direction,
                    children: Vec::new(),
                });
                let old = std::mem::replace(self, placeholder);
                *self = new_split(next_split(), direction, old, Node::Pane(new_pane));
            }
            Node::Split(split) => {
                if split.direction == direction
                    && matches!(split.children.first(), Some(c) if matches!(c.node, Node::Pane(_)))
                {
                    insert_sibling(&mut split.children, 0, new_pane);
                    return;
                }
                if let Some(first) = split.children.first_mut() {
                    first.node.split_first_leaf(direction, new_pane, next_split);
                }
            }
        }
    }

    pub(crate) fn swap_panes(&mut self, a: PaneId, b: PaneId) -> bool {
        if a == b || !self.contains(a) || !self.contains(b) {
            return false;
        }
        let (Some(pane_a), Some(pane_b)) = (self.pane(a).cloned(), self.pane(b).cloned()) else {
            return false;
        };
        for pane in self.panes_mut() {
            if pane.id == a {
                *pane = pane_b.clone();
            } else if pane.id == b {
                *pane = pane_a.clone();
            }
        }
        true
    }

    pub(crate) fn close_pane(self, target: PaneId) -> Closed {
        match self {
            Node::Pane(pane) if pane.id == target => Closed {
                node: None,
                closed: true,
                focus: None,
                pane: Some(pane),
            },
            Node::Pane(pane) => Closed {
                node: Some(Node::Pane(pane)),
                closed: false,
                focus: None,
                pane: None,
            },
            Node::Split(Split {
                id,
                direction,
                children,
            }) => {
                let mut kept: Vec<Child> = Vec::with_capacity(children.len());
                let mut closed = false;
                let mut closed_idx = None;
                let mut removed_ratio = 0.0_f32;
                let mut focus = None;
                let mut removed_pane = None;
                for (i, child) in children.into_iter().enumerate() {
                    if closed {
                        kept.push(child);
                        continue;
                    }
                    let ratio = child.ratio;
                    let result = child.node.close_pane(target);
                    if result.closed {
                        closed = true;
                        closed_idx = Some(i);
                        focus = result.focus;
                        removed_pane = result.pane;
                        match result.node {
                            Some(node) => kept.push(Child { node, ratio }),
                            None => removed_ratio = ratio,
                        }
                    } else if let Some(node) = result.node {
                        kept.push(Child { node, ratio });
                    }
                }
                if !closed {
                    return Closed {
                        node: Some(Node::Split(Split {
                            id,
                            direction,
                            children: kept,
                        })),
                        closed: false,
                        focus: None,
                        pane: None,
                    };
                }
                if focus.is_none()
                    && let Some(idx) = closed_idx
                {
                    focus = if idx > 0 {
                        kept.get(idx - 1)
                            .and_then(|c| c.node.last_pane())
                            .map(|p| p.id)
                    } else {
                        kept.first().and_then(|c| c.node.first_pane()).map(|p| p.id)
                    };
                }
                match kept.len() {
                    0 => Closed {
                        node: None,
                        closed: true,
                        focus: None,
                        pane: removed_pane,
                    },
                    1 => Closed {
                        node: kept.into_iter().next().map(|c| c.node),
                        closed: true,
                        focus,
                        pane: removed_pane,
                    },
                    _ => {
                        if removed_ratio > 0.0 {
                            redistribute_equal(&mut kept, removed_ratio);
                        } else {
                            normalize_ratios(&mut kept);
                        }
                        Closed {
                            node: Some(Node::Split(Split {
                                id,
                                direction,
                                children: kept,
                            })),
                            closed: true,
                            focus,
                            pane: removed_pane,
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn remove_pane(self, target: PaneId) -> (Option<Node>, Option<Pane>) {
        match self {
            Node::Pane(pane) if pane.id == target => (None, Some(pane)),
            Node::Pane(pane) => (Some(Node::Pane(pane)), None),
            Node::Split(Split {
                id,
                direction,
                children,
            }) => {
                let mut kept = Vec::with_capacity(children.len());
                let mut removed_ratio = 0.0_f32;
                let mut removed = None;
                for child in children {
                    let ratio = child.ratio;
                    let (node, pane) = child.node.remove_pane(target);
                    if pane.is_some() {
                        removed = pane;
                    }
                    match node {
                        Some(node) => kept.push(Child { node, ratio }),
                        None => removed_ratio += ratio,
                    }
                }
                if removed.is_none() {
                    return (
                        Some(Node::Split(Split {
                            id,
                            direction,
                            children: kept,
                        })),
                        None,
                    );
                }
                let node = match kept.len() {
                    0 => None,
                    1 => kept.into_iter().next().map(|c| c.node),
                    _ => {
                        if removed_ratio > 0.0 {
                            redistribute_equal(&mut kept, removed_ratio);
                        } else {
                            normalize_ratios(&mut kept);
                        }
                        Some(Node::Split(Split {
                            id,
                            direction,
                            children: kept,
                        }))
                    }
                };
                (node, removed)
            }
        }
    }

    pub(crate) fn equalize(&mut self) {
        if let Node::Split(split) = self {
            let n = split.children.len();
            let equal = 1.0 / n as f32;
            for (i, child) in split.children.iter_mut().enumerate() {
                child.ratio = if i == n - 1 {
                    1.0 - equal * (n - 1) as f32
                } else {
                    equal
                };
                child.node.equalize();
            }
        }
    }
}
