pub use super::{
    NoRecv,
    RecvErr::{self, *},
};
use crate::ptr::check_null_align;
use owned_alloc::OwnedAlloc;
use std::{
    fmt,
    ptr::{null_mut, NonNull},
    sync::atomic::{AtomicPtr, Ordering::*},
};

/// Creates an asynchronous lock-free Single-Producer-Single-Consumer (SPSC)
/// channel.
pub fn create<T>() -> (Sender<T>, Receiver<T>) {
    check_null_align::<Node<T>>();

    // A single empty node shared between two ends.
    let alloc = OwnedAlloc::new(Node { message: None, next: AtomicPtr::new(null_mut()) });
    let nnptr = alloc.into_raw();

    (Sender { back: nnptr }, Receiver { front: nnptr })
}

/// The `Sender` handle of a SPSC channel. Created by [`create`] function.
pub struct Sender<T> {
    back: NonNull<Node<T>>,
}

impl<T> Sender<T> {
    /// Sends a message and if the receiver disconnected, an error is returned.
    pub fn send(&mut self, message: T) -> Result<(), NoRecv<T>> {
        // First we create a node for our message.
        let alloc =
            OwnedAlloc::new(Node { message: Some(message), next: AtomicPtr::new(null_mut()) });
        let nnptr = alloc.into_raw();

        // This dereferral is safe because the queue will always have at least
        // one node. Our back is a single node. In any case, back will always be
        // present. Also, we only put valid pointers allocated via `OwnedAlloc`.
        let res = unsafe {
            // First we try to publish the new node through the back's next
            // field. The receiver will see our changes because at some point it
            // will reach our current back.
            //
            // We compare to null because, when disconnecting, the receiver will
            // mark the lower bit of the pointer. In order words, it will be
            // null | 1. We do not need to publish the new node if we receiver
            // disconnected.
            self.back.as_ref().next.compare_exchange(null_mut(), nnptr.as_ptr(), Release, Relaxed)
        };

        if res.is_ok() {
            // If we succeeded, let's update our back so we respect the rule of
            // having a single node in the back.
            self.back = nnptr;
            Ok(())
        } else {
            // If we failed, the receiver disconnected and marked the bit.
            let mut alloc = unsafe { OwnedAlloc::from_raw(nnptr) };
            let message = alloc.message.take().unwrap();
            Err(NoRecv { message })
        }
    }

    /// Tests if the [`Receiver`] is still connected. There are no guarantees
    /// that [`send`](Sender::send) will succeed if this method returns `true`
    /// because the [`Receiver`] may disconnect meanwhile.
    pub fn is_connected(&self) -> bool {
        // Safe because we always have at least one node, which is only dropped
        // in the last side to disconnect's drop.
        let back = unsafe { self.back.as_ref() };
        back.next.load(Relaxed).is_null()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // This dereferral is safe because the queue will always have at least
        // one node. Also, we only put nodes allocated from `OwnedAlloc`.
        let res = unsafe {
            // Let's try to mark next's bit so that receiver will see we
            // disconnected, if it hasn't disconnected by itself. It is ok to
            // just swap, since we have only two possible values (null and
            // null | 1) and we everyone will be setting to the same value
            // (null | 1).
            self.back.as_ref().next.swap((null_mut::<Node<T>>() as usize | 1) as *mut _, Relaxed)
        };

        // If the previously stored value was not null, receiver has already
        // disconnected. It is safe to drop because we are the only ones that
        // have a pointer to the node.
        if !res.is_null() {
            unsafe { OwnedAlloc::from_raw(self.back) };
        }
    }
}

unsafe impl<T> Send for Sender<T> where T: Send {}
unsafe impl<T> Sync for Sender<T> where T: Send {}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        fmtr.write_str("spsc::Sender")
    }
}

/// The [`Receiver`] handle of a SPSC channel. Created by [`create`] function.
pub struct Receiver<T> {
    front: NonNull<Node<T>>,
}

impl<T> Receiver<T> {
    /// Tries to receive a message. If no message is available,
    /// [`Err`]`(`[`RecvErr::NoMessage`]`)` is returned. If the sender
    /// disconnected, [`Err`]`(`[`RecvErr::NoSender`]`)` is returned.
    pub fn recv(&mut self) -> Result<T, RecvErr> {
        loop {
            // This dereferral is safe because we only put nodes allocated from
            // `OwnedAlloc`.
            let node = unsafe { &mut *self.front.as_ptr() };

            // We will try to replace the current node with this.
            let next = node.next.load(Acquire);

            // First we remove a node logically.
            match node.message.take() {
                Some(message) => {
                    let cleared = (next as usize & !1) as *mut _;
                    // But only if we have a new node. Otherwise we will not
                    // remove the only node of the queue. Also, let's clear the
                    // bit flag so null pointers are not misused.
                    if let Some(nnptr) = NonNull::new(cleared) {
                        // This is safe because the node was allocated with
                        // `OwnedAlloc` and we have the only pointer to it (back
                        // is something else).
                        unsafe { OwnedAlloc::from_raw(self.front) };
                        self.front = nnptr;
                    }

                    break Ok(message);
                }

                None => {
                    if next as usize & 1 == 0 {
                        // Lower bit clean. Let's try to remove the next.
                        match NonNull::new(next) {
                            Some(nnptr) => {
                                // This is safe because the node was allocated
                                // with `OwnedAlloc` and we have the only
                                // pointer to it (back is something else since
                                // it has a single node).
                                unsafe { OwnedAlloc::from_raw(self.front) };
                                self.front = nnptr;
                            }

                            // If the next is null, we have no message and we
                            // will not remove this list's single node.
                            None => break Err(RecvErr::NoMessage),
                        }
                    } else {
                        // If the sender marked the lower bit of the pointer, it
                        // has disconnected.
                        break Err(RecvErr::NoSender);
                    }
                }
            }
        }
    }

    /// Tests if the [`Sender`] is still connected. There are no guarantees
    /// that [`recv`](Receiver::recv) will succeed if this method returns `true`
    /// because the [`Receiver`] may disconnect meanwhile. This method may
    /// also return `true` if the [`Sender`] disconnected but there are
    /// messages pending in the buffer.
    pub fn is_connected(&self) -> bool {
        // Safe because we always have at least one node, which is only dropped
        // in the last side to disconnect's drop.
        let front = unsafe { self.front.as_ref() };
        front.message.is_some() || front.next.load(Relaxed) as usize & 1 == 0
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        loop {
            // This dereferral is safe because we only put nodes allocated from
            // `OwnedAlloc`.
            let next = unsafe {
                // Let's try to mark next's bit so that sender will see we
                // disconnected, if it hasn't disconnected by itself. It is ok
                // to just swap, since we have only two possible
                // values (null and null | 1) and we everyone
                // will be setting to the same value (null | 1).
                self.front
                    .as_ref()
                    .next
                    .swap((null_mut::<Node<T>>() as usize | 1) as *mut _, Acquire)
            };

            // Then we check for null (success of our swap).
            let next_nnptr = match NonNull::new(next) {
                Some(nnptr) => nnptr,
                // If it was null, no other action is required. We should not
                // deallocate it because the sender still sees it through back.
                None => break,
            };

            // It is safe to drop because we are the only ones that
            // have a pointer to the node.
            unsafe { OwnedAlloc::from_raw(self.front) };

            // if next is marked, it is actually null | 1, but we can deallocate
            // it because the sender already disconnected.
            if next as usize & 1 == 1 {
                break;
            }

            // Update the front just like in pop.
            self.front = next_nnptr;
        }
    }
}

unsafe impl<T> Send for Receiver<T> where T: Send {}
unsafe impl<T> Sync for Receiver<T> where T: Send {}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        fmtr.write_str("spsc::Receiver")
    }
}

#[repr(align(/* at least */ 2))]
struct Node<T> {
    message: Option<T>,
    // lower bit is 1 for "disconnected" and 0 for "connected"
    next: AtomicPtr<Node<T>>,
}

#[cfg(test)]
mod test {
    use crate::channel::spsc;
    use std::thread;

    #[test]
    fn correct_sequence() {
        const MSGS: usize = 512;

        let (mut sender, mut receiver) = spsc::create::<usize>();
        let thread = thread::spawn(move || {
            for i in 0..MSGS {
                loop {
                    match receiver.recv() {
                        Ok(j) => {
                            assert_eq!(i, j);
                            break;
                        }

                        Err(spsc::NoMessage) => (),

                        _ => unreachable!(),
                    }
                }
            }
        });

        for i in 0..MSGS {
            sender.send(i).unwrap();
        }

        thread.join().unwrap();
    }
}
