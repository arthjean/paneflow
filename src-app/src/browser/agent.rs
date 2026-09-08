use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use paneflow_browser_protocol::{
    AgentAccess, BrowserError, BrowserId, Document, Event, MAX_AGENT_CAPTURE_BYTES,
    MAX_AGENT_CAPTURE_CHUNK_BYTES, MAX_AGENT_CAPTURE_PIXELS, MAX_AGENT_LEASE_MS,
    MAX_AGENT_OPERATION_RECORDS, MAX_AGENT_OPERATIONS_PER_WORKSPACE, MAX_AGENT_OPERATIONS_TOTAL,
    MAX_AGENT_TYPING_BYTES, OperationId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserContextSelection {
    pub(crate) document: Document,
    pub(crate) node_id: i64,
    pub(crate) url: Option<String>,
    pub(crate) rect: [i32; 4],
    pub(crate) summary: String,
    pub(crate) text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentError {
    ScopeRequired,
    AccessDenied,
    StaleTarget,
    Busy,
    LimitReached,
    Cancelled,
    TimedOut,
    Replay,
    InvalidInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationState {
    Pending,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl OperationState {
    fn name(self) -> &'static str {
        match self {
            Self::Pending => "accepted",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Clone, Debug)]
struct Operation {
    workspace_id: u64,
    browser: BrowserId,
    generation: u64,
    permission_generation: u64,
    mutation: bool,
    deadline: Instant,
    state: OperationState,
    deferred: bool,
    transports: BTreeSet<OperationId>,
    error: Option<BrowserError>,
    capture: Option<AgentCapture>,
}

#[derive(Clone, Debug)]
struct AgentCapture {
    mime: String,
    width: u32,
    height: u32,
    data: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentLease {
    pub(crate) id: String,
    pub(crate) workspace_id: u64,
    pub(crate) browser: BrowserId,
    pub(crate) generation: u64,
    permission_generation: u64,
    mutation: bool,
    deferred: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentOperationInfo {
    pub(crate) id: String,
    pub(crate) workspace_id: u64,
    pub(crate) browser: BrowserId,
    pub(crate) generation: u64,
    pub(crate) status: &'static str,
    pub(crate) error: Option<BrowserError>,
    pub(crate) capture: Option<AgentCaptureInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentCaptureInfo {
    pub(crate) mime: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) encoded_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentCaptureChunk {
    pub(crate) mime: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) offset: usize,
    pub(crate) next_offset: usize,
    pub(crate) total_bytes: usize,
    pub(crate) data: String,
    pub(crate) done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgentPermission {
    pub(crate) access: AgentAccess,
    pub(crate) generation: u64,
}

pub(crate) struct AgentService {
    permissions: BTreeMap<u64, AgentPermission>,
    operations: BTreeMap<String, Operation>,
    transport_operations: BTreeMap<(BrowserId, OperationId), String>,
    next_operation: u64,
}

struct AgentBegin {
    workspace_id: u64,
    browser: BrowserId,
    generation: u64,
    mutation: bool,
    text_bytes: usize,
    deadline_ms: u64,
    deferred: bool,
}

impl Default for AgentService {
    fn default() -> Self {
        Self {
            permissions: BTreeMap::new(),
            operations: BTreeMap::new(),
            transport_operations: BTreeMap::new(),
            next_operation: 1,
        }
    }
}

impl AgentService {
    pub(crate) fn permission(&self, workspace_id: u64) -> AgentPermission {
        self.permissions
            .get(&workspace_id)
            .copied()
            .unwrap_or(AgentPermission {
                access: AgentAccess::Disabled,
                generation: 0,
            })
    }

    pub(crate) fn set_access(&mut self, workspace_id: u64, access: AgentAccess) -> AgentPermission {
        let current = self.permission(workspace_id);
        if current.access != access {
            for operation in self.operations.values_mut() {
                let cancelled = operation.workspace_id == workspace_id
                    && operation.state == OperationState::Pending
                    && (!access.permits_read()
                        || (operation.mutation && !access.permits_interact()));
                if cancelled {
                    operation.state = OperationState::Cancelled;
                }
            }
        }
        let permission = AgentPermission {
            access,
            generation: if current.access == access {
                current.generation
            } else {
                current.generation.saturating_add(1)
            },
        };
        self.permissions.insert(workspace_id, permission);
        permission
    }

    pub(crate) fn require_scope(scope: Option<u64>) -> Result<u64, AgentError> {
        scope.ok_or(AgentError::ScopeRequired)
    }

    pub(crate) fn require_read(&self, workspace_id: u64) -> Result<AgentPermission, AgentError> {
        let permission = self.permission(workspace_id);
        permission
            .access
            .permits_read()
            .then_some(permission)
            .ok_or(AgentError::AccessDenied)
    }

    pub(crate) fn begin(
        &mut self,
        workspace_id: u64,
        browser: BrowserId,
        generation: u64,
        mutation: bool,
        text_bytes: usize,
        deadline_ms: u64,
    ) -> Result<AgentLease, AgentError> {
        self.begin_with_mode(AgentBegin {
            workspace_id,
            browser,
            generation,
            mutation,
            text_bytes,
            deadline_ms,
            deferred: false,
        })
    }

    pub(crate) fn begin_deferred(
        &mut self,
        workspace_id: u64,
        browser: BrowserId,
        generation: u64,
        mutation: bool,
        text_bytes: usize,
        deadline_ms: u64,
    ) -> Result<AgentLease, AgentError> {
        self.begin_with_mode(AgentBegin {
            workspace_id,
            browser,
            generation,
            mutation,
            text_bytes,
            deadline_ms,
            deferred: true,
        })
    }

    fn begin_with_mode(&mut self, request: AgentBegin) -> Result<AgentLease, AgentError> {
        if request.mutation && request.text_bytes > MAX_AGENT_TYPING_BYTES {
            return Err(AgentError::InvalidInput);
        }
        let permission = self.permission(request.workspace_id);
        let permitted = if request.mutation {
            permission.access.permits_interact()
        } else {
            permission.access.permits_read()
        };
        if !permitted {
            return Err(AgentError::AccessDenied);
        }
        self.expire(Instant::now());
        while self.operations.len() >= MAX_AGENT_OPERATION_RECORDS {
            let Some(id) = self
                .operations
                .iter()
                .find(|(_, operation)| operation.state != OperationState::Pending)
                .map(|(id, _)| id.clone())
            else {
                return Err(AgentError::LimitReached);
            };
            if let Some(operation) = self.operations.remove(&id) {
                for transport in operation.transports {
                    self.transport_operations
                        .remove(&(operation.browser.clone(), transport));
                }
            }
        }
        let active = self
            .operations
            .values()
            .filter(|operation| operation.state == OperationState::Pending);
        let total = active.clone().count();
        if total >= MAX_AGENT_OPERATIONS_TOTAL {
            return Err(AgentError::LimitReached);
        }
        if active
            .filter(|operation| operation.workspace_id == request.workspace_id)
            .count()
            >= MAX_AGENT_OPERATIONS_PER_WORKSPACE
        {
            return Err(AgentError::Busy);
        }
        if request.mutation
            && self.operations.values().any(|operation| {
                operation.state == OperationState::Pending
                    && operation.workspace_id == request.workspace_id
                    && operation.browser == request.browser
                    && operation.mutation
            })
        {
            return Err(AgentError::Busy);
        }
        let id = format!("agent-{}", self.next_operation);
        self.next_operation = self.next_operation.wrapping_add(1).max(1);
        let operation = Operation {
            workspace_id: request.workspace_id,
            browser: request.browser.clone(),
            generation: request.generation,
            permission_generation: permission.generation,
            mutation: request.mutation,
            deadline: Instant::now()
                + Duration::from_millis(request.deadline_ms.clamp(1, MAX_AGENT_LEASE_MS)),
            state: OperationState::Pending,
            deferred: request.deferred,
            transports: BTreeSet::new(),
            error: None,
            capture: None,
        };
        self.operations.insert(id.clone(), operation);
        Ok(AgentLease {
            id,
            workspace_id: request.workspace_id,
            browser: request.browser,
            generation: request.generation,
            permission_generation: permission.generation,
            mutation: request.mutation,
            deferred: request.deferred,
        })
    }

    pub(crate) fn complete(&mut self, lease: &AgentLease) -> Result<(), AgentError> {
        self.expire(Instant::now());
        let permission = self.permission(lease.workspace_id);
        let Some(operation) = self.operations.get_mut(&lease.id) else {
            return Err(AgentError::Replay);
        };
        if operation.state != OperationState::Pending {
            return Err(match operation.state {
                OperationState::TimedOut => AgentError::TimedOut,
                OperationState::Cancelled => AgentError::Cancelled,
                OperationState::Completed => AgentError::Replay,
                OperationState::Failed => AgentError::Cancelled,
                OperationState::Pending => AgentError::Replay,
            });
        }
        let target_matches = operation.workspace_id == lease.workspace_id
            && operation.browser == lease.browser
            && operation.generation == lease.generation
            && operation.permission_generation == lease.permission_generation
            && operation.mutation == lease.mutation
            && operation.deferred == lease.deferred
            && permission.generation == lease.permission_generation
            && if lease.mutation {
                permission.access.permits_interact()
            } else {
                permission.access.permits_read()
            };
        if !target_matches {
            operation.state = OperationState::Cancelled;
            return Err(AgentError::Cancelled);
        }
        operation.state = OperationState::Completed;
        Ok(())
    }

    pub(crate) fn bind_transport(
        &mut self,
        lease: &AgentLease,
        transport: OperationId,
    ) -> Result<(), AgentError> {
        let key = (lease.browser.clone(), transport.clone());
        if self.transport_operations.contains_key(&key) {
            return Err(AgentError::Busy);
        }
        {
            let Some(operation) = self.operations.get_mut(&lease.id) else {
                return Err(AgentError::Replay);
            };
            let matches = operation.state == OperationState::Pending
                && operation.workspace_id == lease.workspace_id
                && operation.browser == lease.browser
                && operation.generation == lease.generation
                && operation.permission_generation == lease.permission_generation
                && operation.mutation == lease.mutation
                && operation.deferred == lease.deferred;
            if !matches {
                return Err(AgentError::Cancelled);
            }
            operation.transports.insert(transport.clone());
        }
        self.transport_operations.insert(key, lease.id.clone());
        Ok(())
    }

    pub(crate) fn finish_transport(
        &mut self,
        browser: &BrowserId,
        transport: &OperationId,
        result: &Result<Event, BrowserError>,
    ) -> Option<AgentOperationInfo> {
        let id = self
            .transport_operations
            .get(&(browser.clone(), transport.clone()))?
            .clone();
        let operation = self.operations.get_mut(&id)?;
        let deferred_ack = operation.deferred
            && matches!(
                result,
                Ok(Event::NavigationStarted { .. }) | Ok(Event::ScreenshotAccepted)
            );
        if deferred_ack {
            return None;
        }
        let was_pending = operation.state == OperationState::Pending;
        self.transport_operations
            .remove(&(browser.clone(), transport.clone()));
        operation.transports.remove(transport);
        if !was_pending {
            return None;
        }
        if operation.transports.is_empty() {
            operation.state = if result.is_ok() {
                OperationState::Completed
            } else {
                OperationState::Failed
            };
            operation.error = result.as_ref().err().copied();
            if let Ok(Event::Screenshot {
                mime,
                width,
                height,
                data,
            }) = result
            {
                let encoded_limit = (MAX_AGENT_CAPTURE_BYTES * 4).div_ceil(3) + 4;
                if mime != "image/png"
                    || data.len() > encoded_limit
                    || !data.is_ascii()
                    || *width == 0
                    || *height == 0
                    || u64::from(*width).saturating_mul(u64::from(*height))
                        > MAX_AGENT_CAPTURE_PIXELS
                {
                    operation.state = OperationState::Failed;
                    operation.error = Some(if mime != "image/png" {
                        BrowserError::InvalidMessage
                    } else {
                        BrowserError::TooLarge
                    });
                } else {
                    operation.capture = Some(AgentCapture {
                        mime: mime.clone(),
                        width: *width,
                        height: *height,
                        data: data.clone(),
                    });
                }
            }
        }
        if operation.state == OperationState::Pending {
            None
        } else {
            Some(operation_info(&id, operation))
        }
    }

    pub(crate) fn operation(
        &mut self,
        workspace_id: u64,
        id: &OperationId,
    ) -> Result<AgentOperationInfo, AgentError> {
        self.require_read(workspace_id)?;
        self.expire(Instant::now());
        let Some(operation) = self.operations.get(id.as_str()) else {
            return Err(AgentError::StaleTarget);
        };
        if operation.workspace_id != workspace_id {
            return Err(AgentError::AccessDenied);
        }
        Ok(operation_info(id.as_str(), operation))
    }

    pub(crate) fn capture_chunk(
        &mut self,
        workspace_id: u64,
        id: &OperationId,
        offset: usize,
        limit: usize,
    ) -> Result<Option<AgentCaptureChunk>, AgentError> {
        self.require_read(workspace_id)?;
        self.expire(Instant::now());
        let operation = self
            .operations
            .get(id.as_str())
            .ok_or(AgentError::StaleTarget)?;
        if operation.workspace_id != workspace_id {
            return Err(AgentError::AccessDenied);
        }
        let Some(capture) = &operation.capture else {
            return Ok(None);
        };
        if offset > capture.data.len() {
            return Err(AgentError::InvalidInput);
        }
        let limit = limit.clamp(1, MAX_AGENT_CAPTURE_CHUNK_BYTES);
        let next_offset = offset.saturating_add(limit).min(capture.data.len());
        Ok(Some(AgentCaptureChunk {
            mime: capture.mime.clone(),
            width: capture.width,
            height: capture.height,
            offset,
            next_offset,
            total_bytes: capture.data.len(),
            data: capture.data[offset..next_offset].to_owned(),
            done: next_offset == capture.data.len(),
        }))
    }

    pub(crate) fn renew(
        &mut self,
        workspace_id: u64,
        id: &OperationId,
        extension_ms: u64,
    ) -> Result<AgentOperationInfo, AgentError> {
        self.require_read(workspace_id)?;
        self.expire(Instant::now());
        let permission = self.permission(workspace_id);
        let operation = self
            .operations
            .get_mut(id.as_str())
            .ok_or(AgentError::StaleTarget)?;
        if operation.workspace_id != workspace_id {
            return Err(AgentError::AccessDenied);
        }
        if operation.state != OperationState::Pending {
            return Err(match operation.state {
                OperationState::TimedOut => AgentError::TimedOut,
                OperationState::Cancelled => AgentError::Cancelled,
                OperationState::Completed => AgentError::Replay,
                OperationState::Failed => AgentError::Cancelled,
                OperationState::Pending => AgentError::Replay,
            });
        }
        if (operation.mutation && !permission.access.permits_interact())
            || (!operation.mutation && !permission.access.permits_read())
            || permission.generation != operation.permission_generation
        {
            operation.state = OperationState::Cancelled;
            return Err(AgentError::Cancelled);
        }
        operation.deadline =
            Instant::now() + Duration::from_millis(extension_ms.clamp(1, MAX_AGENT_LEASE_MS));
        Ok(operation_info(id.as_str(), operation))
    }

    pub(crate) fn abort(&mut self, lease: &AgentLease, error: AgentError) {
        if let Some(operation) = self.operations.get_mut(&lease.id)
            && operation.state == OperationState::Pending
        {
            operation.state = match error {
                AgentError::TimedOut => OperationState::TimedOut,
                _ => OperationState::Cancelled,
            };
            operation.error = None;
        }
    }

    pub(crate) fn cancel_browser(&mut self, workspace_id: u64, browser: &BrowserId) {
        for operation in self.operations.values_mut() {
            if operation.state == OperationState::Pending
                && operation.workspace_id == workspace_id
                && &operation.browser == browser
            {
                operation.state = OperationState::Cancelled;
                operation.error = None;
            }
        }
    }

    pub(crate) fn expire(&mut self, now: Instant) {
        for operation in self.operations.values_mut() {
            if operation.state == OperationState::Pending && operation.deadline <= now {
                operation.state = OperationState::TimedOut;
            }
        }
    }
}

fn operation_info(id: &str, operation: &Operation) -> AgentOperationInfo {
    AgentOperationInfo {
        id: id.to_owned(),
        workspace_id: operation.workspace_id,
        browser: operation.browser.clone(),
        generation: operation.generation,
        status: operation.state.name(),
        error: operation.error,
        capture: operation.capture.as_ref().map(|capture| AgentCaptureInfo {
            mime: capture.mime.clone(),
            width: capture.width,
            height: capture.height,
            encoded_bytes: capture.data.len(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser(value: &str) -> BrowserId {
        value.to_string().try_into().unwrap()
    }

    #[test]
    fn permissions_start_disabled_and_change_generation() {
        let mut service = AgentService::default();
        assert_eq!(service.permission(7).access, AgentAccess::Disabled);
        assert_eq!(service.permission(7).generation, 0);
        assert_eq!(service.set_access(7, AgentAccess::Read).generation, 1);
        assert_eq!(service.set_access(7, AgentAccess::Read).generation, 1);
        assert_eq!(service.set_access(7, AgentAccess::Interact).generation, 2);
    }

    #[test]
    fn human_takeover_cancels_pending_mutations_and_replay_is_rejected() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Interact);
        let lease = service
            .begin(7, browser("b"), 3, true, 4, MAX_AGENT_LEASE_MS)
            .unwrap();
        let transport = OperationId::try_from("human-input".to_owned()).unwrap();
        service.bind_transport(&lease, transport.clone()).unwrap();
        service.cancel_browser(7, &browser("b"));
        assert_eq!(service.complete(&lease), Err(AgentError::Cancelled));
        assert_eq!(service.complete(&lease), Err(AgentError::Cancelled));
        assert!(
            service
                .finish_transport(&browser("b"), &transport, &Ok(Event::InputAccepted))
                .is_none()
        );
    }

    #[test]
    fn mutation_capacity_is_one_per_browser_and_four_per_workspace() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Interact);
        let first = service
            .begin(7, browser("b"), 1, true, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        assert!(matches!(
            service.begin(7, browser("b"), 1, true, 0, MAX_AGENT_LEASE_MS),
            Err(AgentError::Busy)
        ));
        for id in ["c", "d", "e"] {
            service
                .begin(7, browser(id), 1, true, 0, MAX_AGENT_LEASE_MS)
                .unwrap();
        }
        assert!(matches!(
            service.begin(7, browser("f"), 1, true, 0, MAX_AGENT_LEASE_MS),
            Err(AgentError::Busy)
        ));
        service.complete(&first).unwrap();
    }

    #[test]
    fn revoking_read_access_invalidates_read_leases() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Read);
        let lease = service
            .begin(7, browser("b"), 1, false, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        service.set_access(7, AgentAccess::Disabled);
        assert_eq!(service.complete(&lease), Err(AgentError::Cancelled));
    }

    #[test]
    fn downgrading_to_read_cancels_mutations_but_keeps_read_access() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Interact);
        let read = service
            .begin(7, browser("read"), 1, false, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        let mutation = service
            .begin(7, browser("write"), 1, true, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        service.set_access(7, AgentAccess::Read);
        assert_eq!(service.complete(&mutation), Err(AgentError::Cancelled));
        assert_eq!(service.complete(&read), Err(AgentError::Cancelled));
        assert!(service.permission(7).access.permits_read());
    }

    #[test]
    fn transport_completion_exposes_one_terminal_operation_record() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Interact);
        let lease = service
            .begin(7, browser("b"), 1, true, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        let transport = OperationId::try_from("host-1".to_owned()).unwrap();
        service.bind_transport(&lease, transport.clone()).unwrap();
        let completed = service
            .finish_transport(&browser("b"), &transport, &Ok(Event::InputAccepted))
            .unwrap();
        assert_eq!(completed.id, lease.id);
        assert_eq!(completed.status, "completed");
        let id = OperationId::try_from(lease.id.clone()).unwrap();
        assert_eq!(service.operation(7, &id).unwrap().status, "completed");
        assert!(
            service
                .finish_transport(&browser("b"), &transport, &Ok(Event::InputAccepted))
                .is_none()
        );
    }

    #[test]
    fn operation_deadline_expires_before_transport_completion() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Read);
        let lease = service.begin(7, browser("b"), 1, false, 0, 1).unwrap();
        let id = OperationId::try_from(lease.id).unwrap();
        service.expire(Instant::now() + Duration::from_secs(1));
        assert_eq!(service.operation(7, &id).unwrap().status, "timed_out");
    }

    #[test]
    fn deferred_transport_ack_stays_open_until_native_completion() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Interact);
        let lease = service
            .begin_deferred(7, browser("b"), 1, true, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        let transport = OperationId::try_from("native-navigation".to_owned()).unwrap();
        service.bind_transport(&lease, transport.clone()).unwrap();
        assert!(
            service
                .finish_transport(
                    &browser("b"),
                    &transport,
                    &Ok(Event::NavigationStarted {
                        session: test_session(),
                    }),
                )
                .is_none()
        );
        assert_eq!(
            service
                .operation(7, &OperationId::try_from(lease.id.clone()).unwrap())
                .unwrap()
                .status,
            "accepted"
        );
        let completed = service
            .finish_transport(
                &browser("b"),
                &transport,
                &Ok(Event::Completed {
                    operation: transport.clone(),
                }),
            )
            .unwrap();
        assert_eq!(completed.status, "completed");
    }

    #[test]
    fn multi_transport_action_has_one_terminal_completion() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Interact);
        let lease = service
            .begin(7, browser("b"), 1, true, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        let first = OperationId::try_from("input-1".to_owned()).unwrap();
        let second = OperationId::try_from("input-2".to_owned()).unwrap();
        service.bind_transport(&lease, first.clone()).unwrap();
        service.bind_transport(&lease, second.clone()).unwrap();
        assert!(
            service
                .finish_transport(&browser("b"), &first, &Ok(Event::InputAccepted))
                .is_none()
        );
        assert_eq!(
            service
                .finish_transport(&browser("b"), &second, &Ok(Event::InputAccepted))
                .unwrap()
                .status,
            "completed"
        );
    }

    #[test]
    fn renewal_replaces_the_deadline_without_reopening_terminal_operations() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Read);
        let lease = service.begin(7, browser("b"), 1, false, 0, 1).unwrap();
        let id = OperationId::try_from(lease.id.clone()).unwrap();
        let before = service.operations.get(&lease.id).unwrap().deadline;
        assert_eq!(
            service.renew(7, &id, MAX_AGENT_LEASE_MS).unwrap().status,
            "accepted"
        );
        assert!(service.operations.get(&lease.id).unwrap().deadline > before);
        service.complete(&lease).unwrap();
        assert_eq!(
            service.renew(7, &id, MAX_AGENT_LEASE_MS),
            Err(AgentError::Replay)
        );
    }

    #[test]
    fn completed_captures_are_bounded_and_paged() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Read);
        let lease = service
            .begin_deferred(7, browser("b"), 1, false, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        let transport = OperationId::try_from("screenshot".to_owned()).unwrap();
        service.bind_transport(&lease, transport.clone()).unwrap();
        assert!(
            service
                .finish_transport(&browser("b"), &transport, &Ok(Event::ScreenshotAccepted))
                .is_none()
        );
        let data = "a".repeat(MAX_AGENT_CAPTURE_CHUNK_BYTES + 17);
        let info = service
            .finish_transport(
                &browser("b"),
                &transport,
                &Ok(Event::Screenshot {
                    mime: "image/png".to_owned(),
                    width: 2,
                    height: 3,
                    data: data.clone(),
                }),
            )
            .unwrap();
        assert_eq!(info.capture.unwrap().encoded_bytes, data.len());
        let id = OperationId::try_from(lease.id).unwrap();
        let first = service
            .capture_chunk(7, &id, 0, MAX_AGENT_CAPTURE_CHUNK_BYTES * 2)
            .unwrap()
            .unwrap();
        assert_eq!(first.data.len(), MAX_AGENT_CAPTURE_CHUNK_BYTES);
        assert_eq!(first.next_offset, MAX_AGENT_CAPTURE_CHUNK_BYTES);
        assert!(!first.done);
        let second = service
            .capture_chunk(7, &id, first.next_offset, MAX_AGENT_CAPTURE_CHUNK_BYTES)
            .unwrap()
            .unwrap();
        assert_eq!(second.data.len(), 17);
        assert!(second.done);
    }

    #[test]
    fn security_scope_revocation_and_stale_identity_fail_closed() {
        let mut service = AgentService::default();
        service.set_access(11, AgentAccess::Interact);
        service.set_access(22, AgentAccess::Read);
        let lease = service
            .begin_deferred(11, browser("workspace-a"), 7, true, 0, MAX_AGENT_LEASE_MS)
            .unwrap();
        let id = OperationId::try_from(lease.id.clone()).unwrap();
        let transport = OperationId::try_from("native-a".to_owned()).unwrap();
        service.bind_transport(&lease, transport.clone()).unwrap();
        assert_eq!(service.operation(22, &id), Err(AgentError::AccessDenied));
        let mut stale = lease.clone();
        stale.generation += 1;
        assert_eq!(service.complete(&stale), Err(AgentError::Cancelled));
        assert_eq!(service.operation(11, &id).unwrap().status, "cancelled");
        assert!(
            service
                .finish_transport(
                    &browser("workspace-a"),
                    &transport,
                    &Ok(Event::Completed {
                        operation: transport.clone(),
                    })
                )
                .is_none()
        );

        let read_lease = service
            .begin(
                11,
                browser("workspace-a-read"),
                7,
                false,
                0,
                MAX_AGENT_LEASE_MS,
            )
            .unwrap();
        let read_id = OperationId::try_from(read_lease.id).unwrap();
        service.set_access(11, AgentAccess::Disabled);
        assert_eq!(
            service.operation(11, &read_id),
            Err(AgentError::AccessDenied)
        );
    }

    #[test]
    fn read_access_cannot_start_mutations_and_total_quota_has_no_queue() {
        let mut service = AgentService::default();
        assert!(matches!(
            service.begin(31, browser("read-only"), 1, true, 0, MAX_AGENT_LEASE_MS),
            Err(AgentError::AccessDenied)
        ));
        let mut leases = Vec::new();
        for workspace in 0..4 {
            service.set_access(workspace, AgentAccess::Interact);
            for index in 0..4 {
                leases.push(
                    service
                        .begin(
                            workspace,
                            browser(&format!("quota-{workspace}-{index}")),
                            1,
                            true,
                            0,
                            MAX_AGENT_LEASE_MS,
                        )
                        .unwrap(),
                );
            }
        }
        assert_eq!(leases.len(), MAX_AGENT_OPERATIONS_TOTAL);
        assert!(matches!(
            service.begin(0, browser("quota-overflow"), 1, true, 0, MAX_AGENT_LEASE_MS),
            Err(AgentError::LimitReached)
        ));
        service.complete(&leases[0]).unwrap();
        assert!(
            service
                .begin(
                    0,
                    browser("quota-replacement"),
                    1,
                    true,
                    0,
                    MAX_AGENT_LEASE_MS
                )
                .is_ok()
        );
    }

    #[test]
    fn screenshot_completion_rejects_wrong_mime_and_pixel_budget() {
        let mut service = AgentService::default();
        service.set_access(7, AgentAccess::Read);
        for (transport_name, mime, width, height, expected) in [
            (
                "wrong-mime",
                "image/jpeg",
                2,
                2,
                BrowserError::InvalidMessage,
            ),
            (
                "too-many-pixels",
                "image/png",
                20_000,
                20_000,
                BrowserError::TooLarge,
            ),
        ] {
            let lease = service
                .begin_deferred(7, browser(transport_name), 1, false, 0, MAX_AGENT_LEASE_MS)
                .unwrap();
            let transport = OperationId::try_from(transport_name.to_owned()).unwrap();
            service.bind_transport(&lease, transport.clone()).unwrap();
            assert!(
                service
                    .finish_transport(
                        &browser(transport_name),
                        &transport,
                        &Ok(Event::ScreenshotAccepted),
                    )
                    .is_none()
            );
            let info = service
                .finish_transport(
                    &browser(transport_name),
                    &transport,
                    &Ok(Event::Screenshot {
                        mime: mime.to_owned(),
                        width,
                        height,
                        data: "a".to_owned(),
                    }),
                )
                .unwrap();
            assert_eq!(info.error, Some(expected));
        }
    }

    fn test_session() -> paneflow_browser_protocol::BrowserSession {
        paneflow_browser_protocol::BrowserSession {
            document: paneflow_browser_protocol::Document {
                owner: paneflow_browser_protocol::Owner {
                    workspace: "workspace".to_owned().try_into().unwrap(),
                    session: "session".to_owned().try_into().unwrap(),
                },
                browser: browser("b"),
                generation: 2,
            },
            profile: "profile".to_owned().try_into().unwrap(),
            url: "https://example.com".to_owned(),
            title: String::new(),
            state: paneflow_browser_protocol::SessionState::Hidden,
            presentation: paneflow_browser_protocol::BrowserPresentation::unmounted(),
        }
    }
}
