use std::cmp::Ordering;
use std::collections::BTreeSet;

use gix::bstr::ByteSlice;
use gix::index::entry::Flags;

pub(crate) const CARRY_LIMIT_PERCENT: usize = 10;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Carried {
    pub(crate) changed: BTreeSet<Vec<u8>>,
    pub(crate) added_or_removed: BTreeSet<Vec<u8>>,
    pub(crate) stat_only: BTreeSet<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    SkipWorktree,
    TooManyChanges,
}

pub(crate) fn carry(old: &gix::index::State, new: &gix::index::State) -> Result<Carried, Refused> {
    let mut carried = Carried::default();
    let (old_entries, new_entries) = (old.entries(), new.entries());
    let (mut left, mut right) = (0, 0);
    loop {
        let order = match (old_entries.get(left), new_entries.get(right)) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(before), Some(after)) => {
                (before.path(old), before.stage_raw()).cmp(&(after.path(new), after.stage_raw()))
            }
        };
        match order {
            Ordering::Less => {
                let before = &old_entries[left];
                refuse_skip_worktree(before.flags)?;
                let path = before.path(old).as_bytes().to_vec();
                carried.added_or_removed.insert(path.clone());
                carried.changed.insert(path);
                left += 1;
            }
            Ordering::Greater => {
                let after = &new_entries[right];
                refuse_skip_worktree(after.flags)?;
                let path = after.path(new).as_bytes().to_vec();
                carried.added_or_removed.insert(path.clone());
                carried.changed.insert(path);
                right += 1;
            }
            Ordering::Equal => {
                let (before, after) = (&old_entries[left], &new_entries[right]);
                if (before.flags & Flags::SKIP_WORKTREE) != (after.flags & Flags::SKIP_WORKTREE) {
                    return Err(Refused::SkipWorktree);
                }
                let path = after.path(new).as_bytes();
                if before.id != after.id || before.mode != after.mode || before.flags != after.flags
                {
                    carried.changed.insert(path.to_vec());
                } else if before.stat != after.stat {
                    carried.stat_only.insert(path.to_vec());
                }
                left += 1;
                right += 1;
            }
        }
    }
    let entries = new_entries.len().max(old_entries.len()).max(1);
    if carried.changed.len() * 100 > entries * CARRY_LIMIT_PERCENT {
        return Err(Refused::TooManyChanges);
    }
    Ok(carried)
}

fn refuse_skip_worktree(flags: Flags) -> Result<(), Refused> {
    if flags.contains(Flags::SKIP_WORKTREE) {
        return Err(Refused::SkipWorktree);
    }
    Ok(())
}
