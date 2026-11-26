use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use paneflow_browser_proto::limits::MAX_FRAME_BYTES;
use paneflow_browser_proto::surface::{BYTES_PER_PIXEL, FRAME_SLOTS, Rect, frame_bytes};

pub struct Layer<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub at: Rect,
}

struct Slot {
    fd: OwnedFd,
    base: *mut u8,
    busy: bool,
}

pub struct Slots {
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    len: usize,
    slots: Vec<Slot>,
}

impl Slots {
    pub fn create(generation: u64, width: u32, height: u32) -> Result<Self, String> {
        let len = frame_bytes(width, height)
            .ok_or_else(|| format!("a {width} x {height} frame has no addressable size"))?;
        if len > MAX_FRAME_BYTES {
            return Err(format!(
                "a {width} x {height} frame needs {} MiB of shared memory per slot, over the {} MiB limit",
                len.div_ceil(1024 * 1024),
                MAX_FRAME_BYTES / (1024 * 1024)
            ));
        }
        let mut slots = Vec::with_capacity(FRAME_SLOTS as usize);
        for index in 0..FRAME_SLOTS {
            let fd = segment(len)
                .map_err(|error| format!("cannot create shared frame slot {index}: {error}"))?;
            let base = map(&fd, len)
                .map_err(|error| format!("cannot map shared frame slot {index}: {error}"))?;
            slots.push(Slot {
                fd,
                base,
                busy: false,
            });
        }
        Ok(Self {
            generation,
            width,
            height,
            stride: width * BYTES_PER_PIXEL,
            len,
            slots,
        })
    }

    pub fn fds(&self) -> io::Result<Vec<OwnedFd>> {
        self.slots.iter().map(|slot| slot.fd.try_clone()).collect()
    }

    pub fn slot_count(&self) -> u32 {
        u32::try_from(self.slots.len()).unwrap_or(0)
    }

    pub fn release(&mut self, slot: u32) {
        if let Some(slot) = self.slots.get_mut(slot as usize) {
            slot.busy = false;
        }
    }

    pub fn free_slot(&self) -> Option<u32> {
        self.slots
            .iter()
            .position(|slot| !slot.busy)
            .and_then(|index| u32::try_from(index).ok())
    }

    pub fn write(&mut self, index: u32, view: &[u8], popup: Option<Layer<'_>>) -> bool {
        let len = self.len;
        let stride = self.stride as usize;
        let height = self.height as usize;
        let Some(slot) = self.slots.get_mut(index as usize) else {
            return false;
        };
        if view.len() != len {
            return false;
        }
        let target = unsafe { std::slice::from_raw_parts_mut(slot.base, len) };
        target.copy_from_slice(view);
        if let Some(layer) = popup {
            blit(target, stride, height, &layer);
        }
        slot.busy = true;
        true
    }
}

fn blit(target: &mut [u8], stride: usize, height: usize, layer: &Layer<'_>) {
    let layer_stride = layer.width as usize * BYTES_PER_PIXEL as usize;
    let rows = (layer.at.height.max(0) as usize).min(layer.height as usize);
    let columns = (layer.at.width.max(0) as usize).min(layer.width as usize);
    let x = layer.at.x.max(0) as usize;
    let y = layer.at.y.max(0) as usize;
    for row in 0..rows {
        let target_row = y + row;
        if target_row >= height {
            break;
        }
        let start = target_row * stride + x * BYTES_PER_PIXEL as usize;
        let room = stride.saturating_sub(x * BYTES_PER_PIXEL as usize);
        let bytes = (columns * BYTES_PER_PIXEL as usize).min(room);
        let source_start = row * layer_stride;
        let (Some(destination), Some(source)) = (
            target.get_mut(start..start + bytes),
            layer.pixels.get(source_start..source_start + bytes),
        ) else {
            break;
        };
        destination.copy_from_slice(source);
    }
}

impl Drop for Slots {
    fn drop(&mut self) {
        for slot in &self.slots {
            unsafe {
                libc::munmap(slot.base.cast(), self.len);
            }
        }
    }
}

fn map(fd: &OwnedFd, len: usize) -> io::Result<*mut u8> {
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(base.cast())
}

fn resize(fd: &OwnedFd, len: usize) -> io::Result<()> {
    let length = libc::off_t::try_from(len).map_err(|_| io::Error::other("frame too large"))?;
    if unsafe { libc::ftruncate(fd.as_raw_fd(), length) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn segment(len: usize) -> io::Result<OwnedFd> {
    let name = c"paneflow-browser-frame";
    let raw =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    resize(&fd, len)?;
    let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn segment(len: usize) -> io::Result<OwnedFd> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "/paneflow-frame-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let name = std::ffi::CString::new(name).map_err(io::Error::other)?;
    let raw = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
            0o600 as libc::c_uint,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    unsafe {
        libc::shm_unlink(name.as_ptr());
    }
    resize(&fd, len)?;
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_slots_cycle_until_released() {
        let mut slots = Slots::create(1, 4, 2).unwrap();
        let view = vec![7u8; 4 * 2 * 4];
        for expected in 0..3 {
            let slot = slots.free_slot().unwrap();
            assert_eq!(slot, expected);
            assert!(slots.write(slot, &view, None));
        }
        assert_eq!(slots.free_slot(), None);
        slots.release(1);
        assert_eq!(slots.free_slot(), Some(1));
        assert_eq!(slots.fds().unwrap().len(), 3);
    }

    #[test]
    fn a_frame_of_the_wrong_size_is_not_written() {
        let mut slots = Slots::create(1, 4, 2).unwrap();
        assert!(!slots.write(0, &[0u8; 3], None));
        assert_eq!(slots.free_slot(), Some(0));
    }

    #[test]
    fn a_frame_over_the_limit_is_refused_before_any_segment_exists() {
        let error = Slots::create(1, 32_768, 32_768).err().unwrap();
        assert!(error.contains("over the 128 MiB limit"), "{error}");
    }

    #[test]
    fn a_popup_layer_lands_at_its_rectangle_and_stays_inside() {
        let mut target = vec![0u8; 4 * 4 * 4];
        let popup = vec![9u8; 3 * 2 * 4];
        blit(
            &mut target,
            16,
            4,
            &Layer {
                pixels: &popup,
                width: 3,
                height: 2,
                at: Rect {
                    x: 2,
                    y: 3,
                    width: 3,
                    height: 2,
                },
            },
        );
        let pixel = |x: usize, y: usize| target[y * 16 + x * 4];
        assert_eq!(pixel(2, 3), 9);
        assert_eq!(pixel(3, 3), 9);
        assert_eq!(pixel(1, 3), 0);
        assert_eq!(pixel(2, 2), 0);
        assert_eq!(target.len(), 64);
    }
}
