pub fn home_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

pub fn claude_config_dir() -> Option<std::path::PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        Some(explicit) => Some(explicit),
        None => home_dir().map(|home| home.join(".claude")),
    }
}
