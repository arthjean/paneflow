use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake as TaskWake, Waker};
use std::time::Duration;

use crate::clock::{Clock, Wake};

pub type TaskId = u64;

type LocalFuture = Pin<Box<dyn Future<Output = ()>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    TopLevel,
    Timer,
    Handler,
    Callback,
    Internal,
    Daemon,
}

struct Task {
    kind: TaskKind,
    future: LocalFuture,
}

struct ReadyQueue {
    ids: Mutex<VecDeque<TaskId>>,
    wake: Arc<Wake>,
}

struct TaskWaker {
    id: TaskId,
    queue: Arc<ReadyQueue>,
}

impl TaskWake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.queue
            .ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(self.id);
        self.queue.wake.notify();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    Finished,
    Stopped,
    Stalled,
}

type EndHook = Box<dyn Fn(TaskId)>;

pub struct Executor {
    clock: Arc<dyn Clock>,
    queue: Arc<ReadyQueue>,
    tasks: RefCell<HashMap<TaskId, Task>>,
    spawned: RefCell<Vec<(TaskId, Task)>>,
    timers: RefCell<BinaryHeap<Reverse<(Duration, u64)>>>,
    timer_wakers: RefCell<HashMap<u64, Waker>>,
    next_timer: Cell<u64>,
    next_task: Cell<TaskId>,
    current: Cell<Option<(TaskId, TaskKind)>>,
    outstanding: Arc<AtomicUsize>,
    holds: Cell<usize>,
    finished: RefCell<BTreeMap<TaskId, Vec<Waker>>>,
    done: RefCell<std::collections::HashSet<TaskId>>,
    end_hooks: RefCell<Vec<EndHook>>,
    slice_started: Cell<Duration>,
}

impl Executor {
    pub fn new(clock: Arc<dyn Clock>, wake: Arc<Wake>) -> Rc<Self> {
        Rc::new(Self {
            clock,
            queue: Arc::new(ReadyQueue {
                ids: Mutex::new(VecDeque::new()),
                wake,
            }),
            tasks: RefCell::new(HashMap::new()),
            spawned: RefCell::new(Vec::new()),
            timers: RefCell::new(BinaryHeap::new()),
            timer_wakers: RefCell::new(HashMap::new()),
            next_timer: Cell::new(1),
            next_task: Cell::new(1),
            current: Cell::new(None),
            outstanding: Arc::new(AtomicUsize::new(0)),
            holds: Cell::new(0),
            finished: RefCell::new(BTreeMap::new()),
            done: RefCell::new(std::collections::HashSet::new()),
            end_hooks: RefCell::new(Vec::new()),
            slice_started: Cell::new(Duration::ZERO),
        })
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    pub fn now(&self) -> Duration {
        self.clock.now()
    }

    pub fn wake_handle(&self) -> Arc<Wake> {
        self.queue.wake.clone()
    }

    pub fn slice_started(&self) -> Duration {
        self.slice_started.get()
    }

    pub fn current(&self) -> Option<(TaskId, TaskKind)> {
        self.current.get()
    }

    pub fn on_task_end(&self, hook: impl Fn(TaskId) + 'static) {
        self.end_hooks.borrow_mut().push(Box::new(hook));
    }

    pub fn spawn(&self, kind: TaskKind, future: impl Future<Output = ()> + 'static) -> TaskId {
        let id = self.next_task.get();
        self.next_task.set(id + 1);
        self.spawned.borrow_mut().push((
            id,
            Task {
                kind,
                future: Box::pin(future),
            },
        ));
        self.queue
            .ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(id);
        id
    }

    pub fn join(self: &Rc<Self>, id: TaskId) -> Join {
        Join {
            executor: self.clone(),
            id,
        }
    }

    pub fn sleep_until(self: &Rc<Self>, deadline: Duration) -> Sleep {
        Sleep {
            executor: self.clone(),
            deadline,
            timer: None,
        }
    }

    pub fn hold(self: &Rc<Self>) -> Hold {
        self.holds.set(self.holds.get() + 1);
        Hold {
            executor: self.clone(),
        }
    }

    pub fn offload<T: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Offloaded<T> {
        let slot = Arc::new(Mutex::new(OffloadSlot {
            value: None,
            waker: None,
        }));
        let outstanding = self.outstanding.clone();
        outstanding.fetch_add(1, Ordering::AcqRel);
        let wake = self.queue.wake.clone();
        let filled = slot.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("paneflow-script-{name}"))
            .spawn(move || {
                let value = work();
                let waker = {
                    let mut slot = filled.lock().unwrap_or_else(PoisonError::into_inner);
                    slot.value = Some(value);
                    slot.waker.take()
                };
                outstanding.fetch_sub(1, Ordering::AcqRel);
                if let Some(waker) = waker {
                    waker.wake();
                }
                wake.notify();
            });
        if spawned.is_err() {
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
            return Offloaded { slot, failed: true };
        }
        Offloaded {
            slot,
            failed: false,
        }
    }

    pub fn external(&self) -> External {
        External {
            outstanding: self.outstanding.clone(),
            wake: self.queue.wake.clone(),
        }
    }

    fn adopt_spawned(&self) {
        let spawned: Vec<(TaskId, Task)> = self.spawned.borrow_mut().drain(..).collect();
        let mut tasks = self.tasks.borrow_mut();
        for (id, task) in spawned {
            tasks.insert(id, task);
        }
    }

    fn pop_ready(&self) -> Option<TaskId> {
        self.queue
            .ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    fn poll_task(&self, id: TaskId) {
        let Some(mut task) = self.tasks.borrow_mut().remove(&id) else {
            return;
        };
        let waker = Waker::from(Arc::new(TaskWaker {
            id,
            queue: self.queue.clone(),
        }));
        let mut context = Context::from_waker(&waker);
        let previous = self.current.replace(Some((id, task.kind)));
        self.slice_started.set(self.clock.now());
        let polled = task.future.as_mut().poll(&mut context);
        self.current.set(previous);
        match polled {
            Poll::Pending => {
                self.tasks.borrow_mut().insert(id, task);
            }
            Poll::Ready(()) => {
                drop(task);
                self.finish(id);
            }
        }
    }

    fn finish(&self, id: TaskId) {
        self.done.borrow_mut().insert(id);
        for hook in self.end_hooks.borrow().iter() {
            hook(id);
        }
        if let Some(waiters) = self.finished.borrow_mut().remove(&id) {
            for waker in waiters {
                waker.wake();
            }
        }
    }

    fn fire_due_timers(&self) -> bool {
        let now = self.clock.now();
        let mut fired = false;
        loop {
            let due = {
                let timers = self.timers.borrow();
                timers
                    .peek()
                    .is_some_and(|Reverse((deadline, _))| *deadline <= now)
            };
            if !due {
                break;
            }
            let Some(Reverse((_, timer))) = self.timers.borrow_mut().pop() else {
                break;
            };
            if let Some(waker) = self.timer_wakers.borrow_mut().remove(&timer) {
                waker.wake();
                fired = true;
            }
        }
        fired
    }

    fn next_deadline(&self) -> Option<Duration> {
        let mut timers = self.timers.borrow_mut();
        let wakers = self.timer_wakers.borrow();
        while let Some(Reverse((deadline, timer))) = timers.peek().copied() {
            if wakers.contains_key(&timer) {
                return Some(deadline);
            }
            timers.pop();
        }
        None
    }

    pub fn cancel(&self, id: TaskId) {
        let removed = self.tasks.borrow_mut().remove(&id);
        let pending = {
            let mut spawned = self.spawned.borrow_mut();
            spawned
                .iter()
                .position(|(task, _)| *task == id)
                .map(|index| spawned.remove(index))
        };
        let found = removed.is_some() || pending.is_some();
        drop(removed);
        drop(pending);
        if found {
            self.finish(id);
        }
    }

    pub fn cancel_all(&self) {
        self.spawned.borrow_mut().clear();
        let ids: Vec<TaskId> = self.tasks.borrow().keys().copied().collect();
        for id in ids {
            let task = self.tasks.borrow_mut().remove(&id);
            drop(task);
            self.finish(id);
        }
    }

    pub fn run(&self, stop: &dyn Fn() -> bool) -> RunEnd {
        loop {
            self.adopt_spawned();
            while let Some(id) = self.pop_ready() {
                if stop() {
                    return RunEnd::Stopped;
                }
                self.adopt_spawned();
                self.poll_task(id);
            }
            if stop() {
                return RunEnd::Stopped;
            }
            self.adopt_spawned();
            if self.fire_due_timers() {
                continue;
            }
            if self.spawned.borrow().is_empty()
                && self
                    .tasks
                    .borrow()
                    .values()
                    .all(|task| task.kind == TaskKind::Daemon)
            {
                return RunEnd::Finished;
            }
            if !self
                .queue
                .ids
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
            {
                continue;
            }
            let deadline = self.next_deadline();
            let busy = self.outstanding.load(Ordering::Acquire) > 0;
            if deadline.is_none() && !busy && self.holds.get() == 0 {
                return RunEnd::Stalled;
            }
            self.clock.park(&self.queue.wake, deadline, busy);
        }
    }
}

pub struct Hold {
    executor: Rc<Executor>,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.executor
            .holds
            .set(self.executor.holds.get().saturating_sub(1));
    }
}

#[derive(Clone)]
pub struct External {
    outstanding: Arc<AtomicUsize>,
    wake: Arc<Wake>,
}

impl External {
    pub fn begin(&self) -> ExternalGuard {
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        ExternalGuard {
            outstanding: self.outstanding.clone(),
            wake: self.wake.clone(),
        }
    }

    pub fn notify(&self) {
        self.wake.notify();
    }
}

pub struct ExternalGuard {
    outstanding: Arc<AtomicUsize>,
    wake: Arc<Wake>,
}

impl Drop for ExternalGuard {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
        self.wake.notify();
    }
}

pub struct Sleep {
    executor: Rc<Executor>,
    deadline: Duration,
    timer: Option<u64>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.executor.clock.now() >= self.deadline {
            if let Some(timer) = self.timer.take() {
                self.executor.timer_wakers.borrow_mut().remove(&timer);
            }
            return Poll::Ready(());
        }
        let timer = match self.timer {
            Some(timer) => timer,
            None => {
                let timer = self.executor.next_timer.get();
                self.executor.next_timer.set(timer + 1);
                self.executor
                    .timers
                    .borrow_mut()
                    .push(Reverse((self.deadline, timer)));
                self.timer = Some(timer);
                timer
            }
        };
        self.executor
            .timer_wakers
            .borrow_mut()
            .insert(timer, context.waker().clone());
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            self.executor.timer_wakers.borrow_mut().remove(&timer);
        }
    }
}

pub struct Join {
    executor: Rc<Executor>,
    id: TaskId,
}

impl Future for Join {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.executor.done.borrow().contains(&self.id) {
            return Poll::Ready(());
        }
        self.executor
            .finished
            .borrow_mut()
            .entry(self.id)
            .or_default()
            .push(context.waker().clone());
        Poll::Pending
    }
}

struct OffloadSlot<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

pub struct Offloaded<T> {
    slot: Arc<Mutex<OffloadSlot<T>>>,
    failed: bool,
}

impl<T> Future for Offloaded<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        if self.failed {
            return Poll::Ready(None);
        }
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.value.take() {
            Some(value) => Poll::Ready(Some(value)),
            None => {
                slot.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            return Poll::Ready(());
        }
        self.yielded = true;
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;

    fn executor() -> Rc<Executor> {
        Executor::new(Arc::new(FakeClock::new()), Arc::new(Wake::default()))
    }

    #[test]
    fn timers_fire_in_deadline_order_on_a_fake_clock() {
        let executor = executor();
        let order = Rc::new(RefCell::new(Vec::new()));
        for (label, at) in [("late", 3), ("early", 1), ("middle", 2)] {
            let sleep = executor.sleep_until(Duration::from_secs(at));
            let order = order.clone();
            executor.spawn(TaskKind::Internal, async move {
                sleep.await;
                order.borrow_mut().push(label);
            });
        }
        assert_eq!(executor.run(&|| false), RunEnd::Finished);
        assert_eq!(*order.borrow(), ["early", "middle", "late"]);
        assert_eq!(executor.now(), Duration::from_secs(3));
    }

    #[test]
    fn an_offloaded_result_wakes_the_task_that_awaits_it() {
        let executor = executor();
        let seen = Rc::new(Cell::new(0));
        let work = executor.offload("test", || 41 + 1);
        let into = seen.clone();
        executor.spawn(TaskKind::Internal, async move {
            into.set(work.await.unwrap_or_default());
        });
        assert_eq!(executor.run(&|| false), RunEnd::Finished);
        assert_eq!(seen.get(), 42);
    }

    #[test]
    fn a_task_that_waits_on_nothing_that_can_happen_stalls_instead_of_hanging() {
        let executor = executor();
        executor.spawn(TaskKind::Internal, std::future::pending());
        assert_eq!(executor.run(&|| false), RunEnd::Stalled);
    }

    #[test]
    fn join_resolves_once_the_joined_task_has_finished() {
        let executor = executor();
        let log = Rc::new(RefCell::new(Vec::new()));
        let inner_log = log.clone();
        let sleep = executor.sleep_until(Duration::from_secs(1));
        let child = executor.spawn(TaskKind::Internal, async move {
            sleep.await;
            inner_log.borrow_mut().push("child");
        });
        let join = executor.join(child);
        let outer_log = log.clone();
        executor.spawn(TaskKind::Internal, async move {
            join.await;
            outer_log.borrow_mut().push("parent");
        });
        executor.run(&|| false);
        assert_eq!(*log.borrow(), ["child", "parent"]);
    }
}
