use lockfree::queue::Queue;
use owned_alloc::OwnedAlloc;
use std::{
    cell::Cell,
    fmt,
    ptr::{null_mut, NonNull},
    sync::{
        atomic::{AtomicPtr, AtomicUsize, Ordering::*},
        Arc,
    },
    thread,
};

pub fn spawn<F, T>(task: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let shared = Arc::new(SharedHandle {
        ret: Cell::new(None),
        status: AtomicUsize::new(RUNNING),
        waiting: Cell::new(None),
    });

    let thread = THREADS.pop().unwrap_or_else(new_thread);
    let exiter = ExitHandle { shared: shared.clone() };
    thread.spawn(move || {
        let _ = exiter.exit(task());
    });

    let joiner = JoinHandle { spawner: Some(thread), shared };
    joiner
}

const NO_TASK: usize = 0;
const HAS_TASK: usize = 1;
const DISCONNECTED: usize = 2;

const RUNNING: usize = 0;
const WAIT_RECV: usize = 1;
const DONE: usize = 2;
const PANICKED: usize = 3;

lazy_static! {
    static ref THREADS: Queue<ThreadSpawner> = Queue::new();
}

trait Task: Send {
    fn run(self: Box<Self>);
}

impl<F> Task for F
where
    F: FnOnce() + Send,
{
    fn run(self: Box<Self>) {
        self()
    }
}

struct ThreadInner {
    std: thread::Thread,
    status: AtomicUsize,
    task: Cell<Option<Box<dyn Task>>>,
}

struct ThreadExec {
    inner: NonNull<ThreadInner>,
}

impl ThreadExec {
    fn next_task(&self) -> Option<Box<dyn Task>> {
        let inner = unsafe { self.inner.as_ref() };
        loop {
            match inner.status.swap(NO_TASK, Acquire) {
                HAS_TASK => break inner.task.take(),
                NO_TASK => thread::park(),
                DISCONNECTED => break None,
                _ => unreachable!(),
            }
        }
    }
}

impl Drop for ThreadExec {
    fn drop(&mut self) {
        let prev =
            unsafe { self.inner.as_ref().status.swap(DISCONNECTED, Relaxed) };
        if prev == DISCONNECTED {
            unsafe { OwnedAlloc::from_raw(self.inner) };
        }
    }
}

unsafe impl Send for ThreadExec {}
unsafe impl Sync for ThreadExec {}

struct ThreadSpawner {
    joiner: Option<thread::JoinHandle<()>>,
    inner: NonNull<ThreadInner>,
}

impl ThreadSpawner {
    fn spawn<F>(&self, task: F)
    where
        F: Task + 'static,
    {
        let inner = unsafe { self.inner.as_ref() };
        inner.task.set(Some(Box::new(task)));
        inner.status.store(HAS_TASK, Release);

        while inner.status.load(Relaxed) == HAS_TASK {
            inner.std.unpark();
            thread::yield_now();
        }
    }

    fn join_for_real(mut self) -> thread::Result<()> {
        self.joiner.take().unwrap().join()
    }
}

impl Drop for ThreadSpawner {
    fn drop(&mut self) {
        let prev =
            unsafe { self.inner.as_ref().status.swap(DISCONNECTED, Relaxed) };
        if prev == DISCONNECTED {
            unsafe { OwnedAlloc::from_raw(self.inner) };
        }
    }
}

unsafe impl Send for ThreadSpawner {}
unsafe impl Sync for ThreadSpawner {}

pub struct JoinHandle<T> {
    spawner: Option<ThreadSpawner>,
    shared: Arc<SharedHandle<T>>,
}

impl<T> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(
            fmtr,
            "JoinHandle {} addr: {:?} {}",
            '{', &self.shared as *const _, '}'
        )
    }
}

impl<T> JoinHandle<T> {
    pub fn join(mut self) -> thread::Result<T> {
        let curr = thread::current();
        self.shared.waiting.set(Some(curr));
        let mut prev = self.shared.status.swap(WAIT_RECV, Acquire);

        loop {
            if prev == DONE {
                break Ok(self.shared.ret.take().unwrap());
            }

            if prev == PANICKED {
                break Err(self
                    .spawner
                    .take()
                    .unwrap()
                    .join_for_real()
                    .unwrap_err());
            }

            thread::park();
            prev = self.shared.status.swap(WAIT_RECV, Acquire);
        }
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if let Some(spawner) = self.spawner.take() {
            THREADS.push(spawner)
        }
    }
}

struct ExitHandle<T> {
    shared: Arc<SharedHandle<T>>,
}

impl<T> ExitHandle<T> {
    fn exit(self, ret: T) {
        self.shared.ret.set(Some(ret));
        let prev = self.shared.status.swap(DONE, Release);

        if prev == WAIT_RECV {
            let waiting = self.shared.waiting.take().unwrap();
            while self.shared.status.load(Relaxed) != WAIT_RECV {
                waiting.unpark();
                thread::yield_now();
            }
        }
    }
}

impl<T> Drop for ExitHandle<T> {
    fn drop(&mut self) {
        if thread::panicking() {
            let prev = self.shared.status.swap(PANICKED, Relaxed);

            if prev == WAIT_RECV {
                let waiting = self.shared.waiting.take().unwrap();
                while self.shared.status.load(Relaxed) != WAIT_RECV {
                    waiting.unpark();
                    thread::yield_now();
                }
            }
        }
    }
}

struct SharedHandle<T> {
    ret: Cell<Option<T>>,
    status: AtomicUsize,
    waiting: Cell<Option<thread::Thread>>,
}

unsafe impl<T> Send for SharedHandle<T> where T: Send {}
unsafe impl<T> Sync for SharedHandle<T> where T: Send {}

fn new_thread() -> ThreadSpawner {
    let bootstrap = Arc::new(AtomicPtr::new(null_mut()));

    let joiner = {
        let bootstrap = bootstrap.clone();

        thread::spawn(move || {
            let exec = loop {
                match NonNull::new(bootstrap.load(Acquire)) {
                    Some(inner) => break ThreadExec { inner },

                    None => thread::park(),
                }
            };

            while let Some(task) = exec.next_task() {
                task.run()
            }
        })
    };

    let inner = ThreadInner {
        std: joiner.thread().clone(),
        status: AtomicUsize::new(NO_TASK),
        task: Cell::new(None),
    };
    let inner = OwnedAlloc::new(inner).into_raw();
    bootstrap.store(inner.as_ptr(), Release);
    ThreadSpawner { inner, joiner: Some(joiner) }
}
