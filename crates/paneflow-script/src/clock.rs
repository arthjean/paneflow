use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct Wake {
    pending: Mutex<bool>,
    ready: Condvar,
}

impl Wake {
    pub fn notify(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        *pending = true;
        self.ready.notify_all();
    }

    pub fn take(&self) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::replace(&mut *pending, false)
    }

    pub fn wait(&self, timeout: Option<Duration>) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        match timeout {
            Some(timeout) => {
                let deadline = Instant::now() + timeout;
                while !*pending {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    pending = self
                        .ready
                        .wait_timeout(pending, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
            None => {
                while !*pending {
                    pending = self
                        .ready
                        .wait(pending)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        *pending = false;
    }
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
    fn park(&self, wake: &Wake, until: Option<Duration>, busy: bool);
}

#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn park(&self, wake: &Wake, until: Option<Duration>, _busy: bool) {
        let timeout = until.map(|until| until.saturating_sub(self.now()));
        if timeout.is_some_and(|timeout| timeout.is_zero()) {
            return;
        }
        wake.wait(timeout);
    }
}

#[derive(Debug, Default)]
pub struct FakeClock {
    now: Mutex<Duration>,
    step: Duration,
}

impl FakeClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ticking(step: Duration) -> Self {
        Self {
            now: Mutex::new(Duration::ZERO),
            step,
        }
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        let current = *now;
        *now += self.step;
        current
    }

    fn park(&self, wake: &Wake, until: Option<Duration>, busy: bool) {
        if wake.take() {
            return;
        }
        match until {
            Some(_) if busy => wake.wait(None),
            Some(until) => {
                let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
                if *now < until {
                    *now = until;
                }
            }
            None => wake.wait(None),
        }
    }
}
