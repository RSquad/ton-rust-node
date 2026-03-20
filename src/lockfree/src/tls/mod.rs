mod tid;

pub use self::tid::ThreadId;
use crate::ptr::check_null_align;
use owned_alloc::{Cache, OwnedAlloc, UninitAlloc};
use std::{
    fmt,
    marker::PhantomData,
    mem::{forget, replace},
    ptr::{null_mut, NonNull},
    sync::atomic::{AtomicPtr, Ordering::*},
};

const BITS: usize = 8;

/// Per Object Thread Local Storage. The stored data is not dropped on thread
/// exit. It is only dropped when the structure itself is dropped. After the
/// thread exited, the data might be reused for other threads. This TLS's
/// operation are also wait-free.
///
/// # Example
/// ```
/// extern crate lockfree;
///
/// use lockfree::tls::ThreadLocal;
/// use std::{cell::Cell, sync::Arc, thread};
///
/// let tls = Arc::new(ThreadLocal::<Cell<usize>>::new());
/// let mut threads = Vec::with_capacity(32);
///
/// for i in 1 ..= 32 {
///     let tls = tls.clone();
///     threads.push(thread::spawn(move || {
///         tls.with_default().set(i);
///
///         if tls.get().map(|c| assert_eq!(c.get(), i)).is_none() {
///             // Some OSes mis-run the destructors for their TLS
///             // implementation.
///             eprintln!("Warning: OS mis-reset the global thread state")
///         }
///     }))
/// }
///
/// for thread in threads {
///     thread.join().unwrap();
/// }
/// ```
pub struct ThreadLocal<T> {
    top: OwnedAlloc<Table<T>>,
}

impl<T> ThreadLocal<T> {
    /// Creates an empty thread local storage.
    pub fn new() -> Self {
        check_null_align::<Table<T>>();
        check_null_align::<Entry<T>>();
        Self { top: Table::new_alloc() }
    }

    /// Removes and drops all entries. The TLS is considered empty then. This
    /// method is only available with exclusive references. This method is
    /// merely for optimization since the TLS is cleared at drop.
    pub fn clear(&mut self) {
        let mut tables = Vec::new();

        // Method clear means we are also resetting all node pointers to null.
        //
        // Safe because we store nodes only correctly.
        unsafe { self.top.clear(&mut tables) }

        while let Some(mut table) = tables.pop() {
            // Method free_nodes means we are only freeing node pointers but not
            // clearing them.
            //
            // Safe because we will never refer to these nodes again.
            unsafe { table.free_nodes(&mut tables) }
        }
    }

    /// Creates an iterator over immutable refereces of entries.
    pub fn iter(&self) -> Iter<'_, T>
    where
        T: Sync,
    {
        Iter { curr_table: Some((&self.top, 0)), tables: Vec::new() }
    }

    /// Creates an iterator over mutable refereces of entries.
    pub fn iter_mut(&mut self) -> IterMut<'_, T>
    where
        T: Send,
    {
        IterMut { curr_table: Some((&mut self.top, 0)), tables: Vec::new() }
    }

    /// Accesses the entry for the current thread. No initialization is
    /// performed.
    #[inline]
    pub fn get(&self) -> Option<&T> {
        self.get_with_id(ThreadId::current())
    }

    /// Accesses the entry for the current thread with a given cached ID.
    /// Repeated calls with cached IDs should be faster than reloading the ID
    /// everytime. No initialization is performed.
    pub fn get_with_id(&self, id: ThreadId) -> Option<&T> {
        let mut table = &*self.top;
        let mut shifted = id.bits();

        loop {
            // The index of the node for our id.
            let index = shifted & ((1 << BITS) - 1);

            // Load what is in there.
            let in_place = table.nodes[index].atomic.load(Acquire);

            // Null means there is nothing.
            if in_place.is_null() {
                break None;
            }

            // Having in_place's lower bit set to 0 means it is a
            // pointer to entry.
            if in_place as usize & 1 == 0 {
                // This is safe since:
                //
                // 1. We only store nodes with cleared lower bit if it is an
                // entry.
                //
                // 2. We only delete stuff when we are behind mutable
                // references.
                let entry = unsafe { &*(in_place as *mut Entry<T>) };
                break if entry.id == id {
                    // We only have an entry for the thread if the ids
                    // match.
                    Some(&entry.data)
                } else {
                    None
                };
            }

            // The remaining case (non-null with lower bit set to 1) means
            // we have a child table.
            // Clear the pointer first lower bit so we can dereference it.
            let table_ptr = (in_place as usize & !1) as *mut Table<T>;
            // Set it as the table to be checked in the next iteration.
            // This is safe since:
            //
            // 1. We only store nodes with marked lower bit if it is an
            // table.
            //
            // 2. W cleared up the bit above so we can get the original
            // pointer.
            //
            // 3. We only delete stuff when we are behind mutable
            // references.
            table = unsafe { &*table_ptr };
            // Shift our "hash" for the next level.
            shifted >>= BITS;
        }
    }

    /// Accesses the entry for the current thread. If necessary, the `init`
    /// closure is called to initialize the entry.
    #[inline]
    pub fn with_init<F>(&self, init: F) -> &T
    where
        F: FnOnce() -> T,
    {
        self.with_id_and_init(ThreadId::current(), init)
    }

    /// Accesses the entry for the current thread with a given cached ID.
    /// Repeated calls with cached IDs should be faster than reloading the ID
    /// everytime. If necessary, the `init` closure is called to initialize the
    /// entry.
    pub fn with_id_and_init<F>(&self, id: ThreadId, init: F) -> &T
    where
        F: FnOnce() -> T,
    {
        let mut table = &*self.top;
        // The depth of the iterations.
        let mut depth = 1;
        let mut shifted = id.bits();
        // The pointer stored in place.
        let mut index = shifted & ((1 << BITS) - 1);
        let mut in_place = table.nodes[index].atomic.load(Acquire);
        // Using `LazyInit` to make sure we only initialize if there is no
        // entry.
        let mut init = LazyInit::Pending(move || Entry { id, data: init() });
        let mut tbl_cache = Cache::<OwnedAlloc<Table<T>>>::new();

        loop {
            if in_place.is_null() {
                // Null means we have an empty node and also our thread has
                // not stored anything. Let's initialize.
                let nnptr = init.get();
                // First lower bit set to 0 means this is a pointer to
                // entry. This should be guaranteed by the alignment,
                // however, always good to ensure it.
                debug_assert!(nnptr.as_ptr() as usize & 1 == 0);

                // Trying to publish our freshly created entry.
                match table.nodes[index].atomic.compare_exchange(
                    in_place,
                    nnptr.as_ptr() as *mut (),
                    AcqRel,
                    Acquire,
                ) {
                    Ok(_) => {
                        // If the stored value still was null, we succeeded.
                        // Let's read the entry.
                        //
                        // This is safe since... This is the pointer we just
                        // allocated and we only delete nodes through mutable
                        // references to the TLS.
                        break unsafe { &(*nnptr.as_ptr()).data };
                    }

                    Err(new) => in_place = new,
                }
            } else if in_place as usize & 1 == 0 {
                // First lower bit set to 0 means we have an entry.
                //
                // This is safe since:
                //
                // 1. We only store nodes with cleared lower bit if it is an
                // entry.
                //
                // 2. We only delete stuff when we are behind mutable
                // references.
                let entry = unsafe { &*(in_place as *mut Entry<T>) };
                // If ids match, this is the entry for our thread.
                if entry.id == id {
                    // There is no possible way we have initialized the
                    // `LazyInit`. It will only happen if we found an empty
                    // node while searching, and the only way of putting a
                    // non-empty node is either we put it or some other
                    // thread (with different id obviously) put it.
                    debug_assert!(init.is_pending());
                    // And let's read it...
                    break &entry.data;
                }

                // Get a table allocation from the cache.
                let new_tbl = tbl_cache.take_or(Table::new_alloc);

                // Calculate index for the collided entry.
                let other_shifted = entry.id.bits() >> (depth * BITS);
                let other_index = other_shifted & ((1 << BITS) - 1);

                // Pre-insert it in the table from the cache.
                new_tbl.nodes[other_index].atomic.store(in_place, Relaxed);

                // Forget about the owned allocation and turn it into a
                // pointer.
                let new_tbl_ptr = new_tbl.into_raw();

                // Let's try to publish our work.
                match table.nodes[index].atomic.compare_exchange(
                    in_place,
                    // First lower bit set to 1 means it is a table
                    // pointer.
                    (new_tbl_ptr.as_ptr() as usize | 1) as *mut (),
                    AcqRel,
                    Relaxed, //Release,
                ) {
                    Ok(_) => {
                        // If the old node was still stored, we succeeded.
                        // Let's set the new table as the table for the next
                        // iteration.
                        //
                        // This is safe since it is the table we just allocated
                        // and we only delete it through mutable references to
                        // the TLS.
                        table = unsafe { &*new_tbl_ptr.as_ptr() };
                        // We are going one depth further.
                        depth += 1;
                        // Shift our "hash" for the next level.
                        shifted >>= BITS;
                        // Load new in place pointer.
                        index = shifted & ((1 << BITS) - 1);
                        in_place = table.nodes[index].atomic.load(Acquire);
                    }

                    Err(new) => {
                        // If we failed, let's rebuild the owned allocation.
                        //
                        // This is safe since it is the table we just allocated
                        // and we don't share it.
                        let new_tbl = unsafe { OwnedAlloc::from_raw(new_tbl_ptr) };
                        // Clear that pre-inserted node.
                        new_tbl.nodes[other_index].atomic.store(null_mut(), Relaxed);

                        // Store it into the cache for later.
                        tbl_cache.store(new_tbl);
                        in_place = new;
                    }
                }
            } else {
                // The remaining case (non-null with first lower bit set to
                // 1) is a table. Clear the pointer first lower bit so we
                // can dereference it.
                let table_ptr = (in_place as usize & !1) as *mut Table<T>;

                // Set it as table for the next iteration.
                //
                // 1. We only store nodes with marked lower bit if it is an
                // table.
                //
                // 2. W cleared up the bit above so we can get the original
                // pointer.
                //
                // 3. We only delete stuff when we are behind mutable
                // references.
                table = unsafe { &*table_ptr };
                // We are going one depth further.
                depth += 1;
                // Shift our "hash" for the next level.
                shifted >>= BITS;
                // Load new in place pointer.
                index = shifted & ((1 << BITS) - 1);
                in_place = table.nodes[index].atomic.load(Acquire);
            }
        }
    }

    /// Accesses the entry for the current thread. If necessary, the entry is
    /// initialized with default value.
    #[inline]
    pub fn with_default(&self) -> &T
    where
        T: Default,
    {
        self.with_init(T::default)
    }

    /// Accesses the entry for the current thread with a given cached ID.
    /// Repeated calls with cached IDs should be faster than reloading the ID
    /// everytime. If necessary, the entry is initialized with default
    /// value.
    #[inline]
    pub fn with_id_and_default(&self, id: ThreadId) -> &T
    where
        T: Default,
    {
        self.with_id_and_init(id, T::default)
    }
}

impl<T> Drop for ThreadLocal<T> {
    fn drop(&mut self) {
        let mut tables = Vec::new();

        // Method free_nodes means we are only freeing node pointers but not
        // clearing them (no need to clear since nobody will ever use them
        // again, we are dropping the TLS).
        //
        // This is safe because we never load the nodes again.
        unsafe { self.top.free_nodes(&mut tables) }

        while let Some(mut table) = tables.pop() {
            // This is safe because we never load the nodes again.
            unsafe { table.free_nodes(&mut tables) }
        }
    }
}

impl<T> fmt::Debug for ThreadLocal<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "ThreadLocal {{ storage: ")?;
        match self.get() {
            Some(val) => write!(fmtr, "Some({:?})", val)?,
            None => write!(fmtr, "None")?,
        }
        write!(fmtr, "}}")
    }
}

impl<T> Default for ThreadLocal<T> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T> Send for ThreadLocal<T> {}
unsafe impl<T> Sync for ThreadLocal<T> {}

impl<T> IntoIterator for ThreadLocal<T>
where
    T: Send,
{
    type IntoIter = IntoIter<T>;
    type Item = T;

    fn into_iter(self) -> Self::IntoIter {
        let raw = self.top.raw();
        forget(self);
        // Safe since this is the allocation we just forgot about.
        let top = unsafe { OwnedAlloc::from_raw(raw) };

        IntoIter { curr_table: Some((top, 0)), tables: Vec::new() }
    }
}

impl<'tls, T> IntoIterator for &'tls ThreadLocal<T>
where
    T: Sync,
{
    type IntoIter = Iter<'tls, T>;
    type Item = &'tls T;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'tls, T> IntoIterator for &'tls mut ThreadLocal<T>
where
    T: Send,
{
    type IntoIter = IterMut<'tls, T>;
    type Item = &'tls mut T;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// An iterator over immutable references to entries of TLS.
pub struct Iter<'tls, T>
where
    T: 'tls,
{
    tables: Vec<&'tls Table<T>>,
    curr_table: Option<(&'tls Table<T>, usize)>,
}

impl<'tls, T> Iterator for Iter<'tls, T> {
    type Item = &'tls T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (table, index) = self.curr_table.take()?;
            match table.nodes.get(index).map(|node| node.atomic.load(Acquire)) {
                Some(ptr) if ptr.is_null() => self.curr_table = Some((table, index + 1)),

                Some(ptr) if ptr as usize & 1 == 0 => {
                    let ptr = ptr as *mut Entry<T>;
                    self.curr_table = Some((table, index + 1));
                    // This is safe since:
                    //
                    // 1. We only store nodes with cleared lower bit if it is an
                    // entry.
                    //
                    // 2. We only delete stuff when we are behind mutable
                    // references *and* there are no mutable references to the
                    // TLS as we are a shared one.
                    break Some(unsafe { &(*ptr).data });
                }

                Some(ptr) => {
                    let ptr = (ptr as usize & !1) as *mut Table<T>;
                    // Set it as table for the next iteration.
                    //
                    // 1. We only store nodes with marked lower bit if it is an
                    // table.
                    //
                    // 2. We cleared up the bit above so we can get the original
                    // pointer.
                    //
                    // 3. We only delete stuff when we are behind mutable
                    // references *and* there are no mutable references to the
                    // TLS as we are a shared one.
                    self.tables.push(unsafe { &mut *ptr });
                    self.curr_table = Some((table, index + 1));
                }

                None => self.curr_table = self.tables.pop().map(|tbl| (tbl, 0)),
            };
        }
    }
}

/// An iterator over mutable references to entries of TLS.
pub struct IterMut<'tls, T>
where
    T: 'tls,
{
    tables: Vec<&'tls mut Table<T>>,
    curr_table: Option<(&'tls mut Table<T>, usize)>,
}

impl<'tls, T> Iterator for IterMut<'tls, T> {
    type Item = &'tls mut T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (table, index) = self.curr_table.take()?;
            match table.nodes.get_mut(index).map(|node| *node.atomic.get_mut()) {
                Some(ptr) if ptr.is_null() => self.curr_table = Some((table, index + 1)),

                Some(ptr) if ptr as usize & 1 == 0 => {
                    let ptr = ptr as *mut Entry<T>;
                    self.curr_table = Some((table, index + 1));
                    // This is safe since:
                    //
                    // 1. We only store nodes with cleared lower bit if it is an
                    // entry.
                    //
                    // 2. We only delete stuff when we are behind mutable
                    // references *and* we are the only mutable reference to the
                    // TLS. We are not deleting it.
                    break Some(unsafe { &mut (*ptr).data });
                }

                Some(ptr) => {
                    let ptr = (ptr as usize & !1) as *mut Table<T>;
                    // Set it as table for the next iteration.
                    //
                    // 1. We only store nodes with marked lower bit if it is an
                    // table.
                    //
                    // 2. We cleared up the bit above so we can get the original
                    // pointer.
                    //
                    // 3. We only delete stuff when we are behind mutable
                    // references *and* we are the only mutable reference to the
                    // TLS. We are not deleting it.
                    self.tables.push(unsafe { &mut *ptr });
                    self.curr_table = Some((table, index + 1));
                }

                None => self.curr_table = self.tables.pop().map(|tbl| (tbl, 0)),
            };
        }
    }
}

impl<'tls, T> fmt::Debug for IterMut<'tls, T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "IterMut {{ tables: {:?}, curr_table: {:?} }}", self.tables, self.curr_table)
    }
}

/// An iterator over owned references to entries of TLS.
pub struct IntoIter<T> {
    tables: Vec<OwnedAlloc<Table<T>>>,
    curr_table: Option<(OwnedAlloc<Table<T>>, usize)>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (mut table, index) = self.curr_table.take()?;
            match table.nodes.get_mut(index).map(|node| *node.atomic.get_mut()) {
                Some(ptr) if ptr.is_null() => self.curr_table = Some((table, index + 1)),

                Some(ptr) if ptr as usize & 1 == 0 => {
                    let ptr = ptr as *mut Entry<T>;
                    // This is safe since:
                    //
                    // 1. We only store nodes with cleared lower bit if it is an
                    // entry.
                    //
                    // 2. We have ownership over the TLS, so no one else is
                    // reading or writing or deleting.
                    let alloc = unsafe { OwnedAlloc::from_raw(NonNull::new_unchecked(ptr)) };
                    let (entry, _) = alloc.move_inner();
                    self.curr_table = Some((table, index + 1));
                    break Some(entry.data);
                }

                Some(ptr) => {
                    let ptr = (ptr as usize & !1) as *mut Table<T>;
                    // This is safe since:
                    //
                    // 1. We only store nodes with marked lower bit if it is an
                    // table.
                    //
                    // 2. We have ownership over the TLS, so no one else is
                    // reading or writing or deleting.
                    self.tables.push(unsafe { OwnedAlloc::from_raw(NonNull::new_unchecked(ptr)) });
                    self.curr_table = Some((table, index + 1));
                }

                None => self.curr_table = self.tables.pop().map(|tbl| (tbl, 0)),
            };
        }
    }
}

impl<T> fmt::Debug for IntoIter<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "IntoIter {{ tables: {:?}, curr_table: {:?} }}", self.tables, self.curr_table)
    }
}

struct Node<T> {
    // lower bit marked 0 for Entry, 1 for Table
    atomic: AtomicPtr<()>,
    _marker: PhantomData<T>,
}

impl<T> Node<T> {
    // Unsafe because it is *pretty easy* to make undefined behavior out of this
    // because the pointer does not have even a fixed type.
    unsafe fn free_ptr(ptr: *mut (), tbl_stack: &mut Vec<OwnedAlloc<Table<T>>>) {
        if ptr.is_null() {
            return;
        }

        if ptr as usize & 1 == 0 {
            OwnedAlloc::from_raw(NonNull::new_unchecked(ptr as *mut Entry<T>));
        } else {
            let table_ptr = (ptr as usize & !1) as *mut Table<T>;

            debug_assert!(!table_ptr.is_null());
            tbl_stack.push(OwnedAlloc::from_raw(NonNull::new_unchecked(table_ptr)));
        }
    }
}

impl<T> fmt::Debug for Node<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "Node {{ pointer: {:?} }}", self.atomic)
    }
}

#[repr(align(/* at least */ 2))]
struct Table<T> {
    nodes: [Node<T>; 1 << BITS],
}

impl<T> Table<T> {
    #[inline]
    fn new_alloc() -> OwnedAlloc<Self> {
        // Safe because it calls a correctly a function which correctly
        // initializes uninitialized memory with, indeed, uninitialized memory.
        unsafe { UninitAlloc::<Self>::new().init_in_place(|this| this.init()) }
    }

    // Unsafe because passing ininitialized memory may cause leaks.
    #[inline]
    unsafe fn init(&mut self) {
        for node_ref in &mut self.nodes as &mut [_] {
            (node_ref as *mut Node<T>)
                .write(Node { atomic: AtomicPtr::new(null_mut()), _marker: PhantomData })
        }
    }

    // Unsafe because calling this function and using the table again later will
    // cause undefined behavior.
    #[inline]
    unsafe fn free_nodes(&mut self, tbl_stack: &mut Vec<OwnedAlloc<Table<T>>>) {
        for node in &mut self.nodes as &mut [Node<T>] {
            Node::free_ptr(*node.atomic.get_mut(), tbl_stack);
        }
    }

    // Unsafe because storing the wrong pointers in the table will lead to
    // undefined behavior.
    #[inline]
    unsafe fn clear(&mut self, tbl_stack: &mut Vec<OwnedAlloc<Table<T>>>) {
        for node in &mut self.nodes as &mut [Node<T>] {
            let ptr = node.atomic.get_mut();
            Node::free_ptr(*ptr, tbl_stack);
            *ptr = null_mut();
        }
    }
}

impl<T> fmt::Debug for Table<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "Table {{ nodes: {:?} }}", &self.nodes as &[Node<T>])
    }
}

#[repr(align(64))]
struct Entry<T> {
    data: T,
    id: ThreadId,
}

enum LazyInit<T, F> {
    Done(NonNull<T>),
    Pending(F),
}

impl<T, F> LazyInit<T, F>
where
    F: FnOnce() -> T,
{
    fn is_pending(&self) -> bool {
        matches!(self, LazyInit::Pending(_))
    }

    fn get(&mut self) -> NonNull<T> {
        let old = replace(self, LazyInit::Done(NonNull::dangling()));

        let ptr = match old {
            LazyInit::Done(ptr) => ptr,

            LazyInit::Pending(init) => OwnedAlloc::new(init()).into_raw(),
        };

        *self = LazyInit::Done(ptr);
        ptr
    }
}

#[cfg(test)]
mod test {
    use super::ThreadLocal;
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    #[test]
    fn threads_with_their_id() {
        const THREADS: usize = 32;

        let tls = Arc::new(ThreadLocal::new());
        let mut threads = Vec::with_capacity(THREADS);
        // prevent IDs from being reused.
        let barrier = Arc::new(Barrier::new(THREADS));

        for i in 0..THREADS {
            let tls = tls.clone();
            let barrier = barrier.clone();
            threads.push(thread::spawn(move || {
                assert_eq!(*tls.with_init(|| i), i);
                barrier.wait();
            }))
        }

        for thread in threads {
            thread.join().unwrap();
        }
    }

    #[test]
    fn iter() {
        const THREADS: usize = 32;

        let tls = Arc::new(ThreadLocal::new());
        let mut threads = Vec::with_capacity(THREADS);
        // prevent IDs from being reused.
        let barrier = Arc::new(Barrier::new(THREADS));

        for i in 0..THREADS {
            let tls = tls.clone();
            let barrier = barrier.clone();
            threads.push(thread::spawn(move || {
                tls.with_init(|| i);
                barrier.wait();
            }))
        }

        for entry in &*tls {
            assert!(*entry < THREADS);
        }
    }

    #[test]
    fn iter_mut() {
        const THREADS: usize = 32;

        let tls = Arc::new(ThreadLocal::new());
        let mut threads = Vec::with_capacity(THREADS);
        // prevent IDs from being reused.
        let barrier = Arc::new(Barrier::new(THREADS));

        for i in 0..THREADS {
            let tls = tls.clone();
            let barrier = barrier.clone();
            threads.push(thread::spawn(move || {
                tls.with_init(|| i);
                barrier.wait();
            }))
        }

        for thread in threads {
            thread.join().unwrap();
        }

        let mut done = [0; THREADS];
        let mut tls = Arc::try_unwrap(tls).unwrap();

        for entry in &mut tls {
            done[*entry] += 1;
            *entry = (*entry + 1) % THREADS;
        }

        for entry in tls {
            done[entry] += 1;
        }

        for &status in &done as &[_] {
            assert_eq!(status, 2);
        }
    }
}
