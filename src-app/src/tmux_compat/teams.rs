use std::collections::HashMap;

use super::command::Target;

pub(crate) const LEADER: u32 = 0;
const MAX_TEAM_PANES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TeamPane {
    pub(crate) local: u32,
    pub(crate) surface: Option<u64>,
    pub(crate) anchor: u32,
    pub(crate) side_by_side: bool,
    pub(crate) title: String,
    pub(crate) spawning: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Team {
    leader: Option<u64>,
    leader_title: String,
    panes: Vec<TeamPane>,
    next_local: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TeamError {
    UnknownTeam,
    LeaderClosed,
    NotInTeam,
    NoSuchPane(String),
    TooManyPanes,
}

impl TeamError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::UnknownTeam => {
                "paneflow tmux-compat: this pane does not belong to a Paneflow team".to_string()
            }
            Self::LeaderClosed => {
                "paneflow tmux-compat: the team leader pane was closed".to_string()
            }
            Self::NotInTeam => {
                "paneflow tmux-compat: the calling pane is not part of this team".to_string()
            }
            Self::NoSuchPane(target) => format!("can't find pane: {target}"),
            Self::TooManyPanes => {
                format!("paneflow tmux-compat: a team holds at most {MAX_TEAM_PANES} panes")
            }
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TmuxTeams {
    teams: HashMap<String, Team>,
}

impl TmuxTeams {
    pub(crate) fn issue(&mut self) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.teams.insert(
            token.clone(),
            Team {
                next_local: LEADER + 1,
                ..Team::default()
            },
        );
        token
    }

    pub(crate) fn team_mut(&mut self, token: &str) -> Result<&mut Team, TeamError> {
        self.teams.get_mut(token).ok_or(TeamError::UnknownTeam)
    }

    pub(crate) fn enter(
        &mut self,
        token: &str,
        caller: u64,
        alive: impl Fn(u64) -> bool,
    ) -> Result<u32, TeamError> {
        let team = self.team_mut(token)?;
        if team.leader.is_some_and(|leader| !alive(leader)) {
            self.teams.remove(token);
            return Err(TeamError::LeaderClosed);
        }
        let team = self.team_mut(token)?;
        team.forget_closed(&alive);
        team.caller_local(caller)
    }
}

impl Team {
    fn forget_closed(&mut self, alive: &impl Fn(u64) -> bool) {
        self.panes.retain(|pane| pane.surface.is_none_or(alive));
    }

    fn caller_local(&mut self, caller: u64) -> Result<u32, TeamError> {
        match self.leader {
            None => {
                self.leader = Some(caller);
                Ok(LEADER)
            }
            Some(leader) if leader == caller => Ok(LEADER),
            Some(_) => self
                .panes
                .iter()
                .find(|pane| pane.surface == Some(caller))
                .map(|pane| pane.local)
                .ok_or(TeamError::NotInTeam),
        }
    }

    pub(crate) fn resolve(&self, target: &Target, caller: u32) -> Result<u32, TeamError> {
        match target {
            Target::Caller => Ok(caller),
            Target::Window => Ok(LEADER),
            Target::Pane(local) if *local == LEADER || self.pane(*local).is_some() => Ok(*local),
            Target::Pane(local) => Err(TeamError::NoSuchPane(format!("%{local}"))),
            Target::Unknown(raw) => Err(TeamError::NoSuchPane(raw.clone())),
        }
    }

    pub(crate) fn pane(&self, local: u32) -> Option<&TeamPane> {
        self.panes.iter().find(|pane| pane.local == local)
    }

    pub(crate) fn pane_mut(&mut self, local: u32) -> Option<&mut TeamPane> {
        self.panes.iter_mut().find(|pane| pane.local == local)
    }

    pub(crate) fn reserve(&mut self, anchor: u32, side_by_side: bool) -> Result<u32, TeamError> {
        if self.panes.len() >= MAX_TEAM_PANES {
            return Err(TeamError::TooManyPanes);
        }
        let local = self.next_local;
        self.next_local += 1;
        self.panes.push(TeamPane {
            local,
            surface: None,
            anchor,
            side_by_side,
            title: String::new(),
            spawning: false,
        });
        Ok(local)
    }

    pub(crate) fn surface_of(&self, local: u32) -> Option<u64> {
        if local == LEADER {
            self.leader
        } else {
            self.pane(local).and_then(|pane| pane.surface)
        }
    }

    pub(crate) fn anchor_surface(&self, local: u32) -> Option<u64> {
        self.pane(local)
            .and_then(|pane| self.surface_of(pane.anchor))
            .or(self.leader)
    }

    pub(crate) fn attach(&mut self, local: u32, surface: u64) {
        if let Some(pane) = self.pane_mut(local) {
            pane.surface = Some(surface);
            pane.spawning = false;
        }
    }

    pub(crate) fn remove(&mut self, local: u32) -> Option<TeamPane> {
        let index = self.panes.iter().position(|pane| pane.local == local)?;
        Some(self.panes.remove(index))
    }

    pub(crate) fn title(&self, local: u32) -> &str {
        if local == LEADER {
            &self.leader_title
        } else {
            self.pane(local).map_or("", |pane| pane.title.as_str())
        }
    }

    pub(crate) fn set_title(&mut self, local: u32, title: String) {
        if local == LEADER {
            self.leader_title = title;
        } else if let Some(pane) = self.pane_mut(local) {
            pane.title = title;
        }
    }

    pub(crate) fn locals(&self) -> Vec<u32> {
        std::iter::once(LEADER)
            .chain(self.panes.iter().map(|pane| pane.local))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive_all(_: u64) -> bool {
        true
    }

    #[test]
    fn the_first_caller_becomes_the_leader_and_owns_pane_zero() {
        let mut teams = TmuxTeams::default();
        let token = teams.issue();
        assert_eq!(teams.enter(&token, 10, alive_all), Ok(LEADER));
        assert_eq!(teams.enter(&token, 10, alive_all), Ok(LEADER));
        assert_eq!(
            teams.enter(&token, 11, alive_all),
            Err(TeamError::NotInTeam),
            "a pane outside the team cannot speak for it"
        );
    }

    #[test]
    fn the_shim_only_addresses_panes_created_for_the_same_team() {
        let mut teams = TmuxTeams::default();
        let alpha = teams.issue();
        let beta = teams.issue();
        teams.enter(&alpha, 10, alive_all).expect("alpha leader");
        teams.enter(&beta, 20, alive_all).expect("beta leader");
        let beta_member = {
            let team = teams.team_mut(&beta).expect("beta");
            let local = team.reserve(LEADER, true).expect("reserve");
            team.attach(local, 21);
            local
        };
        let alpha_team = teams.team_mut(&alpha).expect("alpha");
        assert_eq!(
            alpha_team.resolve(&Target::Pane(beta_member), LEADER),
            Err(TeamError::NoSuchPane(format!("%{beta_member}"))),
            "a pane id from another team never resolves"
        );
        assert_eq!(
            teams.enter(&alpha, 21, alive_all),
            Err(TeamError::NotInTeam),
            "a teammate of beta cannot use alpha's token"
        );
        assert_eq!(teams.enter(&beta, 21, alive_all), Ok(beta_member));
        assert_eq!(
            teams.enter("not-a-token", 10, alive_all),
            Err(TeamError::UnknownTeam)
        );
    }

    #[test]
    fn closing_the_leader_dissolves_the_team_but_not_its_teammates() {
        let mut teams = TmuxTeams::default();
        let token = teams.issue();
        teams.enter(&token, 10, alive_all).expect("leader");
        {
            let team = teams.team_mut(&token).expect("team");
            let local = team.reserve(LEADER, true).expect("reserve");
            team.attach(local, 11);
        }
        let leader_closed = |surface: u64| surface != 10;
        assert_eq!(
            teams.enter(&token, 11, leader_closed),
            Err(TeamError::LeaderClosed)
        );
        assert_eq!(
            teams.enter(&token, 11, leader_closed),
            Err(TeamError::UnknownTeam),
            "the teammate pane is now an ordinary pane"
        );
    }

    #[test]
    fn a_pane_closed_by_hand_leaves_the_listing() {
        let mut teams = TmuxTeams::default();
        let token = teams.issue();
        teams.enter(&token, 10, alive_all).expect("leader");
        let team = teams.team_mut(&token).expect("team");
        let first = team.reserve(LEADER, true).expect("reserve");
        team.attach(first, 11);
        let pending = team.reserve(first, false).expect("reserve");
        teams
            .enter(&token, 10, |surface| surface != 11)
            .expect("leader alive");
        let team = teams.team_mut(&token).expect("team");
        assert_eq!(team.locals(), vec![LEADER, pending]);
        assert_eq!(
            team.anchor_surface(pending),
            Some(10),
            "a pane anchored on a closed pane opens next to the leader"
        );
    }

    #[test]
    fn titles_and_anchors_follow_the_reserved_pane() {
        let mut teams = TmuxTeams::default();
        let token = teams.issue();
        teams.enter(&token, 10, alive_all).expect("leader");
        let team = teams.team_mut(&token).expect("team");
        let first = team.reserve(LEADER, true).expect("reserve");
        team.set_title(first, "teammate-1".to_string());
        team.attach(first, 11);
        let second = team.reserve(first, false).expect("reserve");
        assert_eq!(team.title(first), "teammate-1");
        assert_eq!(team.anchor_surface(second), Some(11));
        assert_eq!(team.surface_of(LEADER), Some(10));
        assert_eq!(team.remove(second).map(|pane| pane.local), Some(second));
        assert_eq!(team.locals(), vec![LEADER, first]);
    }
}
