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
    children: Vec<thread::JoinHandle<()>>,
    sender: Option<spsc::Sender<Box<u8>>>,
    receiver: spsc::Receiver<Box<u8>>,
    state: u8,
}

impl Spawn for SubVm {
    fn spawn() -> Self {
        let (sender, receiver) = spsc::create();
        Self { children: Vec::new(), sender: Some(sender), receiver, state: 0 }
    }

    fn fork(&self) -> Self {
        let mut this = Self::spawn();
        this.state = self.state;
        this
    }
}

impl Machine for SubVm {
    fn interpret(&mut self, byte: u8, bytecode: &mut Bytecode) {
        match byte % 7 {
            0 | 3 | 4 | 6 => match self.receiver.recv() {
                Ok(i) => self.state = self.state.wrapping_add(*i),
                _ => (),
            },

            1 | 5 => {
                if self.children.len() == MAX_THREADS_PER_SUB_VM {
                    return ();
                }

                let mut sender = match self.sender.take() {
                    Some(sender) => sender,
                    None => {
                        let (sender, receiver) = spsc::create();
                        self.receiver = receiver;
                        sender
                    },
                };

                let mut bytecode = bytecode.clone();
                let state = self.state;
                self.children.push(thread::spawn(move || {
                    let mut vm = SenderVm { sender, state, end: false };
                    vm.run(&mut bytecode);
                }))
            },

            2 => {
                if let Some(thread) = self.children.pop() {
                    thread.join().unwrap()
                }
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
struct SenderVm {
    sender: spsc::Sender<Box<u8>>,
    state: u8,
    end: bool,
}

impl Machine for SenderVm {
    #[allow(unused_must_use)]
    fn interpret(&mut self, byte: u8, _bytecode: &mut Bytecode) {
        match byte % 4 {
            0 | 1 | 3 => {
                self.sender.send(Box::new(self.state));
                self.state = self.state.wrapping_add(1);
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
