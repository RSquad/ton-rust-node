extern crate lockfree;

use lockfree::channel::spsc;
use std::{
    collections::VecDeque,
    sync::{mpsc as std_mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn measure<C>(niter: u128) -> Duration
where
    C: Channel,
{
    let (mut sender, mut receiver) = C::create();
    let then = Instant::now();
    let thread = thread::spawn(move || {
        let mut idle = 0u128;
        let mut res = 0u128;
        loop {
            match receiver.recv() {
                Ok(val) => res = res.wrapping_add(val),
                Err(spsc::NoSender) => break (idle, res),
                Err(spsc::NoMessage) => idle = idle.wrapping_add(1),
            }
        }
    });

    for i in 0 .. niter {
        sender.send(i).unwrap();
    }

    drop(sender);

    thread.join().unwrap();

    then.elapsed()
}

trait Channel {
    type Sender: Sender + Send + 'static;
    type Receiver: Receiver + Send + 'static;

    fn create() -> (Self::Sender, Self::Receiver);
}

trait Sender {
    fn send(&mut self, val: u128) -> Result<(), spsc::NoRecv<u128>>;
}

trait Receiver {
    fn recv(&mut self) -> Result<u128, spsc::RecvErr>;
}

struct Lockfree;

impl Channel for Lockfree {
    type Sender = spsc::Sender<u128>;
    type Receiver = spsc::Receiver<u128>;

    fn create() -> (Self::Sender, Self::Receiver) {
        spsc::create()
    }
}

impl Sender for spsc::Sender<u128> {
    fn send(&mut self, val: u128) -> Result<(), spsc::NoRecv<u128>> {
        self.send(val)
    }
}

impl Receiver for spsc::Receiver<u128> {
    fn recv(&mut self) -> Result<u128, spsc::RecvErr> {
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
    fn send(&mut self, val: u128) -> Result<(), spsc::NoRecv<u128>> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.connected {
            Err(spsc::NoRecv { message: val })
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

struct MutexedReceiver {
    inner: Arc<Mutex<MutexedInner>>,
}

impl Receiver for MutexedReceiver {
    fn recv(&mut self) -> Result<u128, spsc::RecvErr> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.connected {
            Err(spsc::NoSender)
        } else if let Some(msg) = inner.queue.pop_front() {
            Ok(msg)
        } else {
            Err(spsc::NoMessage)
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
    fn send(&mut self, val: u128) -> Result<(), spsc::NoRecv<u128>> {
        (&*self)
            .send(val)
            .map_err(|std_mpsc::SendError(message)| spsc::NoRecv { message })
    }
}

impl Receiver for std_mpsc::Receiver<u128> {
    fn recv(&mut self) -> Result<u128, spsc::RecvErr> {
        self.try_recv().map_err(|err| match err {
            std_mpsc::TryRecvError::Empty => spsc::NoMessage,
            std_mpsc::TryRecvError::Disconnected => spsc::NoSender,
        })
    }
}

fn main() {
    const SAMPLES: usize = 5;
    const NITER: u128 = 0x80000;

    let mut deque = Duration::default();
    let mut std = Duration::default();
    let mut lockfree = Duration::default();

    for _ in 0 .. SAMPLES {
        deque += measure::<Mutexed>(NITER);
        std += measure::<Std>(NITER);
        lockfree += measure::<Lockfree>(NITER);
    }

    println!("Mutexed VecDeque total time: {:?}", deque);
    println!("Std's MPSC (as SPSC) total time: {:?}", std);
    println!("Lockfree SPSC total time: {:?}", lockfree);
}
