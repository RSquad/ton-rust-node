#[macro_use]
extern crate lazy_static;
extern crate lockfree;
extern crate owned_alloc;

pub mod thread;

use std::sync::Arc;

pub trait Spawn: Machine {
    fn spawn() -> Self;

    fn fork(&self) -> Self;
}

pub trait Machine: Send + Sync + 'static {
    fn interpret(&mut self, byte: u8, bytecode: &mut Bytecode);

    fn run(&mut self, bytecode: &mut Bytecode) {
        while let Some(byte) = bytecode.next() {
            self.interpret(byte, bytecode)
        }
    }
}

pub fn test<T>(mut bytecode: Bytecode)
where
    T: Spawn,
{
    MainThread::<T>::spawn().run(&mut bytecode);
}

#[derive(Clone, Debug)]
pub struct Bytecode {
    data: Arc<[u8]>,
    ip: usize,
    sym_size: usize,
}

impl Bytecode {
    pub fn new(fuzz: &[u8]) -> Self {
        if fuzz.len() < 512 {
            let mut i = fuzz.last().map_or(0, |&x| x) as usize;
            let mut buf = Vec::from(fuzz);
            buf.reserve(512 - fuzz.len());
            while buf.len() < 512 {
                buf.push(i as u8);
                i += 1;
            }
            Self { data: buf.into(), ip: 0, sym_size: 1 }
        } else {
            Self { data: fuzz.into(), ip: 0, sym_size: fuzz.len() / 512 }
        }
    }

    pub fn no_symbols(fuzz: &[u8]) -> Self {
        Self { data: fuzz.into(), ip: 0, sym_size: 0 }
    }

    pub fn code_seg(&self) -> &[u8] {
        &self.data[self.sym_size * 256 ..]
    }

    pub fn symbol(&self, index: u8) -> &[u8] {
        let start = self.sym_size * index as usize;
        &self.data[start .. start + self.sym_size]
    }

    pub fn next(&mut self) -> Option<u8> {
        if let Some(&byte) = self.code_seg().get(self.ip) {
            self.ip += 1;
            Some(byte)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct MainThread<T>
where
    T: Spawn,
{
    threads: Vec<thread::JoinHandle<()>>,
    machine: T,
}

impl<T> MainThread<T>
where
    T: Spawn,
{
    pub fn new(machine: T) -> Self {
        Self { machine, threads: Vec::new() }
    }
}

impl<T> Spawn for MainThread<T>
where
    T: Spawn,
{
    fn spawn() -> Self {
        Self::new(T::spawn())
    }

    fn fork(&self) -> Self {
        Self::new(self.machine.fork())
    }
}

impl<T> Machine for MainThread<T>
where
    T: Spawn,
{
    fn interpret(&mut self, byte: u8, bytecode: &mut Bytecode) {
        match byte {
            // let's not blow up the thread limit
            128 if self.threads.len() < 256 => {
                let mut new = self.machine.fork();
                let mut bytecode = bytecode.clone();
                self.threads.push(thread::spawn(move || {
                    new.run(&mut bytecode);
                }))
            },

            129 | 57 => {
                if let Some(thread) = self.threads.pop() {
                    thread.join().unwrap()
                }
            },

            _ => {
                let new_op = bytecode.next().unwrap_or(1);
                let byte = byte.wrapping_mul(new_op) ^ byte;
                self.machine.interpret(byte, bytecode);
            },
        }
    }
}

impl<T> Drop for MainThread<T>
where
    T: Spawn,
{
    fn drop(&mut self) {
        while let Some(thread) = self.threads.pop() {
            thread.join().unwrap();
        }
    }
}
