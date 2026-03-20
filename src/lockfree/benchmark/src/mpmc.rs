extern crate lockfree;

use lockfree::channel::mpmc;
use std::{
    collections::VecDeque,
    sync::{mpsc as std_mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn measure<C>(nthread: usize, niter: u128) -> Duration
where
    C: Channel,
{
    let (sender, receiver) = C::create();
    let then = Instant::now();
    let mut threads = Vec::with_capacity(nthread * 2);

    for i in 0 .. nthread as u128 {
        let sender = sender.clone();
        threads.push(thread::spawn(move || {
            for j in i .. i + niter {
                sender.send(j).unwrap();
            }
            (0, 0)
        }));

        let receiver = receiver.clone();
        threads.push(thread::spawn(move || {
            let mut idle = 0u128;
            let mut res = 0u128;
            loop {
                match receiver.recv() {
                    Ok(val) => res = res.wrapping_add(val),
                    Err(mpmc::NoSender) => break (idle, res),
                    Err(mpmc::NoMessage) => idle = idle.wrapping_add(1),
                }
            }
        }));
    }

    drop(sender);
    drop(receiver);

    for thread in threads {
        thread.join().unwrap();
    }

    then.elapsed()
}

trait Channel {
    type Sender: Clone + Sender + Send + 'static;
    type Receiver: Clone + Receiver + Send + 'static;

    fn create() -> (Self::Sender, Self::Receiver);
}

trait Sender {
    fn send(&self, val: u128) -> Result<(), mpmc::NoRecv<u128>>;
}

trait Receiver {
    fn recv(&self) -> Result<u128, mpmc::RecvErr>;
}

struct Lockfree;

impl Channel for Lockfree {
    type Sender = mpmc::Sender<u128>;
    type Receiver = mpmc::Receiver<u128>;

    fn create() -> (Self::Sender, Self::Receiver) {
        mpmc::create()
    }
}

impl Sender for mpmc::Sender<u128> {
    fn send(&self, val: u128) -> Result<(), mpmc::NoRecv<u128>> {
        self.send(val)
    }
}

impl Receiver for mpmc::Receiver<u128> {
    fn recv(&self) -> Result<u128, mpmc::RecvErr> {
        self.recv()
    }
}

struct Mutexed;

impl Channel for Mutexed {
    type Sender = MutexedSender;
    type Receiver = MutexedReceiver;

    fn create() -> (Self::Sender, Self::Receiver) {
        let inner =
            MutexedInner { senders: 1, receivers: 1, queue: VecDeque::new() };
        let inner = Arc::new(Mutex::new(inner));

        let sender = MutexedSender { inner: inner.clone() };
        let receiver = MutexedReceiver { inner };
        (sender, receiver)
    }
}

struct MutexedInner {
    senders: usize,
    receivers: usize,
    queue: VecDeque<u128>,
}

struct MutexedSender {
    inner: Arc<Mutex<MutexedInner>>,
}

impl Sender for MutexedSender {
    fn send(&self, val: u128) -> Result<(), mpmc::NoRecv<u128>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.receivers == 0 {
            Err(mpmc::NoRecv { message: val })
        } else {
            inner.queue.push_back(val);
            Ok(())
        }
    }
}

impl Clone for MutexedSender {
    fn clone(&self) -> Self {
        self.inner.lock().unwrap().senders += 1;
        Self { inner: self.inner.clone() }
    }
}

impl Drop for MutexedSender {
    fn drop(&mut self) {
        self.inner.lock().unwrap().senders -= 1;
    }
}

struct MutexedReceiver {
    inner: Arc<Mutex<MutexedInner>>,
}

impl Receiver for MutexedReceiver {
    fn recv(&self) -> Result<u128, mpmc::RecvErr> {
        let mut inner = self.inner.lock().unwrap();
        if inner.senders == 0 {
            Err(mpmc::NoSender)
        } else if let Some(msg) = inner.queue.pop_front() {
            Ok(msg)
        } else {
            Err(mpmc::NoMessage)
        }
    }
}

impl Drop for MutexedReceiver {
    fn drop(&mut self) {
        self.inner.lock().unwrap().receivers -= 1;
    }
}

impl Clone for MutexedReceiver {
    fn clone(&self) -> Self {
        self.inner.lock().unwrap().receivers += 1;
        Self { inner: self.inner.clone() }
    }
}

struct Std;

impl Channel for Std {
    type Sender = std_mpsc::Sender<u128>;
    type Receiver = Arc<Mutex<std_mpsc::Receiver<u128>>>;

    fn create() -> (Self::Sender, Self::Receiver) {
        let (sender, receiver) = std_mpsc::channel();
        (sender, Arc::new(Mutex::new(receiver)))
    }
}

impl Sender for std_mpsc::Sender<u128> {
    fn send(&self, val: u128) -> Result<(), mpmc::NoRecv<u128>> {
        self.send(val)
            .map_err(|std_mpsc::SendError(message)| mpmc::NoRecv { message })
    }
}

impl Receiver for Arc<Mutex<std_mpsc::Receiver<u128>>> {
    fn recv(&self) -> Result<u128, mpmc::RecvErr> {
        self.lock().unwrap().try_recv().map_err(|err| match err {
            std_mpsc::TryRecvError::Empty => mpmc::NoMessage,
            std_mpsc::TryRecvError::Disconnected => mpmc::NoSender,
        })
    }
}

fn main() {
    const SAMPLES: usize = 5;
    const NITER: u128 = 0x8000;

    let mut first = true;

    for &nthread in &[2, 4, 8] {
        if first {
            first = false;
        } else {
            println!();
        }

        let mut deque = Duration::default();
        let mut std = Duration::default();
        let mut lockfree = Duration::default();

        for _ in 0 .. SAMPLES {
            deque += measure::<Mutexed>(nthread, NITER);
            std += measure::<Std>(nthread, NITER);
            lockfree += measure::<Lockfree>(nthread, NITER);
        }

        println!(
            "Mutexed VecDeque with {} threads total time: {:?}",
            nthread * 2,
            deque
        );
        println!(
            "Mutexed Std's MPSC (as MPMC)  with {} threads total time: {:?}",
            nthread * 2,
            std
        );
        println!(
            "Lockfree MPMC with {} threads total time: {:?}",
            nthread * 2,
            lockfree
        );
    }
}
