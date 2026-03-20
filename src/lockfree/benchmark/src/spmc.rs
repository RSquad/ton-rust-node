extern crate lockfree;

use lockfree::channel::spmc;
use std::{
    collections::VecDeque,
    sync::{mpsc as std_mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn measure<C>(receivers: usize, niter: u128) -> Duration
where
    C: Channel,
{
    let (mut sender, receiver) = C::create();
    let then = Instant::now();
    let mut threads = Vec::with_capacity(receivers);

    for _ in 0 .. receivers {
        let receiver = receiver.clone();
        threads.push(thread::spawn(move || {
            let mut idle = 0u128;
            let mut res = 0u128;
            loop {
                match receiver.recv() {
                    Ok(val) => res = res.wrapping_add(val),
                    Err(spmc::NoSender) => break (idle, res),
                    Err(spmc::NoMessage) => idle = idle.wrapping_add(1),
                }
            }
        }))
    }

    for i in 0 .. niter {
        sender.send(i).unwrap();
    }

    drop(sender);

    for thread in threads {
        thread.join().unwrap();
    }

    then.elapsed()
}

trait Channel {
    type Sender: Sender + Send + 'static;
    type Receiver: Clone + Receiver + Send + 'static;

    fn create() -> (Self::Sender, Self::Receiver);
}

trait Sender {
    fn send(&mut self, val: u128) -> Result<(), spmc::NoRecv<u128>>;
}

trait Receiver {
    fn recv(&self) -> Result<u128, spmc::RecvErr>;
}

struct Lockfree;

impl Channel for Lockfree {
    type Sender = spmc::Sender<u128>;
    type Receiver = spmc::Receiver<u128>;

    fn create() -> (Self::Sender, Self::Receiver) {
        spmc::create()
    }
}

impl Sender for spmc::Sender<u128> {
    fn send(&mut self, val: u128) -> Result<(), spmc::NoRecv<u128>> {
        self.send(val)
    }
}

impl Receiver for spmc::Receiver<u128> {
    fn recv(&self) -> Result<u128, spmc::RecvErr> {
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

struct MutexedSender {
    inner: Arc<Mutex<MutexedInner>>,
}

impl Sender for MutexedSender {
    fn send(&mut self, val: u128) -> Result<(), spmc::NoRecv<u128>> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.connected {
            Err(spmc::NoRecv { message: val })
        } else {
            inner.queue.push_back(val);
            Ok(())
        }
    }
}

impl Drop for MutexedSender {
    fn drop(&mut self) {
        self.inner.lock().unwrap().connected = false;
    }
}

#[derive(Clone)]
struct MutexedReceiver {
    inner: Arc<Mutex<MutexedInner>>,
}

impl Receiver for MutexedReceiver {
    fn recv(&self) -> Result<u128, spmc::RecvErr> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.connected {
            Err(spmc::NoSender)
        } else if let Some(msg) = inner.queue.pop_front() {
            Ok(msg)
        } else {
            Err(spmc::NoMessage)
        }
    }
}

impl Drop for MutexedReceiver {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if Arc::strong_count(&self.inner) <= 2 {
            inner.connected = false;
        }
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
    fn send(&mut self, val: u128) -> Result<(), spmc::NoRecv<u128>> {
        (&*self)
            .send(val)
            .map_err(|std_mpsc::SendError(message)| spmc::NoRecv { message })
    }
}

impl Receiver for Arc<Mutex<std_mpsc::Receiver<u128>>> {
    fn recv(&self) -> Result<u128, spmc::RecvErr> {
        self.lock().unwrap().try_recv().map_err(|err| match err {
            std_mpsc::TryRecvError::Empty => spmc::NoMessage,
            std_mpsc::TryRecvError::Disconnected => spmc::NoSender,
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
            "Mutexed Std's MPSC (as SPMC) with {} threads total time: {:?}",
            nthread + 1,
            std
        );
        println!(
            "Lockfree SPMC with {} threads total time: {:?}",
            nthread + 1,
            lockfree
        );
    }
}
