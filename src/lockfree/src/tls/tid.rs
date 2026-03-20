use owned_alloc::OwnedAlloc;
use std::{
    fmt,
    marker::PhantomData,
    ptr::null_mut,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering::*},
};

/// A cached thread-id. Repeated calls to [`ThreadLocal`](super::ThreadLocal)'s
/// methods with cached IDs should be faster than reloading the ID everytime.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ThreadId {
    bits: usize,
    _non_tsafe: PhantomData<*mut ()>,
}

impl ThreadId {
    /// Loads the ID for this thread.
    #[inline]
    pub fn current() -> Self {
        ID.with(|id| Self { bits: id.bits, _non_tsafe: PhantomData })
    }

    pub(super) fn bits(self) -> usize {
        self.bits
    }
}

impl Default for ThreadId {
    fn default() -> Self {
        Self::current()
    }
}

impl fmt::Debug for ThreadId {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "ThreadId({:?})", self.bits)
    }
}

static ID_COUNTER: AtomicUsize = AtomicUsize::new(1);

static ID_LIST: Node = Node { free: AtomicUsize::new(0), next: AtomicPtr::new(null_mut()) };

static ID_LIST_BACK: AtomicPtr<Node> = AtomicPtr::new(&ID_LIST as *const _ as *mut _);

thread_local! {
    static ID: IdGuard = IdGuard::new();
}

struct IdGuard {
    bits: usize,
    node: &'static Node,
}

impl IdGuard {
    fn new() -> Self {
        // We load the back at a given time so our thread-id can be truly
        // wait-free. We won't go further than this node before creating a new
        // one. Remember nodes are never deleted.
        let back_then = ID_LIST_BACK.load(Acquire);

        let mut node = &ID_LIST;

        loop {
            // First we try to acquire the current node.
            let bits = node.free.swap(usize::MAX, Relaxed);
            if bits != usize::MAX {
                break Self { node, bits };
            }

            let next = node.next.load(Acquire);

            // Then we check if we reached the limited we loaded previously.
            if next.is_null() || std::ptr::eq(node, back_then) {
                // If so, we create a new node.
                break Self::create_node();
            }

            // Ok because nodes are either static variables or heap-allocations
            // turned into static variables.
            node = unsafe { &*next };
        }
    }

    fn create_node() -> Self {
        let new = Node { free: AtomicUsize::new(usize::MAX), next: AtomicPtr::new(null_mut()) };

        let alloc = OwnedAlloc::new(new);
        let nnptr = alloc.into_raw();

        let prev = ID_LIST_BACK.swap(nnptr.as_ptr(), AcqRel);

        // Let's change the counter after the allocation and insertion, so
        // memory will be exhausted before the counter overflows. Allocation
        // can be seen as side-effect, since it may perform a syscall.
        // Therefore, it is not reordered.
        let bits = ID_COUNTER.fetch_add(1, Relaxed);

        // Ok because the list back is never null. Also, nodes are either static
        // variables or heap-allocations turned into static variables.
        let node = unsafe {
            (*prev).next.store(nnptr.as_ptr(), Release);
            &*nnptr.as_ptr()
        };

        Self { node, bits }
    }
}

impl Drop for IdGuard {
    fn drop(&mut self) {
        self.node.free.store(self.bits, Relaxed);
    }
}

struct Node {
    // Set to usize::MAX when not free.
    free: AtomicUsize,
    next: AtomicPtr<Node>,
}
