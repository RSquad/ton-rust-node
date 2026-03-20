pub use super::{
    NoRecv,
    RecvErr::{self, *},
};
use crate::{
    incin::Pause,
    ptr::{bypass_null, check_null_align},
    removable::Removable,
};
use owned_alloc::OwnedAlloc;
use std::{
    fmt,
    ptr::{null_mut, NonNull},
    sync::{
        atomic::{AtomicPtr, Ordering::*},
        Arc,
    },
};

/// Creates an asynchronous lock-free Multi-Producer-Multi-Consumer (MPMC)
/// channel. In order to allow multiple producers and multiple receivers,
/// [`Sender`] and [`Receiver`] are clonable and do not require mutability.
pub fn create<T>() -> (Sender<T>, Receiver<T>) {
    with_incin(SharedIncin::new())
}

/// Same as [`create`], but use a passed incinerator instead of creating a new
/// one.
pub fn with_incin<T>(incin: SharedIncin<T>) -> (Sender<T>, Receiver<T>) {
    check_null_align::<Node<T>>();

    // First we allocate this single node.
    let alloc =
        OwnedAlloc::new(Node { message: Removable::empty(), next: AtomicPtr::new(null_mut()) });
    let single_node = alloc.into_raw();

    // The we put it in a shared back.
    let shared = SharedBack { ptr: AtomicPtr::new(single_node.as_ptr()) };
    let alloc = OwnedAlloc::new(shared);
    let back = alloc.into_raw();

    // Put the shared back in the sender.
    let sender = Sender { inner: Arc::new(SenderInner { back }) };

    // And put the shared back and the single node (again) as front in the
    // receiver.
    let receiver = Receiver {
        inner: Arc::new(ReceiverInner { front: AtomicPtr::new(single_node.as_ptr()), back, incin }),
    };

    (sender, receiver)
}

/// The [`Sender`] handle of a MPMC channel. Created by [`create`] or
/// [`with_incin`] function. It is clonable and does not require mutability.
pub struct Sender<T> {
    inner: Arc<SenderInner<T>>,
}

impl<T> Sender<T> {
    /// Sends a message and if the receiver disconnected, an error is returned.
    pub fn send(&self, message: T) -> Result<(), NoRecv<T>> {
        // First of all we create a node for our message.
        let alloc = OwnedAlloc::new(Node {
            message: Removable::new(message),
            next: AtomicPtr::new(null_mut()),
        });
        let node = alloc.into_raw();

        // Then we load the back pointer so we can check if the receivers
        // disconnected. This dereferral is safe because we only deallocate
        // the shared back when both sides disconnect.
        let mut loaded = unsafe { self.inner.back.as_ref().ptr.load(Relaxed) };

        loop {
            // When the receiver disconnect, it will bit-mark the back. Let's
            // check it.
            if loaded as usize & 1 == 1 {
                // Safe because we are deallocating the node we just created
                // without sharing it.
                let mut alloc = unsafe { OwnedAlloc::from_raw(node) };
                let message = alloc.message.replace(None).unwrap();
                break Err(NoRecv { message });
            }

            // This dereferral is safe because we only deallocate
            // the shared back when both sides disconnect.
            let res = unsafe {
                // Let's try to replace previous back with our freshly created
                // node.
                self.inner.back.as_ref().ptr.compare_exchange(
                    loaded,
                    node.as_ptr(),
                    AcqRel,
                    Relaxed,
                )
            };

            match res {
                Ok(_) => {
                    // Bypassing null check is safe because we never store null
                    // in the front.
                    let prev = unsafe { bypass_null(loaded) };
                    // This dereferral is safe because we are the only senders
                    // with access to it *and* the receiver still sees it as the
                    // non-deletable last node.
                    let res = unsafe {
                        // We then try to update the next field of the previous
                        // back with the new back.
                        prev.as_ref().next.compare_exchange(
                            null_mut(),
                            node.as_ptr(),
                            Release,
                            Relaxed,
                        )
                    };

                    if res.is_err() {
                        // Then it is (null | 1). The previous back, our node
                        // and all its successors need to be deleted.
                        unsafe {
                            // Safe to delete previous back because we are the
                            // only ones which can access it.
                            OwnedAlloc::from_raw(prev);
                            // We also don't have a known back (second argument
                            // of `delete_before_last` is `None`). This is ok
                            // because the senders are the only ones with access
                            // to the back, which will be dropped only when all
                            // senders disconnect.
                            delete_before_last(node, None);
                        }
                    }

                    break Ok(());
                }

                Err(new) => loaded = new,
            }
        }
    }

    /// Tests if there are any [`Receiver`]s still connected. There are no
    /// guarantees that [`send`](Sender::send) will succeed if this method
    /// returns `true` because the [`Receiver`] may disconnect meanwhile.
    pub fn is_connected(&self) -> bool {
        // This is safe because we only store nodes allocated via
        // `OwnedAlloc`. Also, the shared back is only deallocated
        // when both sides disconnected. We load it to check for bit
        // marking (since it means sender disconnected).
        let back = unsafe { self.inner.back.as_ref() };
        back.ptr.load(Relaxed) as usize & 1 == 0
    }
}

unsafe impl<T> Send for Sender<T> where T: Send {}
unsafe impl<T> Sync for Sender<T> where T: Send {}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "mpmc::Sender {{ ptr: {:p} }}", self.inner)
    }
}

/// The [`Receiver`] handle of a MPMC channel. Created by [`create`] or
/// [`with_incin`] function. It is clonable and does not require mutability.
#[derive(Clone)]
pub struct Receiver<T> {
    inner: Arc<ReceiverInner<T>>,
}

impl<T> Receiver<T> {
    /// Tries to receive a message. If no message is available,
    /// [`Err`]`(`[`RecvErr::NoMessage`]`)` is returned. If the sender
    /// disconnected, [`Err`]`(`[`RecvErr::NoSender`]`)` is returned.
    #[allow(unused_must_use)]
    pub fn recv(&self) -> Result<T, RecvErr> {
        // We need this because of the infamous ABA problem (and
        // use-after-free).
        let pause = self.inner.incin.inner.pause();

        // Bypassing null check is safe because we never store null in
        // the front.
        let mut front_nnptr = unsafe {
            // First we load pointer stored in the front.
            bypass_null(self.inner.front.load(Relaxed))
        };

        loop {
            // Let's remove the node logically first. Safe to derefer this
            // pointer because we paused the incinerator and we only
            // delete nodes via incinerator.
            match unsafe { front_nnptr.as_ref().message.take(AcqRel) } {
                Some(val) => {
                    // Safe to call because we passed a pointer from the front
                    // which was loaded during the very same pause we are
                    // passing.
                    unsafe { self.try_clear_first(front_nnptr, &pause) };
                    break Ok(val);
                }

                // Safe to call because we passed a pointer from the front
                // which was loaded during the very same pause we are passing.
                None => unsafe {
                    front_nnptr = self.try_clear_first(front_nnptr, &pause)?;
                },
            }
        }
    }

    /// Tests if there are any [`Sender`]s still connected. There are no
    /// guarantees that [`recv`](Receiver::recv) will succeed if this method
    /// returns `true` because the [`Receiver`] may disconnect meanwhile.
    /// This method may also return `true` if the [`Sender`] disconnected
    /// but there are messages pending in the buffer. Note that another
    /// [`Receiver`] may pop out the pending messages after this method was
    /// called.
    pub fn is_connected(&self) -> bool {
        // We need this pause because of use-after-free.
        let _pause = self.inner.incin.inner.pause();
        // Safe to derefer this pointer because we paused the incinerator and we
        // only delete nodes via incinerator.
        let front = unsafe { &*self.inner.front.load(Relaxed) };
        // This is safe because the shared back is only deallocated
        // when both sides disconnected. We load it to check for bit
        // marking (since it means sender disconnected).
        let back = unsafe { self.inner.back.as_ref() };
        back.ptr.load(Relaxed) as usize & 1 == 0
            || front.message.is_present(Relaxed)
            || !front.next.load(Relaxed).is_null()
    }

    /// The shared incinerator used by this [`Receiver`].
    pub fn incin(&self) -> SharedIncin<T> {
        self.inner.incin.clone()
    }

    // This function is unsafe because passing the wrong pointer will lead to
    // undefined behavior. The pointer must have been loaded from the front
    // during the passed pause.
    unsafe fn try_clear_first(
        &self,
        expected: NonNull<Node<T>>,
        pause: &Pause<OwnedAlloc<Node<T>>>,
    ) -> Result<NonNull<Node<T>>, RecvErr> {
        let next = expected.as_ref().next.load(Acquire);

        if let Some(next_nnptr) = NonNull::new(next) {
            let res = self.inner.front.compare_exchange(expected.as_ptr(), next, Relaxed, Relaxed);

            // We are not oblied to succeed. This is just cleanup and some other
            // thread might do it.
            match res {
                Ok(_) => {
                    pause.add_to_incin(OwnedAlloc::from_raw(expected));
                    Ok(next_nnptr)
                }

                // Safe to by-pass the check since we only store non-null
                // pointers on the front.
                Err(found) => Ok(bypass_null(found)),
            }
        } else if self.inner.back.as_ref().ptr.load(Relaxed) as usize & 1 == 1 {
            // If the back is bit flagged, sender disconnected, no more messages
            // ever.
            Err(RecvErr::NoSender)
        } else {
            // No bit flag means sender is still there but we have no message.
            Err(RecvErr::NoMessage)
        }
    }
}

unsafe impl<T> Send for Receiver<T> where T: Send {}
unsafe impl<T> Sync for Receiver<T> where T: Send {}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "mpmc::Receiver {{ ptr: {:p} }}", self.inner)
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
                self.back.as_ref().ptr.swap((ptr as usize | 1) as *mut _, Release)
            };

            if res == ptr {
                // If we succeeded, we will left everything to be deallocated by
                // the receiver.
                return;
            }
        }

        let ptr = (ptr as usize & !1) as *mut Node<T>;
        // This is safe because the pointer stored in the back will
        // never be null. Also, the sender disconnected and we are the
        // only sender left.
        unsafe {
            OwnedAlloc::from_raw(bypass_null(ptr));
            OwnedAlloc::from_raw(self.back);
        }
    }
}

struct ReceiverInner<T> {
    // never null
    front: AtomicPtr<Node<T>>,
    back: NonNull<SharedBack<T>>,
    incin: SharedIncin<T>,
}

impl<T> ReceiverInner<T> {
    // This is unsafe because some conditions need to be met. Senders must have
    // disconnected.
    unsafe fn delete_all(&mut self) {
        let mut node_ptr = NonNull::new(*self.front.get_mut());

        while let Some(mut node) = node_ptr {
            node_ptr = NonNull::new(node.as_mut().next.load(Acquire));
            OwnedAlloc::from_raw(node);
        }
    }
}

impl<T> Drop for ReceiverInner<T> {
    fn drop(&mut self) {
        // This is safe because when senders disconnect, they won't drop the
        // back. And we are the only receiver.
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
                    debug_assert!(!ptr.is_null());
                    unsafe {
                        delete_before_last(
                            NonNull::new_unchecked(self.front.load(Relaxed)),
                            NonNull::new(ptr),
                        )
                    }
                    break;
                }

                Err(new) => ptr = new,
            }
        }
    }
}

struct SharedBack<T> {
    // lower bit is 0 when both sides connect, 1 when one disconnect
    // never null
    ptr: AtomicPtr<Node<T>>,
}

#[repr(align(/* at least */ 2))]
struct Node<T> {
    message: Removable<T>,
    // lower bit is 1 means this node (and its subsequent ones) need to be
    // thrown away.
    next: AtomicPtr<Node<T>>,
}

make_shared_incin! {
    { "`mpmc::Receiver`" }
    pub SharedIncin<T> of OwnedAlloc<Node<T>>
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
    use crate::channel::mpmc;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering::*},
            Arc,
        },
        thread,
    };

    #[test]
    fn correct_numbers() {
        const THREADS: usize = 8;
        const MSGS_PER_THREAD: usize = 64;
        const MSGS: usize = THREADS * MSGS_PER_THREAD;

        let mut done = Vec::with_capacity(MSGS);
        for _ in 0..MSGS {
            done.push(AtomicBool::new(false));
        }
        let done = Arc::<[AtomicBool]>::from(done);
        let (sender, receiver) = mpmc::create::<usize>();
        let mut threads = Vec::with_capacity(THREADS);

        for i in 0..THREADS {
            let sender = sender.clone();
            threads.push(thread::spawn(move || {
                let start = i * MSGS_PER_THREAD;
                for j in start..start + MSGS_PER_THREAD {
                    sender.send(j).unwrap();
                }
            }));

            let receiver = receiver.clone();
            let done = done.clone();
            threads.push(thread::spawn(move || loop {
                match receiver.recv() {
                    Ok(i) => assert!(!done[i].swap(true, AcqRel)),

                    Err(mpmc::NoMessage) => (),

                    Err(mpmc::NoSender) => break,
                }
            }));
        }

        drop(sender);
        drop(receiver);

        for thread in threads {
            thread.join().unwrap();
        }

        for status in done.iter() {
            assert!(status.load(Relaxed));
        }
    }
}
