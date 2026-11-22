pub(crate) const OK: i32 = 0;
pub(crate) const TRAP: i32 = 1;
pub(crate) const MEMORY_LIMIT: i32 = 2;
pub(crate) const POISONED: i32 = 4;
pub(crate) const ALLOCATION: i32 = 5;

pub(crate) const STAT_PENDING_LINES: u32 = 9;
pub(crate) const WIDE_RECORDS_FLAG: u32 = 4;
pub(crate) const TERMINATOR_MASK: u32 = 3;
pub(crate) const LONE_CR_FLAG: u32 = 16;
pub(crate) const PACKED_END_MASK: u32 = 0x00ff_ffff;

pub(crate) type ClassFn = unsafe extern "C" fn(bytes: *const u8, len: u32) -> u32;

#[repr(C)]
pub(crate) struct Raw {
    _opaque: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct RawLine {
    pub(crate) text: *const u8,
    pub(crate) byte_len: u32,
    pub(crate) utf16_len: u32,
    pub(crate) flags: u32,
    pub(crate) record_count: u32,
    pub(crate) records: *const u32,
}

impl RawLine {
    pub(crate) const fn empty() -> Self {
        Self {
            text: std::ptr::null(),
            byte_len: 0,
            utf16_len: 0,
            flags: 0,
            record_count: 0,
            records: std::ptr::null(),
        }
    }
}

#[repr(C)]
pub(crate) struct RawMemory {
    pub(crate) data: *const u8,
    pub(crate) size: u64,
}

unsafe extern "C" {
    pub(crate) fn paneflow_hl_new(
        id_start: ClassFn,
        id_continue: ClassFn,
        max_memory_bytes: u64,
        out: *mut *mut Raw,
    ) -> i32;
    pub(crate) fn paneflow_hl_free(hl: *mut Raw);
    pub(crate) fn paneflow_hl_init(hl: *mut Raw, text: *const u8, len: u32, language: u32) -> i32;
    pub(crate) fn paneflow_hl_edit(
        hl: *mut Raw,
        start_line: u32,
        start_column: u32,
        end_line: u32,
        end_column: u32,
        text: *const u8,
        len: u32,
    ) -> i32;
    pub(crate) fn paneflow_hl_run(
        hl: *mut Raw,
        line_budget: u32,
        end_line: u32,
        byte_budget: u32,
    ) -> i32;
    pub(crate) fn paneflow_hl_stat(hl: *mut Raw, index: u32, out: *mut u32) -> i32;
    pub(crate) fn paneflow_hl_line_count(hl: *mut Raw, out: *mut u32) -> i32;
    pub(crate) fn paneflow_hl_read_line(hl: *mut Raw, line: u32, out: *mut RawLine) -> i32;
    pub(crate) fn paneflow_hl_read_lines(
        hl: *mut Raw,
        first: u32,
        count: u32,
        out: *mut RawLine,
    ) -> i32;
    pub(crate) fn paneflow_hl_changes(
        hl: *mut Raw,
        records: *mut *const u32,
        count: *mut u32,
    ) -> i32;
    pub(crate) fn paneflow_hl_memory_view(hl: *const Raw, out: *mut RawMemory) -> i32;
    #[cfg(test)]
    pub(crate) fn paneflow_hl_probe_trap(hl: *mut Raw) -> i32;
}
