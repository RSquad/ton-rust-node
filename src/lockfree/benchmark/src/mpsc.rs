extern crate lockfree;

use lockfree::channel::mpsc;
use std::{
    collections::VecDeque,
    sync::{mpsc as std_mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn measure<C>(senders: usize, niter: u128) -> Duration
where
    C: Channel,
{
    let (sender, mut receiver) = C::create();
    let then = Instant::now();
    let mut threads = Vec::with_capacity(senders);

    for i in 0 .. senders as u128 {
        let sender = sender.clone();
        threads.push(thread::spawn(move || {
            for j in i .. i + niter {
                sender.send(j).unwrap();
            }
        }))
    }

    drop(sender);

    let mut idle = 0u128;
    let mut res = 0u128;
    let _ = loop {
        match receiver.recv() {
            Ok(val) => res = res.wrapping_add(val),
            Err(mpsc::NoSender) => break (idle, res),
            Err(mpsc::NoMessage) => idle = idle.wrapping_add(1),
        }
    };

    for thread in threads {
        thread.join().unwrap();
    }

    then.elapsed()
}

trait Channel {
    type Sender: Clone + Sender + Send + 'static;
    type Receiver: Receiver + Send + 'static;

    fn create() -> (Self::Sender, Self::Receiver);
}

trait Sender {
    fn send(&self, val: u128) -> Result<(), mpsc::NoRecv<u128>>;
}

trait Receiver {
    fn recv(&mut self) -> Result<u128, mpsc::RecvErr>;
}

struct Lockfree;

impl Channel for Lockfree {
    type Sender = mpsc::Sender<u128>;
    type Receiver = mpsc::Receiver<u128>;

    fn create() -> (Self::Sender, Self::Receiver) {
        mpsc::create()
    }
}

impl Sender for mpsc::Sender<u128> {
    fn send(&self, val: u128) -> Result<(), mpsc::NoRecv<u128>> {
        self.send(val)
    }
}

impl Receiver for mpsc::Receiver<u128> {
    fn recv(&mut self) -> Result<u128, mpsc::RecvErr> {
        self.recv()
    }
}

struct Mutexed;

impl Channel for Mutexed {
    type Sender = MutexedSender;
    type Receiver = MutexedReceiver;

    fn create() -> (Self::Sender, Self::Receiver) {
        let inner = MutexedInner { connected: true, queue: VecDeque::new() };
        let inner = Arc::new(Mutex::new(inner));

        let sender = MutexedSender { inner: inner.clone() };
        let receiver = MutexedReceiver { inner };
        (sender, receiver)
    }
}

struct MutexedInner {
    connected: bool,
    queue: VecDeque<u128>,
}

#[derive(Clone)]
struct MutexedSender {
    inner: Arc<Mutex<MutexedInner>>,
}

impl Sender for MutexedSender {
    fn send(&self, val: u128) -> Result<(), mpsc::NoRecv<u128>> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.connected {
            Err(mpsc::NoRecv { message: val })
        } else {
            inner.queue.push_back(val);
            Ok(())
        }
    }
}

impl Drop for MutexedSender {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if Arc::strong_count(&self.inner) <= 2 {
            inner.connected = false;
        }
    }
}

struct MutexedReceiver {
    inner: Arc<Mutex<MutexedInner>>,
}

impl Receiver for MutexedReceiver {
    fn recv(&mut self) -> Result<u128, mpsc::RecvErr> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.connected {
            Err(mpsc::NoSender)
        } else if let Some(msg) = inner.queue.pop_front() {
            Ok(msg)
        } else {
            Err(mpsc::NoMessage)
        }
    }
}

impl Drop for MutexedReceiver {
    fn drop(&mut self) {
        self.inner.lock().unwrap().connected = false;
    }
}

struct Std;

impl Channel for Std {
    type Sender = std_mpsc::Sender<u128>;
    type Receiver = std_mpsc::Receiver<u128>;

    fn create() -> (Self::Sender, Self::Receiver) {
        std_mpsc::channel()
    }
}

impl Sender for std_mpsc::Sender<u128> {
    fn send(&self, val: u128) -> Result<(), mpsc::NoRecv<u128>> {
        self.send(val)
            .map_err(|std_mpsc::SendError(message)| mpsc::NoRecv { message })
    }
}

impl Receiver for std_mpsc::Receiver<u128> {
    fn recv(&mut self) -> Result<u128, mpsc::RecvErr> {
        self.try_recv().map_err(|err| match err {
            std_mpsc::TryRecvError::Empty => mpsc::NoMessage,
            std_mpsc::TryRecvError::Disconnected => mpsc::NoSender,
        })
    }
}

fn main() {
    const SAMPLES: usize = 5;
    const NITER: u128 = 0x20000;

    let mut first = true;

    for &nthread in &[2, 4, 8, 16, 32] {
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
            nthread + 1,
            deque
        );
        println!(
            "Std's MPSC with {} threads total time: {:?}",
            nthread + 1,
            std
        );
        println!(
            "Lockfree MPSC with {} threads total time: {:?}",
            nthread + 1,
            lockfree
        );
    }
}
