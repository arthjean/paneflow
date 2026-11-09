use super::{ProbeKind, Reason};
use crate::engine::MonitorCounter;
use crate::feed::{Ineligible, Loss};

impl ProbeKind {
    pub fn counter(&self) -> MonitorCounter {
        match self {
            ProbeKind::Incremental => MonitorCounter::Incremental,
            ProbeKind::Deferred(_) => MonitorCounter::Deferred,
            ProbeKind::Baseline(reason) => reason.counter(),
            ProbeKind::Full(ineligible) => ineligible_counter(ineligible),
            ProbeKind::Stopped(loss) => loss_counter(*loss),
            ProbeKind::Recovering(_) => MonitorCounter::Recovering,
            ProbeKind::WorktreeDependentIndex => MonitorCounter::WorktreeDependentIndex,
            ProbeKind::MonitorError(_) => MonitorCounter::MonitorError,
            ProbeKind::Disabled => MonitorCounter::Disabled,
        }
    }
}

impl Reason {
    pub fn counter(&self) -> MonitorCounter {
        match self {
            Reason::FirstProbe => MonitorCounter::FirstProbe,
            Reason::Recovered => MonitorCounter::Recovered,
            Reason::Loss(loss) => loss_counter(*loss),
            Reason::Fence(_) => MonitorCounter::Fence,
            Reason::DrainGap => MonitorCounter::DrainGap,
            Reason::Input(_) => MonitorCounter::Input,
            Reason::IndexFormat => MonitorCounter::IndexFormat,
            Reason::IndexCarryLimit => MonitorCounter::IndexCarryLimit,
            Reason::SkipWorktree => MonitorCounter::SkipWorktree,
            Reason::DirtyLimit => MonitorCounter::DirtyLimit,
            Reason::WorktreeRoot => MonitorCounter::WorktreeRoot,
            Reason::Coverage => MonitorCounter::Coverage,
            Reason::Verification => MonitorCounter::Verification,
            Reason::Drift(_) => MonitorCounter::Drift,
        }
    }
}

fn loss_counter(loss: Loss) -> MonitorCounter {
    match loss {
        Loss::WatchSetChanged => MonitorCounter::LossWatchSetChanged,
        Loss::Rescan => MonitorCounter::LossRescan,
        Loss::Overflow => MonitorCounter::LossOverflow,
        Loss::BackendError => MonitorCounter::LossBackendError,
        Loss::BackendStopped => MonitorCounter::LossBackendStopped,
    }
}

fn ineligible_counter(ineligible: &Ineligible) -> MonitorCounter {
    match ineligible {
        Ineligible::NetworkFileSystem { .. } => MonitorCounter::IneligibleNetworkFileSystem,
        Ineligible::Fuse { .. } => MonitorCounter::IneligibleFuse,
        Ineligible::WslDrive { .. } => MonitorCounter::IneligibleWslDrive,
        Ineligible::WslNetworkPath { .. } => MonitorCounter::IneligibleWslNetworkPath,
        Ineligible::UnsupportedFileSystem { .. } => MonitorCounter::IneligibleUnsupportedFileSystem,
        Ineligible::Unidentified { .. } => MonitorCounter::IneligibleUnidentified,
        Ineligible::NoCacheDirectory => MonitorCounter::IneligibleNoCacheDirectory,
        Ineligible::MarkerDirectory { .. } => MonitorCounter::IneligibleMarkerDirectory,
        Ineligible::WatchBudget { .. } => MonitorCounter::IneligibleWatchBudget,
        Ineligible::Scan { .. } => MonitorCounter::IneligibleScan,
        Ineligible::Registration { .. } => MonitorCounter::IneligibleRegistration,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::*;
    use crate::feed::FenceError;
    use crate::monitor::Input;

    #[test]
    fn every_probe_outcome_lands_on_its_own_named_counter() {
        let path = PathBuf::from("/worktree");
        let kinds = [
            ProbeKind::Incremental,
            ProbeKind::Deferred(Reason::Loss(Loss::Overflow)),
            ProbeKind::Baseline(Reason::FirstProbe),
            ProbeKind::Baseline(Reason::Recovered),
            ProbeKind::Baseline(Reason::Loss(Loss::WatchSetChanged)),
            ProbeKind::Baseline(Reason::Loss(Loss::Rescan)),
            ProbeKind::Baseline(Reason::Loss(Loss::Overflow)),
            ProbeKind::Stopped(Loss::BackendError),
            ProbeKind::Stopped(Loss::BackendStopped),
            ProbeKind::Recovering(Loss::BackendStopped),
            ProbeKind::Baseline(Reason::Fence(FenceError::TimedOut)),
            ProbeKind::Baseline(Reason::DrainGap),
            ProbeKind::Baseline(Reason::Input(Input::Excludes)),
            ProbeKind::Baseline(Reason::IndexFormat),
            ProbeKind::Baseline(Reason::IndexCarryLimit),
            ProbeKind::Baseline(Reason::SkipWorktree),
            ProbeKind::Baseline(Reason::DirtyLimit),
            ProbeKind::Baseline(Reason::WorktreeRoot),
            ProbeKind::Baseline(Reason::Coverage),
            ProbeKind::Baseline(Reason::Verification),
            ProbeKind::Baseline(Reason::Drift(2)),
            ProbeKind::Full(Ineligible::NetworkFileSystem {
                path: path.clone(),
                kind: "nfs".into(),
            }),
            ProbeKind::Full(Ineligible::Fuse {
                path: path.clone(),
                kind: "fuse".into(),
            }),
            ProbeKind::Full(Ineligible::WslDrive {
                path: path.clone(),
                kind: "9p".into(),
            }),
            ProbeKind::Full(Ineligible::WslNetworkPath { path: path.clone() }),
            ProbeKind::Full(Ineligible::UnsupportedFileSystem {
                path: path.clone(),
                kind: "unknown".into(),
            }),
            ProbeKind::Full(Ineligible::Unidentified {
                path: path.clone(),
                cause: "statfs".into(),
            }),
            ProbeKind::Full(Ineligible::NoCacheDirectory),
            ProbeKind::Full(Ineligible::MarkerDirectory {
                path,
                cause: "denied".into(),
            }),
            ProbeKind::Full(Ineligible::WatchBudget {
                needed: 1,
                in_use: 1,
                limit: 1,
            }),
            ProbeKind::Full(Ineligible::Scan {
                cause: "gone".into(),
            }),
            ProbeKind::Full(Ineligible::Registration {
                cause: "ENOSPC".into(),
            }),
            ProbeKind::WorktreeDependentIndex,
            ProbeKind::MonitorError("broken".into()),
            ProbeKind::Disabled,
        ];
        let counters: Vec<MonitorCounter> = kinds.iter().map(ProbeKind::counter).collect();
        assert_eq!(counters, MonitorCounter::ALL);
        let names: BTreeSet<&str> = MonitorCounter::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), MonitorCounter::ALL.len());
    }
}
