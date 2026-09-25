use super::*;

fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid > i32::MAX as u32 {
            return false;
        }
        let ret = unsafe { libc::kill(pid as i32, 0) };
        if ret == -1 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return errno != libc::ESRCH;
        }
        true
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::OpenProcess;
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        if pid == 0 {
            return false;
        }
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let _ = CloseHandle(handle);
            true
        }
    }
}

fn pid_matches(pid: u32, pinned_start: Option<u64>) -> bool {
    if !pid_is_alive(pid) {
        return false;
    }
    match (
        pinned_start,
        paneflow_host::process::process_start_time(pid),
    ) {
        (Some(pinned), Some(current)) => pinned == current,
        _ => true,
    }
}

fn keep_session_after_surface_purge(
    dying_surface_id: u64,
    pid: u32,
    session: &ai_types::AgentSession,
) -> bool {
    if session.surface_id == Some(dying_surface_id) {
        return false;
    }
    session.surface_id.is_some() || pid > i32::MAX as u32 || pid_matches(pid, session.proc_start)
}

fn keep_session_at_shell_prompt(
    prompt_surface_id: u64,
    surface_child_pid: u32,
    pid: u32,
    session: &ai_types::AgentSession,
) -> bool {
    if session.surface_id != Some(prompt_surface_id) {
        return true;
    }
    session.state == ai_types::AgentState::Errored
        || (pid != surface_child_pid
            && pid <= i32::MAX as u32
            && pid_matches(pid, session.proc_start))
}

fn keep_session_without_agent_in_pane(
    surface_id: u64,
    surface_child_pid: u32,
    pid: u32,
    session: &ai_types::AgentSession,
) -> bool {
    if session.surface_id != Some(surface_id) {
        return true;
    }
    session.state == ai_types::AgentState::Errored
        || (pid != surface_child_pid
            && pid <= i32::MAX as u32
            && pid_matches(pid, session.proc_start))
}

fn stale_sweep_keeps_without_pid_probe(
    pid: u32,
    session: &ai_types::AgentSession,
    live_surfaces: &std::collections::HashSet<u64>,
) -> bool {
    pid > i32::MAX as u32
        || (session.state == ai_types::AgentState::Errored
            && session
                .surface_id
                .is_some_and(|sid| live_surfaces.contains(&sid)))
}

impl PaneFlowApp {
    pub(crate) fn reap_sessions_at_shell_prompt(
        &mut self,
        surface_id: u64,
        surface_child_pid: u32,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions.retain(|&pid, session| {
                keep_session_at_shell_prompt(surface_id, surface_child_pid, pid, session)
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    pub(crate) fn reap_sessions_without_agent(
        &mut self,
        agentless: &[(u64, u32)],
        cx: &mut Context<Self>,
    ) {
        if agentless.is_empty() {
            return;
        }
        let mut changed = false;
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions.retain(|&pid, session| {
                agentless.iter().all(|&(surface_id, child_pid)| {
                    keep_session_without_agent_in_pane(surface_id, child_pid, pid, session)
                })
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    pub(crate) fn purge_sessions_for_surface(&mut self, surface_id: u64, cx: &mut Context<Self>) {
        let mut changed = false;
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions
                .retain(|&pid, session| keep_session_after_surface_purge(surface_id, pid, session));
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    pub(crate) fn sweep_stale_pids(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let live_surfaces: std::collections::HashSet<u64> = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.collect_panes())
            .flat_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .map(|t| t.entity_id().as_u64())
                    .collect::<Vec<_>>()
            })
            .collect();
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions.retain(|&pid, session| {
                stale_sweep_keeps_without_pid_probe(pid, session, &live_surfaces)
                    || pid_matches(pid, session.proc_start)
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    pub(in crate::app) fn spawn_stale_pid_sweep(cx: &mut Context<Self>) {
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_secs(30)).await;
                    if cx
                        .update(|cx| {
                            this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                app.sweep_stale_pids(cx);
                            })
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            },
        )
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};
    use std::collections::HashSet;

    #[test]
    fn surface_purge_drops_sessions_bound_to_dying_surface() {
        let mut session = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Errored);
        session.surface_id = Some(7);

        assert!(!keep_session_after_surface_purge(7, u32::MAX, &session));
        assert!(keep_session_after_surface_purge(8, u32::MAX, &session));
    }

    #[test]
    fn shell_prompt_reaps_the_surface_it_fired_on() {
        const SHELL: u32 = 4242;
        let mut thinking = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Thinking);
        thinking.surface_id = Some(7);
        assert!(!keep_session_at_shell_prompt(7, SHELL, u32::MAX, &thinking));
        assert!(keep_session_at_shell_prompt(8, SHELL, u32::MAX, &thinking));

        assert!(!keep_session_at_shell_prompt(7, SHELL, SHELL, &thinking));

        let mut errored = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Errored);
        errored.surface_id = Some(7);
        assert!(keep_session_at_shell_prompt(7, SHELL, u32::MAX, &errored));

        let mut backgrounded = AgentSession::new(TerminalAgent::Codex, AgentState::Thinking);
        backgrounded.surface_id = Some(7);
        let own_pid = std::process::id();
        backgrounded.proc_start = paneflow_host::process::process_start_time(own_pid);
        assert!(keep_session_at_shell_prompt(
            7,
            SHELL,
            own_pid,
            &backgrounded
        ));
    }

    #[test]
    fn a_pane_that_lost_its_agent_drops_the_row_keyed_on_its_own_shell() {
        const SHELL: u32 = 4242;
        let mut waiting = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::WaitingForInput);
        waiting.surface_id = Some(7);
        assert!(!keep_session_without_agent_in_pane(
            7, SHELL, SHELL, &waiting
        ));
        assert!(keep_session_without_agent_in_pane(
            8, SHELL, SHELL, &waiting
        ));

        assert!(!keep_session_without_agent_in_pane(
            7,
            SHELL,
            u32::MAX,
            &waiting
        ));

        let mut errored = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Errored);
        errored.surface_id = Some(7);
        assert!(keep_session_without_agent_in_pane(
            7, SHELL, SHELL, &errored
        ));

        let mut backgrounded = AgentSession::new(TerminalAgent::Codex, AgentState::Thinking);
        backgrounded.surface_id = Some(7);
        let own_pid = std::process::id();
        backgrounded.proc_start = paneflow_host::process::process_start_time(own_pid);
        assert!(keep_session_without_agent_in_pane(
            7,
            SHELL,
            own_pid,
            &backgrounded
        ));
    }

    #[test]
    fn stale_sweep_keeps_synthetic_pid_without_os_probe() {
        let session = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Thinking);
        let live_surfaces = HashSet::new();

        assert!(stale_sweep_keeps_without_pid_probe(
            u32::MAX,
            &session,
            &live_surfaces
        ));
    }

    #[test]
    fn stale_sweep_keeps_errored_session_while_surface_is_live() {
        let mut session = AgentSession::new(TerminalAgent::Codex, AgentState::Errored);
        session.surface_id = Some(42);
        let live_surfaces = HashSet::from([42]);

        assert!(stale_sweep_keeps_without_pid_probe(
            1234,
            &session,
            &live_surfaces
        ));

        let live_surfaces = HashSet::new();
        assert!(!stale_sweep_keeps_without_pid_probe(
            1234,
            &session,
            &live_surfaces
        ));
    }
}
