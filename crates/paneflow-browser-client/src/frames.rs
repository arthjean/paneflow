use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use paneflow_browser_proto::surface::BYTES_PER_PIXEL;
use paneflow_browser_proto::{FrameDescriptor, TabId};

use crate::shared_memory::{Mapping, SharedFd};

pub type FrameWaker = Arc<dyn Fn(TabId) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserFrame {
    pub tab: TabId,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

struct Surface {
    generation: u64,
    width: u32,
    height: u32,
    stride: u32,
    maps: Vec<Mapping>,
}

#[derive(Default)]
struct TabFrames {
    surface: Option<Surface>,
    latest: Option<BrowserFrame>,
    sequence: u64,
    copied: u64,
    replaced: u64,
}

#[derive(Default)]
pub struct FrameStore {
    tabs: Mutex<HashMap<TabId, TabFrames>>,
    waker: Mutex<Option<FrameWaker>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub copied: u64,
    pub replaced_before_taken: u64,
}

pub struct Release {
    pub tab: TabId,
    pub generation: u64,
    pub slot: u32,
}

impl FrameStore {
    pub fn set_waker(&self, waker: Option<FrameWaker>) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = waker;
    }

    pub fn adopt_surface(
        &self,
        tab: TabId,
        generation: u64,
        width: u32,
        height: u32,
        stride: u32,
        fds: Vec<SharedFd>,
    ) -> Result<(), String> {
        let len = (stride as usize)
            .checked_mul(height as usize)
            .ok_or("the frame surface has no addressable size")?;
        if stride < width.saturating_mul(BYTES_PER_PIXEL) {
            return Err(format!(
                "the frame stride {stride} is narrower than {width} pixels"
            ));
        }
        let maps = fds
            .into_iter()
            .map(|fd| Mapping::read_only(fd, len))
            .collect::<Result<Vec<_>, _>>()?;
        let mut tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
        tabs.entry(tab).or_default().surface = Some(Surface {
            generation,
            width,
            height,
            stride,
            maps,
        });
        Ok(())
    }

    pub fn accept(&self, frame: &FrameDescriptor) -> Option<Release> {
        let FrameDescriptor::SharedMemory {
            tab,
            slot,
            generation,
            width,
            height,
            stride,
            ..
        } = frame
        else {
            return None;
        };
        let release = Release {
            tab: *tab,
            generation: *generation,
            slot: *slot,
        };
        let woken = {
            let mut tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = tabs.entry(*tab).or_default();
            let Some(surface) = entry.surface.as_ref().filter(|surface| {
                surface.generation == *generation
                    && surface.width == *width
                    && surface.height == *height
                    && surface.stride == *stride
            }) else {
                return Some(release);
            };
            let Some(mapping) = surface.maps.get(*slot as usize) else {
                return Some(release);
            };
            let Some(row) = width
                .checked_mul(BYTES_PER_PIXEL)
                .and_then(|row| usize::try_from(row).ok())
            else {
                return Some(release);
            };
            let mut pixels = Vec::with_capacity(mapping.bytes().len().min(row * *height as usize));
            let bytes = mapping.bytes();
            for line in 0..*height as usize {
                let start = line * *stride as usize;
                match bytes.get(start..start + row) {
                    Some(source) => pixels.extend_from_slice(source),
                    None => return Some(release),
                }
            }
            entry.sequence += 1;
            entry.copied += 1;
            if entry.latest.is_some() {
                entry.replaced += 1;
            }
            entry.latest = Some(BrowserFrame {
                tab: *tab,
                sequence: entry.sequence,
                width: *width,
                height: *height,
                pixels,
            });
            true
        };
        if woken {
            let waker = self
                .waker
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(waker) = waker {
                waker(*tab);
            }
        }
        Some(release)
    }

    pub fn take(&self, tab: TabId) -> Option<BrowserFrame> {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&tab)
            .and_then(|entry| entry.latest.take())
    }

    pub fn stats(&self, tab: TabId) -> FrameStats {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&tab)
            .map(|entry| FrameStats {
                copied: entry.copied,
                replaced_before_taken: entry.replaced,
            })
            .unwrap_or_default()
    }

    pub fn forget(&self, tab: TabId) {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&tab);
    }

    pub fn drop_surface(&self, tab: TabId) {
        if let Some(entry) = self
            .tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&tab)
        {
            entry.surface = None;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::shared_memory::anonymous_for_test;
    use paneflow_browser_proto::PixelFormat;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn descriptor(slot: u32, generation: u64) -> FrameDescriptor {
        FrameDescriptor::SharedMemory {
            tab: TabId(1),
            slot,
            generation,
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Bgra8,
        }
    }

    #[test]
    fn only_the_most_recent_frame_is_kept_until_taken_and_each_slot_is_released() {
        let store = FrameStore::default();
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        store.set_waker(Some(Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        let fds = (0..3)
            .map(|value| anonymous_for_test(&[value as u8; 16]))
            .collect();
        store.adopt_surface(TabId(1), 5, 2, 2, 8, fds).unwrap();
        for slot in 0..3 {
            let release = store.accept(&descriptor(slot, 5)).unwrap();
            assert_eq!((release.generation, release.slot), (5, slot));
        }
        let latest = store.take(TabId(1)).unwrap();
        assert_eq!(latest.pixels, vec![2u8; 16]);
        assert_eq!(latest.sequence, 3);
        assert!(store.take(TabId(1)).is_none());
        assert_eq!(
            store.stats(TabId(1)),
            FrameStats {
                copied: 3,
                replaced_before_taken: 2
            }
        );
        assert_eq!(woken.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_frame_of_an_old_generation_is_released_without_being_kept() {
        let store = FrameStore::default();
        let fds = vec![anonymous_for_test(&[1u8; 16])];
        store.adopt_surface(TabId(1), 5, 2, 2, 8, fds).unwrap();
        let release = store.accept(&descriptor(0, 4)).unwrap();
        assert_eq!(release.generation, 4);
        assert!(store.take(TabId(1)).is_none());
        assert!(store.accept(&descriptor(7, 5)).is_some());
        assert!(store.take(TabId(1)).is_none());
    }
}
