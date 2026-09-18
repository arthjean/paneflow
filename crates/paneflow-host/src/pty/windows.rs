use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

const VERSION: &str = env!("PANEFLOW_CONPTY_VERSION");
const TARGET: &str = env!("PANEFLOW_CONPTY_TARGET");
const DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/conpty.dll"));
const HOST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/OpenConsole.exe"));
const LICENSE: &[u8] = include_bytes!("../../../../native/conpty/LICENSE");

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

pub(super) fn initialize() -> Result<(), String> {
    INITIALIZED.get_or_init(load).clone()
}

fn load() -> Result<(), String> {
    let root = paneflow_home::cache_dir()
        .ok_or("cannot locate Paneflow's cache for ConPTY")?
        .join("conpty")
        .join(VERSION)
        .join(TARGET);
    install(&root).map_err(|error| format!("cannot install ConPTY: {error}"))?;
    let dll_path = root.join("conpty.dll");
    let wide: Vec<u16> = dll_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let module = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        return Err(format!(
            "cannot load {}: {}",
            dll_path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let basename: Vec<u16> = "conpty.dll\0".encode_utf16().collect();
    if unsafe { GetModuleHandleW(basename.as_ptr()) } != module {
        return Err("a different ConPTY runtime is already loaded".into());
    }
    for name in [
        c"CreatePseudoConsole",
        c"ResizePseudoConsole",
        c"ClosePseudoConsole",
    ] {
        if unsafe { GetProcAddress(module, name.as_ptr().cast()) }.is_none() {
            return Err(format!("ConPTY is missing {}", name.to_string_lossy()));
        }
    }
    log::info!("loaded ConPTY {VERSION} from {}", dll_path.display());
    Ok(())
}

fn install(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("install.lock"))?;
    lock.lock()?;
    for (name, bytes) in [
        ("conpty.dll", DLL),
        ("OpenConsole.exe", HOST),
        ("LICENSE", LICENSE),
    ] {
        install_file(root, name, bytes)?;
    }
    Ok(())
}

fn install_file(root: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = root.join(name);
    if File::open(&path).is_ok_and(|file| {
        file.metadata()
            .is_ok_and(|meta| meta.len() == bytes.len() as u64)
            && std::fs::read(&path).is_ok_and(|existing| existing == bytes)
    }) {
        return Ok(());
    }
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.persist(&path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_restores_incomplete_files_and_reuses_complete_files() {
        let root = tempfile::tempdir().unwrap();
        install(root.path()).unwrap();
        let dll = root.path().join("conpty.dll");
        let modified = dll.metadata().unwrap().modified().unwrap();
        install(root.path()).unwrap();
        assert_eq!(dll.metadata().unwrap().modified().unwrap(), modified);
        std::fs::write(&dll, b"incomplete").unwrap();
        install(root.path()).unwrap();
        assert_eq!(std::fs::read(dll).unwrap(), DLL);
        assert_eq!(
            std::fs::read(root.path().join("OpenConsole.exe")).unwrap(),
            HOST
        );
    }
}
