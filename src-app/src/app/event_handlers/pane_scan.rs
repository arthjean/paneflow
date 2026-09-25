use super::*;

pub(in crate::app) fn merge_service_label(
    labels: &mut std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    info: crate::terminal::ServiceInfo,
) -> bool {
    if let Some(existing) = labels.get(&info.port)
        && existing.is_frontend
        && !info.is_frontend
    {
        return false;
    }
    if labels.get(&info.port) == Some(&info) {
        return false;
    }
    labels.insert(info.port, info);
    true
}

fn scan_workspace_ports(
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> Vec<u16> {
    let mut ports: Vec<u16> = scan
        .values()
        .flat_map(|s| s.ports.iter().map(|e| e.port))
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn declaration_survives_scan(
    scanned: Option<crate::agent_launcher::TerminalAgent>,
    declared_until: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    scanned.is_none() && declared_until.is_some_and(|until| now < until)
}

fn scan_detected_agents(
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> std::collections::HashSet<String> {
    scan.values()
        .flat_map(|s| s.agents.iter().cloned())
        .collect()
}

fn merge_frontend_scan_labels(
    labels: &mut std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> bool {
    let mut changed = false;
    for entry in scan.values().flat_map(|s| s.ports.iter()) {
        let Some(label) = entry.frontend else {
            continue;
        };
        let fallback_url = || format!("http://localhost:{}", entry.port);
        match labels.entry(entry.port) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let info = e.get_mut();
                if !info.is_frontend {
                    info.is_frontend = true;
                    info.label = Some(label.to_string());
                    if info.url.is_none() {
                        info.url = Some(fallback_url());
                    }
                    changed = true;
                    continue;
                }
                if info.label.is_none() {
                    info.label = Some(label.to_string());
                    changed = true;
                }
                if info.url.is_none() {
                    info.url = Some(fallback_url());
                    changed = true;
                }
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(crate::terminal::ServiceInfo {
                    port: entry.port,
                    url: Some(fallback_url()),
                    label: Some(label.to_string()),
                    is_frontend: true,
                });
                changed = true;
            }
        }
    }
    changed
}

fn merge_scan_workspace_state(
    active_ports: &mut Vec<u16>,
    service_labels: &mut std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    detected_agents: &mut std::collections::HashSet<String>,
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> bool {
    let ports = scan_workspace_ports(scan);
    let next_agents = scan_detected_agents(scan);
    let mut changed = false;

    if *active_ports != ports {
        *active_ports = ports;
        changed = true;
    }
    let before = service_labels.len();
    service_labels.retain(|port, _| active_ports.contains(port));
    if service_labels.len() != before {
        changed = true;
    }
    let frontend_ports: std::collections::HashSet<u16> = scan
        .values()
        .flat_map(|s| s.ports.iter())
        .filter(|entry| entry.frontend.is_some())
        .map(|entry| entry.port)
        .collect();
    for info in service_labels.values_mut() {
        if info.is_frontend && !frontend_ports.contains(&info.port) {
            info.is_frontend = false;
            changed = true;
        }
    }
    if *detected_agents != next_agents {
        *detected_agents = next_agents;
        changed = true;
    }
    merge_frontend_scan_labels(service_labels, scan) || changed
}

fn port_ownership(
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> (
    std::collections::HashMap<u16, u64>,
    std::collections::HashSet<u16>,
) {
    let mut owner = std::collections::HashMap::new();
    let mut shared = std::collections::HashSet::new();
    for (tid, s) in scan {
        for e in &s.ports {
            match owner.entry(e.port) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    if *o.get() != *tid {
                        shared.insert(e.port);
                    }
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(*tid);
                }
            }
        }
    }
    (owner, shared)
}

fn announced_port_conflicts(
    announced_ports: &[u16],
    tid: u64,
    owner: &std::collections::HashMap<u16, u64>,
    shared: &std::collections::HashSet<u16>,
    display_names: &std::collections::HashMap<u64, String>,
) -> Vec<(u16, String)> {
    announced_ports
        .iter()
        .filter_map(|p| match owner.get(p) {
            Some(&o) if o != tid && !shared.contains(p) => {
                Some((*p, display_names.get(&o).cloned().unwrap_or_default()))
            }
            _ => None,
        })
        .collect()
}

impl PaneFlowApp {
    fn has_unscanned_surface(&self, ws_idx: usize, cx: &Context<Self>) -> bool {
        self.workspaces.get(ws_idx).is_some_and(|ws| {
            ws.collect_panes().iter().any(|pane| {
                pane.read(cx).terminals().any(|tv| {
                    let t = &tv.read(cx).terminal;
                    t.child_pid > 0 && !t.agent_confirmed
                })
            })
        })
    }

    pub(in crate::app) fn schedule_port_scan(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        let unscanned = self.has_unscanned_surface(ws_idx, cx);
        let ws = &mut self.workspaces[ws_idx];
        if ws.port_scan_pending {
            return;
        }
        ws.port_scan_pending = true;
        ws.port_scan_generation += 1;
        let generation = ws.port_scan_generation;
        let ws_id = ws.id;

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                if !unscanned {
                    smol::Timer::after(std::time::Duration::from_millis(500)).await;
                }

                for delay_ms in [0u64, 2000, 6000] {
                    if delay_ms > 0 {
                        smol::Timer::after(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    let should_continue = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            app.run_port_scan(ws_id, generation, cx)
                        })
                    });
                    match should_continue {
                        Ok(true) => {}
                        _ => break,
                    }
                }

                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let Some(ws_idx) = app.workspaces.iter().position(|ws| ws.id == ws_id)
                        else {
                            return;
                        };
                        app.workspaces[ws_idx].port_scan_pending = false;
                        if app.has_unscanned_surface(ws_idx, cx) {
                            app.schedule_port_scan(ws_idx, cx);
                        }
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn schedule_active_port_rescans(&mut self, cx: &mut Context<Self>) {
        let workspace_ids: Vec<u64> = self
            .workspaces
            .iter()
            .filter(|ws| !ws.active_ports.is_empty() && !ws.port_scan_pending)
            .map(|ws| ws.id)
            .collect();

        for ws_id in workspace_ids {
            if let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) {
                self.schedule_port_scan(ws_idx, cx);
            }
        }
    }

    fn run_port_scan(&mut self, ws_id: u64, generation: u64, cx: &mut Context<Self>) -> bool {
        let ws = match self.workspaces.iter().find(|ws| ws.id == ws_id) {
            Some(ws) if ws.port_scan_generation == generation => ws,
            _ => return false,
        };

        let roots: Vec<(u64, u32)> = ws
            .collect_panes()
            .iter()
            .flat_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .filter_map(|tv| {
                        let child_pid = tv.read(cx).terminal.child_pid;
                        (child_pid > 0).then_some((tv.entity_id().as_u64(), child_pid))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        if roots.is_empty() {
            return true;
        }

        let submitted: Vec<u64> = roots.iter().map(|(key, _)| *key).collect();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut scan = smol::unblock(move || {
                    let agent_binaries: Vec<&'static str> =
                        crate::agent_launcher::TerminalAgent::all()
                            .map(|agent| agent.binary())
                            .collect();
                    crate::workspace::scan_panes(&roots, &agent_binaries)
                })
                .await;
                for key in submitted {
                    scan.entry(key).or_default();
                }
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.apply_pane_scan(ws_id, generation, scan, cx);
                    })
                });
            },
        )
        .detach();
        true
    }

    fn apply_pane_scan(
        &mut self,
        ws_id: u64,
        generation: u64,
        scan: std::collections::HashMap<u64, crate::workspace::PaneScan>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self
            .workspaces
            .iter_mut()
            .find(|ws| ws.id == ws_id && ws.port_scan_generation == generation)
        else {
            return;
        };

        let mut changed = merge_scan_workspace_state(
            &mut ws.active_ports,
            &mut ws.service_labels,
            &mut ws.detected_agents,
            &scan,
        );

        let live_ports: Vec<u16> = ws.active_ports.clone();

        let frontend_urls: std::collections::HashMap<u16, String> = ws
            .service_labels
            .iter()
            .filter(|(_, info)| info.is_frontend)
            .filter_map(|(port, info)| info.url.clone().map(|u| (*port, u)))
            .collect();

        let leaves: Vec<gpui::Entity<crate::pane::Pane>> = ws.collect_panes();

        let (owner, shared) = port_ownership(&scan);

        let mut display_names: std::collections::HashMap<u64, String> =
            std::collections::HashMap::new();
        for pane in &leaves {
            for tv in pane.read(cx).terminals() {
                let tid = tv.entity_id().as_u64();
                let r = tv.read(cx);
                let name = r
                    .terminal
                    .custom_name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        if r.terminal.title.is_empty() {
                            format!("surface {tid}")
                        } else {
                            r.terminal.title.clone()
                        }
                    });
                let name = crate::markdown::strip_bidi_zero_width(name.chars().take(64).collect());
                display_names.insert(tid, name);
            }
        }

        let mut agentless: Vec<(u64, u32)> = Vec::new();

        for pane in &leaves {
            let terminals: Vec<gpui::Entity<crate::terminal::TerminalView>> =
                pane.read(cx).terminals().cloned().collect();
            let mut pane_changed = false;
            for tv in terminals {
                let tid = tv.entity_id().as_u64();
                let Some(s) = scan.get(&tid) else {
                    continue;
                };
                let agent = s
                    .agents
                    .first()
                    .and_then(|b| crate::agent_launcher::TerminalAgent::from_binary(b));
                tv.update(cx, |view, _cx| {
                    let t = &mut view.terminal;
                    t.retain_reported_ports(&live_ports);
                    let in_grace = declaration_survives_scan(
                        agent,
                        t.agent_declared_until,
                        std::time::Instant::now(),
                    );
                    if !in_grace {
                        t.agent_declared_until = None;
                        if t.detected_agent != agent || !t.agent_confirmed {
                            if agent.is_none() && t.detected_agent.is_some() {
                                agentless.push((tid, t.child_pid));
                            }
                            t.detected_agent = agent;
                            t.agent_confirmed = true;
                            pane_changed = true;
                        }
                    }
                    let ports_with_links: Vec<(u16, Option<String>)> = s
                        .ports
                        .iter()
                        .map(|e| (e.port, frontend_urls.get(&e.port).cloned()))
                        .collect();
                    if t.detected_ports != ports_with_links {
                        t.detected_ports = ports_with_links;
                        pane_changed = true;
                    }
                    if t.cached_foreground_command != s.foreground_command {
                        t.cached_foreground_command = s.foreground_command.clone();
                        pane_changed = true;
                    }
                    let conflicts = announced_port_conflicts(
                        &t.announced_ports,
                        tid,
                        &owner,
                        &shared,
                        &display_names,
                    );
                    if t.port_conflicts != conflicts {
                        t.port_conflicts = conflicts;
                        pane_changed = true;
                    }
                });
            }
            if pane_changed {
                pane.update(cx, |_, cx| cx.notify());
                changed = true;
            }
        }

        self.reap_sessions_without_agent(&agentless, cx);

        if changed {
            cx.notify();
        }
    }

    pub(in crate::app) fn spawn_port_rescans(cx: &mut Context<Self>) {
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_secs(5)).await;
                    if cx
                        .update(|cx| {
                            this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                app.schedule_active_port_rescans(cx);
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
    use crate::terminal::ServiceInfo;
    use crate::workspace::{PaneScan, PortEntry};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn merge_service_label_keeps_frontend_when_backend_mentions_same_port() {
        let mut labels = HashMap::new();
        assert!(merge_service_label(
            &mut labels,
            ServiceInfo {
                port: 3000,
                url: Some("http://localhost:3000/app".to_string()),
                label: Some("Next.js".to_string()),
                is_frontend: true,
            },
        ));

        assert!(!merge_service_label(
            &mut labels,
            ServiceInfo {
                port: 3000,
                url: Some("http://localhost:3000".to_string()),
                label: Some("Fastify".to_string()),
                is_frontend: false,
            },
        ));

        let info = labels.get(&3000).unwrap();
        assert_eq!(info.label.as_deref(), Some("Next.js"));
        assert_eq!(info.url.as_deref(), Some("http://localhost:3000/app"));
        assert!(info.is_frontend);
    }

    #[test]
    fn declaration_survives_only_absent_evidence_before_its_deadline() {
        use crate::agent_launcher::TerminalAgent;
        let now = std::time::Instant::now();
        let future = now.checked_add(std::time::Duration::from_secs(5));
        let past = now.checked_sub(std::time::Duration::from_secs(5));

        assert!(declaration_survives_scan(None, future, now));
        assert!(!declaration_survives_scan(None, past, now));
        assert!(!declaration_survives_scan(None, None, now));
        assert!(!declaration_survives_scan(
            Some(TerminalAgent::ClaudeCode),
            future,
            now
        ));
        assert!(!declaration_survives_scan(
            Some(TerminalAgent::Codex),
            future,
            now
        ));
    }

    #[test]
    fn merge_scan_workspace_state_adds_frontend_fallback_and_prunes_stale_labels() {
        let mut active_ports = vec![9999];
        let mut service_labels = HashMap::from([(
            9999,
            ServiceInfo {
                port: 9999,
                url: Some("http://localhost:9999".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: true,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: vec!["codex".to_string()],
                foreground_command: None,
            },
        )]);

        assert!(merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));

        assert_eq!(active_ports, vec![5173]);
        assert!(!service_labels.contains_key(&9999));
        let info = service_labels.get(&5173).unwrap();
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173"));
        assert_eq!(info.label.as_deref(), Some("Vite"));
        assert!(info.is_frontend);
        assert!(detected_agents.contains("codex"));
    }

    #[test]
    fn merge_scan_workspace_state_preserves_exact_frontend_url() {
        let mut active_ports = vec![5173];
        let mut service_labels = HashMap::from([(
            5173,
            ServiceInfo {
                port: 5173,
                url: Some("http://localhost:5173/app".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: true,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);

        assert!(!merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));
        assert_eq!(
            service_labels.get(&5173).unwrap().url.as_deref(),
            Some("http://localhost:5173/app")
        );
    }

    #[test]
    fn merge_scan_workspace_state_downgrades_unconfirmed_frontend_label() {
        let mut active_ports = vec![5173];
        let mut service_labels = HashMap::from([(
            5173,
            ServiceInfo {
                port: 5173,
                url: Some("http://localhost:5173/app".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: true,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: None,
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);

        assert!(merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));
        let info = service_labels.get(&5173).unwrap();
        assert!(!info.is_frontend);
        assert_eq!(info.label.as_deref(), Some("Vite"));
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173/app"));
    }

    #[test]
    fn merge_scan_workspace_state_upgrades_terminal_label_from_frontend_scan() {
        let mut active_ports = vec![5173];
        let mut service_labels = HashMap::from([(
            5173,
            ServiceInfo {
                port: 5173,
                url: Some("http://localhost:5173/app".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: false,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);

        assert!(merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));
        let info = service_labels.get(&5173).unwrap();
        assert!(info.is_frontend);
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173/app"));
    }

    #[test]
    fn announced_port_conflicts_ignore_shared_ports() {
        let shared_scan = HashMap::from([
            (
                1,
                PaneScan {
                    ports: vec![PortEntry {
                        port: 3000,
                        frontend: None,
                    }],
                    agents: Vec::new(),
                    foreground_command: None,
                },
            ),
            (
                2,
                PaneScan {
                    ports: vec![PortEntry {
                        port: 3000,
                        frontend: None,
                    }],
                    agents: Vec::new(),
                    foreground_command: None,
                },
            ),
        ]);
        let (owner, shared) = port_ownership(&shared_scan);
        let display_names = HashMap::from([(1, "frontend".to_string())]);

        assert!(announced_port_conflicts(&[3000], 2, &owner, &shared, &display_names).is_empty());

        let single_owner_scan = HashMap::from([(
            1,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);
        let (owner, shared) = port_ownership(&single_owner_scan);
        let display_names = HashMap::from([(1, "vite pane".to_string())]);

        assert_eq!(
            announced_port_conflicts(&[5173], 2, &owner, &shared, &display_names),
            vec![(5173, "vite pane".to_string())]
        );
    }
}
