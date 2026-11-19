use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) const DESKTOP_FILE: &str = "paneflow.desktop";
const INSTALLED_ICON: &str = "paneflow";
const MAIN_GROUP: &str = "[Desktop Entry]";
const ICON_KEY: &str = "Icon";
const EXEC_KEY: &str = "Exec";
const TRY_EXEC_KEY: &str = "TryExec";
const MIRROR_KEY: &str = "X-Paneflow-Mirror-Of";
const ENV_PROGRAM: &str = "env";
const SYSTEM_DATA_DIRS: [&str; 2] = ["/usr/local/share", "/usr/share"];
const APPLICATIONS: &str = "applications";

pub(super) struct Applications {
    pub(super) user: PathBuf,
    pub(super) system: Vec<PathBuf>,
}

impl Applications {
    pub(super) fn from_environment() -> Option<Self> {
        let user = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))?
            .join(APPLICATIONS);
        let mut data_dirs: Vec<PathBuf> = std::env::var_os("XDG_DATA_DIRS")
            .map(|listed| {
                std::env::split_paths(&listed)
                    .filter(|dir| dir.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        if data_dirs.is_empty() {
            data_dirs = SYSTEM_DATA_DIRS.iter().map(PathBuf::from).collect();
        }
        let system = data_dirs
            .into_iter()
            .map(|dir| dir.join(APPLICATIONS))
            .filter(|dir| *dir != user)
            .collect();
        Some(Self { user, system })
    }
}

pub(super) struct Launcher<'a> {
    pub(super) executable: &'a Path,
    pub(super) search_path: Option<&'a OsStr>,
}

impl Launcher<'_> {
    fn program_of(&self, entry: &str) -> Option<PathBuf> {
        let exec = value(entry, EXEC_KEY)?;
        let program = program_name(&arguments(&unescape(exec)))?;
        let candidates: Vec<PathBuf> = if program.contains('/') {
            let path = PathBuf::from(&program);
            if !path.is_absolute() {
                return None;
            }
            vec![path]
        } else {
            self.search_path
                .map(|listed| {
                    std::env::split_paths(listed)
                        .filter(|dir| dir.is_absolute())
                        .map(|dir| dir.join(&program))
                        .collect()
                })
                .unwrap_or_default()
        };
        candidates
            .into_iter()
            .find(|candidate| same_file(candidate, self.executable))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Unchanged,
    Mirrored,
    Pointed,
    Removed,
}

pub(super) fn sync(
    applications: &Applications,
    launcher: &Launcher<'_>,
    icon: Option<&Path>,
    icons: &Path,
) -> io::Result<Change> {
    let user_entry = applications.user.join(DESKTOP_FILE);
    match std::fs::symlink_metadata(&user_entry) {
        Ok(meta) if meta.is_file() => {
            let entry = std::fs::read_to_string(&user_entry)?;
            return match value(&entry, MIRROR_KEY) {
                Some(source) => sync_mirror(
                    &user_entry,
                    &entry,
                    Path::new(&unescape(source)),
                    launcher,
                    icon,
                ),
                None => point_own_entry(&user_entry, &entry, launcher, icon, icons),
            };
        }
        Ok(_) => return Ok(Change::Unchanged),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let Some(icon) = icon else {
        return Ok(Change::Unchanged);
    };
    let Some(source) = applications
        .system
        .iter()
        .map(|dir| dir.join(DESKTOP_FILE))
        .find(|path| path.is_file())
    else {
        return Ok(Change::Unchanged);
    };
    let entry = std::fs::read_to_string(&source)?;
    let Some(program) = launcher.program_of(&entry) else {
        return Ok(Change::Unchanged);
    };
    let Some(mirror) = mirror_of(&entry, &source, icon, &program) else {
        return Ok(Change::Unchanged);
    };
    write_entry(&user_entry, &mirror)?;
    Ok(Change::Mirrored)
}

fn sync_mirror(
    user_entry: &Path,
    mirror: &str,
    source: &Path,
    launcher: &Launcher<'_>,
    icon: Option<&Path>,
) -> io::Result<Change> {
    let Some(icon) = icon else {
        return remove_entry(user_entry);
    };
    let entry = match std::fs::read_to_string(source) {
        Ok(entry) => entry,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return remove_entry(user_entry),
        Err(err) => return Err(err),
    };
    let Some(program) = launcher.program_of(&entry) else {
        return Ok(Change::Unchanged);
    };
    let Some(expected) = mirror_of(&entry, source, icon, &program) else {
        return Ok(Change::Unchanged);
    };
    if expected == mirror {
        return Ok(Change::Unchanged);
    }
    write_entry(user_entry, &expected)?;
    Ok(Change::Mirrored)
}

fn point_own_entry(
    user_entry: &Path,
    entry: &str,
    launcher: &Launcher<'_>,
    icon: Option<&Path>,
    icons: &Path,
) -> io::Result<Change> {
    if launcher.program_of(entry).is_none() {
        return Ok(Change::Unchanged);
    }
    let current = value(entry, ICON_KEY);
    if !icon_is_paneflows(current, icons) {
        return Ok(Change::Unchanged);
    }
    let wanted = match icon {
        Some(path) => match escape_path(path) {
            Some(escaped) => escaped,
            None => return Ok(Change::Unchanged),
        },
        None => INSTALLED_ICON.to_string(),
    };
    if current == Some(wanted.as_str()) {
        return Ok(Change::Unchanged);
    }
    write_entry(user_entry, &rewrite(entry, &[(ICON_KEY, wanted)]))?;
    Ok(Change::Pointed)
}

fn icon_is_paneflows(current: Option<&str>, icons: &Path) -> bool {
    current.is_none_or(|escaped| {
        let named = unescape(escaped);
        named == INSTALLED_ICON || Path::new(&named).starts_with(icons)
    })
}

fn mirror_of(entry: &str, source: &Path, icon: &Path, program: &Path) -> Option<String> {
    let mut set = vec![
        (ICON_KEY, escape_path(icon)?),
        (MIRROR_KEY, escape_path(source)?),
    ];
    if value(entry, TRY_EXEC_KEY).is_none() {
        set.push((TRY_EXEC_KEY, escape_path(program)?));
    }
    Some(rewrite(entry, &set))
}

fn remove_entry(user_entry: &Path) -> io::Result<Change> {
    match std::fs::remove_file(user_entry) {
        Ok(()) => Ok(Change::Removed),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Change::Unchanged),
        Err(err) => Err(err),
    }
}

fn write_entry(path: &Path, contents: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("the desktop entry has no folder"))?;
    std::fs::create_dir_all(directory)?;
    let staging = directory.join(format!(".{DESKTOP_FILE}.{}", std::process::id()));
    let written = std::fs::write(&staging, contents)
        .and_then(|()| match std::fs::metadata(path) {
            Ok(existing) => std::fs::set_permissions(&staging, existing.permissions()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        })
        .and_then(|()| std::fs::rename(&staging, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    written
}

fn same_file(candidate: &Path, executable: &Path) -> bool {
    match (std::fs::metadata(candidate), std::fs::metadata(executable)) {
        (Ok(candidate), Ok(executable)) => {
            candidate.is_file()
                && candidate.dev() == executable.dev()
                && candidate.ino() == executable.ino()
        }
        _ => false,
    }
}

fn group_header(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    trimmed.starts_with('[').then_some(trimmed)
}

fn key_value(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let (key, value) = trimmed.split_once('=')?;
    Some((key.trim_end(), value.trim_start()))
}

fn value<'a>(entry: &'a str, key: &str) -> Option<&'a str> {
    let mut in_main = false;
    for line in entry.lines() {
        if let Some(header) = group_header(line) {
            if in_main {
                return None;
            }
            in_main = header == MAIN_GROUP;
            continue;
        }
        if in_main
            && let Some((found, value)) = key_value(line)
            && found == key
        {
            return Some(value);
        }
    }
    None
}

fn rewrite(entry: &str, set: &[(&str, String)]) -> String {
    let mut written = vec![false; set.len()];
    let missing: Vec<usize> = set
        .iter()
        .enumerate()
        .filter(|(_, (key, _))| value(entry, key).is_none())
        .map(|(index, _)| index)
        .collect();
    let mut out = String::with_capacity(entry.len() + 256);
    let mut in_main = false;
    let mut main_seen = false;
    for line in entry.lines() {
        if let Some(header) = group_header(line) {
            in_main = header == MAIN_GROUP && !main_seen;
            out.push_str(line);
            out.push('\n');
            if in_main {
                main_seen = true;
                for &index in &missing {
                    let (key, value) = &set[index];
                    out.push_str(&format!("{key}={value}\n"));
                    written[index] = true;
                }
            }
            continue;
        }
        if in_main
            && let Some((key, _)) = key_value(line)
            && let Some(index) = set.iter().position(|(wanted, _)| *wanted == key)
        {
            if !written[index] {
                let (key, value) = &set[index];
                out.push_str(&format!("{key}={value}\n"));
                written[index] = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn arguments(exec: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = exec.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut argument = String::new();
        if first == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            argument.push(escaped);
                        }
                    }
                    other => argument.push(other),
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                argument.push(c);
            }
        }
        found.push(argument);
    }
    found
}

fn program_name(arguments: &[String]) -> Option<String> {
    let mut remaining = arguments.iter();
    let first = remaining.next()?;
    if Path::new(first).file_name() != Some(OsStr::new(ENV_PROGRAM)) {
        return Some(first.clone());
    }
    remaining
        .find(|argument| !argument.contains('=') && !argument.starts_with('-'))
        .cloned()
}

fn escape_path(path: &Path) -> Option<String> {
    path.to_str().map(escape)
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (index, c) in value.chars().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            ' ' if index == 0 => out.push_str("\\s"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE_ENTRY: &str = "[Desktop Entry]\n\
        Name=Paneflow\n\
        Exec=paneflow %U\n\
        Icon=paneflow\n\
        Type=Application\n\
        StartupWMClass=paneflow\n\
        Actions=new-window;\n\
        \n\
        [Desktop Action new-window]\n\
        Name=New Window\n\
        Exec=paneflow --new-window\n\
        Icon=paneflow\n";

    struct Desktop {
        _root: tempfile::TempDir,
        applications: Applications,
        bin: PathBuf,
        executable: PathBuf,
        icons: PathBuf,
        icon: PathBuf,
    }

    impl Desktop {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let applications = Applications {
                user: root.path().join("home").join("applications"),
                system: vec![
                    root.path().join("local").join("applications"),
                    root.path().join("usr").join("applications"),
                ],
            };
            let bin = root.path().join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let executable = bin.join("paneflow");
            std::fs::write(&executable, b"paneflow").unwrap();
            let icons = root.path().join("paneflow-home").join("icons");
            let icon = icons.join("paneflow-amethyst-0a1b2c3d.png");
            Self {
                _root: root,
                applications,
                bin,
                executable,
                icons,
                icon,
            }
        }

        fn install_package_entry(&self, entry: &str) -> PathBuf {
            let directory = &self.applications.system[1];
            std::fs::create_dir_all(directory).unwrap();
            let path = directory.join(DESKTOP_FILE);
            std::fs::write(&path, entry).unwrap();
            path
        }

        fn write_user_entry(&self, entry: &str) -> PathBuf {
            std::fs::create_dir_all(&self.applications.user).unwrap();
            let path = self.user_entry();
            std::fs::write(&path, entry).unwrap();
            path
        }

        fn user_entry(&self) -> PathBuf {
            self.applications.user.join(DESKTOP_FILE)
        }

        fn sync(&self, icon: Option<&Path>) -> Change {
            let launcher = Launcher {
                executable: &self.executable,
                search_path: Some(self.bin.as_os_str()),
            };
            sync(&self.applications, &launcher, icon, &self.icons).unwrap()
        }

        fn shown_icon(&self) -> Option<String> {
            std::iter::once(&self.applications.user)
                .chain(&self.applications.system)
                .map(|dir| dir.join(DESKTOP_FILE))
                .find(|path| path.is_file())
                .and_then(|path| {
                    let entry = std::fs::read_to_string(path).unwrap();
                    value(&entry, ICON_KEY).map(unescape)
                })
        }
    }

    fn icon_text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_package_entry_is_mirrored_with_the_chosen_icon_and_glass_removes_the_mirror() {
        let desktop = Desktop::new();
        let source = desktop.install_package_entry(PACKAGE_ENTRY);
        assert_eq!(desktop.shown_icon().as_deref(), Some(INSTALLED_ICON));

        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Mirrored);
        assert_eq!(desktop.shown_icon(), Some(icon_text(&desktop.icon)));
        let mirror = std::fs::read_to_string(desktop.user_entry()).unwrap();
        assert_eq!(
            value(&mirror, MIRROR_KEY).map(unescape),
            Some(icon_text(&source))
        );
        assert_eq!(
            value(&mirror, TRY_EXEC_KEY).map(unescape),
            Some(icon_text(&desktop.bin.join("paneflow")))
        );
        assert_eq!(value(&mirror, "Name"), Some("Paneflow"));
        assert_eq!(value(&mirror, EXEC_KEY), Some("paneflow %U"));
        assert!(mirror.contains("[Desktop Action new-window]\nName=New Window\nExec=paneflow --new-window\nIcon=paneflow\n"));
        assert_eq!(mirror.matches("Icon=").count(), 2);
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Unchanged);

        assert_eq!(desktop.sync(None), Change::Removed);
        assert!(!desktop.user_entry().exists());
        assert_eq!(desktop.shown_icon().as_deref(), Some(INSTALLED_ICON));
        assert_eq!(desktop.sync(None), Change::Unchanged);
    }

    #[test]
    fn the_mirror_follows_an_updated_package_entry_and_leaves_with_it() {
        let desktop = Desktop::new();
        let source = desktop.install_package_entry(PACKAGE_ENTRY);
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Mirrored);

        std::fs::write(
            &source,
            PACKAGE_ENTRY.replace("Name=Paneflow\n", "Name=Paneflow Nightly\n"),
        )
        .unwrap();
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Mirrored);
        let mirror = std::fs::read_to_string(desktop.user_entry()).unwrap();
        assert_eq!(value(&mirror, "Name"), Some("Paneflow Nightly"));
        assert_eq!(desktop.shown_icon(), Some(icon_text(&desktop.icon)));

        let other = desktop.icons.join("paneflow-chip-0f0f0f0f.png");
        assert_eq!(desktop.sync(Some(&other)), Change::Mirrored);
        assert_eq!(desktop.shown_icon(), Some(icon_text(&other)));

        std::fs::remove_file(&source).unwrap();
        assert_eq!(desktop.sync(Some(&other)), Change::Removed);
        assert!(!desktop.user_entry().exists());
    }

    #[test]
    fn a_package_entry_with_its_own_try_exec_keeps_it() {
        let desktop = Desktop::new();
        desktop.install_package_entry(&PACKAGE_ENTRY.replace(
            "Type=Application\n",
            "Type=Application\nTryExec=/opt/paneflow/paneflow\n",
        ));

        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Mirrored);
        let mirror = std::fs::read_to_string(desktop.user_entry()).unwrap();
        assert_eq!(value(&mirror, TRY_EXEC_KEY), Some("/opt/paneflow/paneflow"));
        assert_eq!(mirror.matches("TryExec=").count(), 1);
    }

    #[test]
    fn the_first_package_entry_on_the_data_dirs_is_the_one_mirrored() {
        let desktop = Desktop::new();
        desktop.install_package_entry(PACKAGE_ENTRY);
        let preferred = desktop.applications.system[0].join(DESKTOP_FILE);
        std::fs::create_dir_all(&desktop.applications.system[0]).unwrap();
        std::fs::write(
            &preferred,
            PACKAGE_ENTRY.replace("Name=Paneflow\n", "Name=Local\n"),
        )
        .unwrap();

        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Mirrored);
        let mirror = std::fs::read_to_string(desktop.user_entry()).unwrap();
        assert_eq!(value(&mirror, "Name"), Some("Local"));
        assert_eq!(
            value(&mirror, MIRROR_KEY).map(unescape),
            Some(icon_text(&preferred))
        );
    }

    #[test]
    fn a_user_entry_of_this_paneflow_is_pointed_at_the_icon_and_back() {
        let desktop = Desktop::new();
        let own = format!(
            "[Desktop Entry]\nName=PaneFlow\nExec={}\nIcon=paneflow\nType=Application\n",
            desktop.executable.display()
        );
        desktop.write_user_entry(&own);

        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Pointed);
        assert_eq!(desktop.shown_icon(), Some(icon_text(&desktop.icon)));
        let pointed = std::fs::read_to_string(desktop.user_entry()).unwrap();
        assert_eq!(value(&pointed, MIRROR_KEY), None);
        assert_eq!(
            pointed,
            own.replace(
                "Icon=paneflow\n",
                &format!("Icon={}\n", icon_text(&desktop.icon))
            )
        );
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Unchanged);

        assert_eq!(desktop.sync(None), Change::Pointed);
        assert_eq!(std::fs::read_to_string(desktop.user_entry()).unwrap(), own);
        assert_eq!(desktop.sync(None), Change::Unchanged);
    }

    #[test]
    fn a_user_entry_keeps_an_icon_the_user_chose_and_its_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let desktop = Desktop::new();
        let custom = format!(
            "[Desktop Entry]\nExec={}\nIcon=/home/someone/my-icon.png\n",
            desktop.executable.display()
        );
        let path = desktop.write_user_entry(&custom);
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Unchanged);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);

        let own = format!(
            "[Desktop Entry]\nExec={}\nIcon=paneflow\n",
            desktop.executable.display()
        );
        std::fs::write(&path, &own).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Pointed);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750);
        let staged: Vec<_> = std::fs::read_dir(&desktop.applications.user)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(staged, vec![std::ffi::OsString::from(DESKTOP_FILE)]);
    }

    #[test]
    fn an_entry_that_launches_another_program_is_left_alone() {
        let desktop = Desktop::new();
        let other = desktop.bin.join("other");
        std::fs::write(&other, b"other").unwrap();
        desktop.install_package_entry(&PACKAGE_ENTRY.replace("Exec=paneflow %U", "Exec=other"));
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Unchanged);
        assert!(!desktop.user_entry().exists());

        let foreign = format!("[Desktop Entry]\nExec={}\nIcon=paneflow\n", other.display());
        desktop.write_user_entry(&foreign);
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Unchanged);
        assert_eq!(
            std::fs::read_to_string(desktop.user_entry()).unwrap(),
            foreign
        );
    }

    #[test]
    fn nothing_is_written_without_an_entry_or_through_a_symlink() {
        let desktop = Desktop::new();
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Unchanged);
        assert!(!desktop.applications.user.exists());

        let source = desktop.install_package_entry(PACKAGE_ENTRY);
        std::fs::create_dir_all(&desktop.applications.user).unwrap();
        std::os::unix::fs::symlink(&source, desktop.user_entry()).unwrap();
        assert_eq!(desktop.sync(Some(&desktop.icon)), Change::Unchanged);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), PACKAGE_ENTRY);
    }

    #[test]
    fn the_program_is_read_through_quotes_env_and_a_symlink_on_the_search_path() {
        let desktop = Desktop::new();
        let linked = desktop.bin.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(&desktop.executable, linked.join("paneflow")).unwrap();
        let launcher = Launcher {
            executable: &desktop.executable,
            search_path: Some(linked.as_os_str()),
        };
        let entry = |exec: &str| format!("[Desktop Entry]\nExec={exec}\n");

        assert_eq!(
            launcher.program_of(&entry("paneflow %U")),
            Some(linked.join("paneflow"))
        );
        assert_eq!(
            launcher.program_of(&entry("env PANEFLOW_X=1 paneflow --flag")),
            Some(linked.join("paneflow"))
        );
        let quoted = format!("\"{}\" %F", desktop.executable.display());
        assert_eq!(
            launcher.program_of(&entry(&quoted)),
            Some(desktop.executable.clone())
        );
        assert_eq!(launcher.program_of(&entry("bin/paneflow")), None);
        assert_eq!(launcher.program_of(&entry("missing")), None);
        assert_eq!(launcher.program_of("[Other]\nExec=paneflow\n"), None);
    }

    #[test]
    fn exec_arguments_follow_the_desktop_entry_quoting_rules() {
        assert_eq!(
            arguments(r#""/opt/Pane Flow/paneflow" --title "say \"hi\"" %U"#),
            vec!["/opt/Pane Flow/paneflow", "--title", "say \"hi\"", "%U"]
        );
        assert_eq!(
            program_name(&arguments("/usr/bin/env -i A=1 B=2 paneflow")),
            Some("paneflow".to_string())
        );
        assert_eq!(program_name(&arguments("   ")), None);
    }

    #[test]
    fn string_values_escape_and_unescape_back() {
        for raw in [
            " lead",
            "a\\b",
            "line\nbreak",
            "tab\there",
            "/plain/path.png",
        ] {
            assert_eq!(unescape(&escape(raw)), raw);
        }
        assert_eq!(escape(" a b"), "\\sa b");
        assert_eq!(unescape("\\q\\"), "\\q\\");
    }

    #[test]
    fn values_are_read_from_the_main_group_only() {
        let entry = "# Icon=comment\n[Desktop Entry]\nName = Spaced\nIcon[fr]=fr\n[Desktop Action a]\nIcon=action\n";
        assert_eq!(value(entry, "Name"), Some("Spaced"));
        assert_eq!(value(entry, ICON_KEY), None);
        let rewritten = rewrite(entry, &[(ICON_KEY, "new".to_string())]);
        assert_eq!(
            rewritten,
            "# Icon=comment\n[Desktop Entry]\nIcon=new\nName = Spaced\nIcon[fr]=fr\n[Desktop Action a]\nIcon=action\n"
        );
    }
}
