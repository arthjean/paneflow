use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use paneflow_agent_config::runtime_catalog::SCREEN_RULE_SOURCES;
use paneflow_agent_config::screen_rules::{
    RuleEntry, RuleOrigin, ScreenRule, apply_overrides, parse_base_rules, parse_rule_file,
};

pub const LOCAL_RULES_FILE: &str = "screen.toml";

#[derive(Debug, Clone)]
pub struct RemoteRules {
    pub version: u64,
    pub runtimes: BTreeMap<String, Vec<ScreenRule>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSourceStatus {
    pub remote_version: Option<u64>,
    pub remote_rejection: Option<String>,
    pub local_path: Option<String>,
    pub local_error: Option<String>,
}

type FileStamp = (SystemTime, u64);

#[derive(Debug, Default)]
struct LocalSource {
    path: PathBuf,
    stamp: Option<FileStamp>,
    entries: Vec<RuleEntry>,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct Sources {
    builtin: BTreeMap<String, Vec<ScreenRule>>,
    remote: Option<RemoteRules>,
    remote_rejection: Option<String>,
    local: BTreeMap<String, LocalSource>,
    active: BTreeMap<String, Arc<[ScreenRule]>>,
}

#[derive(Debug, Default)]
pub struct ScreenRuleRegistry {
    sources: RwLock<Sources>,
    generation: AtomicU64,
}

impl ScreenRuleRegistry {
    pub fn with_builtin() -> Self {
        let mut builtin = BTreeMap::new();
        for (slug, text) in SCREEN_RULE_SOURCES {
            match parse_base_rules(text, RuleOrigin::Builtin) {
                Ok(rules) => {
                    builtin.insert((*slug).to_string(), rules);
                }
                Err(error) => {
                    log::error!("paneflow-host: builtin screen rules of {slug} rejected: {error}");
                }
            }
        }
        let registry = Self::default();
        registry.update(|sources| sources.builtin = builtin);
        registry
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn rules_for(&self, slug: &str) -> Option<Arc<[ScreenRule]>> {
        self.read().active.get(slug).cloned()
    }

    pub fn status(&self, slug: &str) -> RuleSourceStatus {
        let sources = self.read();
        let local = sources.local.get(slug);
        RuleSourceStatus {
            remote_version: sources.remote.as_ref().map(|remote| remote.version),
            remote_rejection: sources.remote_rejection.clone(),
            local_path: local.map(|local| local.path.display().to_string()),
            local_error: local.and_then(|local| local.error.clone()),
        }
    }

    pub fn remote_version(&self) -> Option<u64> {
        self.read().remote.as_ref().map(|remote| remote.version)
    }

    pub fn apply_remote(&self, remote: RemoteRules) {
        log::info!(
            "paneflow-host: remote screen catalog v{} applied for {} runtime(s)",
            remote.version,
            remote.runtimes.len()
        );
        self.update(|sources| {
            sources.remote = Some(remote);
            sources.remote_rejection = None;
        });
    }

    pub fn reject_remote(&self, reason: String) {
        log::warn!("paneflow-host: remote screen catalog rejected: {reason}");
        let mut sources = self.write();
        sources.remote_rejection = Some(reason);
    }

    pub fn reload_local(&self, home: &Path) -> bool {
        let root = paneflow_home::screen_rule_overrides_dir_in(home);
        let mut found: BTreeMap<String, (PathBuf, FileStamp)> = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let path = entry.path().join(LOCAL_RULES_FILE);
                if let Some(stamp) = file_stamp(&path) {
                    found.insert(slug, (path, stamp));
                }
            }
        }
        let changed: Vec<String> = {
            let sources = self.read();
            let held: BTreeSet<&String> = sources.local.keys().collect();
            found
                .iter()
                .filter(|(slug, (_, stamp))| {
                    sources
                        .local
                        .get(*slug)
                        .is_none_or(|local| local.stamp != Some(*stamp))
                })
                .map(|(slug, _)| slug.clone())
                .chain(
                    held.into_iter()
                        .filter(|slug| !found.contains_key(*slug))
                        .cloned(),
                )
                .collect()
        };
        if changed.is_empty() {
            return false;
        }
        let mut loaded = Vec::with_capacity(changed.len());
        for slug in changed {
            match found.get(&slug) {
                None => loaded.push((slug, None)),
                Some((path, stamp)) => {
                    let parsed = std::fs::read_to_string(path)
                        .map_err(|error| error.to_string())
                        .and_then(|text| {
                            parse_rule_file(&text, RuleOrigin::Local)
                                .map_err(|error| error.to_string())
                        });
                    loaded.push((slug, Some((path.clone(), *stamp, parsed))));
                }
            }
        }
        self.update(|sources| {
            for (slug, outcome) in loaded {
                match outcome {
                    None => {
                        log::info!("paneflow-host: local screen rules of {slug} removed");
                        sources.local.remove(&slug);
                    }
                    Some((path, stamp, Ok(entries))) => {
                        log::info!(
                            "paneflow-host: local screen rules of {slug} loaded from {}",
                            path.display()
                        );
                        sources.local.insert(
                            slug,
                            LocalSource {
                                path,
                                stamp: Some(stamp),
                                entries,
                                error: None,
                            },
                        );
                    }
                    Some((path, stamp, Err(error))) => {
                        let error = format!("{}: {error}", path.display());
                        log::warn!(
                            "paneflow-host: local screen rules of {slug} rejected, the previous rules stay active: {error}"
                        );
                        let local = sources.local.entry(slug).or_default();
                        local.path = path;
                        local.stamp = Some(stamp);
                        local.error = Some(error);
                    }
                }
            }
        });
        true
    }

    fn update(&self, change: impl FnOnce(&mut Sources)) {
        let mut sources = self.write();
        change(&mut sources);
        let mut slugs: BTreeSet<String> = sources.builtin.keys().cloned().collect();
        if let Some(remote) = &sources.remote {
            slugs.extend(remote.runtimes.keys().cloned());
        }
        slugs.extend(sources.local.keys().cloned());
        let mut active = BTreeMap::new();
        for slug in slugs {
            let base = sources
                .remote
                .as_ref()
                .and_then(|remote| remote.runtimes.get(&slug))
                .or_else(|| sources.builtin.get(&slug))
                .cloned()
                .unwrap_or_default();
            let rules = match sources.local.get(&slug) {
                Some(local) => apply_overrides(&base, local.entries.clone()),
                None => base,
            };
            if !rules.is_empty() {
                active.insert(slug, Arc::from(rules));
            }
        }
        sources.active = active;
        drop(sources);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Sources> {
        self.sources
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Sources> {
        self.sources
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata.is_file().then(|| {
        (
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata.len(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_agent_config::screen_rules::{ScreenInput, ScreenState, evaluate};

    fn state_of(registry: &ScreenRuleRegistry, slug: &str, screen: &str) -> Option<ScreenState> {
        let rules = registry.rules_for(slug)?;
        evaluate(
            &rules,
            &ScreenInput {
                screen,
                ..ScreenInput::default()
            },
        )
        .state(&rules)
    }

    fn write_local(home: &Path, slug: &str, text: &str) {
        let dir = paneflow_home::screen_rule_overrides_dir_in(home).join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LOCAL_RULES_FILE), text).unwrap();
    }

    #[test]
    fn the_builtin_rules_are_loaded_by_the_runtime_parser_at_startup() {
        let registry = ScreenRuleRegistry::with_builtin();
        for slug in ["claude-code", "codex", "gemini"] {
            assert!(registry.rules_for(slug).is_some(), "{slug}");
        }
        assert_eq!(
            state_of(&registry, "claude-code", "✻ Levitating… (1m 52s)\n❯"),
            Some(ScreenState::Working)
        );
        assert!(registry.rules_for("pi").is_none());
    }

    #[test]
    fn an_invalid_runtime_source_is_rejected_whole_and_the_previous_rules_stay() {
        let home = tempfile::tempdir().unwrap();
        let registry = ScreenRuleRegistry::with_builtin();
        write_local(
            home.path(),
            "claude-code",
            "engine = 2\n[[rules]]\nid = \"idle-prompt\"\nstate = \"idle\"\npriority = 10\nany = ['(?m)^>']\n",
        );
        assert!(registry.reload_local(home.path()));
        assert_eq!(
            state_of(&registry, "claude-code", "> "),
            Some(ScreenState::Idle)
        );
        let generation = registry.generation();

        std::thread::sleep(std::time::Duration::from_millis(20));
        write_local(
            home.path(),
            "claude-code",
            "engine = 2\n[[rules]]\nid = \"idle-prompt\"\nstate = \"idle\"\nany = ['(broken']\n[[rules]]\nid = \"other\"\nstate = \"asleep\"\nany = ['x']\n",
        );
        assert!(registry.reload_local(home.path()));
        assert_eq!(
            state_of(&registry, "claude-code", "> "),
            Some(ScreenState::Idle),
            "the previous local rules stay active"
        );
        let status = registry.status("claude-code");
        let error = status.local_error.expect("the error is kept for explain");
        assert!(error.contains("line 5"), "{error}");
        assert!(error.contains("invalid regex"), "{error}");
        assert!(registry.generation() > generation);
        assert!(
            !registry.reload_local(home.path()),
            "an unchanged file is not reparsed"
        );
    }

    #[test]
    fn removing_the_local_file_restores_the_builtin_rules() {
        let home = tempfile::tempdir().unwrap();
        let registry = ScreenRuleRegistry::with_builtin();
        write_local(
            home.path(),
            "claude-code",
            "engine = 2\n[[rules]]\nid = \"working-spinner\"\ndisabled = true\n",
        );
        registry.reload_local(home.path());
        assert_eq!(state_of(&registry, "claude-code", "esc to interrupt"), None);
        std::fs::remove_file(
            paneflow_home::screen_rule_overrides_dir_in(home.path())
                .join("claude-code")
                .join(LOCAL_RULES_FILE),
        )
        .unwrap();
        assert!(registry.reload_local(home.path()));
        assert_eq!(
            state_of(&registry, "claude-code", "esc to interrupt"),
            Some(ScreenState::Working)
        );
    }

    #[test]
    fn local_rules_override_the_remote_catalog_which_overrides_the_builtin_rules() {
        let home = tempfile::tempdir().unwrap();
        let registry = ScreenRuleRegistry::with_builtin();
        let remote = parse_base_rules(
            "engine = 2\n[[rules]]\nid = \"idle-prompt\"\nstate = \"idle\"\nany = ['(?m)^remote-prompt']\n[[rules]]\nid = \"working-remote\"\nstate = \"working\"\npriority = 20\nany = ['remote-busy']\n",
            RuleOrigin::Remote(7),
        )
        .unwrap();
        registry.apply_remote(RemoteRules {
            version: 7,
            runtimes: BTreeMap::from([("claude-code".to_string(), remote)]),
        });
        assert_eq!(
            state_of(&registry, "claude-code", "remote-prompt"),
            Some(ScreenState::Idle)
        );
        assert_eq!(state_of(&registry, "claude-code", "esc to interrupt"), None);
        write_local(
            home.path(),
            "claude-code",
            "engine = 2\n[[rules]]\nid = \"working-remote\"\nstate = \"working\"\npriority = 20\nany = ['local-busy']\n",
        );
        registry.reload_local(home.path());
        assert_eq!(state_of(&registry, "claude-code", "remote-busy"), None);
        assert_eq!(
            state_of(&registry, "claude-code", "local-busy"),
            Some(ScreenState::Working)
        );
        let rules = registry.rules_for("claude-code").unwrap();
        let origins: Vec<RuleOrigin> = rules.iter().map(|rule| rule.origin).collect();
        assert_eq!(origins, vec![RuleOrigin::Remote(7), RuleOrigin::Local]);
        assert_eq!(registry.status("claude-code").remote_version, Some(7));
    }
}
