extern crate lockfree;

use lockfree::queue::Queue as LfQueue;
use std::{
    collections::{LinkedList, VecDeque},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn measure<Q>(nthread: usize, niter: u128) -> Duration
where
    Q: Queue + Send + Sync + 'static,
{
    let queue = Arc::new(Q::default());
    let then = Instant::now();
    let mut threads = Vec::with_capacity(nthread);

    for i in 0 .. nthread {
        let queue = queue.clone();
        threads.push(thread::spawn(move || {
            let start = i as u128 * niter / nthread as u128;
            let end = if i + 1 == nthread {
                niter
            } else {
                (i as u128 + 1) * niter / nthread as u128
            };

            for j in start .. end {
                let popped = queue.pop();
                queue.push(j);
                queue.push(i as u128 + j);
                if let Some(num) = popped {
                    queue.push(num + j);
                }
            }

            while let Some(_) = queue.pop() {}
        }))
    }

    for thread in threads {
        thread.join().expect("thread failed");
    }

    then.elapsed()
}

trait Queue: Default {
    fn push(&self, num: u128);
    fn pop(&self) -> Option<u128>;
}

impl Queue for LfQueue<u128> {
    fn push(&self, num: u128) {
        self.push(num)
    }

    fn pop(&self) -> Option<u128> {
        self.pop()
    }
}

impl Queue for Mutex<VecDeque<u128>> {
    fn push(&self, num: u128) {
        self.lock().unwrap().push_back(num)
    }

    fn pop(&self) -> Option<u128> {
        self.lock().unwrap().pop_front()
    }
}

impl Queue for Mutex<LinkedList<u128>> {
    fn push(&self, num: u128) {
        self.lock().unwrap().push_back(num)
    }

    fn pop(&self) -> Option<u128> {
        self.lock().unwrap().pop_front()
    }
}

fn main() {
    println!(
        "A program which reverberates messages through a plain queue channel"
    );

    const SAMPLES: usize = 5;
    const NITER: u128 = 0x10000;

    for &nthread in &[2, 4, 8, 16] {
        println!();

        let mut deque = Duration::default();
        let mut linked = Duration::default();
        let mut lockfree = Duration::default();

        for _ in 0 .. SAMPLES {
            deque += measure::<Mutex<VecDeque<_>>>(nthread, NITER);
            linked += measure::<Mutex<LinkedList<_>>>(nthread, NITER);
            lockfree += measure::<LfQueue<_>>(nthread, NITER);
        }

        println!(
            "Mutexed VecDeque with {} threads total time: {:?}",
            nthread, deque
        );
        println!(
            "Mutexed LinkedList with {} threads total time: {:?}",
            nthread, linked
        );
        println!(
            "Lockfree Queue with {} threads total time: {:?}",
            nthread, lockfree
        );
    }
}
