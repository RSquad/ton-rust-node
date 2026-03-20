use super::{
    bucket::{Bucket, Garbage, GetRes, InsertRes},
    guard::{ReadGuard, Removed},
    insertion::{Inserter, Insertion},
};
use crate::incin::{Incinerator, Pause};
use owned_alloc::{Cache, OwnedAlloc, UninitAlloc};
use std::{
    borrow::Borrow,
    fmt,
    marker::PhantomData,
    ptr::{null_mut, NonNull},
    sync::{
        atomic::{
            AtomicPtr,
            Ordering::{self, *},
        },
        Arc,
    },
};

const BITS: usize = 8;

// If you remove this alignment, don't remove it. Please, set it to 2.
#[repr(align(64))]
pub struct Table<K, V> {
    nodes: [Node<K, V>; 1 << BITS],
}

impl<K, V> Table<K, V> {
    pub fn new_alloc() -> OwnedAlloc<Self> {
        // Safe because it calls a correctly a function which correctly
        // initializes uninitialized memory with, indeed, uninitialized memory.
        unsafe { UninitAlloc::<Self>::new().init_in_place(|val| val.init_in_place()) }
    }

    // Unsafe because passing ininitialized memory may cause leaks.
    #[inline]
    pub unsafe fn init_in_place(&mut self) {
        for node in &mut self.nodes as &mut [_] {
            (node as *mut Node<K, V>).write(Node::new())
        }
    }

    // Unsafe because the incinerator needs to be paused and there are no
    // guarantees the passed pause comes from the incinerator used with the map
    // by other threads. Map implementation guarantees that.
    pub unsafe fn get<'map, Q>(
        &self,
        key: &Q,
        hash: u64,
        pause: Pause<'map, Garbage<K, V>>,
    ) -> Option<ReadGuard<'map, K, V>>
    where
        Q: ?Sized + Ord,
        K: Borrow<Q>,
    {
        let mut shifted = hash;
        let mut table = self;

        loop {
            // Compute the index from the shifted hash's lower bits.
            let index = shifted as usize & ((1 << BITS) - 1);
            let loaded = table.nodes[index].atomic.load(Acquire);

            // Null means we have nothing.
            if loaded.is_null() {
                break None;
            }

            // Cleared lower bit means this is a bucket.
            if loaded as usize & 1 == 0 {
                let bucket = &*(loaded as *mut Bucket<K, V>);

                // This bucket only matters if it has the same hash we do.
                if bucket.hash() != hash {
                    break None;
                }

                break match bucket.get(key, pause) {
                    // Success.
                    GetRes::Found(pair) => Some(pair),

                    // Not here.
                    GetRes::NotFound => None,

                    // Delete the bucket completely.
                    GetRes::Delete(pause) => {
                        let res = table.nodes[index].atomic.compare_exchange(
                            loaded,
                            null_mut(),
                            Relaxed,
                            Relaxed,
                        );

                        if res.is_ok() {
                            let alloc =
                                OwnedAlloc::from_raw(NonNull::new_unchecked(loaded as *mut _));
                            // Needs to be destroyed by the incinerator as it is
                            // shared.
                            pause.add_to_incin(Garbage::Bucket(alloc));
                        }

                        None
                    }
                };
            }

            // If none of other cases have been confirmed, the only remaining
            // case is a branching table. Let's try to look at it.
            table = &*((loaded as usize & !1) as *mut Self);
            // Shifting the hash so we test some other bits.
            shifted >>= BITS;
        }
    }

    // Unsafe because the incinerator needs to be paused and there are no
    // guarantees the passed pause comes from the incinerator used with the map
    // by other threads. Map implementation guarantees that.
    #[inline(never)]
    pub unsafe fn insert<I>(
        &self,
        mut inserter: I,
        hash: u64,
        pause: &Pause<Garbage<K, V>>,
        incin: &Arc<Incinerator<Garbage<K, V>>>,
    ) -> Insertion<K, V, I>
    where
        I: Inserter<K, V>,
        K: Ord,
    {
        let mut table = self;
        let mut shifted = hash;
        let mut depth = 1;
        let mut tbl_cache = Cache::<OwnedAlloc<Self>>::new();

        // Compute the index from the shifted hash's lower bits.
        let mut index = shifted as usize & ((1 << BITS) - 1);
        // Load what is in the index before trying to insert.
        let mut loaded = table.nodes[index].atomic.load(Acquire);

        loop {
            if loaded.is_null() {
                // Let's test the found conditions.
                inserter.input(None);
                let pair = match inserter.pointer() {
                    // The inserter accepted the conditions.
                    Some(nnptr) => nnptr,
                    // The inserter rejected the conditions.
                    None => break Insertion::Failed(inserter),
                };

                // Allocation of a bucket containing a single entry. Our pair.
                let bucket = Bucket::new(hash, pair);
                let bucket_nnptr = OwnedAlloc::new(bucket).into_raw();

                // We try to put it in the index.
                let res = table.nodes[index].atomic.compare_exchange(
                    loaded,
                    bucket_nnptr.as_ptr() as *mut (),
                    AcqRel,
                    Acquire,
                );

                match res {
                    Ok(_) => {
                        // Let's not forget to prevent the inserter from
                        // deallocating the pointer.
                        inserter.take_pointer();
                        break Insertion::Created;
                    }

                    Err(new) => {
                        // If we failed this try, we have to clean up.
                        let mut bucket = OwnedAlloc::from_raw(bucket_nnptr);
                        bucket.take_first();
                        loaded = new;
                    }
                }
            } else if loaded as usize & 1 == 0 {
                // We keep pointers to Buckets with the lower bit cleared.
                let bucket = &*(loaded as *mut Bucket<K, V>);

                // If the hash of the bucket is equal to ours, there is no need
                // for us to branch. Actually, we must not do it. We must insert
                // in the bucket.
                if bucket.hash() == hash {
                    match bucket.insert(inserter, pause, incin) {
                        InsertRes::Created => break Insertion::Created,

                        InsertRes::Updated(old) => {
                            break Insertion::Updated(old);
                        }

                        InsertRes::Failed(inserter) => {
                            break Insertion::Failed(inserter);
                        }

                        // This means we must delete the bucket entirely. And
                        // try again, obviously.
                        InsertRes::Delete(returned) => {
                            let ptr = &table.nodes[index].atomic;
                            let res = ptr.compare_exchange(loaded, null_mut(), AcqRel, Acquire);

                            match res {
                                Ok(_) => {
                                    let alloc = OwnedAlloc::from_raw(NonNull::new_unchecked(
                                        loaded as *mut _,
                                    ));
                                    incin.add(Garbage::Bucket(alloc));
                                    loaded = null_mut()
                                }

                                Err(new) => {
                                    loaded = new;
                                }
                            }

                            inserter = returned;
                        }
                    }
                } else {
                    // In the case hashes aren't equal, we will branch!
                    let new_table = tbl_cache.take_or(|| Self::new_alloc());
                    let other_shifted = bucket.hash() >> (depth * BITS);
                    let other_index = other_shifted as usize & ((1 << BITS) - 1);

                    // Placing the found bucket into the new table first.
                    new_table.nodes[other_index].atomic.store(loaded, Relaxed);

                    let new_table_nnptr = new_table.into_raw();
                    let res = table.nodes[index].atomic.compare_exchange(
                        loaded,
                        // Note we mark the lower bit!
                        (new_table_nnptr.as_ptr() as usize | 1) as *mut (),
                        AcqRel,
                        Acquire,
                    );

                    match res {
                        Ok(_) => {
                            // If we succeeded, let's act like we found another
                            // table in this index.
                            depth += 1;
                            table = &*new_table_nnptr.as_ptr();
                            shifted >>= BITS;
                            // Compute the index from the shifted hash's lower
                            // bits.
                            index = shifted as usize & ((1 << BITS) - 1);
                            // Load what is in the index before trying to
                            // insert.
                            loaded = table.nodes[index].atomic.load(Acquire);
                        }

                        Err(new) => {
                            // If we failed -> clean up! And store the
                            // allocation in
                            // some cache, since allocating a table can be
                            // really expensive due
                            // to it's size.
                            let new_table = OwnedAlloc::from_raw(new_table_nnptr);
                            new_table.nodes[other_index].atomic.store(null_mut(), Relaxed);
                            tbl_cache.store(new_table);
                            loaded = new;
                        }
                    }
                }
            } else {
                // If none of other cases have been confirmed, the only
                // remaining case is a branching table. Let's
                // try to look at it.
                depth += 1;
                table = &*((loaded as usize & !1) as *mut Self);
                shifted >>= BITS;

                // Compute the index from the shifted hash's lower
                // bits.
                index = shifted as usize & ((1 << BITS) - 1);
                // Load what is in the index before trying to
                // insert.
                loaded = table.nodes[index].atomic.load(Acquire);
            }
        }
    }

    // Unsafe because the incinerator needs to be paused and there are no
    // guarantees the passed pause comes from the incinerator used with the map
    // by other threads. Map implementation guarantees that.
    pub unsafe fn remove<Q, F>(
        &self,
        key: &Q,
        interactive: F,
        hash: u64,
        pause: &Pause<Garbage<K, V>>,
        incin: &Arc<Incinerator<Garbage<K, V>>>,
    ) -> Option<Removed<K, V>>
    where
        Q: ?Sized + Ord,
        K: Borrow<Q>,
        F: FnMut(&(K, V)) -> bool,
    {
        let mut table = self;
        let mut shifted = hash;

        loop {
            // Compute the index from the shifted hash's lower bits.
            let index = shifted as usize & ((1 << BITS) - 1);
            // Let's load to see what is in there.
            let loaded = table.nodes[index].atomic.load(Acquire);

            // Null means we have nothing.
            if loaded.is_null() {
                break None;
            }

            // Cleared lower bit means this is a bucket.
            if loaded as usize & 1 == 0 {
                let bucket = &*(loaded as *mut Bucket<K, V>);

                // This bucket only matters if it has the same hash we do.
                if bucket.hash() != hash {
                    break None;
                }

                let res = bucket.remove(key, interactive, pause, incin);

                // If this field is true it means the whole bucket must be
                // removed. Regardless of failure or success.
                if res.delete {
                    let res = table.nodes[index].atomic.compare_exchange(
                        loaded,
                        null_mut(),
                        Relaxed,
                        Relaxed,
                    );

                    if res.is_ok() {
                        let alloc = OwnedAlloc::from_raw(NonNull::new_unchecked(loaded as *mut _));
                        incin.add(Garbage::Bucket(alloc));
                    }
                }
                break res.pair;
            }

            // If none of other cases have been confirmed, the only remaining
            // case is a branching table. Let's try to look at it.
            table = &*((loaded as usize & !1) as *mut Self);
            // Shifting the hash so we test some other bits.
            shifted >>= BITS;
        }
    }

    // Unsafe because calling this function and using the table again later will
    // cause undefined behavior.
    #[inline]
    pub unsafe fn free_nodes(&mut self, tbl_stack: &mut Vec<OwnedAlloc<Table<K, V>>>) {
        for node in &self.nodes as &[Node<K, V>] {
            Node::free_ptr(node.atomic.load(Relaxed), tbl_stack);
        }
    }

    #[inline]
    pub fn clear(&mut self, tbl_stack: &mut Vec<OwnedAlloc<Table<K, V>>>) {
        for node in &self.nodes as &[Node<K, V>] {
            // This should be safe because we store only proper pointers.
            unsafe {
                Node::free_ptr(node.atomic.swap(null_mut(), Relaxed), tbl_stack);
            }
        }
    }

    pub fn optimize_space(&mut self) -> OptSpaceRes<K, V> {
        let mut removed = 0usize;
        let mut last_bucket = None;

        for node in &self.nodes as &[Node<K, V>] {
            let loaded = node.atomic.load(Relaxed);

            if loaded.is_null() {
                removed += 1;
            } else if loaded as usize & 1 == 0 {
                let bucket_ptr = loaded as *mut Bucket<K, V>;
                // This is safe because:
                //
                // 1. We have exclusive reference to the table.
                //
                // 2. We proper allocate pointers stored in the table.
                //
                // 3. Bucket pointers are not marked and we checked for it.
                if unsafe { (*bucket_ptr).is_empty() } {
                    node.atomic.store(null_mut(), Release);
                    removed += 1;

                    // This is safe because we have exclusive reference to the
                    // map. Also, we remove the bucket from the table so no one
                    // else will find it.
                    unsafe {
                        OwnedAlloc::from_raw(NonNull::new_unchecked(bucket_ptr));
                    }
                } else {
                    // Safe because of the same things in the list above. Also,
                    // we checked for null already, we can by-pass this check.
                    let nnptr = unsafe { NonNull::new_unchecked(bucket_ptr) };
                    last_bucket = Some(nnptr);
                }
            } else {
                let table_ptr = (loaded as usize & !1) as *mut Table<K, V>;

                // This is safe because:
                //
                // 1. We have exclusive reference to the table.
                //
                // 2. We proper allocate pointers stored in the table.
                //
                // 3. Table pointers are marked and we checked for it.
                //
                // 4. We cleared the marked bit.
                match unsafe { &mut *table_ptr }.optimize_space() {
                    OptSpaceRes::NoOpt => (),

                    OptSpaceRes::Remove => {
                        node.atomic.store(null_mut(), Relaxed);
                        // This is safe because we have exclusive reference to
                        // the map. Also, we remove the inner table from the
                        // outer table so no one else will find it.
                        unsafe {
                            let nnptr = NonNull::new_unchecked(table_ptr);
                            OwnedAlloc::from_raw(nnptr);
                        }
                        removed += 1;
                    }

                    OptSpaceRes::TableToBucket(bucket) => {
                        unsafe {
                            // This is safe because we have exclusive reference
                            // to the map. Also, we remove the inner table from
                            // the outer table so no one else will find it.
                            let nnptr = NonNull::new_unchecked(table_ptr);
                            OwnedAlloc::from_raw(nnptr);
                        }
                        node.atomic.store(bucket.as_ptr() as *mut _, Relaxed)
                    }
                }
            }
        }

        match (last_bucket, self.nodes.len() - removed) {
            (Some(nnptr), 1) => OptSpaceRes::TableToBucket(nnptr),

            (_, 0) => OptSpaceRes::Remove,

            _ => OptSpaceRes::NoOpt,
        }
    }

    pub fn load_index(&self, index: usize, ordering: Ordering) -> Option<*mut ()> {
        self.nodes.get(index).map(|node| node.atomic.load(ordering))
    }
}

impl<K, V> fmt::Debug for Table<K, V> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "Table {{ nodes: {:?} }}", &self.nodes as &[Node<K, V>])
    }
}

struct Node<K, V> {
    // First lower bit is 0 for leaf and 1 for branch
    atomic: AtomicPtr<()>,
    _marker: PhantomData<(K, V)>,
}

impl<K, V> Node<K, V> {
    // Unsafe because it is *pretty easy* to make undefined behavior out of this
    // because the pointer does not have even a fixed type.
    unsafe fn free_ptr(ptr: *mut (), tbl_stack: &mut Vec<OwnedAlloc<Table<K, V>>>) {
        if ptr.is_null() {
            return;
        }

        if ptr as usize & 1 == 0 {
            OwnedAlloc::from_raw(NonNull::new_unchecked(ptr as *mut Bucket<K, V>));
        } else {
            let table_ptr = (ptr as usize & !1) as *mut Table<K, V>;

            debug_assert!(!table_ptr.is_null());
            tbl_stack.push(OwnedAlloc::from_raw(NonNull::new_unchecked(table_ptr)));
        }
    }
}

impl<K, V> Node<K, V> {
    fn new() -> Self {
        Self { atomic: AtomicPtr::new(null_mut()), _marker: PhantomData }
    }
}

impl<K, V> fmt::Debug for Node<K, V> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "Node {{ pointer: {:?} }}", self.atomic)
    }
}

pub enum OptSpaceRes<K, V> {
    NoOpt,
    Remove,
    TableToBucket(NonNull<Bucket<K, V>>),
}
