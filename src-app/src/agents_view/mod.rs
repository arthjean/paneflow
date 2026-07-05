//! Agents-view skills browser.
//!
//! The discovery/welcome shell (agent list, signed-in chips, login
//! terminal, missing-agents card) was removed with the "Connect" page:
//! agents now authenticate in their own launched terminal, so a
//! separate sign-in surface is redundant. Only the Skills browser
//! remains here.

mod skills;

pub(crate) use skills::{SkillEntry, SkillsTab, discover_skills, render_skills_page};
