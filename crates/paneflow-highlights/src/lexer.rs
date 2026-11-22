use std::ptr::NonNull;

use crate::document::LexDocument;
use crate::ffi::{self, Raw, RawMemory};
use crate::language::Language;
use crate::{status, LexError, MAX_MEMORY_BYTES};

#[cfg(test)]
thread_local! {
    pub(crate) static LIVE_INSTANCES: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    pub(crate) static HOST_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub struct Lexer {
    raw: NonNull<Raw>,
    memory_limit: usize,
}

unsafe impl Send for Lexer {}

fn code_point(bytes: *const u8, len: u32) -> Option<char> {
    #[cfg(test)]
    HOST_CALLS.with(|calls| calls.set(calls.get() + 1));
    if bytes.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(bytes, usize::try_from(len).ok()?) };
    std::str::from_utf8(bytes).ok()?.chars().next()
}

pub(crate) fn is_id_start(ch: char) -> bool {
    unicode_ident::is_xid_start(ch)
}

pub(crate) fn is_id_continue(ch: char) -> bool {
    unicode_ident::is_xid_continue(ch) || matches!(ch, '\u{200C}' | '\u{200D}')
}

pub(crate) unsafe extern "C" fn host_is_id_start(bytes: *const u8, len: u32) -> u32 {
    u32::from(code_point(bytes, len).is_some_and(is_id_start))
}

pub(crate) unsafe extern "C" fn host_is_id_continue(bytes: *const u8, len: u32) -> u32 {
    u32::from(code_point(bytes, len).is_some_and(is_id_continue))
}

impl Lexer {
    pub fn new() -> Result<Self, LexError> {
        Self::with_memory_limit(MAX_MEMORY_BYTES)
    }

    pub(crate) fn with_memory_limit(memory_limit: usize) -> Result<Self, LexError> {
        let mut raw = std::ptr::null_mut();
        let code = unsafe {
            ffi::paneflow_hl_new(
                host_is_id_start,
                host_is_id_continue,
                u64::try_from(memory_limit).unwrap_or(u64::MAX),
                &mut raw,
            )
        };
        status(code, 0, memory_limit)?;
        let raw = NonNull::new(raw).ok_or(LexError::Allocation)?;
        #[cfg(test)]
        LIVE_INSTANCES.with(|live| live.set(live.get() + 1));
        Ok(Self { raw, memory_limit })
    }

    pub fn document(self, text: &str, language: Language) -> Result<LexDocument, LexError> {
        LexDocument::new(self, text, language)
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory().map_or(0, |(_, size)| size)
    }

    pub(crate) fn memory_limit(&self) -> usize {
        self.memory_limit
    }

    pub(crate) fn raw(&self) -> *mut Raw {
        self.raw.as_ptr()
    }

    pub(crate) fn memory(&self) -> Option<(*const u8, usize)> {
        let mut view = RawMemory {
            data: std::ptr::null(),
            size: 0,
        };
        unsafe { ffi::paneflow_hl_memory_view(self.raw.as_ptr(), &mut view) };
        let size = usize::try_from(view.size).ok()?;
        (!view.data.is_null()).then_some((view.data, size))
    }
}

impl Drop for Lexer {
    fn drop(&mut self) {
        unsafe { ffi::paneflow_hl_free(self.raw.as_ptr()) };
        #[cfg(test)]
        LIVE_INSTANCES.with(|live| live.set(live.get() - 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::TOKEN_TYPES;

    fn static_memory(lexer: &Lexer) -> &[u8] {
        let (data, size) = lexer.memory().expect("an instantiated memory");
        unsafe { std::slice::from_raw_parts(data, size) }
    }

    fn u16_at(memory: &[u8], at: usize) -> usize {
        usize::from(u16::from_le_bytes([memory[at], memory[at + 1]]))
    }

    fn wasm_token_names(memory: &[u8]) -> Option<Vec<String>> {
        let scan = memory.len().min(64 * 1024);
        (0..scan.saturating_sub(3)).find_map(|table| {
            let blob = u16_at(memory, table);
            if memory[table + 2] != 0 || blob <= table || !(blob - table).is_multiple_of(3) {
                return None;
            }
            let records = (table..blob).step_by(3);
            let names: Option<Vec<String>> = records
                .map(|record| {
                    let start = u16_at(memory, record);
                    let end = start + usize::from(memory[record + 2]);
                    (start >= blob && end <= scan)
                        .then(|| std::str::from_utf8(&memory[start..end]).ok())
                        .flatten()
                        .map(str::to_owned)
                })
                .collect();
            names.filter(|names| names.len() > 1 && names.iter().skip(1).all(|n| !n.is_empty()))
        })
    }

    #[test]
    fn a_lexer_is_one_heap_instance_freed_on_drop() {
        let before = LIVE_INSTANCES.with(std::cell::Cell::get);
        let first = Lexer::new().expect("a lexer");
        let second = Lexer::new().expect("a lexer");
        assert_ne!(first.raw(), second.raw());
        assert_eq!(LIVE_INSTANCES.with(std::cell::Cell::get), before + 2);
        assert!(first.memory_bytes() > 0);
        drop(first);
        assert_eq!(LIVE_INSTANCES.with(std::cell::Cell::get), before + 1);
        drop(second);
        assert_eq!(LIVE_INSTANCES.with(std::cell::Cell::get), before);
    }

    #[test]
    fn the_host_imports_answer_xid_classes_plus_zwnj_and_zwj_for_continuation() {
        assert!(is_id_start('a') && is_id_start('é') && is_id_start('字'));
        assert!(!is_id_start('1') && !is_id_start('-') && !is_id_start('\u{200D}'));
        assert!(is_id_continue('1') && is_id_continue('_'));
        assert!(is_id_continue('\u{200D}') && is_id_continue('\u{200C}'));
        let call = |f: unsafe extern "C" fn(*const u8, u32) -> u32, text: &str| unsafe {
            f(text.as_ptr(), u32::try_from(text.len()).unwrap_or(0))
        };
        assert_eq!(call(host_is_id_start, "é"), 1);
        assert_eq!(call(host_is_id_start, "1"), 0);
        assert_eq!(call(host_is_id_continue, "\u{200C}"), 1);
        assert_eq!(unsafe { host_is_id_start([0xffu8].as_ptr(), 1) }, 0);
        assert_eq!(unsafe { host_is_id_start(std::ptr::null(), 2) }, 0);
    }

    #[test]
    fn the_token_table_has_the_count_and_names_the_wasm_reports() {
        let lexer = Lexer::new().expect("a lexer");
        let names = wasm_token_names(static_memory(&lexer))
            .expect("the lexer's static data holds its token name table");
        assert_eq!(
            TOKEN_TYPES.len(),
            names.len(),
            "tables.rs holds {} token types, the WASM reports {}",
            TOKEN_TYPES.len(),
            names.len()
        );
        for (index, (ours, wasm)) in TOKEN_TYPES.iter().zip(&names).skip(1).enumerate() {
            assert_eq!(
                ours.replace(['.', '_'], "-"),
                *wasm,
                "token {} is {ours} in tables.rs and {wasm} in the WASM",
                index + 1
            );
        }
    }
}
