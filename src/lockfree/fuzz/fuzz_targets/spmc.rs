#![no_main]
#[macro_use]
extern crate libfuzzer_sys;
extern crate fuzzsuite;
extern crate lockfree;

use fuzzsuite::*;
use lockfree::prelude::*;

const MAX_THREADS_PER_SUB_VM: usize = 64;

#[derive(Debug)]
struct SubVm {
    children: Vec<thread::JoinHandle<u8>>,
    sender: spmc::Sender<Box<u8>>,
    receiver: spmc::Receiver<Box<u8>>,
    state: u8,
}

impl Spawn for SubVm {
    fn spawn() -> Self {
        let (sender, receiver) = spmc::create();
        Self { children: Vec::new(), sender, receiver, state: 0 }
    }

    fn fork(&self) -> Self {
        let mut this = Self::spawn();
        this.state = self.state;
        this
    }
}

impl Machine for SubVm {
    #[allow(unused_must_use)]
    fn interpret(&mut self, byte: u8, bytecode: &mut Bytecode) {
        match byte % 7 {
            0 | 3 | 4 | 6 => {
                self.sender.send(Box::new(self.state));
                self.state = self.state.wrapping_add(1);
            },

            1 => {
                if self.children.len() == MAX_THREADS_PER_SUB_VM {
                    return ();
                }

                let receiver = self.receiver.clone();
                let mut bytecode = bytecode.clone();
                let state = self.state;
                self.children.push(thread::spawn(move || {
                    let mut vm = ReceiverVm { receiver, state, end: false };
                    vm.run(&mut bytecode);
                    vm.state
                }))
            },

            2 => {
                if let Some(thread) = self.children.pop() {
                    self.state = self.state.wrapping_add(thread.join().unwrap())
                }
            },

            5 => {
                let (sender, receiver) = spmc::create();
                self.sender = sender;
                self.receiver = receiver;
            },

            _ => unreachable!(),
        }
    }
}

impl Drop for SubVm {
    fn drop(&mut self) {
        while let Some(thread) = self.children.pop() {
            thread.join().unwrap();
        }
    }
}

#[derive(Debug)]
struct ReceiverVm {
    receiver: spmc::Receiver<Box<u8>>,
    state: u8,
    end: bool,
}

impl Machine for ReceiverVm {
    #[allow(unused_must_use)]
    fn interpret(&mut self, byte: u8, _bytecode: &mut Bytecode) {
        match byte % 4 {
            0 | 1 | 3 => match self.receiver.recv() {
                Ok(i) => self.state = self.state.wrapping_add(*i),
                Err(spmc::NoMessage) => self.state = self.state.wrapping_sub(1),
                Err(_) => self.end = true,
            },

            2 => self.end = true,

            _ => unreachable!(),
        }
    }

    fn run(&mut self, bytecode: &mut Bytecode) {
        while let Some(byte) = bytecode.next().filter(|_| !self.end) {
            self.interpret(byte, bytecode)
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = test::<SubVm>(Bytecode::no_symbols(data));
});
