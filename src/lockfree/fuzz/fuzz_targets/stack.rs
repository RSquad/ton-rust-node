#![no_main]
#[macro_use]
extern crate libfuzzer_sys;
extern crate fuzzsuite;
extern crate lockfree;

use fuzzsuite::*;
use lockfree::prelude::*;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
struct StackMachine {
    stack: Arc<Stack<Box<u8>>>,
}

impl Spawn for StackMachine {
    fn spawn() -> Self {
        Self::default()
    }

    fn fork(&self) -> Self {
        self.clone()
    }
}

impl Machine for StackMachine {
    fn interpret(&mut self, mut byte: u8, bytecode: &mut Bytecode) {
        loop {
            match byte % 4 {
                0 => byte = self.stack.pop().map_or(1, |x| *x),

                1 => {
                    self.stack.pop();
                    break;
                },

                2 | 3 => {
                    let val = bytecode.next().unwrap_or(0);
                    self.stack.push(Box::new(val));
                    break;
                },

                _ => unreachable!(),
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = test::<StackMachine>(Bytecode::no_symbols(data));
});
