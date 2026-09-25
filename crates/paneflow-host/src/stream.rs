use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use crate::runtime::OutputSlice;
use crate::tail::{OutputTail, TailEvicted};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamWait {
    Published,
    Ended,
    Deadline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamStatus {
    pub end_offset: u64,
    pub live: bool,
    pub retained_bytes: usize,
    pub allocated_bytes: usize,
}

struct StreamState {
    tail: Option<OutputTail>,
    end_offset: u64,
    live: bool,
    publications: u64,
}

pub struct OutputStream {
    state: Mutex<StreamState>,
    changed: Condvar,
}

impl OutputStream {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(StreamState {
                tail: Some(OutputTail::new(capacity)),
                end_offset: 0,
                live: true,
                publications: 0,
            }),
            changed: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, StreamState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn append(&self, chunk: &[u8]) -> u64 {
        let mut state = self.lock();
        if let Some(tail) = state.tail.as_mut() {
            tail.append(chunk);
            state.end_offset = tail.end_offset();
        }
        state.publications += 1;
        let end = state.end_offset;
        drop(state);
        self.changed.notify_all();
        end
    }

    pub fn end_offset(&self) -> u64 {
        self.lock().end_offset
    }

    pub fn status(&self) -> StreamStatus {
        let state = self.lock();
        StreamStatus {
            end_offset: state.end_offset,
            live: state.live,
            retained_bytes: state.tail.as_ref().map_or(0, OutputTail::retained_bytes),
            allocated_bytes: state.tail.as_ref().map_or(0, OutputTail::allocated_bytes),
        }
    }

    pub fn finish(&self) {
        let mut state = self.lock();
        state.live = false;
        state.publications += 1;
        drop(state);
        self.changed.notify_all();
    }

    pub fn release(&self) {
        let mut state = self.lock();
        state.live = false;
        state.tail = None;
        state.publications += 1;
        drop(state);
        self.changed.notify_all();
    }

    pub fn wake(&self) {
        let mut state = self.lock();
        state.publications += 1;
        drop(state);
        self.changed.notify_all();
    }

    pub fn read(&self, from: u64, max: usize) -> Result<OutputSlice, TailEvicted> {
        let state = self.lock();
        let end = state.end_offset;
        let (offset, data) = match state.tail.as_ref() {
            Some(tail) => tail.read_from(from, max)?,
            None if from == end => (from, Vec::new()),
            None => {
                return Err(TailEvicted {
                    requested: from,
                    tail_start: end,
                    tail_end: end,
                });
            }
        };
        Ok(OutputSlice {
            offset,
            data,
            end_offset: end,
            live: state.live,
        })
    }

    pub fn wait_past(&self, offset: u64, deadline: Instant) -> StreamWait {
        let mut state = self.lock();
        loop {
            if state.end_offset > offset {
                return StreamWait::Published;
            }
            if !state.live {
                return StreamWait::Ended;
            }
            let now = Instant::now();
            if now >= deadline {
                return StreamWait::Deadline;
            }
            let seen = state.publications;
            let (next, _) = self
                .changed
                .wait_timeout_while(state, deadline - now, |state| state.publications == seen)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_publication_between_read_and_wait_is_never_lost() {
        let stream = OutputStream::new(1024);
        let waiter = Arc::clone(&stream);
        let end = stream.append(b"abc");
        let handle = std::thread::spawn(move || {
            waiter.wait_past(end, Instant::now() + Duration::from_secs(5))
        });
        std::thread::sleep(Duration::from_millis(20));
        stream.append(b"d");
        assert_eq!(handle.join().unwrap(), StreamWait::Published);
        let slice = stream.read(end, 16).unwrap();
        assert_eq!(slice.data, b"d");
        assert_eq!(slice.end_offset, 4);
    }

    #[test]
    fn a_notification_before_the_wait_returns_immediately() {
        let stream = OutputStream::new(1024);
        stream.append(b"early");
        assert_eq!(
            stream.wait_past(0, Instant::now() + Duration::from_secs(5)),
            StreamWait::Published
        );
        assert_eq!(
            stream.wait_past(5, Instant::now() + Duration::from_millis(20)),
            StreamWait::Deadline
        );
    }

    #[test]
    fn finishing_and_releasing_wake_waiters_and_keep_the_final_offset() {
        let stream = OutputStream::new(4);
        stream.append(b"0123456789");
        let waiter = Arc::clone(&stream);
        let handle = std::thread::spawn(move || {
            waiter.wait_past(10, Instant::now() + Duration::from_secs(5))
        });
        std::thread::sleep(Duration::from_millis(20));
        stream.finish();
        assert_eq!(handle.join().unwrap(), StreamWait::Ended);
        assert!(stream.read(10, 4).unwrap().data.is_empty());
        stream.release();
        assert_eq!(stream.status().retained_bytes, 0);
        assert_eq!(stream.read(10, 4).unwrap().end_offset, 10);
        assert!(!stream.read(10, 4).unwrap().live);
        assert!(stream.read(8, 4).is_err(), "released bytes are evicted");
    }
}
