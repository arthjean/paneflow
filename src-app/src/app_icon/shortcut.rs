use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IPersistFile, STGM, STGM_READ, STGM_READWRITE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHCNE_CREATE,
    SHCNE_DELETE, SHCNE_ID, SHCNE_UPDATEITEM, SHCNF_PATHW, SHChangeNotify, SHGetKnownFolderPath,
    ShellLink,
};
use windows::core::{GUID, HSTRING, Interface};

const PATH_BUFFER_UNITS: usize = 1024;
const START_MENU_DEPTH: usize = 2;

pub(super) struct StartMenu {
    pub(super) machine: PathBuf,
    pub(super) user: PathBuf,
}

struct Apartment {
    entered: bool,
}

impl Apartment {
    fn enter() -> Self {
        Self {
            entered: unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok(),
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.entered {
            unsafe { CoUninitialize() };
        }
    }
}

fn text(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn collect_shortcuts(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                collect_shortcuts(&path, depth - 1, out);
            }
        } else if is_shortcut(&path) {
            out.push(path);
        }
    }
}

fn is_shortcut(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

fn load(
    shortcut: &Path,
    mode: STGM,
) -> windows::core::Result<(IShellLinkW, IPersistFile, HSTRING)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        let shortcut_path = HSTRING::from(shortcut.as_os_str());
        file.Load(&shortcut_path, mode)?;
        Ok((link, file, shortcut_path))
    }
}

fn opens(link: &IShellLinkW, target: &Path) -> windows::core::Result<bool> {
    let mut linked = [0u16; PATH_BUFFER_UNITS];
    unsafe { link.GetPath(&mut linked, std::ptr::null_mut(), 0)? };
    Ok(same_file(Path::new(&text(&linked)), target))
}

fn same_file(linked: &Path, target: &Path) -> bool {
    if linked.as_os_str().is_empty() {
        return false;
    }
    if linked.as_os_str().eq_ignore_ascii_case(target.as_os_str()) {
        return true;
    }
    match (std::fs::canonicalize(linked), std::fs::canonicalize(target)) {
        (Ok(linked), Ok(target)) => linked == target,
        _ => false,
    }
}

fn notify(event: SHCNE_ID, path: &Path) {
    let wide = HSTRING::from(path.as_os_str());
    unsafe { SHChangeNotify(event, SHCNF_PATHW, Some(wide.as_ptr().cast()), None) };
}

fn point_shortcut_at(shortcut: &Path, target: &Path, icon: &str) -> windows::core::Result<bool> {
    let (link, file, shortcut_path) = load(shortcut, STGM_READWRITE)?;
    if !opens(&link, target)? {
        return Ok(false);
    }
    unsafe {
        let mut current = [0u16; PATH_BUFFER_UNITS];
        let mut index = 0i32;
        link.GetIconLocation(&mut current, &mut index)?;
        if index == 0 && text(&current).eq_ignore_ascii_case(icon) {
            return Ok(false);
        }

        link.SetIconLocation(&HSTRING::from(icon), 0)?;
        file.Save(&shortcut_path, true)?;
    }
    notify(SHCNE_UPDATEITEM, shortcut);
    Ok(true)
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let raw = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let folder = raw.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(raw.as_ptr().cast()));
        folder
    }
}

fn user_programs() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs).or_else(|| {
        dirs::config_dir().map(|roaming| {
            roaming
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
        })
    })
}

pub(super) fn start_menu() -> Option<StartMenu> {
    Some(StartMenu {
        machine: known_folder(&FOLDERID_CommonPrograms)?,
        user: user_programs()?,
    })
}

fn recorded(record: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(record)
        .map(|listed| {
            listed
                .lines()
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn write_record(record: &Path, mirrors: &[PathBuf]) -> std::io::Result<()> {
    if mirrors.is_empty() {
        return match std::fs::remove_file(record) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    if let Some(parent) = record.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listed: Vec<String> = mirrors
        .iter()
        .map(|mirror| mirror.to_string_lossy().into_owned())
        .collect();
    std::fs::write(record, listed.join("\n"))
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

fn copy_shortcut(source: &Path, mirror: &Path) -> std::io::Result<()> {
    if let Some(parent) = mirror.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, mirror).map(|_| ())
}

pub(super) fn mirror_machine_shortcuts(menu: &StartMenu, target: &Path, record: &Path) -> usize {
    let _apartment = Apartment::enter();
    let mut sources = Vec::new();
    collect_shortcuts(&menu.machine, START_MENU_DEPTH, &mut sources);
    let mut owned = recorded(record);
    let mut copied = 0;
    for source in sources {
        let Ok(relative) = source.strip_prefix(&menu.machine) else {
            continue;
        };
        match load(&source, STGM_READ).and_then(|(link, ..)| opens(&link, target)) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) => {
                log::debug!("Could not read the shortcut {}: {err}", source.display());
                continue;
            }
        }
        let mirror = menu.user.join(relative);
        let ours = owned.contains(&mirror);
        let present = mirror.exists();
        let outdated = match (modified(&source), modified(&mirror)) {
            (Some(machine), Some(user)) => machine > user,
            _ => false,
        };
        if present && !(ours && outdated) {
            continue;
        }
        if !ours {
            owned.push(mirror.clone());
            if let Err(err) = write_record(record, &owned) {
                log::warn!("Could not record the Start menu shortcut: {err}");
                owned.pop();
                continue;
            }
        }
        match copy_shortcut(&source, &mirror) {
            Ok(()) => {
                let event = if present {
                    SHCNE_UPDATEITEM
                } else {
                    SHCNE_CREATE
                };
                notify(event, &mirror);
                copied += 1;
            }
            Err(err) => log::warn!(
                "Could not copy the Start menu shortcut to {}: {err}",
                mirror.display()
            ),
        }
    }
    copied
}

fn remove_empty_folders(mirror: &Path, root: &Path) {
    let mut folder = mirror.parent();
    while let Some(current) = folder {
        if current == root || !current.starts_with(root) || std::fs::remove_dir(current).is_err() {
            return;
        }
        folder = current.parent();
    }
}

pub(super) fn remove_mirrored_shortcuts(menu: &StartMenu, record: &Path) -> usize {
    let owned = recorded(record);
    if owned.is_empty() {
        return 0;
    }
    let mut kept = Vec::new();
    let mut removed = 0;
    for mirror in owned {
        if !mirror.starts_with(&menu.user) || !is_shortcut(&mirror) {
            continue;
        }
        match std::fs::remove_file(&mirror) {
            Ok(()) => {
                removed += 1;
                notify(SHCNE_DELETE, &mirror);
                remove_empty_folders(&mirror, &menu.user);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::warn!("Could not remove the shortcut {}: {err}", mirror.display());
                kept.push(mirror);
            }
        }
    }
    if let Err(err) = write_record(record, &kept) {
        log::warn!("Could not update the Start menu shortcut record: {err}");
    }
    removed
}

pub(super) fn user_shortcut_roots() -> Vec<(PathBuf, usize)> {
    let mut roots = Vec::new();
    if let Some(roaming) = dirs::config_dir() {
        roots.push((
            roaming
                .join("Microsoft")
                .join("Internet Explorer")
                .join("Quick Launch")
                .join("User Pinned")
                .join("TaskBar"),
            0,
        ));
    }
    if let Some(programs) = user_programs() {
        roots.push((programs, START_MENU_DEPTH));
    }
    if let Some(desktop) = dirs::desktop_dir() {
        roots.push((desktop, 0));
    }
    roots
}

pub(super) fn point_shortcuts_at(roots: &[(PathBuf, usize)], target: &Path, icon: &str) -> usize {
    let _apartment = Apartment::enter();
    let mut shortcuts = Vec::new();
    for (root, depth) in roots {
        collect_shortcuts(root, *depth, &mut shortcuts);
    }
    let mut rewritten = 0;
    for shortcut in shortcuts {
        match point_shortcut_at(&shortcut, target, icon) {
            Ok(true) => rewritten += 1,
            Ok(false) => {}
            Err(err) => log::debug!("Could not read the shortcut {}: {err}", shortcut.display()),
        }
    }
    rewritten
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_shortcut(shortcut: &Path, target: &Path) {
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
            let file: IPersistFile = link.cast().unwrap();
            file.Save(&HSTRING::from(shortcut.as_os_str()), true)
                .unwrap();
        }
    }

    fn icon_of(shortcut: &Path) -> (String, i32) {
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            let file: IPersistFile = link.cast().unwrap();
            file.Load(&HSTRING::from(shortcut.as_os_str()), STGM_READWRITE)
                .unwrap();
            let mut location = [0u16; PATH_BUFFER_UNITS];
            let mut index = 0i32;
            link.GetIconLocation(&mut location, &mut index).unwrap();
            (text(&location), index)
        }
    }

    #[test]
    fn only_shortcuts_to_the_target_take_the_icon_and_the_default_comes_back() {
        let _apartment = Apartment::enter();
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.exe");
        let other = directory.path().join("other.exe");
        std::fs::write(&app, b"app").unwrap();
        std::fs::write(&other, b"other").unwrap();
        let pins = directory.path().join("pins");
        let nested = pins.join("Programs").join("Vendor");
        std::fs::create_dir_all(&nested).unwrap();
        let pinned = pins.join("App.lnk");
        let in_menu = nested.join("App.lnk");
        let foreign = pins.join("Other.lnk");
        write_shortcut(&pinned, &app);
        write_shortcut(&in_menu, &app);
        write_shortcut(&foreign, &other);
        let icon = directory.path().join("black.ico");
        let icon_text = icon.to_string_lossy().into_owned();
        let roots = [(pins.clone(), START_MENU_DEPTH)];

        assert_eq!(point_shortcuts_at(&roots, &app, &icon_text), 2);
        assert_eq!(icon_of(&pinned), (icon_text.clone(), 0));
        assert_eq!(icon_of(&in_menu), (icon_text.clone(), 0));
        assert_eq!(icon_of(&foreign), (String::new(), 0));

        assert_eq!(point_shortcuts_at(&roots, &app, &icon_text), 0);

        assert_eq!(point_shortcuts_at(&roots, &app, ""), 2);
        assert_eq!(icon_of(&pinned), (String::new(), 0));
    }

    #[test]
    fn a_target_named_through_another_path_form_is_still_recognized() {
        let _apartment = Apartment::enter();
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.exe");
        std::fs::write(&app, b"app").unwrap();
        std::fs::create_dir_all(directory.path().join("detour")).unwrap();
        let detoured = directory.path().join("detour").join("..").join("app.exe");
        let pins = directory.path().join("pins");
        std::fs::create_dir_all(&pins).unwrap();
        let pinned = pins.join("App.lnk");
        write_shortcut(&pinned, &app);
        let roots = [(pins, 0)];

        assert_eq!(point_shortcuts_at(&roots, &detoured, "black.ico"), 1);
        assert_eq!(icon_of(&pinned), ("black.ico".to_string(), 0));
        assert!(!same_file(Path::new(""), &app));
        assert!(!same_file(&directory.path().join("missing.exe"), &app));
    }

    fn menu_in(directory: &Path) -> StartMenu {
        let menu = StartMenu {
            machine: directory.join("machine"),
            user: directory.join("user"),
        };
        std::fs::create_dir_all(menu.machine.join("Vendor")).unwrap();
        std::fs::create_dir_all(&menu.user).unwrap();
        menu
    }

    fn age(path: &Path) {
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
    }

    #[test]
    fn a_machine_shortcut_to_the_target_is_mirrored_recorded_and_removed_again() {
        let _apartment = Apartment::enter();
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.exe");
        let other = directory.path().join("other.exe");
        std::fs::write(&app, b"app").unwrap();
        std::fs::write(&other, b"other").unwrap();
        let menu = menu_in(directory.path());
        write_shortcut(&menu.machine.join("Vendor").join("App.lnk"), &app);
        write_shortcut(&menu.machine.join("Other.lnk"), &other);
        let record = directory.path().join("home").join("record");
        let mirror = menu.user.join("Vendor").join("App.lnk");

        assert_eq!(mirror_machine_shortcuts(&menu, &app, &record), 1);
        assert!(mirror.exists());
        assert!(!menu.user.join("Other.lnk").exists());
        assert_eq!(recorded(&record), vec![mirror.clone()]);
        assert_eq!(mirror_machine_shortcuts(&menu, &app, &record), 0);

        let roots = [(menu.user.clone(), START_MENU_DEPTH)];
        assert_eq!(point_shortcuts_at(&roots, &app, "black.ico"), 1);
        assert_eq!(icon_of(&mirror), ("black.ico".to_string(), 0));

        assert_eq!(remove_mirrored_shortcuts(&menu, &record), 1);
        assert!(!mirror.exists());
        assert!(!menu.user.join("Vendor").exists());
        assert!(menu.user.exists());
        assert!(!record.exists());
        assert_eq!(remove_mirrored_shortcuts(&menu, &record), 0);
    }

    #[test]
    fn a_shortcut_the_user_already_has_is_neither_replaced_nor_removed() {
        let _apartment = Apartment::enter();
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.exe");
        std::fs::write(&app, b"app").unwrap();
        let menu = menu_in(directory.path());
        write_shortcut(&menu.machine.join("Vendor").join("App.lnk"), &app);
        let own = menu.user.join("Vendor").join("App.lnk");
        std::fs::create_dir_all(menu.user.join("Vendor")).unwrap();
        write_shortcut(&own, &app);
        let roots = [(menu.user.clone(), START_MENU_DEPTH)];
        assert_eq!(point_shortcuts_at(&roots, &app, "mine.ico"), 1);
        age(&own);
        let record = directory.path().join("home").join("record");

        assert_eq!(mirror_machine_shortcuts(&menu, &app, &record), 0);
        assert!(!record.exists());
        assert_eq!(remove_mirrored_shortcuts(&menu, &record), 0);
        assert_eq!(icon_of(&own), ("mine.ico".to_string(), 0));
    }

    #[test]
    fn a_newer_machine_shortcut_replaces_the_mirror_paneflow_made() {
        let _apartment = Apartment::enter();
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.exe");
        std::fs::write(&app, b"app").unwrap();
        let menu = menu_in(directory.path());
        write_shortcut(&menu.machine.join("Vendor").join("App.lnk"), &app);
        let record = directory.path().join("home").join("record");
        let mirror = menu.user.join("Vendor").join("App.lnk");
        assert_eq!(mirror_machine_shortcuts(&menu, &app, &record), 1);
        let roots = [(menu.user.clone(), START_MENU_DEPTH)];
        assert_eq!(point_shortcuts_at(&roots, &app, "black.ico"), 1);

        age(&mirror);

        assert_eq!(mirror_machine_shortcuts(&menu, &app, &record), 1);
        assert_eq!(icon_of(&mirror), (String::new(), 0));
        assert_eq!(recorded(&record), vec![mirror]);
    }

    #[test]
    fn a_recorded_path_outside_the_user_menu_is_never_deleted() {
        let directory = tempfile::tempdir().unwrap();
        let menu = menu_in(directory.path());
        let elsewhere = directory.path().join("elsewhere.lnk");
        std::fs::write(&elsewhere, b"kept").unwrap();
        let record = directory.path().join("record");
        std::fs::write(&record, elsewhere.to_string_lossy().as_bytes()).unwrap();

        assert_eq!(remove_mirrored_shortcuts(&menu, &record), 0);
        assert!(elsewhere.exists());
        assert!(!record.exists());
    }

    #[test]
    fn a_shortcut_deeper_than_the_scan_depth_is_left_alone() {
        let _apartment = Apartment::enter();
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.exe");
        std::fs::write(&app, b"app").unwrap();
        let deep = directory.path().join("pins").join("a");
        std::fs::create_dir_all(&deep).unwrap();
        let shortcut = deep.join("App.lnk");
        write_shortcut(&shortcut, &app);

        let roots = [(directory.path().join("pins"), 0)];
        assert_eq!(point_shortcuts_at(&roots, &app, "x.ico"), 0);
        assert_eq!(icon_of(&shortcut), (String::new(), 0));
    }
}
