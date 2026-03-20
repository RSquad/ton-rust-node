pub use super::{
    NoRecv,
    RecvErr::{self, *},
};
use crate::ptr::{bypass_null, check_null_align};
use owned_alloc::OwnedAlloc;
use std::{
    fmt,
    ptr::{null_mut, NonNull},
    sync::{
        atomic::{AtomicPtr, Ordering::*},
        Arc,
    },
};

/// Creates an asynchronous lock-free Multi-Producer-Single-Consumer (MPSC)
/// channel. In order to allow multiple producers, [`Sender`] is clonable and
/// does not require mutability.
pub fn create<T>() -> (Sender<T>, Receiver<T>) {
    check_null_align::<Node<T>>();

    // A single empty node shared between two ends.
    let alloc = OwnedAlloc::new(Node { message: None, next: AtomicPtr::new(null_mut()) });
    let single_node = alloc.into_raw();

    // Also, we share a pointer to an atomic pointer to a node. This is because
    // we mark the atomic pointer.
    let shared = SharedBack { ptr: AtomicPtr::new(single_node.as_ptr()) };
    let alloc = OwnedAlloc::new(shared);
    let back = alloc.into_raw();

    // Sender with an Arc because it is shared.
    let sender = Sender { inner: Arc::new(SenderInner { back }) };
    let receiver = Receiver { back, front: single_node };

    (sender, receiver)
}

/// The [`Sender`] handle of a MPSC channel. Created by [`create`] function. It
/// is clonable and does not require mutability.
pub struct Sender<T> {
    inner: Arc<SenderInner<T>>,
}

impl<T> Sender<T> {
    /// Sends a message and if the receiver disconnected, an error is returned.
    pub fn send(&self, message: T) -> Result<(), NoRecv<T>> {
        // First we create a node with our message.
        let alloc =
            OwnedAlloc::new(Node { message: Some(message), next: AtomicPtr::new(null_mut()) });
        let node = alloc.into_raw();

        // We first load the back because we need to check it. This is safe
        // because we only store nodes allocated via `OwnedAlloc`. Also, the
        // shared back is only deallocated when both sides disconnected.
        let mut loaded = unsafe { self.inner.back.as_ref().ptr.load(Relaxed) };

        loop {
            // If the lower bit is marked, it means the receiver disconnected.
            if loaded as usize & 1 == 1 {
                // This is safe because we are only recreating the owned
                // allocation for the node we just created. We did not share the
                // node.
                let mut alloc = unsafe { OwnedAlloc::from_raw(node) };
                let message = alloc.message.take().unwrap();
                break Err(NoRecv { message });
            }

            // This is safe because we only store nodes allocated via
            // `OwnedAlloc`. Also, the node is only deallocated when
            // both sides disconnected.
            let res = unsafe {
                // Then we try to update the back.
                self.inner.back.as_ref().ptr.compare_exchange(
                    loaded,
                    node.as_ptr(),
                    AcqRel,
                    Relaxed,
                )
            };

            match res {
                Ok(_) => {
                    debug_assert!(!loaded.is_null());
                    // This is safe because we never store null on the back.
                    let prev = unsafe { NonNull::new_unchecked(loaded) };
                    // This is safe because, in the receiver's view, this is a
                    // node shared between front and back. The front won't
                    // deallocate the node. Of course, after we update the
                    // previous back's next field, this won't be true anymore.
                    let res = unsafe {
                        // The next field is expected to be null. If it is not
                        // null, the receiver marked it
                        // (it will be null | 1).
                        prev.as_ref().next.swap(node.as_ptr(), Release)
                    };

                    // If it was not null, then it means the receiver
                    // disconnected. It marks it so we know we need to throw the
                    // nodes away. All nodes except the last, which back holds,
                    // need to be deleted. We might be the last `send` that
                    // succeeded.
                    if !res.is_null() {
                        // This is safe because the receiver will not access
                        // this node anymore. Also, we are the only sender's
                        // thread which can access it.
                        //
                        // We also don't have a known back (second argument of
                        // `delete_before_last` is `None`). This is ok because
                        // the senders are the only ones with access to the
                        // back, which will be dropped only when all senders
                        // disconnect.
                        unsafe {
                            OwnedAlloc::from_raw(prev);
                            delete_before_last(node, None);
                        }
                    }

                    break Ok(());
                }

                Err(new) => loaded = new,
            }
        }
    }

    /// Tests if the [`Receiver`] is still connected. There are no guarantees
    /// that [`send`](Sender::send) will succeed if this method returns `true`
    /// because the [`Receiver`] may disconnect meanwhile.
    pub fn is_connected(&self) -> bool {
        // This is safe because we only store nodes allocated via
        // `OwnedAlloc`. Also, the shared back is only deallocated
        // when both sides disconnected. We load it to check for bit
        // marking (since it means sender disconnected).
        let back = unsafe { self.inner.back.as_ref() };
        back.ptr.load(Relaxed) as usize & 1 == 0
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

unsafe impl<T> Send for Sender<T> where T: Send {}
unsafe impl<T> Sync for Sender<T> where T: Send {}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "mpsc::Sender {{ ptr: {:p} }}", self.inner)
    }
}

/// The [`Receiver`] handle of a MPSC channel. Created by [`create`] function.
pub struct Receiver<T> {
    back: NonNull<SharedBack<T>>,
    front: NonNull<Node<T>>,
}

impl<T> Receiver<T> {
    /// Tries to receive a message. If no message is available,
    /// [`Err`]`(`[`RecvErr::NoMessage`]`)` is returned. If the sender
    /// disconnected, [`Err`]`(`[`RecvErr::NoSender`]`)` is returned.
    pub fn recv(&mut self) -> Result<T, RecvErr> {
        // This is safe because we only store nodes allocated via `OwnedAlloc`.
        // We are also the only ones with access to front and... The queue will
        // always have at least one node. The senders will not delete it. We are
        // also the only receiver.
        let mut node = unsafe { &mut *self.front.as_ptr() };
        loop {
            // Let's see what is in the next node of the front.
            let next = node.next.load(Acquire);

            // Then we remove logicaly.
            match node.message.take() {
                Some(message) => {
                    // No need to clear the lower bit since the receiver is the
                    // only one that marks next field.
                    if let Some(nnptr) = NonNull::new(next) {
                        // This is safe because we only store nodes allocated
                        // via `OwnedAlloc`. The queue will always have at
                        // least one node, but if the next field was not null,
                        // this is not the only node.
                        unsafe {
                            OwnedAlloc::from_raw(self.front);
                        };
                        // Setting the front to the next pointer.
                        self.front = nnptr;
                    }

                    break Ok(message);
                }

                None => {
                    match NonNull::new(next) {
                        // It is null.
                        None => {
                            // This is safe because we only store nodes
                            // allocated via
                            // `OwnedAlloc`. Also, the shared back is only
                            // deallocated
                            // when both sides disconnected. We load it to check
                            // for bit
                            // marking (since it means sender disconnected).
                            let back = unsafe { self.back.as_ref().ptr.load(Relaxed) as usize };

                            break if back & 1 == 0 || (back & !1) as *mut _ != self.front.as_ptr() {
                                // If back is not marked, we just don't have
                                // messages.
                                Err(RecvErr::NoMessage)
                            } else {
                                // Back is marked, sender disconnected.
                                Err(RecvErr::NoSender)
                            };
                        }

                        Some(nnptr) => {
                            // This is safe because we only store nodes
                            // allocated via `OwnedAlloc`. We are also the only
                            // ones with access to front and... The queue will
                            // always have at least one node. The senders will
                            // not delete it. We are also the only receiver.
                            //
                            // Also, if front has a successor, this means the
                            // queue is not empty, the back does not have access
                            // to the front, and thus it is safe to delete it.
                            unsafe {
                                node = &mut *nnptr.as_ptr();
                                OwnedAlloc::from_raw(self.front);
                            };
                            // Update our front to its successor. And let's try
                            // again.
                            self.front = nnptr;
                        }
                    }
                }
            }
        }
    }

    /// Tests if there any [`Sender`]s still connected. There are no guarantees
    /// that [`recv`](Receiver::recv) will succeed if this method returns `true`
    /// because the [`Receiver`] may disconnect meanwhile. This method may
    /// also return `true` if the [`Sender`] disconnected but there are
    /// messages pending in the buffer.
    pub fn is_connected(&self) -> bool {
        // Safe because we always have at least one node, which is only dropped
        // in the last side to disconnect's drop.
        let front = unsafe { self.front.as_ref() };
        // This is safe because we only store nodes allocated via
        // `OwnedAlloc`. Also, the shared back is only deallocated
        // when both sides disconnected. We load it to check for bit
        // marking (since it means sender disconnected).
        let back = unsafe { self.back.as_ref() };
        back.ptr.load(Acquire) as usize & 1 == 0
            || front.message.is_some()
            || !front.next.load(Acquire).is_null()
    }

    // This is unsafe because some conditions need to be met. Senders must have
    // disconnected.
    unsafe fn delete_all(&mut self) {
        let mut node_ptr = Some(self.front);

        while let Some(mut node) = node_ptr {
            node_ptr = NonNull::new(node.as_mut().next.load(Acquire));
            OwnedAlloc::from_raw(node);
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        // This is safe because when senders disconnect, they won't drop the
        // back. The shared back is only deleted when both sides disconnect.
        // And we are the only receiver.
        //
        // Let's check if sender disconnected.
        let mut ptr = unsafe { self.back.as_ref().ptr.load(Relaxed) };
        loop {
            // Bit is marked, sender disconnected.
            if ptr as usize & 1 == 1 {
                // Safe to delete all nodes because sender disconnected and we
                // are the only receiver.
                //
                // Same thing about deleting the shared back (a pointer to a
                // pointer).
                unsafe {
                    self.delete_all();
                    OwnedAlloc::from_raw(self.back);
                }
                break;
            }

            // This is safe because we only store nodes allocated via
            // `OwnedAlloc`. Also, the shared back is only deallocated when both
            // sides have disconnected.
            let res = unsafe {
                // Let's try to mark the back. Needs to be a CAS because the
                // back might change the back to some other pointer it
                // meanwhile.
                self.back.as_ref().ptr.compare_exchange(
                    ptr,
                    (ptr as usize | 1) as *mut _,
                    Relaxed,
                    Relaxed,
                )
            };

            match res {
                // If we succeeded, we need to delete all nodes unreachable by
                // the senders.
                Ok(_) => {
                    // Safe because we pass a pointer to the loaded back as
                    // "last". We cannot even dereference
                    // it. We are also the only ones with
                    // reference to nodes from the front until before last.
                    unsafe { delete_before_last(self.front, Some(bypass_null(ptr))) }
                    break;
                }

                Err(new) => ptr = new,
            }
        }
    }
}

unsafe impl<T> Send for Receiver<T> where T: Send {}
unsafe impl<T> Sync for Receiver<T> where T: Send {}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        fmtr.write_str("mpsc::Receiver")
    }
}

struct SenderInner<T> {
    back: NonNull<SharedBack<T>>,
}

impl<T> Drop for SenderInner<T> {
    fn drop(&mut self) {
        // This is safe because we only store nodes allocated via
        // `OwnedAlloc`. Also, the shared back is only deallocated when both
        // sides disconnected.
        let ptr = unsafe { self.back.as_ref().ptr.load(Relaxed) };

        // Let's check for bit marking. If 1 the receiver is already
        // disconnected. If 0, nobody disconnected yet.
        if ptr as usize & 1 == 0 {
            // This is safe because we only store nodes allocated via
            // `OwnedAlloc`. Also, the shared back is only deallocated when both
            // sides disconnected.
            let res = unsafe {
                // Let's try to bit mark it so receiver will know we
                // disconnected.
                //
                // Safe to be a swap since we are the only ones which store
                // something different from ptr and ptr | 1 and we are not doing
                // so.
                self.back.as_ref().ptr.swap((ptr as usize | 1) as *mut _, Relaxed)
            };

            if res == ptr {
                // If we succeeded, we will left everything to be deallocated by
                // the receiver.
                return;
            }
        }

        // Falling here means sender disconnected.
        let ptr = (ptr as usize & !1) as *mut Node<T>;
        // This is safe because the pointer stored in the back will
        // never be null. Also, the sender disconnected and we are the
        // only sender left.
        unsafe {
            OwnedAlloc::from_raw(bypass_null(ptr));
            OwnedAlloc::from_raw(self.back);
        };
    }
}

struct SharedBack<T> {
    // lower bit is 0 when both sides connect, 1 when one disconnect
    // never null
    ptr: AtomicPtr<Node<T>>,
}

#[repr(align(/* at least */ 2))]
struct Node<T> {
    message: Option<T>,
    // lower bit is 1 means this node (and its subsequent ones) need to be
    // thrown away.
    next: AtomicPtr<Node<T>>,
}

// This function is unsafe because passing the wrong pointer may lead to
// undefined behavior. The pointer `last` needs to be a pointer previously
// loaded from the back, and must be reachable from `curr` if non-null. Also,
// the conditions for removal of the back needs to be respected. The function
// stops whenever the pointer or a node whose next field is null is reached.
unsafe fn delete_before_last<T>(mut curr: NonNull<Node<T>>, last: Option<NonNull<Node<T>>>) {
    while last != Some(curr) {
        // Let's try to mark the next field so other threads can see this node
        // needs to be thrown away. It is ok to swap since we are the
        // only ones accessing this node if the update fails.
        let next = curr.as_ref().next.swap((null_mut::<Node<T>>() as usize | 1) as *mut _, Acquire);

        match NonNull::new(next) {
            // Failure. The node was not null. It was a plain node. We need to
            // deallocate our current node and continue the job with the found
            // node.
            Some(next) => {
                OwnedAlloc::from_raw(curr);
                curr = next;
            }

            // Success. We have nothing more to do.
            None => break,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::channel::mpsc;
    use std::thread;

    #[test]
    fn correct_numbers() {
        const THREADS: usize = 8;
        const MSGS_PER_THREAD: usize = 64;
        const MSGS: usize = THREADS * MSGS_PER_THREAD;

        let mut done = [false; MSGS];
        let (sender, mut receiver) = mpsc::create::<usize>();
        let mut threads = Vec::with_capacity(THREADS);

        for i in 0..THREADS {
            let sender = sender.clone();
            threads.push(thread::spawn(move || {
                let start = i * MSGS_PER_THREAD;
                for j in start..start + MSGS_PER_THREAD {
                    sender.send(j).unwrap();
                }
            }))
        }

        drop(sender);

        loop {
            match receiver.recv() {
                Ok(i) => {
                    assert!(!done[i]);
                    done[i] = true;
                }

                Err(mpsc::NoMessage) => (),

                Err(mpsc::NoSender) => break,
            }
        }

        for thread in threads {
            thread.join().unwrap();
        }

        for status in done.iter() {
            assert!(*status);
        }
    }
}
