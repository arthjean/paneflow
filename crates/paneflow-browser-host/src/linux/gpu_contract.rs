use std::ffi::CStr;
use std::mem::{offset_of, size_of};

#[repr(C)]
#[derive(Default, Debug)]
struct Snapshot {
    abi_version: u32,
    struct_size: u32,
    status: u32,
    flags: u32,
    gpu_host_id: u64,
    gpu_process_id: u64,
    gpu_info_revision: u64,
    vendor_id: u32,
    device_id: u32,
    skia_backend: u32,
    gl_implementation: u32,
    angle_implementation: u32,
    reserved: u32,
}

const _: () = assert!(size_of::<Snapshot>() == 64 && offset_of!(Snapshot, gpu_host_id) == 16);

type Observe = unsafe extern "C" fn(u32, *mut Snapshot) -> i32;

unsafe extern "C" {
    fn cef_version_info(entry: libc::c_int) -> libc::c_int;
}

pub(super) struct GpuContract {
    library: *mut libc::c_void,
    observe: Observe,
    epoch: Option<(u64, u64)>,
    revision: u64,
    gpu: (u32, u32),
}

impl GpuContract {
    pub(super) fn load(gpu: (u32, u32)) -> Result<Self, String> {
        let mut cef_location = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
        if unsafe {
            libc::dladdr(
                cef_version_info as *const () as *const _,
                cef_location.as_mut_ptr(),
            )
        } == 0
        {
            return Err("cannot identify the linked CEF library".into());
        }
        let cef_location = unsafe { cef_location.assume_init() };
        if cef_location.dli_fname.is_null() {
            return Err("the linked CEF library has no path".into());
        }
        let library =
            unsafe { libc::dlopen(cef_location.dli_fname, libc::RTLD_NOW | libc::RTLD_NOLOAD) };
        if library.is_null() {
            return Err("cannot retain the linked CEF library".into());
        }
        let address = unsafe { libc::dlsym(library, c"cef_paneflow_gpu_contract_v1".as_ptr()) };
        let mut symbol_location = std::mem::MaybeUninit::<libc::Dl_info>::zeroed();
        let located = !address.is_null()
            && unsafe { libc::dladdr(address, symbol_location.as_mut_ptr()) } != 0;
        if !located || unsafe { symbol_location.assume_init() }.dli_fbase != cef_location.dli_fbase
        {
            unsafe { libc::dlclose(library) };
            let path = unsafe { CStr::from_ptr(cef_location.dli_fname) }.to_string_lossy();
            return Err(format!("CEF GPU contract export missing from {path}"));
        }
        let observe = unsafe { std::mem::transmute::<*mut libc::c_void, Observe>(address) };
        Ok(Self {
            library,
            observe,
            epoch: None,
            revision: 0,
            gpu,
        })
    }

    pub(super) fn verify(&mut self) -> Result<(), String> {
        let mut snapshot = Snapshot::default();
        let result = unsafe { (self.observe)(size_of::<Snapshot>() as u32, &mut snapshot) };
        self.accept(result, &snapshot)
    }

    fn accept(&mut self, result: i32, snapshot: &Snapshot) -> Result<(), String> {
        if result != 0
            || snapshot.status != 0
            || snapshot.abi_version != 1
            || snapshot.struct_size != size_of::<Snapshot>() as u32
            || snapshot.flags != 3
            || snapshot.skia_backend != 1
            || snapshot.gl_implementation != 1
            || snapshot.angle_implementation != 1
            || snapshot.reserved != 0
            || (snapshot.vendor_id, snapshot.device_id) != self.gpu
            || snapshot.gpu_host_id == 0
            || snapshot.gpu_process_id == 0
            || snapshot.gpu_info_revision == 0
        {
            return Err(format!(
                "CEF external GPU contract rejected: result={result}, snapshot={snapshot:?}"
            ));
        }
        let epoch = (snapshot.gpu_host_id, snapshot.gpu_process_id);
        if self.epoch.is_some_and(|previous| previous != epoch)
            || snapshot.gpu_info_revision < self.revision
        {
            return Err("CEF GPU incarnation changed; the host must be restarted before importing another frame".into());
        }
        self.epoch = Some(epoch);
        self.revision = snapshot.gpu_info_revision;
        Ok(())
    }
}

impl Drop for GpuContract {
    fn drop(&mut self) {
        unsafe { libc::dlclose(self.library) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn unused_observer(_: u32, _: *mut Snapshot) -> i32 {
        7
    }

    fn guard() -> std::mem::ManuallyDrop<GpuContract> {
        std::mem::ManuallyDrop::new(GpuContract {
            library: std::ptr::null_mut(),
            observe: unused_observer,
            epoch: None,
            revision: 0,
            gpu: (0x10de, 0x2705),
        })
    }

    fn valid() -> Snapshot {
        Snapshot {
            abi_version: 1,
            struct_size: 64,
            flags: 3,
            gpu_host_id: 1,
            gpu_process_id: 100,
            gpu_info_revision: 1,
            vendor_id: 0x10de,
            device_id: 0x2705,
            skia_backend: 1,
            gl_implementation: 1,
            angle_implementation: 1,
            ..Snapshot::default()
        }
    }

    #[test]
    fn backend_and_sandbox_must_be_observed_for_every_frame() {
        let mut guard = guard();
        assert!(guard.accept(0, &valid()).is_ok());
        for field in 0..12 {
            let mut invalid = valid();
            match field {
                0 => invalid.flags = 1,
                1 => invalid.flags = 7,
                2 => invalid.skia_backend = 0,
                3 => invalid.gl_implementation = 0,
                4 => invalid.angle_implementation = 0,
                5 => invalid.vendor_id = 0x1002,
                6 => invalid.device_id = 1,
                7 => invalid.reserved = 1,
                8 => invalid.abi_version = 2,
                9 => invalid.struct_size = 63,
                10 => invalid.status = 5,
                _ => invalid.gpu_info_revision = 0,
            }
            assert!(guard.accept(0, &invalid).is_err(), "field {field}");
        }
        assert!(guard.accept(5, &valid()).is_err());
    }

    #[test]
    fn a_gpu_restart_cannot_adopt_an_old_callback() {
        let mut guard = guard();
        assert!(guard.accept(0, &valid()).is_ok());
        let mut updated = valid();
        updated.gpu_info_revision = 2;
        assert!(guard.accept(0, &updated).is_ok());
        assert!(guard.accept(0, &valid()).is_err());
        updated.gpu_host_id = 2;
        assert!(guard.accept(0, &updated).is_err());
        updated.gpu_host_id = 1;
        updated.gpu_process_id = 101;
        assert!(guard.accept(0, &updated).is_err());
    }
}
