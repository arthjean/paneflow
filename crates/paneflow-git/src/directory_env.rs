use std::path::{Path, PathBuf};

pub type Env = std::sync::Arc<[(String, String)]>;

type DirectoryEnvs = Vec<(PathBuf, Env)>;

#[cfg(not(any(test, feature = "test-support")))]
fn with_directory_envs<R>(f: impl FnOnce(&mut DirectoryEnvs) -> R) -> R {
    static DIRECTORY_ENVS: std::sync::Mutex<DirectoryEnvs> = std::sync::Mutex::new(Vec::new());
    f(&mut DIRECTORY_ENVS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

#[cfg(any(test, feature = "test-support"))]
fn with_directory_envs<R>(f: impl FnOnce(&mut DirectoryEnvs) -> R) -> R {
    thread_local! {
        static DIRECTORY_ENVS: std::cell::RefCell<DirectoryEnvs> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    DIRECTORY_ENVS.with(|envs| f(&mut envs.borrow_mut()))
}

pub fn publish(entries: Vec<(PathBuf, Vec<(String, String)>)>) {
    let entries = if entries.iter().all(|(_, env)| env.is_empty()) {
        Vec::new()
    } else {
        entries
            .into_iter()
            .map(|(dir, env)| (dir, Env::from(env)))
            .collect()
    };
    with_directory_envs(|envs| *envs = entries);
}

pub fn directory_env(dir: &Path) -> Option<Env> {
    with_directory_envs(|entries| {
        entries
            .iter()
            .filter(|(root, _)| dir.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, env)| env.clone())
            .filter(|env| !env.is_empty())
    })
}

pub fn apply(command: &mut std::process::Command) {
    if let Some(env) = command.get_current_dir().and_then(directory_env) {
        command.envs(env.iter().map(|(key, value)| (key, value)));
    }
}
