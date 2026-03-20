#[macro_use]
extern crate benchsuite;
extern crate lockfree;

use benchsuite::exec::Target;
use lockfree::stack::Stack;
use std::{
    collections::LinkedList,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Default)]
struct MutexVecTarget {
    inner: Arc<Mutex<Vec<u8>>>,
}

#[derive(Debug, Clone, Default)]
struct MutexListTarget {
    inner: Arc<Mutex<LinkedList<u8>>>,
}

#[derive(Debug, Clone, Default)]
struct LockfreeTarget {
    inner: Arc<Stack<u8>>,
}

impl Target for MutexVecTarget {
    fn round(&mut self) {
        let mut stack = self.inner.lock().unwrap();
        stack.pop();
        stack.push(234);
    }
}

impl Target for MutexListTarget {
    fn round(&mut self) {
        let mut stack = self.inner.lock().unwrap();
        stack.pop_front();
        stack.push_front(234);
    }
}

impl Target for LockfreeTarget {
    fn round(&mut self) {
        self.inner.pop();
        self.inner.push(234);
    }
}

fn main() {
    bench! {
        levels 1, 2, 4, 8, 16;
        "mutex vector" => MutexVecTarget::default(),
        "mutex linked list" => MutexListTarget::default(),
        "lockfree" => LockfreeTarget::default(),
    }
}
