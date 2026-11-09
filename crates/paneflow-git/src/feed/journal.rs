use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    ExtendedFileIdType, FileIdInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
    OpenFileById, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_128, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0,
    FILE_ID_INFO, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, READ_USN_JOURNAL_DATA_V1,
    USN_REASON_BASIC_INFO_CHANGE, USN_REASON_CLOSE, USN_REASON_DATA_EXTEND,
    USN_REASON_DATA_OVERWRITE, USN_REASON_DATA_TRUNCATION, USN_REASON_FILE_CREATE,
    USN_REASON_FILE_DELETE, USN_REASON_HARD_LINK_CHANGE, USN_REASON_RENAME_NEW_NAME,
    USN_REASON_RENAME_OLD_NAME,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::{ChangeKind, Coverage, JournalError};

pub const RESOLVE_LIMIT: usize = 2048;

const READ_BUFFER: usize = 64 * 1024;

const READ_CALL_LIMIT: usize = 1 << 16;

const SKIPPED_GIT_DIRECTORIES: [&str; 2] = ["objects", "lfs"];

const ERROR_INVALID_FUNCTION: u32 = 1;

const ERROR_NOT_SUPPORTED: u32 = 50;

const ERROR_JOURNAL_DELETE_IN_PROGRESS: u32 = 1178;

const ERROR_JOURNAL_NOT_ACTIVE: u32 = 1179;

const ERROR_JOURNAL_ENTRY_DELETED: u32 = 1181;

const MODIFY_REASONS: u32 = USN_REASON_DATA_OVERWRITE
    | USN_REASON_DATA_EXTEND
    | USN_REASON_DATA_TRUNCATION
    | USN_REASON_BASIC_INFO_CHANGE
    | USN_REASON_CLOSE;

const NAME_LEAVING_REASONS: u32 =
    USN_REASON_RENAME_OLD_NAME | USN_REASON_FILE_DELETE | USN_REASON_HARD_LINK_CHANGE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalChange {
    pub path: PathBuf,
    pub kind: ChangeKind,
    pub directory: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct JournalState {
    pub id: u64,
    pub first: i64,
    pub next: i64,
    pub lowest_valid: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) file: u128,
    pub(crate) parent: u128,
    pub(crate) usn: i64,
    pub(crate) reason: u32,
    pub(crate) attributes: u32,
}

#[derive(Default)]
struct Touched {
    directory: bool,
    parents: BTreeSet<u128>,
    reasons: u32,
    open: bool,
}

pub struct Journal {
    handle: File,
    volume: u64,
    pub(crate) id: u64,
    pub(crate) cursor: i64,
    roots: Vec<PathBuf>,
    git_dirs: Vec<PathBuf>,
    directories: HashMap<u128, PathBuf>,
    held: HashMap<u128, u128>,
    pub(crate) buffer: Vec<u8>,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Journal")
            .field("cursor", &self.cursor)
            .field("directories", &self.directories.len())
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

impl Journal {
    pub fn arm(root: &Path) -> Result<Self, JournalError> {
        let handle = open_directory(root).map_err(io_error)?;
        let (volume, _) = file_id(&handle)?;
        let state = query(&handle)?;
        Ok(Self {
            handle,
            volume,
            id: state.id,
            cursor: state.next,
            roots: Vec::new(),
            git_dirs: Vec::new(),
            directories: HashMap::new(),
            held: HashMap::new(),
            buffer: vec![0; READ_BUFFER],
        })
    }

    pub fn index(&mut self, coverage: &Coverage) -> Result<(), JournalError> {
        self.roots = std::iter::once(coverage.worktree.clone())
            .chain(coverage.git_dirs.iter().cloned())
            .collect();
        self.git_dirs = coverage.git_dirs.iter().cloned().collect();
        self.directories.clear();
        self.held.clear();
        for directory in &coverage.directories {
            self.insert_directory(directory)?;
        }
        for git_dir in &coverage.git_dirs {
            self.insert_git_tree(git_dir)?;
        }
        self.seed()
    }

    pub fn cover(&mut self, coverage: &Coverage) -> Result<(), JournalError> {
        let known: BTreeSet<PathBuf> = self.directories.values().cloned().collect();
        for directory in coverage.directories.difference(&known) {
            self.insert_directory(directory)?;
        }
        Ok(())
    }

    pub(crate) fn state(&self) -> Result<JournalState, JournalError> {
        query(&self.handle)
    }

    pub fn read(&mut self, coverage: &Coverage) -> Result<Vec<JournalChange>, JournalError> {
        let state = self.state()?;
        if state.id != self.id {
            return Err(JournalError::Replaced);
        }
        if self.cursor < state.first.max(state.lowest_valid) {
            return Err(JournalError::Wrapped);
        }
        let (records, next) = self.read_range(self.cursor, state.next)?;
        self.cursor = next.max(state.next);
        self.apply(&records, coverage)
    }

    fn seed(&mut self) -> Result<(), JournalError> {
        let state = self.state()?;
        let start = state.first.max(state.lowest_valid);
        let (records, _) = self.read_range(start, self.cursor)?;
        let mut open: HashMap<u128, (u128, bool)> = HashMap::new();
        for record in records.iter().filter(|record| record.usn < self.cursor) {
            if record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                continue;
            }
            open.insert(
                record.file,
                (record.parent, record.reason & USN_REASON_CLOSE == 0),
            );
        }
        for (file, (parent, unclosed)) in open {
            let in_worktree = self
                .directories
                .get(&parent)
                .is_some_and(|path| !self.in_git_dir(path));
            if unclosed && in_worktree {
                self.held.insert(file, parent);
            }
        }
        Ok(())
    }

    fn insert_directory(&mut self, path: &Path) -> Result<(), JournalError> {
        let handle = match open_directory(path) {
            Ok(handle) => handle,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(error)),
        };
        let (volume, id) = file_id(&handle)?;
        if volume != self.volume {
            return Err(JournalError::Volumes);
        }
        self.directories.insert(id, path.to_path_buf());
        Ok(())
    }

    fn insert_git_tree(&mut self, git_dir: &Path) -> Result<(), JournalError> {
        let mut pending = vec![git_dir.to_path_buf()];
        while let Some(directory) = pending.pop() {
            self.insert_directory(&directory)?;
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
                let skipped = directory == git_dir
                    && SKIPPED_GIT_DIRECTORIES
                        .iter()
                        .any(|name| entry.file_name() == *name);
                if is_directory && !skipped {
                    pending.push(entry.path());
                }
            }
        }
        Ok(())
    }

    fn read_range(&mut self, start: i64, end: i64) -> Result<(Vec<Record>, i64), JournalError> {
        let mut records = Vec::new();
        let mut cursor = start;
        let mut calls = 0;
        while cursor < end && calls < READ_CALL_LIMIT {
            calls += 1;
            let input = READ_USN_JOURNAL_DATA_V1 {
                StartUsn: cursor,
                ReasonMask: u32::MAX,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                BytesToWaitFor: 0,
                UsnJournalID: self.id,
                MinMajorVersion: 2,
                MaxMajorVersion: 3,
            };
            let returned = ioctl(
                &self.handle,
                FSCTL_READ_UNPRIVILEGED_USN_JOURNAL,
                std::ptr::from_ref(&input).cast(),
                std::mem::size_of::<READ_USN_JOURNAL_DATA_V1>(),
                &mut self.buffer,
            )
            .map_err(read_error)?;
            let (next, mut batch) = parse_records(&self.buffer[..returned]);
            records.append(&mut batch);
            if next <= cursor {
                break;
            }
            cursor = next;
        }
        if cursor < end {
            return Err(JournalError::Overflow);
        }
        Ok((records, cursor))
    }

    fn apply(
        &mut self,
        records: &[Record],
        coverage: &Coverage,
    ) -> Result<Vec<JournalChange>, JournalError> {
        let mut touched: HashMap<u128, Touched> = HashMap::new();
        let mut order = Vec::new();
        for record in records {
            let entry = touched.entry(record.file).or_insert_with(|| {
                order.push(record.file);
                Touched::default()
            });
            entry.directory |= record.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            entry.parents.insert(record.parent);
            entry.reasons |= record.reason;
            entry.open = record.reason & USN_REASON_CLOSE == 0;
        }
        let mut changes = Vec::new();
        let mut budget = Budget(0);
        self.apply_directories(&order, &touched, coverage, &mut changes, &mut budget)?;
        for file in &order {
            let Some(entry) = touched.get(file).filter(|entry| !entry.directory) else {
                continue;
            };
            self.apply_file(*file, entry, &mut changes, &mut budget)?;
        }
        let held: Vec<(u128, u128)> = self
            .held
            .iter()
            .filter(|(file, _)| !touched.contains_key(file))
            .map(|(file, parent)| (*file, *parent))
            .collect();
        for (file, parent) in held {
            budget.spend()?;
            match self.resolve_inside(file) {
                Some(path) => changes.push(JournalChange {
                    path,
                    kind: ChangeKind::Modify,
                    directory: Some(false),
                }),
                None => {
                    self.held.remove(&file);
                    if let Some(directory) = self.directories.get(&parent) {
                        changes.push(children(directory.clone()));
                    }
                }
            }
        }
        Ok(changes)
    }

    fn apply_directories(
        &mut self,
        order: &[u128],
        touched: &HashMap<u128, Touched>,
        coverage: &Coverage,
        changes: &mut Vec<JournalChange>,
        budget: &mut Budget,
    ) -> Result<(), JournalError> {
        let mut pending: Vec<u128> = order
            .iter()
            .copied()
            .filter(|file| touched.get(file).is_some_and(|entry| entry.directory))
            .collect();
        let mut arriving = BTreeSet::new();
        let mut resolved = Vec::new();
        loop {
            let before = pending.len();
            let mut deferred = Vec::new();
            for directory in pending {
                let Some(entry) = touched.get(&directory) else {
                    continue;
                };
                let relevant = std::iter::once(&directory)
                    .chain(entry.parents.iter())
                    .any(|id| self.directories.contains_key(id) || arriving.contains(id));
                if !relevant {
                    deferred.push(directory);
                    continue;
                }
                budget.spend()?;
                let old = self.directories.get(&directory).cloned();
                let current = self
                    .resolve_inside(directory)
                    .filter(|path| !self.ignored(path, coverage));
                if old.is_none() && current.is_none() {
                    for parent in &entry.parents {
                        if let Some(path) = self.directories.get(parent) {
                            changes.push(children(path.clone()));
                        }
                    }
                    continue;
                }
                if current.is_some() {
                    arriving.insert(directory);
                }
                resolved.push((directory, old, current));
            }
            if deferred.len() == before {
                break;
            }
            pending = deferred;
        }
        self.move_directories(&resolved);
        for (directory, old, current) in resolved {
            if old == current {
                continue;
            }
            if let Some(old) = &old {
                changes.push(JournalChange {
                    path: old.clone(),
                    kind: ChangeKind::Rename,
                    directory: Some(true),
                });
            }
            let Some(current) = current else {
                continue;
            };
            if old.is_none() {
                self.directories.insert(directory, current.clone());
                self.insert_subtree(&current, coverage, budget)?;
            }
            changes.push(JournalChange {
                path: current,
                kind: if old.is_some() {
                    ChangeKind::Rename
                } else {
                    ChangeKind::Create
                },
                directory: Some(true),
            });
        }
        Ok(())
    }

    fn move_directories(&mut self, resolved: &[(u128, Option<PathBuf>, Option<PathBuf>)]) {
        let moves: BTreeMap<PathBuf, Option<PathBuf>> = resolved
            .iter()
            .filter_map(|(_, old, current)| {
                old.as_ref()
                    .filter(|old| Some(*old) != current.as_ref())
                    .map(|old| (old.clone(), current.clone()))
            })
            .collect();
        if moves.is_empty() {
            return;
        }
        let own: HashMap<u128, Option<PathBuf>> = resolved
            .iter()
            .filter(|(_, old, _)| old.is_some())
            .map(|(id, _, current)| (*id, current.clone()))
            .collect();
        let previous = std::mem::take(&mut self.directories);
        for (id, path) in previous {
            let moved = match own.get(&id) {
                Some(current) => current.clone(),
                None => rebase(&path, &moves),
            };
            if let Some(path) = moved {
                self.directories.insert(id, path);
            }
        }
    }

    fn apply_file(
        &mut self,
        file: u128,
        entry: &Touched,
        changes: &mut Vec<JournalChange>,
        budget: &mut Budget,
    ) -> Result<(), JournalError> {
        let parents: Vec<(u128, PathBuf)> = entry
            .parents
            .iter()
            .filter_map(|parent| {
                self.directories
                    .get(parent)
                    .map(|path| (*parent, path.clone()))
            })
            .collect();
        if parents.is_empty() && !self.held.contains_key(&file) {
            return Ok(());
        }
        if !parents.is_empty() && parents.iter().all(|(_, path)| self.in_git_dir(path)) {
            self.held.remove(&file);
            for (_, parent) in parents {
                changes.push(children(parent));
            }
            return Ok(());
        }
        budget.spend()?;
        let current = self.resolve_inside(file);
        if let Some(path) = &current {
            changes.push(JournalChange {
                path: path.clone(),
                kind: kind_of(entry.reasons),
                directory: Some(false),
            });
        }
        let left_a_name = entry.reasons & NAME_LEAVING_REASONS != 0;
        for (_, parent) in &parents {
            if left_a_name || current.as_deref().and_then(Path::parent) != Some(parent.as_path()) {
                changes.push(children(parent.clone()));
            }
        }
        let held_parent = parents
            .first()
            .map(|(parent, _)| *parent)
            .or_else(|| self.held.get(&file).copied());
        match (entry.open, current.is_some(), held_parent) {
            (true, true, Some(parent)) => {
                self.held.insert(file, parent);
            }
            _ => {
                self.held.remove(&file);
            }
        }
        Ok(())
    }

    fn insert_subtree(
        &mut self,
        root: &Path,
        coverage: &Coverage,
        budget: &mut Budget,
    ) -> Result<(), JournalError> {
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let path = entry.path();
                if self.ignored(&path, coverage) {
                    continue;
                }
                budget.spend()?;
                self.insert_directory(&path)?;
                pending.push(path);
            }
        }
        Ok(())
    }

    fn ignored(&self, path: &Path, coverage: &Coverage) -> bool {
        path.strip_prefix(&coverage.worktree).is_ok_and(|relative| {
            coverage.ignored.contains(relative) || coverage.is_ignored(relative)
        })
    }

    fn in_git_dir(&self, path: &Path) -> bool {
        self.git_dirs
            .iter()
            .any(|git_dir| path.starts_with(git_dir))
    }

    fn resolve_inside(&self, file: u128) -> Option<PathBuf> {
        resolve(&self.handle, file)
            .filter(|path| self.roots.iter().any(|root| path.starts_with(root)))
    }
}

struct Budget(usize);

impl Budget {
    fn spend(&mut self) -> Result<(), JournalError> {
        self.0 += 1;
        if self.0 > RESOLVE_LIMIT {
            Err(JournalError::Overflow)
        } else {
            Ok(())
        }
    }
}

fn rebase(path: &Path, moves: &BTreeMap<PathBuf, Option<PathBuf>>) -> Option<PathBuf> {
    for ancestor in path.ancestors() {
        if let Some(target) = moves.get(ancestor) {
            let target = target.as_ref()?;
            let below = path.strip_prefix(ancestor).ok()?;
            return Some(if below.as_os_str().is_empty() {
                target.clone()
            } else {
                target.join(below)
            });
        }
    }
    Some(path.to_path_buf())
}

fn children(directory: PathBuf) -> JournalChange {
    JournalChange {
        path: directory,
        kind: ChangeKind::Children,
        directory: Some(true),
    }
}

pub(crate) fn kind_of(reasons: u32) -> ChangeKind {
    if reasons & USN_REASON_FILE_CREATE != 0 {
        ChangeKind::Create
    } else if reasons & (USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME) != 0 {
        ChangeKind::Rename
    } else if reasons & USN_REASON_FILE_DELETE != 0 {
        ChangeKind::Remove
    } else if reasons & !MODIFY_REASONS == 0 {
        ChangeKind::Modify
    } else {
        ChangeKind::Any
    }
}

pub(crate) fn query_error(code: u32) -> JournalError {
    match code {
        ERROR_JOURNAL_NOT_ACTIVE | ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED => {
            JournalError::Missing
        }
        ERROR_JOURNAL_DELETE_IN_PROGRESS => JournalError::Replaced,
        code => JournalError::Read(code),
    }
}

pub(crate) fn read_error(code: u32) -> JournalError {
    match code {
        ERROR_JOURNAL_ENTRY_DELETED => JournalError::Wrapped,
        ERROR_JOURNAL_NOT_ACTIVE | ERROR_JOURNAL_DELETE_IN_PROGRESS => JournalError::Replaced,
        code => JournalError::Read(code),
    }
}

fn io_error(error: std::io::Error) -> JournalError {
    JournalError::Read(
        error
            .raw_os_error()
            .and_then(|code| u32::try_from(code).ok())
            .unwrap_or(0),
    )
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

fn raw(file: &File) -> HANDLE {
    file.as_raw_handle()
}

fn file_id(file: &File) -> Result<(u64, u128), JournalError> {
    let mut info = FILE_ID_INFO::default();
    let size = u32::try_from(std::mem::size_of::<FILE_ID_INFO>()).unwrap_or(0);
    let ok = unsafe {
        GetFileInformationByHandleEx(
            raw(file),
            FileIdInfo,
            std::ptr::from_mut(&mut info).cast(),
            size,
        )
    };
    if ok == 0 {
        return Err(JournalError::Read(unsafe { GetLastError() }));
    }
    Ok((
        info.VolumeSerialNumber,
        u128::from_le_bytes(info.FileId.Identifier),
    ))
}

fn ioctl(
    file: &File,
    code: u32,
    input: *const std::ffi::c_void,
    input_len: usize,
    output: &mut [u8],
) -> Result<usize, u32> {
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            raw(file),
            code,
            input,
            u32::try_from(input_len).unwrap_or(0),
            output.as_mut_ptr().cast(),
            u32::try_from(output.len()).unwrap_or(u32::MAX),
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(unsafe { GetLastError() });
    }
    Ok(usize::try_from(returned).unwrap_or(0).min(output.len()))
}

fn query(file: &File) -> Result<JournalState, JournalError> {
    let mut output = [0u8; 128];
    let returned = ioctl(
        file,
        FSCTL_QUERY_USN_JOURNAL,
        std::ptr::null(),
        0,
        &mut output,
    )
    .map_err(query_error)?;
    if returned < 32 {
        return Err(JournalError::Read(0));
    }
    Ok(JournalState {
        id: read_u64(&output, 0),
        first: read_i64(&output, 8),
        next: read_i64(&output, 16),
        lowest_valid: read_i64(&output, 24),
    })
}

fn resolve(hint: &File, file: u128) -> Option<PathBuf> {
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: u32::try_from(std::mem::size_of::<FILE_ID_DESCRIPTOR>()).unwrap_or(0),
        Type: ExtendedFileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            ExtendedFileId: FILE_ID_128 {
                Identifier: file.to_le_bytes(),
            },
        },
    };
    let handle = unsafe {
        OpenFileById(
            raw(hint),
            &descriptor,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return None;
    }
    let owned = unsafe { OwnedHandle::from_raw_handle(handle) };
    final_path(&owned)
}

fn final_path(handle: &OwnedHandle) -> Option<PathBuf> {
    let mut wide = vec![0u16; 512];
    loop {
        let capacity = u32::try_from(wide.len()).unwrap_or(u32::MAX);
        let written = unsafe {
            GetFinalPathNameByHandleW(
                handle.as_raw_handle(),
                wide.as_mut_ptr(),
                capacity,
                FILE_NAME_NORMALIZED,
            )
        };
        let written = usize::try_from(written).unwrap_or(0);
        if written == 0 {
            return None;
        }
        if written < wide.len() {
            wide.truncate(written);
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&wide)));
        }
        wide.resize(written + 1, 0);
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    bytes
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, u16::from_le_bytes)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, u64::from_le_bytes)
}

fn read_i64(bytes: &[u8], offset: usize) -> i64 {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, i64::from_le_bytes)
}

fn read_u128(bytes: &[u8], offset: usize) -> u128 {
    bytes
        .get(offset..offset + 16)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, u128::from_le_bytes)
}

pub(crate) fn parse_records(buffer: &[u8]) -> (i64, Vec<Record>) {
    let next = read_i64(buffer, 0);
    let mut records = Vec::new();
    let mut offset = 8;
    while offset + 8 <= buffer.len() {
        let length = usize::try_from(read_u32(buffer, offset)).unwrap_or(0);
        if length < 8 || offset + length > buffer.len() {
            break;
        }
        let record = &buffer[offset..offset + length];
        match read_u16(record, 4) {
            2 if length >= 60 => records.push(Record {
                file: u128::from(read_u64(record, 8)),
                parent: u128::from(read_u64(record, 16)),
                usn: read_i64(record, 24),
                reason: read_u32(record, 40),
                attributes: read_u32(record, 52),
            }),
            3 if length >= 76 => records.push(Record {
                file: read_u128(record, 8),
                parent: read_u128(record, 24),
                usn: read_i64(record, 40),
                reason: read_u32(record, 56),
                attributes: read_u32(record, 68),
            }),
            _ => {}
        }
        offset += length;
    }
    (next, records)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    struct Volume {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        coverage: Coverage,
    }

    fn volume(directories: &[&str]) -> Volume {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonical root");
        let mut coverage = Coverage {
            worktree: root.clone(),
            ..Coverage::default()
        };
        coverage.directories.insert(root.clone());
        for directory in directories {
            let path = root.join(directory);
            std::fs::create_dir_all(&path).expect("directory");
            coverage.directories.insert(path);
        }
        Volume {
            _tmp: tmp,
            root,
            coverage,
        }
    }

    fn armed(volume: &Volume) -> Journal {
        let mut journal = Journal::arm(&volume.root).expect("the temporary volume keeps a journal");
        journal.index(&volume.coverage).expect("index");
        journal
    }

    fn has(changes: &[JournalChange], path: &Path, kind: ChangeKind) -> bool {
        changes
            .iter()
            .any(|change| change.path == path && change.kind == kind)
    }

    #[test]
    fn a_write_on_a_handle_that_stays_open_is_reported_until_its_close_record() {
        let volume = volume(&["src"]);
        let path = volume.root.join("src").join("held.txt");
        std::fs::write(&path, b"seed").expect("seed");
        let mut journal = armed(&volume);
        let mut handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open");
        handle.write_all(b"first").expect("first write");
        let first = journal.read(&volume.coverage).expect("read");
        assert!(has(&first, &path, ChangeKind::Modify), "{first:?}");
        handle.write_all(b"second").expect("second write");
        let second = journal.read(&volume.coverage).expect("read");
        assert!(
            has(&second, &path, ChangeKind::Modify),
            "a second write on the same handle adds no journal record, the file stays held: {second:?}"
        );
        drop(handle);
        let closed = journal.read(&volume.coverage).expect("read");
        assert!(has(&closed, &path, ChangeKind::Modify), "{closed:?}");
        let quiet = journal.read(&volume.coverage).expect("read");
        assert!(
            quiet.iter().all(|change| change.path != path),
            "a closed file is released: {quiet:?}"
        );
    }

    #[test]
    fn a_deleted_file_names_its_directory_as_a_changed_parent() {
        let volume = volume(&["src"]);
        let path = volume.root.join("src").join("doomed.txt");
        std::fs::write(&path, b"doomed").expect("seed");
        let mut journal = armed(&volume);
        std::fs::remove_file(&path).expect("delete");
        let changes = journal.read(&volume.coverage).expect("read");
        assert!(
            has(&changes, &volume.root.join("src"), ChangeKind::Children),
            "{changes:?}"
        );
    }

    #[test]
    fn a_file_renamed_inside_its_directory_names_the_new_path_and_the_directory() {
        let volume = volume(&["src"]);
        let old = volume.root.join("src").join("old.txt");
        let new = volume.root.join("src").join("new.txt");
        std::fs::write(&old, b"moved").expect("seed");
        let mut journal = armed(&volume);
        std::fs::rename(&old, &new).expect("rename");
        let changes = journal.read(&volume.coverage).expect("read");
        assert!(has(&changes, &new, ChangeKind::Rename), "{changes:?}");
        assert!(
            has(&changes, &volume.root.join("src"), ChangeKind::Children),
            "the old name is gone from the journal, its directory is re-listed: {changes:?}"
        );
    }

    #[test]
    fn a_file_replaced_by_a_symlink_resolves_to_the_link_and_not_its_target() {
        let volume = volume(&["src"]);
        let target = volume.root.join("src").join("target.txt");
        let link = volume.root.join("src").join("link.txt");
        std::fs::write(&target, b"target").expect("target");
        std::fs::write(&link, b"plain").expect("plain file");
        let mut journal = armed(&volume);
        std::fs::remove_file(&link).expect("delete");
        if std::os::windows::fs::symlink_file("target.txt", &link).is_err() {
            return;
        }
        let changes = journal.read(&volume.coverage).expect("read");
        assert!(has(&changes, &link, ChangeKind::Create), "{changes:?}");
        assert!(
            changes.iter().all(|change| change.path != target),
            "{changes:?}"
        );
    }

    #[test]
    fn a_renamed_directory_carries_its_descendants_in_the_directory_map() {
        let volume = volume(&["a", "a/b"]);
        let mut journal = armed(&volume);
        std::fs::rename(volume.root.join("a"), volume.root.join("c")).expect("rename");
        let renamed = journal.read(&volume.coverage).expect("read");
        assert!(
            has(&renamed, &volume.root.join("a"), ChangeKind::Rename),
            "{renamed:?}"
        );
        assert!(
            has(&renamed, &volume.root.join("c"), ChangeKind::Rename),
            "{renamed:?}"
        );
        let created = volume.root.join("c").join("b").join("new.txt");
        std::fs::write(&created, b"new").expect("create");
        let changes = journal.read(&volume.coverage).expect("read");
        assert!(has(&changes, &created, ChangeKind::Create), "{changes:?}");
    }

    #[test]
    fn a_directory_created_after_the_index_joins_the_map_with_its_files() {
        let volume = volume(&[]);
        let mut journal = armed(&volume);
        let file = volume.root.join("new").join("inner").join("f.txt");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("directories");
        std::fs::write(&file, b"f").expect("file");
        let changes = journal.read(&volume.coverage).expect("read");
        assert!(
            has(&changes, &volume.root.join("new"), ChangeKind::Create),
            "{changes:?}"
        );
        assert!(has(&changes, &file, ChangeKind::Create), "{changes:?}");
    }

    #[test]
    fn a_directory_swapped_into_place_in_one_drain_keeps_its_own_path() {
        let volume = volume(&["a"]);
        std::fs::write(volume.root.join("a").join("old.txt"), b"old").expect("seed");
        let mut journal = armed(&volume);
        let staged = volume.root.join("staged");
        std::fs::create_dir(&staged).expect("staged directory");
        std::fs::write(staged.join("new.txt"), b"new").expect("staged file");
        std::fs::rename(volume.root.join("a"), volume.root.join("previous")).expect("move away");
        std::fs::rename(&staged, volume.root.join("a")).expect("swap in");
        journal.read(&volume.coverage).expect("read the swap");
        std::fs::remove_file(volume.root.join("a").join("new.txt")).expect("delete");
        std::fs::remove_file(volume.root.join("previous").join("old.txt")).expect("delete");
        let changes = journal.read(&volume.coverage).expect("read the deletes");
        assert!(
            has(&changes, &volume.root.join("a"), ChangeKind::Children),
            "the swapped-in directory is re-listed under its own path: {changes:?}"
        );
        assert!(
            has(
                &changes,
                &volume.root.join("previous"),
                ChangeKind::Children
            ),
            "the moved-away directory is re-listed under its new path: {changes:?}"
        );
    }

    #[test]
    fn a_nested_directory_moved_out_of_a_renamed_parent_keeps_its_own_path() {
        let volume = volume(&["a", "a/sub", "a/other"]);
        let mut journal = armed(&volume);
        std::fs::write(volume.root.join("a").join("other").join("f.txt"), b"f").expect("seed");
        std::fs::rename(volume.root.join("a"), volume.root.join("b")).expect("rename parent");
        std::fs::rename(volume.root.join("b").join("sub"), volume.root.join("out"))
            .expect("move the child out");
        journal.read(&volume.coverage).expect("read the moves");
        std::fs::write(volume.root.join("out").join("g.txt"), b"g").expect("write");
        std::fs::remove_file(volume.root.join("out").join("g.txt")).expect("delete");
        std::fs::remove_file(volume.root.join("b").join("other").join("f.txt")).expect("delete");
        let changes = journal.read(&volume.coverage).expect("read the deletes");
        assert!(
            has(&changes, &volume.root.join("out"), ChangeKind::Children),
            "{changes:?}"
        );
        assert!(
            has(
                &changes,
                &volume.root.join("b").join("other"),
                ChangeKind::Children
            ),
            "{changes:?}"
        );
    }

    #[test]
    fn a_write_in_a_git_dir_names_its_directory_without_opening_the_file() {
        let mut volume = volume(&[]);
        let git_dir = volume.root.join(".git");
        std::fs::create_dir(&git_dir).expect("git dir");
        std::fs::write(git_dir.join("index"), b"index").expect("index");
        volume.coverage.git_dirs.insert(git_dir.clone());
        let mut journal = armed(&volume);
        let lock = git_dir.join("index.lock");
        std::fs::write(&lock, b"next index").expect("lock");
        std::fs::rename(&lock, git_dir.join("index")).expect("replace the index");
        let changes = journal.read(&volume.coverage).expect("read");
        assert!(has(&changes, &git_dir, ChangeKind::Children), "{changes:?}");
        assert!(
            changes
                .iter()
                .all(|change| change.path == git_dir || !change.path.starts_with(&git_dir)),
            "no file under the git dir is resolved, so no handle races git's rename: {changes:?}"
        );
    }

    #[test]
    fn changes_outside_the_indexed_directories_are_not_reported() {
        let volume = volume(&["src"]);
        let outside = tempfile::tempdir().expect("outside");
        let mut journal = armed(&volume);
        std::fs::write(outside.path().join("elsewhere.txt"), b"x").expect("write");
        let changes = journal.read(&volume.coverage).expect("read");
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn a_position_below_the_first_record_reads_as_wrapped_and_another_id_as_replaced() {
        let volume = volume(&[]);
        let mut journal = armed(&volume);
        let state = journal.state().expect("state");
        let cursor = journal.cursor;
        journal.cursor = state.first.max(state.lowest_valid) - 1;
        assert_eq!(journal.read(&volume.coverage), Err(JournalError::Wrapped));
        journal.cursor = cursor;
        journal.id ^= 1;
        assert_eq!(journal.read(&volume.coverage), Err(JournalError::Replaced));
    }

    #[test]
    fn a_failed_journal_read_is_a_read_error() {
        let volume = volume(&[]);
        let mut journal = armed(&volume);
        std::fs::write(volume.root.join("pending.txt"), b"x").expect("write");
        journal.buffer.clear();
        assert!(
            matches!(journal.read(&volume.coverage), Err(JournalError::Read(_))),
            "a read into no buffer fails"
        );
    }

    fn record_v3(file: u128, parent: u128, usn: i64, reason: u32, attributes: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 80];
        bytes[0..4].copy_from_slice(&80u32.to_le_bytes());
        bytes[4..6].copy_from_slice(&3u16.to_le_bytes());
        bytes[8..24].copy_from_slice(&file.to_le_bytes());
        bytes[24..40].copy_from_slice(&parent.to_le_bytes());
        bytes[40..48].copy_from_slice(&usn.to_le_bytes());
        bytes[56..60].copy_from_slice(&reason.to_le_bytes());
        bytes[68..72].copy_from_slice(&attributes.to_le_bytes());
        bytes
    }

    fn record_v2(file: u64, parent: u64, usn: i64, reason: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[0..4].copy_from_slice(&64u32.to_le_bytes());
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        bytes[8..16].copy_from_slice(&file.to_le_bytes());
        bytes[16..24].copy_from_slice(&parent.to_le_bytes());
        bytes[24..32].copy_from_slice(&usn.to_le_bytes());
        bytes[40..44].copy_from_slice(&reason.to_le_bytes());
        bytes
    }

    #[test]
    fn version_two_and_three_records_parse_into_file_and_parent_ids() {
        let mut buffer = 4096i64.to_le_bytes().to_vec();
        buffer.extend(record_v3(7, 5, 100, USN_REASON_FILE_CREATE, 0x10));
        buffer.extend(record_v2(9, 5, 180, USN_REASON_DATA_EXTEND));
        buffer.extend([1, 2, 3]);
        let (next, records) = parse_records(&buffer);
        assert_eq!(next, 4096);
        assert_eq!(
            records,
            vec![
                Record {
                    file: 7,
                    parent: 5,
                    usn: 100,
                    reason: USN_REASON_FILE_CREATE,
                    attributes: 0x10,
                },
                Record {
                    file: 9,
                    parent: 5,
                    usn: 180,
                    reason: USN_REASON_DATA_EXTEND,
                    attributes: 0,
                },
            ]
        );
    }

    #[test]
    fn a_truncated_record_ends_the_parse_instead_of_reading_past_the_buffer() {
        let mut buffer = 0i64.to_le_bytes().to_vec();
        let mut record = record_v3(1, 2, 3, 4, 0);
        record.truncate(40);
        buffer.extend(record);
        assert_eq!(parse_records(&buffer).1, Vec::new());
    }

    #[test]
    fn reasons_map_to_the_change_kind_the_planner_expects() {
        assert_eq!(
            kind_of(USN_REASON_DATA_OVERWRITE | USN_REASON_CLOSE),
            ChangeKind::Modify
        );
        assert_eq!(kind_of(USN_REASON_BASIC_INFO_CHANGE), ChangeKind::Modify);
        assert_eq!(
            kind_of(USN_REASON_FILE_CREATE | USN_REASON_DATA_EXTEND),
            ChangeKind::Create
        );
        assert_eq!(kind_of(USN_REASON_RENAME_NEW_NAME), ChangeKind::Rename);
        assert_eq!(
            kind_of(USN_REASON_FILE_DELETE | USN_REASON_CLOSE),
            ChangeKind::Remove
        );
        assert_eq!(kind_of(0x0000_0800), ChangeKind::Any);
    }

    #[test]
    fn journal_error_codes_map_to_their_reasons() {
        assert_eq!(query_error(ERROR_JOURNAL_NOT_ACTIVE), JournalError::Missing);
        assert_eq!(query_error(ERROR_INVALID_FUNCTION), JournalError::Missing);
        assert_eq!(query_error(ERROR_NOT_SUPPORTED), JournalError::Missing);
        assert_eq!(
            query_error(ERROR_JOURNAL_DELETE_IN_PROGRESS),
            JournalError::Replaced
        );
        assert_eq!(query_error(5), JournalError::Read(5));
        assert_eq!(
            read_error(ERROR_JOURNAL_ENTRY_DELETED),
            JournalError::Wrapped
        );
        assert_eq!(read_error(ERROR_JOURNAL_NOT_ACTIVE), JournalError::Replaced);
        assert_eq!(read_error(87), JournalError::Read(87));
    }
}
