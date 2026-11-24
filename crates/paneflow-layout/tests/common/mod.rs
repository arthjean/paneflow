#![allow(dead_code)]

use std::collections::BTreeMap;

use paneflow_config::schema::{SessionId, SpaceId, SurfaceDefinition, TabTitleSource};
use paneflow_layout::{
    Applied, Direction, Edge, Effect, Model, NewNode, NewPane, NewTab, NewWorkspace, Node, Op,
    PaneId, SurfaceId, TabId, WorkspaceId,
};

pub struct Fixture {
    pub model: Model,
    pub now: u64,
    names: BTreeMap<u64, String>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            model: Model::new(),
            now: 1_000,
            names: BTreeMap::new(),
        }
    }

    pub fn terminal(&self, name: &str) -> SurfaceDefinition {
        SurfaceDefinition {
            name: Some(name.to_string()),
            session: Some(session(name)),
            ..SurfaceDefinition::default()
        }
    }

    pub fn pane(&mut self, names: &[&str]) -> NewPane {
        let definitions = names.iter().map(|name| self.terminal(name)).collect();
        let pane = self.model.reserve_pane(definitions, names.len() - 1);
        for (surface, name) in pane.surfaces.iter().zip(names) {
            self.names.insert(surface.id.get(), (*name).to_string());
        }
        if let Some(first) = names.first() {
            self.names.insert(pane.id.get(), (*first).to_string());
        }
        pane
    }

    pub fn surface(&mut self, name: &str) -> paneflow_layout::NewSurface {
        let surface = self.model.reserve_surface(self.terminal(name));
        self.names.insert(surface.id.get(), name.to_string());
        surface
    }

    pub fn workspace(&mut self, title: &str, first: &str) -> WorkspaceId {
        self.workspace_in(title, first, None)
    }

    pub fn workspace_in(
        &mut self,
        title: &str,
        first: &str,
        space: Option<SpaceId>,
    ) -> WorkspaceId {
        let pane = self.pane(&[first]);
        let id = WorkspaceId(self.model.reserve());
        let tab = TabId(self.model.reserve());
        self.apply(Op::CreateWorkspace {
            workspace: NewWorkspace {
                id,
                title: title.to_string(),
                cwd: format!("/work/{title}"),
                space,
                durable_id: None,
                custom_buttons: Vec::new(),
                managed_worktrees: Vec::new(),
                tabs: vec![NewTab {
                    id: tab,
                    title: String::new(),
                    title_source: TabTitleSource::Preset,
                    worktree: None,
                    root: Some(NewNode::Pane(pane)),
                }],
                active_tab: 0,
            },
            select: true,
        });
        id
    }

    pub fn tab(&mut self, workspace: WorkspaceId, title: &str, first: &str) -> TabId {
        let pane = self.pane(&[first]);
        let tab = TabId(self.model.reserve());
        self.apply(Op::OpenTab {
            workspace,
            tab: NewTab {
                id: tab,
                title: title.to_string(),
                title_source: TabTitleSource::Preset,
                worktree: None,
                root: Some(NewNode::Pane(pane)),
            },
            fill_empty_active: false,
        });
        tab
    }

    pub fn split(&mut self, target: &str, direction: Direction, name: &str) -> Applied {
        let target = self.pane_id(target);
        let pane = self.pane(&[name]);
        self.apply(Op::Split {
            target,
            direction,
            edge: Edge::After,
            pane,
            focus: true,
        })
    }

    pub fn apply(&mut self, op: Op) -> Applied {
        let label = format!("{op:?}");
        self.model
            .apply(op, self.now)
            .unwrap_or_else(|error| panic!("{label} failed: {error}"))
    }

    pub fn try_apply(&mut self, op: Op) -> Result<Applied, paneflow_layout::OpError> {
        self.model.apply(op, self.now)
    }

    pub fn pane_id(&self, name: &str) -> PaneId {
        let state = self.model.state();
        for ws in &state.workspaces {
            for tab in &ws.tabs {
                for pane in tab.root.as_ref().map(Node::panes).unwrap_or_default() {
                    if pane.surfaces.iter().any(|surface| {
                        self.names.get(&surface.id.get()).map(String::as_str) == Some(name)
                    }) {
                        return pane.id;
                    }
                }
            }
        }
        panic!("no pane holds {name}")
    }

    pub fn surface_id(&self, name: &str) -> SurfaceId {
        let id = self
            .names
            .iter()
            .find(|(_, known)| known.as_str() == name)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("no surface is named {name}"));
        let state = self.model.state();
        for ws in &state.workspaces {
            for tab in &ws.tabs {
                for pane in tab.root.as_ref().map(Node::panes).unwrap_or_default() {
                    for surface in &pane.surfaces {
                        if surface.id.get() == id
                            || surface.definition.name.as_deref() == Some(name)
                        {
                            return surface.id;
                        }
                    }
                }
            }
        }
        SurfaceId(id)
    }

    pub fn tab_id(&self, ws: usize, tab: usize) -> TabId {
        self.model.state().workspaces[ws].tabs[tab].id
    }

    pub fn shape(&self, ws: usize, tab: usize) -> String {
        let state = self.model.state();
        match state
            .workspaces
            .get(ws)
            .and_then(|ws| ws.tabs.get(tab))
            .and_then(|tab| tab.root.as_ref())
        {
            Some(root) => render(root),
            None => "empty".to_string(),
        }
    }

    pub fn focused(&self, ws: usize, tab: usize) -> Option<String> {
        let state = self.model.state();
        let tab = state.workspaces.get(ws)?.tabs.get(tab)?;
        let pane = tab.root.as_ref()?.pane(tab.focused?)?;
        Some(surface_name(pane.active_surface()?))
    }

    pub fn advance(&mut self, ms: u64) {
        self.now += ms;
    }
}

pub fn session(name: &str) -> SessionId {
    SessionId::parse(&format!(
        "00000000-0000-4000-8000-{:012x}",
        name_number(name)
    ))
    .unwrap_or_else(|error| panic!("{error}"))
}

fn name_number(name: &str) -> u64 {
    name.bytes()
        .fold(0u64, |acc, byte| acc * 131 + u64::from(byte))
        % 0xffff_ffff_ffff
}

pub fn surface_name(surface: &paneflow_layout::Surface) -> String {
    surface
        .definition
        .name
        .clone()
        .unwrap_or_else(|| match &surface.definition.session {
            Some(_) => "bound".to_string(),
            None => "fresh".to_string(),
        })
}

pub fn render(node: &Node) -> String {
    match node {
        Node::Pane(pane) => {
            let names: Vec<String> = pane
                .surfaces
                .iter()
                .enumerate()
                .map(|(index, surface)| {
                    let name = surface_name(surface);
                    if pane.surfaces.len() > 1 && index == pane.active {
                        format!("{name}*")
                    } else {
                        name
                    }
                })
                .collect();
            names.join("+")
        }
        Node::Split(split) => {
            let letter = match split.direction {
                Direction::Vertical => "V",
                Direction::Horizontal => "H",
            };
            let parts: Vec<String> = split
                .children
                .iter()
                .map(|child| format!("{:.3} {}", child.ratio, render(&child.node)))
                .collect();
            format!("{letter}({})", parts.join(", "))
        }
    }
}

pub fn effects(applied: &Applied, fixture: &Fixture) -> Vec<String> {
    applied
        .effects
        .iter()
        .map(|effect| match effect {
            Effect::Spawn { surface } => format!("spawn {}", fixture.describe(*surface)),
            Effect::Reattach { session, .. } => {
                format!("reattach {}", fixture.session_name(session))
            }
            Effect::Hold { session, .. } => format!("hold {}", fixture.session_name(session)),
            Effect::Stop { session } => format!("stop {}", fixture.session_name(session)),
            Effect::Rehome { session, .. } => format!("rehome {}", fixture.session_name(session)),
        })
        .collect()
}

impl Fixture {
    fn describe(&self, surface: SurfaceId) -> String {
        self.model
            .state()
            .surface(surface)
            .map(surface_name)
            .or_else(|| self.names.get(&surface.get()).cloned())
            .unwrap_or_else(|| "?".to_string())
    }
}

impl Fixture {
    pub fn session_name(&self, wanted: &SessionId) -> String {
        self.names
            .values()
            .find(|name| &session(name) == wanted)
            .cloned()
            .unwrap_or_else(|| wanted.to_string())
    }
}
