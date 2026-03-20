#![no_main]
#[macro_use]
extern crate libfuzzer_sys;
extern crate fuzzsuite;
extern crate lockfree;

use fuzzsuite::*;
use lockfree::prelude::*;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
struct QueueMachine {
    queue: Arc<Queue<Box<u8>>>,
}

impl Spawn for QueueMachine {
    fn spawn() -> Self {
        Self::default()
    }

    fn fork(&self) -> Self {
        self.clone()
    }
}

impl Machine for QueueMachine {
    fn interpret(&mut self, mut byte: u8, bytecode: &mut Bytecode) {
        loop {
            match byte % 4 {
                0 => byte = self.queue.pop().map_or(1, |x| *x),

                1 => {
                    self.queue.pop();
                    break;
                },

                2 | 3 => {
                    let val = bytecode.next().unwrap_or(0);
                    self.queue.push(Box::new(val));
                    break;
                },

                _ => unreachable!(),
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = test::<QueueMachine>(Bytecode::no_symbols(data));
});
