use crate::{
    incin::Pause,
    ptr::{bypass_null, check_null_align},
    removable::Removable,
};
use owned_alloc::OwnedAlloc;
use std::{
    fmt,
    iter::FromIterator,
    ptr::{null_mut, NonNull},
    sync::atomic::{AtomicPtr, Ordering::*},
};

/// A lock-free general-purpouse queue. FIFO semanthics are fully respected.
/// It can be used as multi-producer and multi-consumer channel.
pub struct Queue<T> {
    front: AtomicPtr<Node<T>>,
    back: AtomicPtr<Node<T>>,
    incin: SharedIncin<T>,
}

impl<T> Queue<T> {
    /// Creates a new empty queue.
    pub fn new() -> Self {
        check_null_align::<Node<T>>();
        Self::with_incin(SharedIncin::new())
    }

    /// Creates an empty queue using the passed shared incinerator.
    pub fn with_incin(incin: SharedIncin<T>) -> Self {
        let node = Node::new(Removable::empty());
        let sentinel = OwnedAlloc::new(node).into_raw().as_ptr();
        Self { front: AtomicPtr::new(sentinel), back: AtomicPtr::new(sentinel), incin }
    }

    /// Returns the shared incinerator used by this [`Queue`].
    pub fn incin(&self) -> SharedIncin<T> {
        self.incin.clone()
    }

    /// Creates an iterator over `T`s, based on [`pop`](Queue::pop) operation of
    /// the [`Queue`].
    pub fn pop_iter<'queue>(&'queue self) -> PopIter<'queue, T> {
        PopIter { queue: self }
    }

    /// Pushes a value into the back of the queue. This operation is also
    /// wait-free.
    pub fn push(&self, item: T) {
        // Pretty simple: create a node from the item.
        let node = Node::new(Removable::new(item));
        let alloc = OwnedAlloc::new(node);
        let node_ptr = alloc.into_raw().as_ptr();
        // Swap with the previously stored back.
        let prev_back = self.back.swap(node_ptr, AcqRel);
        unsafe {
            // Updates the previous back's next field to our newly allocated
            // node. This may delay the visibility of the insertion.
            (*prev_back).next.store(node_ptr, Release);
        }
    }

    /// Takes a value from the front of the queue, if it is avaible.
    pub fn pop(&self) -> Option<T> {
        // Pausing because of ABA problem involving remotion from linked lists.
        let pause = self.incin.inner.pause();
        let mut front_nnptr = unsafe {
            // The pointer stored in front and back must never be null. The
            // queue always have at least one node. Front and back are
            // always connected.
            bypass_null(self.front.load(Relaxed))
        };

        loop {
            // This dereferral is safe because we paused the incinerator and
            // only delete nodes via incinerator.
            //
            // We first remove the node logically.
            match unsafe { front_nnptr.as_ref().item.take(AcqRel) } {
                Some(val) => {
                    // Safe to call because we passed a pointer from the front
                    // which was loaded during the very same pause we are
                    // passing.
                    unsafe { self.try_clear_first(front_nnptr, &pause) };
                    break Some(val);
                }

                // Safe to call because we passed a pointer from the front
                // which was loaded during the very same pause we are
                // passing.
                None => unsafe {
                    front_nnptr = self.try_clear_first(front_nnptr, &pause)?;
                },
            }
        }
    }

    /// Pushes elements from the given iterable. Acts just like
    /// [`Extend::extend`] but does not require mutability.
    pub fn extend<I>(&self, iterable: I)
    where
        I: IntoIterator<Item = T>,
    {
        for elem in iterable {
            self.push(elem);
        }
    }

    // Returns an `Option` so we can use the try operator (?) with the function.
    // This function is unsafe because passing the wrong pointer will lead to
    // undefined behavior. The pointer must have been loaded from the front
    // during the passed pause.
    unsafe fn try_clear_first(
        &self,
        expected: NonNull<Node<T>>,
        pause: &Pause<OwnedAlloc<Node<T>>>,
    ) -> Option<NonNull<Node<T>>> {
        let next = expected.as_ref().next.load(Acquire);

        // If this is the only node, we will not remove it. We want front and
        // back to share the same node rather than having to set both to null
        // when the queue is empty.
        NonNull::new(next).map(|next_nnptr| {
            let ptr = expected.as_ptr();

            // We are not oblied to succeed. This is just cleanup and some other
            // thread might do it.
            match self.front.compare_exchange(ptr, next, Relaxed, Relaxed) {
                Ok(_) => {
                    // Only deleting nodes via incinerator due to ABA problem
                    // and use-after-frees.
                    pause.add_to_incin(OwnedAlloc::from_raw(expected));
                    next_nnptr
                }

                Err(found) => {
                    // Safe to by-pass the check since we only store non-null
                    // pointers on the front.
                    bypass_null(found)
                }
            }
        })
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        let front = self.front.get_mut();
        while let Some(nnptr) = NonNull::new(*front) {
            // This is safe because we only store pointers allocated via
            // `OwnedAlloc`. Also, we have exclusive access to this pointer.
            let mut node = unsafe { OwnedAlloc::from_raw(nnptr) };
            *front = *node.next.get_mut();
        }
    }
}

impl<T> FromIterator<T> for Queue<T> {
    fn from_iter<I>(iterable: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        let this = Self::new();
        this.extend(iterable);
        this
    }
}

impl<T> Extend<T> for Queue<T> {
    fn extend<I>(&mut self, iterable: I)
    where
        I: IntoIterator<Item = T>,
    {
        (*self).extend(iterable)
    }
}

impl<T> Iterator for Queue<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let front = self.front.get_mut();
        // Safe to by-pass it because the queue always have at least one node.
        let mut front_node = unsafe { NonNull::new_unchecked(*front) };
        loop {
            // Safe because we allocated everything properly.
            let (item, next) = unsafe {
                let node_ref = front_node.as_mut();
                (node_ref.item.replace(None), *node_ref.next.get_mut())
            };

            match (item, NonNull::new(next)) {
                (Some(item), maybe_next) => {
                    if let Some(next) = maybe_next {
                        // Ok to drop it like this because we have exclusive
                        // reference to the queue.
                        unsafe { OwnedAlloc::from_raw(front_node) };
                        *front = next.as_ptr();
                    }

                    break Some(item);
                }

                (None, None) => break None,

                (None, Some(next)) => {
                    // Ok to drop it like this because we have exclusive
                    // reference to the queue.
                    unsafe { OwnedAlloc::from_raw(front_node) };
                    *front = next.as_ptr();
                    front_node = next;
                }
            }
        }
    }
}

impl<T> fmt::Debug for Queue<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(
            fmtr,
            "Queue {{ front: {:?}, back: {:?}, incin: {:?} }}",
            self.front, self.back, self.incin
        )
    }
}

unsafe impl<T> Send for Queue<T> where T: Send {}
unsafe impl<T> Sync for Queue<T> where T: Send {}

/// An iterator based on [`pop`](Queue::pop) operation of the [`Queue`].
pub struct PopIter<'queue, T>
where
    T: 'queue,
{
    queue: &'queue Queue<T>,
}

impl<'queue, T> Iterator for PopIter<'queue, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.queue.pop()
    }
}

impl<'queue, T> fmt::Debug for PopIter<'queue, T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "PopIter {{ queue: {:?} }}", self.queue)
    }
}

make_shared_incin! {
    { "[`Queue`]" }
    pub SharedIncin<T> of OwnedAlloc<Node<T>>
}

impl<T> fmt::Debug for SharedIncin<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "SharedIncin {{ inner: {:?} }}", self.inner)
    }
}

#[repr(align(/* at least */ 2))]
struct Node<T> {
    item: Removable<T>,
    next: AtomicPtr<Node<T>>,
}

impl<T> Node<T> {
    fn new(item: Removable<T>) -> Self {
        Self { item, next: AtomicPtr::new(null_mut()) }
    }
}

// Testing the safety of `unsafe` in this module is done with random operations
// via fuzzing
#[cfg(test)]
mod test {
    use super::*;
    use std::{
        sync::{atomic::AtomicUsize, Arc},
        thread,
    };

    #[test]
    fn on_empty_first_pop_is_none() {
        let queue = Queue::<usize>::new();
        assert!(queue.pop().is_none());
    }

    #[test]
    fn on_empty_last_pop_is_none() {
        let queue = Queue::new();
        queue.push(3);
        queue.push(1234);
        queue.pop();
        queue.pop();
        assert!(queue.pop().is_none());
    }

    #[test]
    fn order() {
        let queue = Queue::new();
        queue.push(3);
        queue.push(5);
        queue.push(6);
        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), Some(6));
    }

    #[test]
    fn queue_iter() {
        let mut queue = Queue::new();
        queue.push(3);
        queue.push(5);
        queue.push(6);
        assert_eq!(queue.next(), Some(3));
        assert_eq!(queue.next(), Some(5));
        assert_eq!(queue.next(), Some(6));
        assert_eq!(queue.next(), None);
    }

    #[test]
    fn no_data_corruption() {
        const NTHREAD: usize = 20;
        const NITER: usize = 800;
        const NMOD: usize = 55;

        let queue = Arc::new(Queue::new());
        let mut handles = Vec::with_capacity(NTHREAD);
        let removed = Arc::new(AtomicUsize::new(0));

        for i in 0..NTHREAD {
            let removed = removed.clone();
            let queue = queue.clone();
            handles.push(thread::spawn(move || {
                for j in 0..NITER {
                    let val = (i * NITER) + j;
                    queue.push(val);
                    if (val + 1) % NMOD == 0 {
                        if let Some(val) = queue.pop() {
                            removed.fetch_add(1, Relaxed);
                            assert!(val < NITER * NTHREAD);
                        }
                    }
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread failed");
        }

        let expected = NITER * NTHREAD - removed.load(Relaxed);
        let mut res = 0;
        while let Some(val) = queue.pop() {
            assert!(val < NITER * NTHREAD);
            res += 1;
        }

        assert_eq!(res, expected);
    }
}
