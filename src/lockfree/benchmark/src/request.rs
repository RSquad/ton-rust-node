extern crate lockfree;

use lockfree::{channel::spmc, map::Map as LfMap};
use std::{
    collections::HashMap,
    fmt::Write,
    mem::uninitialized,
    sync::{mpsc as std_mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn prevent_opt<T>(val: T) {
    unsafe {
        let mut _local = uninitialized();
        (&mut _local as *mut T).write_volatile(val);
    }
}

#[derive(Debug)]
enum Request {
    Get(Arc<str>),
    Put(Arc<str>, Arc<str>),
    Delete(Arc<str>),
}

fn measure<C, M>(
    nthread: usize,
    step1: usize,
    step2: usize,
    nest1: usize,
    nest2: usize,
    sleep1: u64,
    sleep2: u64,
) -> Duration
where
    C: Channel,
    M: Map + Send + Sync + 'static,
{
    let mut threads = Vec::with_capacity(nthread);
    let map = Arc::new(M::default());
    let (mut sender, receiver) = C::create();

    for _ in 0 .. nthread {
        let map = map.clone();
        let receiver = receiver.clone();
        let joiner = thread::spawn(move || {
            while let Some(request) = receiver.wait() {
                match request {
                    Request::Put(key, val) => map.put(key, val),

                    Request::Get(key) => prevent_opt(map.get(&key)),

                    Request::Delete(key) => prevent_opt(map.delete(&key)),
                }
            }
        });

        threads.push(joiner);
    }

    let then = Instant::now();

    let mut pairs = Vec::with_capacity(step1);
    for i in 0 .. step1 {
        let mut key = String::with_capacity(step1 * nest1 * 2 + 1);
        for j in 0 .. nest1 {
            write!(key, "{}{}", i * j, (i + j) as u8 as char).unwrap();
        }
        let mut val = format!("{}", i);
        pairs.push((key, val));
    }

    while let Some((key, val)) = pairs.pop() {
        let key = Arc::<str>::from(key);
        sender.send(Request::Put(key.clone(), val.into())).unwrap();
        sender.send(Request::Get(key.clone())).unwrap();
        sender.send(Request::Get(key.clone())).unwrap();
        sender.send(Request::Get(key.clone())).unwrap();
        sender.send(Request::Delete(key.clone())).unwrap();
        sender.send(Request::Delete(key)).unwrap();
    }

    thread::sleep(Duration::from_millis(sleep1));

    for i in 0 .. step2 {
        let mut key = String::with_capacity(nest2 * step2 * 2 + 1);
        for j in 0 .. nest2 {
            write!(key, "{}{}", (i + j) as u8 as char, (i + j * 2)).unwrap();
        }
        let mut val = format!("{}", i);
        pairs.push((key, val));
    }

    while let Some((key, val)) = pairs.pop() {
        let key = Arc::<str>::from(key);
        sender.send(Request::Put(key.clone(), val.into())).unwrap();
        sender.send(Request::Get(key.clone())).unwrap();
        sender.send(Request::Get(key.clone())).unwrap();
        sender.send(Request::Delete(key.clone())).unwrap();
        sender.send(Request::Delete(key)).unwrap();
    }

    thread::sleep(Duration::from_millis(sleep2));
    sender.send(Request::Delete(Arc::from("not in the map"))).unwrap();

    drop(sender);

    for thread in threads {
        thread.join().unwrap();
    }

    then.elapsed()
}

trait Map: Default {
    fn put(&self, key: Arc<str>, val: Arc<str>);
    fn get(&self, key: &str) -> Option<Arc<str>>;
    fn delete(&self, key: &str) -> Option<Arc<str>>;
}

impl Map for LfMap<Arc<str>, Arc<str>> {
    fn put(&self, key: Arc<str>, val: Arc<str>) {
        self.insert(key, val);
    }

    fn get(&self, key: &str) -> Option<Arc<str>> {
        self.get(key).map(|guard| guard.key().clone())
    }

    fn delete(&self, key: &str) -> Option<Arc<str>> {
        self.remove(key).map(|rem| rem.key().clone())
    }
}

impl Map for Mutex<HashMap<Arc<str>, Arc<str>>> {
    fn put(&self, key: Arc<str>, val: Arc<str>) {
        self.lock().unwrap().insert(key, val);
    }

    fn get(&self, key: &str) -> Option<Arc<str>> {
        self.lock().unwrap().get(key).map(Clone::clone)
    }

    fn delete(&self, key: &str) -> Option<Arc<str>> {
        self.lock().unwrap().remove(key)
    }
}

trait Channel {
    type Sender: Sender + Send + 'static;
    type Receiver: Clone + Receiver + Send + 'static;

    fn create() -> (Self::Sender, Self::Receiver);
}

trait Sender {
    fn send(&mut self, val: Request) -> Result<(), spmc::NoRecv<Request>>;
}

trait Receiver {
    fn recv(&self) -> Result<Request, spmc::RecvErr>;

    // not really lock-free
    fn wait(&self) -> Option<Request> {
        loop {
            match self.recv() {
                Ok(req) => break Some(req),
                Err(spmc::NoSender) => break None,
                Err(spmc::NoMessage) => {
                    thread::sleep(Duration::from_millis(32))
                },
            }
        }
    }
}

struct Lockfree;

impl Channel for Lockfree {
    type Sender = spmc::Sender<Request>;
    type Receiver = spmc::Receiver<Request>;

    fn create() -> (Self::Sender, Self::Receiver) {
        spmc::create()
    }
}

impl Sender for spmc::Sender<Request> {
    fn send(&mut self, val: Request) -> Result<(), spmc::NoRecv<Request>> {
        self.send(val)
    }
}

impl Receiver for spmc::Receiver<Request> {
    fn recv(&self) -> Result<Request, spmc::RecvErr> {
        self.recv()
    }
}

struct Std;

impl Channel for Std {
    type Sender = std_mpsc::Sender<Request>;
    type Receiver = Arc<Mutex<std_mpsc::Receiver<Request>>>;

    fn create() -> (Self::Sender, Self::Receiver) {
        let (sender, receiver) = std_mpsc::channel();
        (sender, Arc::new(Mutex::new(receiver)))
    }
}

impl Sender for std_mpsc::Sender<Request> {
    fn send(&mut self, val: Request) -> Result<(), spmc::NoRecv<Request>> {
        (&*self)
            .send(val)
            .map_err(|std_mpsc::SendError(message)| spmc::NoRecv { message })
    }
}

impl Receiver for Arc<Mutex<std_mpsc::Receiver<Request>>> {
    fn recv(&self) -> Result<Request, spmc::RecvErr> {
        self.lock().unwrap().try_recv().map_err(|err| match err {
            std_mpsc::TryRecvError::Empty => spmc::NoMessage,
            std_mpsc::TryRecvError::Disconnected => spmc::NoSender,
        })
    }

    fn wait(&self) -> Option<Request> {
        self.lock().unwrap().recv().ok()
    }
}

fn main() {
    println!("A program simulating a concurrent server.");

    const STEP1: usize = 0x10000;
    const STEP2: usize = 0x8000;
    const NEST1: usize = 10;
    const NEST2: usize = 20;
    const SLEEP1: u64 = 62;
    const SLEEP2: u64 = 2;

    for &nthread in &[2, 4, 8, 16] {
        println!();

        let std = measure::<Std, Mutex<_>>(
            nthread, STEP1, STEP2, NEST1, NEST2, SLEEP1, SLEEP2,
        );
        let lockfree = measure::<Lockfree, LfMap<_, _>>(
            nthread, STEP1, STEP2, NEST1, NEST2, SLEEP1, SLEEP2,
        );

        println!(
            "Mutexed HashMap and Std's MPSC with {} threads total time: {:?}",
            nthread, std
        );
        println!(
            "Lockfree structures with {} threads total time: {:?}",
            nthread, lockfree
        );
    }
}
