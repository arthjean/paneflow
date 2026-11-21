use std::time::Instant;

use super::filter::{ShortcutHit, ShortcutSection};
use crate::keybindings::ShortcutGroup;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MotionRow {
    pub(crate) hit: ShortcutHit,
    pub(crate) key: &'static str,
    pub(crate) amount: f32,
    from: f32,
    target: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MotionSection {
    pub(crate) group: ShortcutGroup,
    pub(crate) amount: f32,
    pub(crate) rows: Vec<MotionRow>,
    from: f32,
    target: bool,
}

impl MotionRow {
    pub(crate) fn is_target(&self) -> bool {
        self.target
    }
}

impl MotionSection {
    pub(crate) fn is_target(&self) -> bool {
        self.target
    }
}

#[derive(Default)]
pub(crate) struct ShortcutMotion {
    sections: Vec<MotionSection>,
    started: Option<Instant>,
}

fn goal(target: bool) -> f32 {
    if target { 1. } else { 0. }
}

fn weave<T, K: PartialEq + Copy>(
    mut kept: Vec<T>,
    previous: &[K],
    leaving: Vec<T>,
    key: impl Fn(&T) -> K,
) -> Vec<T> {
    for item in leaving {
        let own = key(&item);
        let at = previous
            .iter()
            .position(|candidate| *candidate == own)
            .unwrap_or(0);
        let anchor = previous[..at]
            .iter()
            .rev()
            .find_map(|candidate| kept.iter().position(|placed| key(placed) == *candidate));
        kept.insert(anchor.map_or(0, |index| index + 1), item);
    }
    kept
}

impl ShortcutMotion {
    pub(crate) fn sections(&self) -> &[MotionSection] {
        &self.sections
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.started.is_some()
    }

    pub(crate) fn retarget(
        &mut self,
        target: &[ShortcutSection],
        key_of: impl Fn(usize) -> &'static str,
        now: Instant,
        animate: bool,
    ) {
        if !animate {
            self.started = None;
            self.sections = target
                .iter()
                .map(|section| MotionSection {
                    group: section.group,
                    amount: 1.,
                    from: 1.,
                    target: true,
                    rows: section
                        .hits
                        .iter()
                        .map(|hit| MotionRow {
                            hit: hit.clone(),
                            key: key_of(hit.index),
                            amount: 1.,
                            from: 1.,
                            target: true,
                        })
                        .collect(),
                })
                .collect();
            return;
        }

        self.sample(now);
        let previous = std::mem::take(&mut self.sections);
        let previous_groups: Vec<ShortcutGroup> =
            previous.iter().map(|section| section.group).collect();

        let kept: Vec<MotionSection> = target
            .iter()
            .map(|section| {
                let before = previous.iter().find(|old| old.group == section.group);
                let rows: Vec<MotionRow> = section
                    .hits
                    .iter()
                    .map(|hit| {
                        let key = key_of(hit.index);
                        let amount = before
                            .and_then(|old| old.rows.iter().find(|row| row.key == key))
                            .map_or(0., |row| row.amount);
                        MotionRow {
                            hit: hit.clone(),
                            key,
                            amount,
                            from: amount,
                            target: true,
                        }
                    })
                    .collect();
                let (rows, amount) = match before {
                    Some(old) => {
                        let previous_keys: Vec<&'static str> =
                            old.rows.iter().map(|row| row.key).collect();
                        let leaving = old
                            .rows
                            .iter()
                            .filter(|row| row.amount > 0. && !rows.iter().any(|r| r.key == row.key))
                            .map(|row| MotionRow {
                                from: row.amount,
                                target: false,
                                ..row.clone()
                            })
                            .collect();
                        (
                            weave(rows, &previous_keys, leaving, |row| row.key),
                            old.amount,
                        )
                    }
                    None => (rows, 0.),
                };
                MotionSection {
                    group: section.group,
                    amount,
                    from: amount,
                    target: true,
                    rows,
                }
            })
            .collect();

        let leaving: Vec<MotionSection> = previous
            .into_iter()
            .filter(|old| old.amount > 0. && !target.iter().any(|t| t.group == old.group))
            .map(|old| MotionSection {
                from: old.amount,
                target: false,
                rows: old
                    .rows
                    .into_iter()
                    .map(|row| MotionRow {
                        from: row.amount,
                        target: false,
                        ..row
                    })
                    .collect(),
                ..old
            })
            .collect();

        self.sections = weave(kept, &previous_groups, leaving, |section| section.group);
        self.started = Some(now);
    }

    pub(crate) fn sample(&mut self, now: Instant) -> bool {
        let Some(started) = self.started else {
            return false;
        };
        let progress = (now.saturating_duration_since(started).as_secs_f32()
            / crate::settings::search::TRANSITION_SECONDS)
            .min(1.);
        let eased = crate::ui_primitives::ease_out_cubic(progress);
        for section in &mut self.sections {
            section.amount = section.from + (goal(section.target) - section.from) * eased;
            for row in &mut section.rows {
                row.amount = row.from + (goal(row.target) - row.from) * eased;
            }
        }
        if progress < 1. {
            return true;
        }
        self.sections.retain(|section| section.target);
        for section in &mut self.sections {
            section.rows.retain(|row| row.target);
        }
        self.started = None;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::{ShortcutEntry, ShortcutGroup};
    use crate::settings::tabs::shortcuts::ShortcutFilter;
    use std::time::Duration;

    const KEYS: [&str; 4] = ["split", "zoom", "tab", "settings"];

    fn entries() -> Vec<ShortcutEntry> {
        [
            ("Split right", ShortcutGroup::Panes),
            ("Toggle zoom", ShortcutGroup::Panes),
            ("New tab", ShortcutGroup::Tabs),
            ("Open settings", ShortcutGroup::Application),
        ]
        .into_iter()
        .zip(KEYS)
        .map(|((description, group), action_name)| ShortcutEntry {
            key: "Unassigned".to_string(),
            raw_key: None,
            extra_raw_keys: Vec::new(),
            customized: false,
            description: description.to_string(),
            action_name,
            group,
            search_key: String::new(),
        })
        .collect()
    }

    fn target(query: &str) -> Vec<ShortcutSection> {
        ShortcutFilter::parse(query).sections(&entries())
    }

    fn shown(motion: &ShortcutMotion) -> Vec<(&'static str, f32)> {
        motion
            .sections()
            .iter()
            .flat_map(|section| section.rows.iter().map(|row| (row.key, row.amount)))
            .collect()
    }

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn without_animation_the_target_shows_at_once() {
        let mut motion = ShortcutMotion::default();
        motion.retarget(&target("zoom"), |i| KEYS[i], Instant::now(), false);
        assert_eq!(shown(&motion), [("zoom", 1.)]);
        assert!(!motion.is_animating());
    }

    #[test]
    fn leaving_rows_fold_in_place_and_go_at_the_end() {
        let start = Instant::now();
        let mut motion = ShortcutMotion::default();
        motion.retarget(&target(""), |i| KEYS[i], start, false);
        motion.retarget(&target("zoom"), |i| KEYS[i], start, true);
        assert_eq!(
            motion
                .sections()
                .iter()
                .map(|s| s.group)
                .collect::<Vec<_>>(),
            [
                ShortcutGroup::Panes,
                ShortcutGroup::Tabs,
                ShortcutGroup::Application
            ],
            "leaving sections keep their place while they fold"
        );
        assert!(motion.sample(at(start, 90)));
        let halfway = shown(&motion);
        assert_eq!(halfway[1], ("zoom", 1.));
        assert!(halfway[0].1 > 0. && halfway[0].1 < 1.);
        assert!(!motion.sample(at(start, 400)));
        assert_eq!(shown(&motion), [("zoom", 1.)]);
    }

    #[test]
    fn entering_rows_grow_from_nothing() {
        let start = Instant::now();
        let mut motion = ShortcutMotion::default();
        motion.retarget(&target("zoom"), |i| KEYS[i], start, false);
        motion.retarget(&target(""), |i| KEYS[i], start, true);
        assert_eq!(
            shown(&motion),
            [("split", 0.), ("zoom", 1.), ("tab", 0.), ("settings", 0.)]
        );
        motion.sample(at(start, 60));
        let early = shown(&motion);
        assert!(early[2].1 > 0. && early[2].1 < 1.);
        assert!(motion.sections()[1].amount > 0.);
        motion.sample(at(start, 400));
        assert!(shown(&motion).iter().all(|(_, amount)| *amount == 1.));
    }

    #[test]
    fn a_new_query_reverses_from_where_the_rows_are() {
        let start = Instant::now();
        let mut motion = ShortcutMotion::default();
        motion.retarget(&target(""), |i| KEYS[i], start, false);
        motion.retarget(&target("zoom"), |i| KEYS[i], start, true);
        motion.sample(at(start, 60));
        let folding = shown(&motion)[0].1;
        motion.retarget(&target(""), |i| KEYS[i], at(start, 60), true);
        assert_eq!(shown(&motion)[0], ("split", folding));
        motion.sample(at(start, 61));
        assert!(shown(&motion)[0].1 >= folding);
    }

    #[test]
    fn weave_keeps_leaving_items_after_their_old_neighbor() {
        let woven = weave(vec!['a', 'd'], &['a', 'b', 'c', 'd'], vec!['b', 'c'], |c| {
            *c
        });
        assert_eq!(woven, ['a', 'b', 'c', 'd']);
        let woven = weave(vec!['d'], &['a', 'b', 'd'], vec!['a', 'b'], |c| *c);
        assert_eq!(woven, ['a', 'b', 'd']);
    }
}
