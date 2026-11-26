use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::path::PathBuf;

use paneflow_config::schema::{SessionGeneration, SessionId};
use paneflow_program_resume::{
    AttemptFailure, AttemptId, CwdSource, EXIT_GRACE_MS, ProcessStamp, Record, RecordId,
    SandboxStamp,
};
use paneflow_resume_handoff::{CwdSource as HandoffSource, Handoff};

use super::{
    AttemptOutcome, Durability, HostError, PreparedResume, ResumeAttemptReport, ResumeCandidate,
    ResumeRefusal, ResumeRequest, SessionHost, SessionManifest, SessionSummary,
};
use crate::manifest::now_ms;
use crate::persistence::WriteClass;
use crate::program_resume::{self, OwnerExit, PaneToken, ReportSequence};
use crate::resume_launch::Delivery;

pub(super) type OwnerKey = (SessionId, ProcessStamp);

const EXIT_GRACE: Duration = Duration::from_millis(EXIT_GRACE_MS);

impl SessionHost {
    pub(super) fn ingest_program_resume(
        &self,
        manifest: &Arc<Mutex<SessionManifest>>,
        durability: &Durability,
        generation: SessionGeneration,
        token: &PaneToken,
        sequence: ReportSequence,
    ) {
        let (session, outcome) = {
            let guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.generation != generation || !guard.lifecycle.is_running() {
                return;
            }
            (
                guard.session.clone(),
                program_resume::classify(&sequence, token, &guard.program_resume),
            )
        };
        log::debug!(
            "paneflow-host: session {session}: OSC 7577 {}",
            program_resume::describe(&outcome)
        );
        let admission = match &outcome {
            paneflow_program_resume::Outcome::Accepted(report) => {
                program_resume::admit(report, &sequence.origin)
            }
            _ => None,
        };
        let owner = program_resume::accepted_owner(&outcome, admission.as_ref());
        let lineage: Vec<u32> = admission
            .as_ref()
            .map(|admission| admission.ancestors.iter().map(|stamp| stamp.pid).collect())
            .unwrap_or_default();
        let now = now_ms();
        let mut changed = false;
        self.commit(
            manifest,
            durability,
            Some(generation),
            WriteClass::Metadata,
            |record| {
                changed = record.program_resume.apply(outcome, |_| admission, now);
                if changed {
                    program_resume::fit_store_budget(&mut record.program_resume);
                }
            },
        );
        if let Some(owner) = owner
            && changed
        {
            self.watch_resume_owner(&session, manifest, durability, owner);
            self.settle_reported_attempt(&session, owner.pid, &lineage);
        }
    }

    fn settle_reported_attempt(&self, session: &SessionId, pid: u32, lineage: &[u32]) {
        if let Some(report) = self
            .lock_sessions()
            .get_mut(session)
            .and_then(|record| record.resume_attempt.as_mut())
            .filter(|report| {
                report.outcome == AttemptOutcome::Started
                    && report
                        .pid
                        .is_some_and(|started| started == pid || lineage.contains(&started))
            })
        {
            report.outcome = AttemptOutcome::Settled;
            report.pid = Some(pid);
        }
    }

    pub(super) fn watch_resume_owner(
        &self,
        session: &SessionId,
        manifest: &Arc<Mutex<SessionManifest>>,
        durability: &Durability,
        owner: ProcessStamp,
    ) {
        let key: OwnerKey = (session.clone(), owner);
        if !self.lock_resume_watches().insert(key.clone()) {
            return;
        }
        let exit = OwnerExit::arm(owner);
        let host = self.weak_self();
        let manifest = Arc::clone(manifest);
        let durability = Arc::clone(durability);
        let watched = key.clone();
        let spawned = std::thread::Builder::new()
            .name("paneflow-host-resume-owner".into())
            .stack_size(program_resume::OWNER_WATCH_STACK_BYTES)
            .spawn(move || {
                let observed = exit.wait();
                log::debug!(
                    "paneflow-host: OSC 7577 owner {} ended ({observed:?})",
                    watched.1.pid
                );
                let marked = host.upgrade().is_some_and(|host| {
                    host.mark_resume_owner_exited(&manifest, &durability, watched.1)
                });
                if marked {
                    std::thread::sleep(EXIT_GRACE);
                    if let Some(host) = host.upgrade() {
                        host.remove_exited_resume_records(&manifest, &durability);
                    }
                }
                if let Some(host) = host.upgrade() {
                    host.lock_resume_watches().remove(&watched);
                }
            });
        if let Err(error) = spawned {
            log::warn!(
                "paneflow-host: the exit of OSC 7577 owner {} cannot be watched ({error}); its record stays until the session ends",
                owner.pid
            );
            self.lock_resume_watches().remove(&key);
        }
    }

    fn mark_resume_owner_exited(
        &self,
        manifest: &Arc<Mutex<SessionManifest>>,
        durability: &Durability,
        owner: ProcessStamp,
    ) -> bool {
        if self.is_shutting_down() {
            return false;
        }
        let now = now_ms();
        let mut marked = false;
        self.commit(manifest, durability, None, WriteClass::Metadata, |record| {
            marked = record.program_resume.owner_exited(owner, now);
        });
        marked
    }

    fn remove_exited_resume_records(
        &self,
        manifest: &Arc<Mutex<SessionManifest>>,
        durability: &Durability,
    ) {
        if self.is_shutting_down() {
            return;
        }
        let now = now_ms();
        self.commit(manifest, durability, None, WriteClass::Metadata, |record| {
            let _ = record.program_resume.remove_exited(now);
        });
    }

    pub(super) fn discard_program_resume(
        &self,
        session: &SessionId,
    ) -> Result<SessionSummary, HostError> {
        let target = {
            let sessions = self.lock_sessions();
            sessions
                .get(session)
                .map(|record| (Arc::clone(&record.manifest), Arc::clone(&record.durability)))
        };
        if let Some((manifest, durability)) = target {
            self.commit(&manifest, &durability, None, WriteClass::Final, |record| {
                let _ = record.program_resume.discard_all();
            });
        }
        self.inspect(session)
    }

    fn lock_resume_watches(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashSet<OwnerKey>> {
        self.resume_watches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(crate) fn watched_resume_owners(&self) -> usize {
        self.lock_resume_watches().len()
    }
}

pub(super) struct ShellView<'a> {
    pub shell: &'a str,
    pub args: &'a [String],
    pub env: &'a std::collections::BTreeMap<String, String>,
}

impl<'a> ShellView<'a> {
    pub(super) fn of(manifest: &'a SessionManifest) -> Self {
        Self {
            shell: &manifest.launch.shell,
            args: &manifest.launch.args,
            env: &manifest.launch.env,
        }
    }

    fn delivery(&self) -> Option<Delivery> {
        crate::resume_launch::delivery(self.shell, self.args, self.env)
    }
}

pub(super) struct ResumePlan {
    pub attempt: AttemptId,
    pub handoff: Handoff,
    pub record: Option<(RecordId, u64)>,
    pub delivery: Delivery,
    pub launcher: PathBuf,
    pub shell_cwd: Option<PathBuf>,
}

impl ResumePlan {
    pub(super) fn report(&self, outcome: AttemptOutcome) -> ResumeAttemptReport {
        ResumeAttemptReport {
            attempt: Some(self.attempt.clone()),
            record: self.record.map(|(id, _)| id),
            outcome,
            reason: None,
            program: self.handoff.argv.first().cloned(),
            pid: None,
        }
    }
}

pub(super) fn refused_report(
    refusal: ResumeRefusal,
    record: Option<RecordId>,
) -> ResumeAttemptReport {
    ResumeAttemptReport {
        attempt: None,
        record,
        outcome: AttemptOutcome::Refused,
        reason: Some(refusal.reason().to_owned()),
        program: None,
        pid: None,
    }
}

pub(super) fn restart_cwd(prior: &SessionManifest) -> PathBuf {
    let resumed = prior
        .current_cwd
        .as_deref()
        .filter(|cwd| std::path::Path::new(cwd).is_dir())
        .unwrap_or(prior.cwd.as_str());
    super::resolve_cwd(Some(resumed))
}

fn sandbox_now(manifest: &SessionManifest, live: bool) -> Option<SandboxStamp> {
    manifest.launch.sandbox.then(|| SandboxStamp {
        root: if live {
            manifest.cwd.clone()
        } else {
            restart_cwd(manifest).display().to_string()
        },
        unix_scope: manifest.launch.sandbox_unix_scope,
    })
}

fn handoff_source(source: CwdSource) -> HandoffSource {
    match source {
        CwdSource::Report => HandoffSource::Report,
        CwdSource::Owner => HandoffSource::Owner,
        CwdSource::Spawn => HandoffSource::Spawn,
    }
}

pub(super) fn record_block(
    view: &ShellView<'_>,
    manifest: &SessionManifest,
    record: &Record,
    live: bool,
) -> Option<ResumeRefusal> {
    if record.first_process {
        return Some(ResumeRefusal::FirstProcess);
    }
    if record.sandbox != sandbox_now(manifest, live) {
        return Some(ResumeRefusal::SandboxChanged);
    }
    if view.delivery().is_none() {
        return Some(ResumeRefusal::ShellUnsupported);
    }
    if record.cwd_source == CwdSource::Report && !std::path::Path::new(&record.cwd).is_dir() {
        return Some(ResumeRefusal::CwdMissing);
    }
    None
}

fn valid_command(argv: &[String], cwd: &str) -> bool {
    let Some(program) = argv.first() else {
        return false;
    };
    let path = std::path::Path::new(program);
    let named = path.is_absolute() || !(program.is_empty() || program.contains(['/', '\\']));
    named
        && std::path::Path::new(cwd).is_absolute()
        && argv
            .iter()
            .chain([&cwd.to_owned()])
            .all(|text| !text.contains('\0'))
}

impl SessionHost {
    pub(super) fn plan_resume(
        &self,
        view: &ShellView<'_>,
        manifest: &SessionManifest,
        request: &ResumeRequest,
        live: bool,
    ) -> Result<ResumePlan, ResumeRefusal> {
        let launcher = self
            .helper_dir()
            .map(crate::resume_launch::launcher_path)
            .filter(|path| path.is_file())
            .ok_or(ResumeRefusal::LauncherMissing)?;
        let delivery = view.delivery().ok_or(ResumeRefusal::ShellUnsupported)?;
        let attempt =
            crate::resume_launch::new_attempt().ok_or(ResumeRefusal::HandoffUnwritable)?;
        let sandboxed = manifest.launch.sandbox;
        let shell_cwd = |cwd: &str| {
            let path = PathBuf::from(cwd);
            (!sandboxed && path.is_dir()).then_some(path)
        };
        match (request.record, request.argv.as_ref()) {
            (Some(id), None) => {
                let record = manifest
                    .program_resume
                    .get(id)
                    .ok_or(ResumeRefusal::UnknownRecord)?;
                if let Some(block) = record_block(view, manifest, record, live) {
                    return Err(block);
                }
                if !record.settled() && !request.confirmed {
                    return Err(ResumeRefusal::Unsettled);
                }
                Ok(ResumePlan {
                    handoff: Handoff {
                        attempt: attempt.to_string(),
                        cwd: record.cwd.clone(),
                        cwd_source: handoff_source(record.cwd_source),
                        argv: record.argv.clone(),
                    },
                    attempt,
                    record: Some((id, record.revision)),
                    delivery,
                    launcher,
                    shell_cwd: shell_cwd(&record.cwd),
                })
            }
            (None, Some(argv)) => {
                let cwd = request.cwd.as_deref().unwrap_or_default();
                if !valid_command(argv, cwd) {
                    return Err(ResumeRefusal::InvalidCommand);
                }
                Ok(ResumePlan {
                    handoff: Handoff {
                        attempt: attempt.to_string(),
                        cwd: cwd.to_owned(),
                        cwd_source: HandoffSource::Report,
                        argv: argv.clone(),
                    },
                    attempt,
                    record: None,
                    delivery,
                    launcher,
                    shell_cwd: shell_cwd(cwd),
                })
            }
            _ => Err(ResumeRefusal::InvalidCommand),
        }
    }

    pub(super) fn write_resume_handoff(
        &self,
        session: &SessionId,
        plan: &ResumePlan,
    ) -> Result<PathBuf, ResumeRefusal> {
        let dir = self.session_data_dir(session);
        crate::resume_launch::remove_handoffs(&dir);
        crate::resume_launch::write_handoff(&dir, &plan.handoff, &plan.attempt).map_err(|error| {
            log::warn!(
                "paneflow-host: session {session}: the resume handoff could not be written: {error}"
            );
            ResumeRefusal::HandoffUnwritable
        })
    }

    pub(super) fn resume_candidate(
        &self,
        manifest: &SessionManifest,
        live: bool,
    ) -> Option<ResumeCandidate> {
        let record = manifest.program_resume.candidate()?;
        let view = ShellView {
            shell: &manifest.launch.shell,
            args: &crate::resume_launch::integration_args(
                &manifest.launch.shell,
                &manifest.launch.args,
            ),
            env: &manifest.launch.env,
        };
        Some(ResumeCandidate {
            record: record.id,
            argv: record.argv.clone(),
            cwd: record.cwd.clone(),
            cwd_source: record.cwd_source,
            owner_pid: record.owner.pid,
            accepted_at_ms: record.accepted_at_ms,
            settled: record.settled(),
            blocked: record_block(&view, manifest, record, live),
        })
    }

    pub(super) fn with_resume_candidate(&self, mut summary: SessionSummary) -> SessionSummary {
        let live = summary.live || summary.pending_launch;
        summary.resume_candidate = self.resume_candidate(&summary.manifest, live);
        summary
    }

    pub fn prepare_resume(
        &self,
        session: &SessionId,
        request: &ResumeRequest,
    ) -> Result<PreparedResume, HostError> {
        let (manifest, durability, snapshot, live) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let snapshot = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            (
                Arc::clone(&record.manifest),
                Arc::clone(&record.durability),
                snapshot,
                record.is_live(),
            )
        };
        if !live {
            return Err(HostError::SessionNotLive(session.clone()));
        }
        let plan = self
            .plan_resume(&ShellView::of(&snapshot), &snapshot, request, true)
            .map_err(|refusal| self.refuse_resume(session, refusal, request.record))?;
        let handoff = self
            .write_resume_handoff(session, &plan)
            .map_err(|refusal| self.refuse_resume(session, refusal, request.record))?;
        if let Some((id, revision)) = plan.record {
            let mut bound = false;
            self.commit(
                &manifest,
                &durability,
                Some(snapshot.generation),
                WriteClass::Critical,
                |current| {
                    let unchanged = current
                        .program_resume
                        .get(id)
                        .is_some_and(|record| record.revision == revision);
                    bound = unchanged
                        && current
                            .program_resume
                            .begin_attempt(id, plan.attempt.clone())
                            .is_ok();
                },
            );
            if !bound {
                let _ = std::fs::remove_file(&handoff);
                return Err(self.refuse_resume(session, ResumeRefusal::UnknownRecord, Some(id)));
            }
        }
        self.note_resume_attempt(session, plan.report(AttemptOutcome::Prepared));
        Ok(PreparedResume {
            attempt: plan.attempt.clone(),
            launcher: plan.launcher.display().to_string(),
            handoff: handoff.display().to_string(),
            record: plan.record.map(|(id, _)| id),
        })
    }

    fn refuse_resume(
        &self,
        session: &SessionId,
        refusal: ResumeRefusal,
        record: Option<RecordId>,
    ) -> HostError {
        self.note_resume_attempt(session, refused_report(refusal, record));
        HostError::ResumeRefused(refusal)
    }

    pub(super) fn note_resume_attempt(&self, session: &SessionId, report: ResumeAttemptReport) {
        if let Some(record) = self.lock_sessions().get_mut(session) {
            record.resume_attempt = Some(report);
        }
    }

    pub fn discard_resume(
        &self,
        session: &SessionId,
        record: RecordId,
    ) -> Result<SessionSummary, HostError> {
        let (manifest, durability) = {
            let sessions = self.lock_sessions();
            let entry = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            (Arc::clone(&entry.manifest), Arc::clone(&entry.durability))
        };
        let mut discarded = false;
        self.commit(
            &manifest,
            &durability,
            None,
            WriteClass::Critical,
            |current| {
                discarded = current.program_resume.discard(record);
            },
        );
        if !discarded {
            return Err(HostError::ResumeRefused(ResumeRefusal::UnknownRecord));
        }
        self.inspect(session)
    }

    pub fn resume_attempt(
        &self,
        session: &SessionId,
        attempt: &AttemptId,
        outcome: ReportedAttempt,
    ) -> Result<ResumeAttemptReport, HostError> {
        let (manifest, durability, runtime, pending) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            (
                Arc::clone(&record.manifest),
                Arc::clone(&record.durability),
                record.runtime.clone(),
                record.resume_attempt.clone(),
            )
        };
        let pending = pending
            .filter(|report| report.attempt.as_ref() == Some(attempt))
            .filter(|report| {
                matches!(
                    report.outcome,
                    AttemptOutcome::Prepared | AttemptOutcome::Started
                )
            })
            .ok_or(HostError::ResumeRefused(ResumeRefusal::UnknownAttempt))?;
        let mut updated = pending.clone();
        match outcome {
            ReportedAttempt::Started { pid } => {
                if pending.outcome != AttemptOutcome::Prepared {
                    return Err(HostError::ResumeRefused(ResumeRefusal::UnknownAttempt));
                }
                let (spawn_cwd, generation) = {
                    let guard = manifest
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (guard.cwd.clone(), guard.generation)
                };
                let origin = runtime
                    .as_deref()
                    .filter(|runtime| runtime.is_live())
                    .and_then(|runtime| pane_origin(runtime, spawn_cwd))
                    .ok_or(HostError::ResumeRefused(ResumeRefusal::NotInPane))?;
                let admission = crate::program_resume::admit_pid(pid, &origin)
                    .ok_or(HostError::ResumeRefused(ResumeRefusal::NotInPane))?;
                let owner = ProcessStamp {
                    pid,
                    started_at: admission.started_at,
                };
                if pending.record.is_some() {
                    let mut moved = false;
                    self.commit(
                        &manifest,
                        &durability,
                        Some(generation),
                        WriteClass::Critical,
                        |current| {
                            moved = current
                                .program_resume
                                .attempt_started(attempt, owner, admission.ancestors.clone())
                                .is_ok();
                        },
                    );
                    if !moved {
                        return Err(HostError::ResumeRefused(ResumeRefusal::UnknownAttempt));
                    }
                    self.watch_resume_owner(session, &manifest, &durability, owner);
                }
                updated.outcome = AttemptOutcome::Started;
                updated.pid = Some(pid);
            }
            ReportedAttempt::Failed(failure) => {
                if pending.record.is_some() {
                    let mut dropped = false;
                    self.commit(
                        &manifest,
                        &durability,
                        None,
                        WriteClass::Critical,
                        |current| {
                            dropped = current
                                .program_resume
                                .attempt_failed(attempt, failure)
                                .is_ok();
                        },
                    );
                    if !dropped {
                        return Err(HostError::ResumeRefused(ResumeRefusal::UnknownAttempt));
                    }
                }
                updated.outcome = AttemptOutcome::Failed;
                updated.reason = Some(attempt_failure_reason(failure).to_owned());
            }
        }
        log::info!(
            "paneflow-host: session {session}: resume attempt {attempt} {:?}{}",
            updated.outcome,
            updated
                .reason
                .as_deref()
                .map(|reason| format!(" ({reason})"))
                .unwrap_or_default()
        );
        self.note_resume_attempt(session, updated.clone());
        Ok(updated)
    }

    pub(super) fn settle_started_attempt(&self, session: &SessionId) {
        let target = {
            let mut sessions = self.lock_sessions();
            let Some(record) = sessions.get_mut(session) else {
                return;
            };
            let Some(report) = record
                .resume_attempt
                .as_mut()
                .filter(|report| report.outcome == AttemptOutcome::Started)
            else {
                return;
            };
            report.outcome = AttemptOutcome::Settled;
            (Arc::clone(&record.manifest), Arc::clone(&record.durability))
        };
        let (manifest, durability) = target;
        self.commit(
            &manifest,
            &durability,
            None,
            WriteClass::Metadata,
            |current| {
                let _ = current.program_resume.settle_on_input();
            },
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportedAttempt {
    Started { pid: u32 },
    Failed(AttemptFailure),
}

pub fn attempt_failure_reason(failure: AttemptFailure) -> &'static str {
    match failure {
        AttemptFailure::NotFound => "not_found",
        AttemptFailure::NotExecutable => "not_executable",
        AttemptFailure::CwdMissing => "cwd_missing",
        AttemptFailure::HandoffInvalid => "handoff_invalid",
    }
}

pub fn parse_attempt_failure(reason: &str) -> Option<AttemptFailure> {
    match reason {
        "not_found" => Some(AttemptFailure::NotFound),
        "not_executable" => Some(AttemptFailure::NotExecutable),
        "cwd_missing" => Some(AttemptFailure::CwdMissing),
        "handoff_invalid" => Some(AttemptFailure::HandoffInvalid),
        _ => None,
    }
}

fn pane_origin(
    runtime: &crate::runtime::SessionRuntime,
    spawn_cwd: String,
) -> Option<crate::program_resume::PaneOrigin> {
    let process = runtime.process();
    Some(crate::program_resume::PaneOrigin {
        leader: ProcessStamp {
            pid: process.pid,
            started_at: process.started_at?,
        },
        process_session: runtime.process_session(),
        spawn_cwd,
        sandbox: None,
        #[cfg(windows)]
        job: runtime.pane_job(),
    })
}
