#![warn(missing_docs)]
//! A crate providing lock-free data structures and a solution for the "ABA
//! problem" related to pointers.
//!
//! The incinerator is the API which tries to solve the "ABA problem" when
//! related to pointer dropping. With incinerator, every thread has a local
//! garbage list. Dropping a shared object consist of first removing the pointer
//! from the shared context, then adding the pointer to the garbage list.
//! A "pause counter" is checked. If the counter is zero, then the whole list
//! is deleted, otherwise, the list will only be deleted later.
//!
//! This counter is counting how many times the incinerator was asked to
//! "pause". A thread may pause the incinerator to load and use the shared
//! pointer, and this is why it is important to remove the pointer from the
//! shared context before deleting. Previous version of lockfree used a global
//! incinerator. Currently, a per-object incinerator is used.
//!
//! This crate is under development, and there are plans for some structures.
//! We have:
//! - `[x]` [Per-Object Thread-Local Storage](tls::ThreadLocal)
//! - `[x]` [Channels (SPSC, MPSC, SPMC, MPMC)](channel)
//! - `[x]` [Map](map::Map)
//! - `[x]` [Set](set::Set)
//! - `[x]` [Stack](stack::Stack)
//! - `[x]` [Queue](queue::Queue)
//! - `[ ]` Deque
//!
//! # Performance Guide
//! In order to achieve a better time performance with lockfree, it is
//! recommended to avoid global locking stuff like heap allocation.

extern crate owned_alloc;

/// Provides convenient re-exports.
pub mod prelude;

/// Incinerator API. The purpouse of this module is to solve the "ABA problem"
/// related to pointers while still being lock-free. See documentation of the
/// inner type for more details.
#[macro_use]
pub mod incin;

/// A wait-free per-object Thread Local Storage (TLS).
pub mod tls;

/// A lock-free queue.
pub mod queue;

/// A lock-free stack.
pub mod stack;

/// A lock-free map.
pub mod map;

/// A lock-free set.
pub mod set;

/// Collection of lock-free FIFO channels. These channels are fully asynchronous
/// and their receivers do not provide any sort of `wait-for-message` operation.
/// It would be blocking otherwise, thus not lock-free. If you need such a
/// mechanism, consider using this channel with a
/// [`Condvar`](std::sync::Condvar) or using things like
/// [`thread::park`](std::thread::park) (not lock-free).
pub mod channel;

/// A shared removable value. No extra allocation is necessary.
pub mod removable;

#[allow(dead_code)]
mod ptr;
