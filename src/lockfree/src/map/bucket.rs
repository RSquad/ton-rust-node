use super::{
    guard::{ReadGuard, Removed},
    insertion::Inserter,
};
use crate::{
    incin::{Incinerator, Pause},
    ptr::non_zero_null,
};
use owned_alloc::OwnedAlloc;
use std::{
    borrow::Borrow,
    cmp::Ordering,
    fmt, mem,
    ptr::{null_mut, NonNull},
    sync::{
        atomic::{AtomicPtr, Ordering::*},
        Arc,
    },
};

#[repr(align(/* at least */ 2))]
pub struct Bucket<K, V> {
    hash: u64,
    list: List<K, V>,
}

impl<K, V> Bucket<K, V> {
    pub fn new(hash: u64, pair: NonNull<(K, V)>) -> Self {
        // We create a bucket with a single entry.

        // First we create an entry for the pair whose next node is null.
        let entry = Entry { pair, next: null_mut() };

        // Then we create an intermediate node to keep the entry.
        let list = List::new(entry);
        let list_ptr = OwnedAlloc::new(list).into_raw().as_ptr();

        Self {
            hash,
            // Then we make the "sentinel" "root" entry (never deleted from the
            // bucket).
            list: List::new(Entry::root(list_ptr)),
        }
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    // Unsafe because it might need incinerator's pause.
    pub unsafe fn is_empty(&self) -> bool {
        (*self.list.atomic.load(Acquire)).is_empty()
    }

    pub fn take_first(&mut self) -> Option<OwnedAlloc<Entry<K, V>>> {
        // First let's load the root entry.
        //
        // Safe because of exclusive reference. We are the only ones accessing
        // it. We also *do not* store null pointers in list's AtomicPtr!
        let entry = unsafe { &mut *self.list.atomic.load(Relaxed) };
        let prev = entry.next;
        // Let's set the root entry's next field to null.
        entry.next = null_mut();

        NonNull::new(prev).map(|nnptr| {
            // It's safe because we only store properly allocated nodes. Also,
            // we have removed the node.
            let list = unsafe { OwnedAlloc::from_raw(nnptr) };
            let ptr = list.atomic.load(Relaxed);
            // Safe to by-pass null check because we never store null pointers
            // in list's AtomicPtr! Safe to deallocate because we removed the
            // node.
            unsafe { OwnedAlloc::from_raw(NonNull::new_unchecked(ptr)) }
        })
    }

    // Unsafe because it might need incinerator's pause and there is no
    // guarantee the passed pause by this thread comes from the same incinerator
    // from which other threads pass pauses.
    pub unsafe fn get<'map, Q>(
        &self,
        key: &Q,
        pause: Pause<'map, Garbage<K, V>>,
    ) -> GetRes<'map, K, V>
    where
        Q: ?Sized + Ord,
        K: Borrow<Q>,
    {
        match self.find(key, &pause) {
            // The table must delete the whole bucket.
            FindRes::Delete => GetRes::Delete(pause),

            // We found the entry.
            FindRes::Exact { curr, .. } => {
                GetRes::Found(ReadGuard::new(&*curr.as_ref().pair.as_ptr(), pause))
            }

            // We found no entry.
            FindRes::After { .. } => GetRes::NotFound,
        }
    }

    // Unsafe because it might need incinerator's pause and there is no
    // guarantee the passed pause by this thread comes from the same incinerator
    // from which other threads pass pauses. Also because the inserter must be
    // implemented correctly and must yield valid pointers.
    pub unsafe fn insert<I>(
        &self,
        mut inserter: I,
        pause: &Pause<Garbage<K, V>>,
        incin: &Arc<Incinerator<Garbage<K, V>>>,
    ) -> InsertRes<I, K, V>
    where
        I: Inserter<K, V>,
        K: Ord,
    {
        loop {
            match self.find(inserter.key(), pause) {
                // The table must delete the whole bucket.
                FindRes::Delete => break InsertRes::Delete(inserter),

                // We found an entry with equal key.
                FindRes::Exact { curr_list, curr } => {
                    // Let's test the found conditions. Let's test if the
                    // inserter "approves" it.
                    inserter.input(Some(curr.as_ref().pair.as_ref()));
                    // Then we try to extract the pair pointer.
                    let pair = match inserter.pointer() {
                        // The inserter approved the conditions.
                        Some(nnptr) => nnptr,
                        // The inserter rejected the conditions.
                        None => break InsertRes::Failed(inserter),
                    };
                    // Create a new entry with a new pair but same next field.
                    let new_entry = Entry { pair, next: curr.as_ref().next };
                    let new_ptr = OwnedAlloc::new(new_entry).into_raw();

                    // We extract the old pair.
                    let old_pair = curr.as_ref().pair;
                    // And now we try to update the place where the old entry
                    // was.
                    if curr_list.try_update(curr, new_ptr, pause) {
                        // Remember to prevent the inserter from deallocating.
                        inserter.take_pointer();
                        // Create a removed entry from the old pair.
                        let pair = OwnedAlloc::from_raw(old_pair);
                        let removed = Removed::new(pair, incin);
                        break InsertRes::Updated(removed);
                    }
                }

                // We found a spot to insert at.
                FindRes::After { prev_list, prev } => {
                    // Let's test the found conditions. Let's test if the
                    // inserter "approves" it.
                    inserter.input(None);
                    // Then we try to extract the pair pointer.
                    let pair = match inserter.pointer() {
                        // The inserter approved the conditions.
                        Some(nnptr) => nnptr,
                        // The inserter rejected the conditions.
                        None => break InsertRes::Failed(inserter),
                    };

                    // Create a new entry with the next field.
                    let curr_entry = Entry { pair, next: prev.as_ref().next };
                    // Make an intermediate node for it.
                    let curr_list = List::new(curr_entry);
                    let curr_nnptr = OwnedAlloc::new(curr_list).into_raw();

                    // Create a new predecessor for our freshly created entry.
                    let new_prev = Entry { pair: prev.as_ref().pair, next: curr_nnptr.as_ptr() };
                    let new_ptr = OwnedAlloc::new(new_prev).into_raw();

                    // And try to update.
                    if prev_list.try_update(prev, new_ptr, pause) {
                        // Remember to prevent the inserter from deallocating.
                        inserter.take_pointer();
                        break InsertRes::Created;
                    }

                    // Clean-up in case of failure.
                    OwnedAlloc::from_raw(curr_nnptr.as_ref().load());
                    OwnedAlloc::from_raw(curr_nnptr);
                }
            }
        }
    }

    // Unsafe because it might need incinerator's pause and there is no
    // guarantee the passed pause by this thread comes from the same incinerator
    // from which other threads pass pauses.
    pub unsafe fn remove<Q, F>(
        &self,
        key: &Q,
        mut interactive: F,
        pause: &Pause<Garbage<K, V>>,
        incin: &Arc<Incinerator<Garbage<K, V>>>,
    ) -> RemoveRes<K, V>
    where
        Q: ?Sized + Ord,
        K: Borrow<Q>,
        F: FnMut(&(K, V)) -> bool,
    {
        loop {
            match self.find(key, pause) {
                // The table must delete the whole bucket.
                FindRes::Delete => break RemoveRes { pair: None, delete: true },

                // We found an entry whose key matches the input.
                FindRes::Exact { curr_list, curr } => {
                    // Let's test if the met conditions are ok!
                    if !interactive(curr.as_ref().pair.as_ref()) {
                        break RemoveRes { pair: None, delete: false };
                    }

                    // Let's first remove it logically. Let's create an entry
                    // with same data... but marked!
                    let pair_ptr = curr.as_ref().pair;
                    let new_entry =
                        Entry { pair: pair_ptr, next: (curr.as_ref().next as usize | 1) as *mut _ };
                    let new_ptr = OwnedAlloc::new(new_entry).into_raw();

                    // Then we try to update where it was before.
                    if curr_list.try_update(curr, new_ptr, pause) {
                        let pair = OwnedAlloc::from_raw(pair_ptr);
                        break RemoveRes {
                            pair: Some(Removed::new(pair, incin)),
                            // Just some clean up.
                            delete: self.try_clear_first(pause),
                        };
                    }
                }

                // This means the entry was not found.
                FindRes::After { .. } => {
                    break RemoveRes { pair: None, delete: false };
                }
            }
        }
    }

    // Unsafe because it might need incinerator's pause and there is no
    // guarantee the passed pause by this thread comes from the same incinerator
    // from which other threads pass pauses.
    pub unsafe fn collect<'map>(
        &'map self,
        pause: &Pause<'map, Garbage<K, V>>,
        out: &mut Vec<ReadGuard<'map, K, V>>,
    ) {
        // The length to which we will truncate the vector at each retry.
        let trunc = out.len();

        'retry: loop {
            // Clean-up previous try.
            out.truncate(trunc);
            let mut prev_list = &self.list;
            let mut prev = prev_list.load();

            loop {
                match prev_list.load_next(prev, pause) {
                    LoadNextRes::Failed => continue 'retry,
                    LoadNextRes::End => break 'retry,
                    LoadNextRes::Cleared { new_prev } => prev = new_prev,
                    LoadNextRes::Ok { list, entry } => {
                        out.push(ReadGuard::new(&*entry.as_ref().pair.as_ptr(), pause.clone()));
                        prev_list = &*list.as_ptr();
                        prev = entry;
                    }
                }
            }
        }
    }

    // Returns whether the bucket is empty. Unsafe because it might need
    // incinerator's pause and there is no guarantee the passed pause by
    // this thread comes from the same incinerator from which other threads
    // pass pauses.
    unsafe fn try_clear_first(&self, pause: &Pause<Garbage<K, V>>) -> bool {
        let mut prev = self.list.load();
        loop {
            match self.list.load_next(prev, pause) {
                LoadNextRes::Failed => break false,
                LoadNextRes::End => break true,
                LoadNextRes::Cleared { new_prev } => prev = new_prev,
                LoadNextRes::Ok { .. } => break false,
            }
        }
    }

    // Unsafe because it might need incinerator's pause and there is no
    // guarantee the passed pause by this thread comes from the same incinerator
    // from which other threads pass pauses.
    unsafe fn find<'map, Q>(
        &'map self,
        key: &Q,
        pause: &Pause<Garbage<K, V>>,
    ) -> FindRes<'map, K, V>
    where
        Q: ?Sized + Ord,
        K: Borrow<Q>,
    {
        'retry: loop {
            let mut prev_list = &self.list;
            let mut prev = prev_list.load();

            loop {
                match prev_list.load_next(prev, pause) {
                    LoadNextRes::Failed => continue 'retry,

                    LoadNextRes::End => {
                        // If the previous is the root and we reached the end we
                        // should delete the whole bucket.
                        break 'retry if prev.as_ref().is_root() {
                            FindRes::Delete
                        } else {
                            // Otherwise it just means the key would fit better
                            // after the previous.
                            FindRes::After { prev_list, prev }
                        };
                    }

                    LoadNextRes::Cleared { new_prev } => prev = new_prev,

                    LoadNextRes::Ok { list, entry } => {
                        let comparison = {
                            let (stored_key, _) = entry.as_ref().pair.as_ref();
                            key.cmp(stored_key.borrow())
                        };

                        match comparison {
                            // The exact key.
                            Ordering::Equal => {
                                break 'retry FindRes::Exact {
                                    curr_list: &*list.as_ptr(),
                                    curr: entry,
                                };
                            }

                            // The previous is the point of insertion.
                            Ordering::Less => {
                                break 'retry FindRes::After { prev_list, prev };
                            }

                            // Let's keep looking.
                            Ordering::Greater => {
                                prev_list = &*list.as_ptr();
                                prev = entry;
                            }
                        }
                    }
                }
            }
        }
    }
}

impl<K, V> IntoIterator for Bucket<K, V> {
    type Item = OwnedAlloc<(K, V)>;

    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        // By-passing this null check is ok because we never store null pointer
        // on the list's AomticPtr.
        let nnptr = unsafe { NonNull::new_unchecked(self.list.atomic.load(Relaxed)) };
        let head = unsafe { OwnedAlloc::from_raw(nnptr) };
        mem::forget(self);
        // Making an owned allocation is safe because we have ownership over the
        // bucket.
        IntoIter {
            curr: NonNull::new(head.next).map(|nnptr| unsafe { OwnedAlloc::from_raw(nnptr) }),
        }
    }
}

impl<'map, K, V> IntoIterator for &'map mut Bucket<K, V> {
    type Item = (&'map K, &'map mut V);

    type IntoIter = IterMut<'map, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        // By-passing this null check is ok because we never store null pointer
        // on the list's AomticPtr.
        let head = unsafe { &mut *self.list.atomic.load(Relaxed) };
        // This dereferral is ok because we have exclusive reference to the
        // bucket.
        IterMut { curr: unsafe { head.next.as_mut() } }
    }
}

impl<K, V> Drop for Bucket<K, V> {
    fn drop(&mut self) {
        unsafe {
            let ptr = self.list.atomic.load(Relaxed);
            let sentinel = NonNull::new_unchecked(ptr);
            let mut top = sentinel.as_ref().next;
            // Ok to deallocate it now since we already retrieved information.
            // Note that we have exclusive access to the bucket.
            OwnedAlloc::from_raw(sentinel);

            while let Some(list) = NonNull::new(top) {
                let ptr = list.as_ref().atomic.load(Relaxed);
                // By-passing this null check is ok because we never store null
                // pointer on the list's AomticPtr.
                let entry = NonNull::new_unchecked(ptr);
                // Ok to deallocate it now since we already retrieved
                // information. Note that we have exclusive
                // access to the bucket.
                OwnedAlloc::from_raw(list);

                let next = if entry.as_ref().next as usize & 1 == 0 {
                    // If the node is *not* marked, this entry was not removed
                    // and the pair needs to be deallocated. Ok to deallocate
                    // since we have exclusive reference.
                    OwnedAlloc::from_raw(entry.as_ref().pair);
                    entry.as_ref().next
                } else {
                    (entry.as_ref().next as usize & !1) as *mut _
                };
                // Ok to deallocate it now since we already retrieved
                // information. Note that we have exclusive
                // access to the bucket.
                OwnedAlloc::from_raw(entry);
                top = next;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Entry<K, V> {
    pair: NonNull<(K, V)>,
    next: *mut List<K, V>,
}

impl<K, V> Entry<K, V> {
    #[inline]
    pub fn root(next: *mut List<K, V>) -> Self {
        Self {
            // Use this dangling pointer to mark an entry as the "sentinel"
            // "root" entry.
            pair: non_zero_null(),
            next,
        }
    }

    #[inline]
    pub fn is_root(&self) -> bool {
        self.pair == non_zero_null()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.is_root() && self.next.is_null()
    }
}

impl<K, V> PartialEq for Entry<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.pair == other.pair && self.next == other.next
    }
}

impl<K, V> Eq for Entry<K, V> {}

#[repr(align(/* at least */ 2))]
pub struct List<K, V> {
    atomic: AtomicPtr<Entry<K, V>>,
}

impl<K, V> List<K, V> {
    #[inline]
    fn new(entry: Entry<K, V>) -> Self {
        let ptr = OwnedAlloc::new(entry).into_raw().as_ptr();
        Self { atomic: AtomicPtr::new(ptr) }
    }

    // Unsafe because `Bucket` needs to store entries correctly.
    unsafe fn load(&self) -> NonNull<Entry<K, V>> {
        NonNull::new_unchecked(self.atomic.load(Acquire))
    }

    // Loads the next and do clean-up if necessary. Unsafe because it might need
    // incinerator's pause and there is no guarantee the passed pause by
    // this thread comes from the same incinerator from which other threads
    // pass pauses. Also, `Bucket` needs to store entries correctly.
    unsafe fn load_next(
        &self,
        prev: NonNull<Entry<K, V>>,
        pause: &Pause<Garbage<K, V>>,
    ) -> LoadNextRes<K, V> {
        // Loading the previous node's next field (e.g. the "current" node).
        let list = match NonNull::new(prev.as_ref().next) {
            Some(nnptr) => nnptr,
            // The next is null; there is no next.
            None => return LoadNextRes::End,
        };

        let entry = list.as_ref().load();
        let next = entry.as_ref().next as usize;

        // If the next field was marked, this node was logically removed. Time
        // to remove it physically.
        if next & 1 == 1 {
            // Make a new previous node. A node with the same pair as the found
            // previous, but with next field pointing to current node's the
            // intermediate list.
            let new_entry = Entry { pair: prev.as_ref().pair, next: (next & !1) as *mut _ };
            let new_ptr = OwnedAlloc::new(new_entry).into_raw();

            // Then we try to update the previous node.
            if self.try_update(prev, new_ptr, pause) {
                // This is shared data. Must be deleted through the incinerator.
                pause.add_to_incin(Garbage::List(OwnedAlloc::from_raw(list)));
                pause.add_to_incin(Garbage::Entry(OwnedAlloc::from_raw(entry)));
                LoadNextRes::Cleared { new_prev: new_ptr }
            } else {
                LoadNextRes::Failed
            }
        } else {
            LoadNextRes::Ok { list, entry }
        }
    }

    // Tries to update this intermediate node and does clean-up of the passed
    // pointers. Unsafe because it might need incinerator's pause and there is
    // no guarantee the passed pause by this thread comes from the same
    // incinerator from which other threads pass pauses. Also, `Bucket`
    // needs to store entries correctly.
    unsafe fn try_update(
        &self,
        loaded: NonNull<Entry<K, V>>,
        new: NonNull<Entry<K, V>>,
        pause: &Pause<Garbage<K, V>>,
    ) -> bool {
        let res = self.atomic.compare_exchange(loaded.as_ptr(), new.as_ptr(), Release, Relaxed);

        if res == Ok(loaded.as_ptr()) {
            // Clean-up of the old pointer.
            let alloc = OwnedAlloc::from_raw(loaded);
            pause.add_to_incin(Garbage::Entry(alloc));
            true
        } else {
            // Clean-up of the tried new pointer.
            OwnedAlloc::from_raw(new);
            false
        }
    }
}

pub enum Garbage<K, V> {
    Pair(OwnedAlloc<(K, V)>),
    Entry(OwnedAlloc<Entry<K, V>>),
    List(OwnedAlloc<List<K, V>>),
    Bucket(OwnedAlloc<Bucket<K, V>>),
}

impl<K, V> fmt::Debug for Garbage<K, V> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Garbage::Pair(ptr) => write!(fmtr, "Garbage::Pair({:?})", ptr),
            Garbage::List(ptr) => write!(fmtr, "Garbage::List({:?})", ptr),
            Garbage::Bucket(ptr) => write!(fmtr, "Garbage::Bucket({:?})", ptr),
            Garbage::Entry(ptr) => write!(fmtr, "Garbage::Entry({:?})", ptr),
        }
    }
}

pub enum GetRes<'map, K, V>
where
    K: 'map,
    V: 'map,
{
    Found(ReadGuard<'map, K, V>),
    NotFound,
    Delete(Pause<'map, Garbage<K, V>>),
}

pub enum InsertRes<I, K, V> {
    Created,
    Updated(Removed<K, V>),
    Failed(I),
    Delete(I),
}

pub struct RemoveRes<K, V> {
    pub pair: Option<Removed<K, V>>,
    pub delete: bool,
}

enum FindRes<'map, K, V>
where
    K: 'map,
    V: 'map,
{
    Delete,

    Exact { curr_list: &'map List<K, V>, curr: NonNull<Entry<K, V>> },

    After { prev_list: &'map List<K, V>, prev: NonNull<Entry<K, V>> },
}

enum LoadNextRes<K, V> {
    Failed,

    End,

    Cleared { new_prev: NonNull<Entry<K, V>> },

    Ok { list: NonNull<List<K, V>>, entry: NonNull<Entry<K, V>> },
}

pub struct IntoIter<K, V> {
    curr: Option<OwnedAlloc<List<K, V>>>,
}

impl<K, V> IntoIter<K, V> {
    pub fn empty() -> Self {
        Self { curr: None }
    }
}

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = OwnedAlloc<(K, V)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let list = self.curr.take()?;
            // Safe because we only store non-null nodes.
            let entry_nnptr = unsafe { list.load() };
            // Safe because we have ownership over the nodes.
            let entry = unsafe { OwnedAlloc::from_raw(entry_nnptr) };
            // Safe because we have ownership over the nodes *and* we clear the
            // bit that may be set.
            self.curr = NonNull::new((entry.next as usize & !1) as *mut _)
                .map(|nnptr| unsafe { OwnedAlloc::from_raw(nnptr) });

            // Safe because, again, we have ownership over the nodes.
            if entry.next as usize & 1 == 0 {
                break Some(unsafe { OwnedAlloc::from_raw(entry.pair) });
            }
        }
    }
}

impl<K, V> Drop for IntoIter<K, V> {
    fn drop(&mut self) {
        for _ in self.by_ref() {}
    }
}

impl<K, V> fmt::Debug for IntoIter<K, V> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "{:?}", self.curr)
    }
}

pub struct IterMut<'map, K, V>
where
    K: 'map,
    V: 'map,
{
    curr: Option<&'map mut List<K, V>>,
}

impl<'map, K, V> IterMut<'map, K, V> {
    pub fn empty() -> Self {
        Self { curr: None }
    }
}

impl<'map, K, V> Iterator for IterMut<'map, K, V> {
    type Item = (&'map K, &'map mut V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let list = self.curr.take()?;
            let ptr = list.atomic.load(Relaxed);
            // Safe because we never store non-null nodes in list's AtomicPtr.
            let entry = unsafe { &mut *ptr };

            // Safe because we clear the only bit we mark. Also, we only store
            // properly allocated nodes.
            self.curr = unsafe {
                let cleared = entry.next as usize & !1;
                (cleared as *mut List<K, V>).as_mut()
            };

            if entry.next as usize & 1 == 0 {
                // Safe because the only case in which entry.pair is dangling is
                // when entry.next is marked. We checked for the mark.
                let (key, val) = unsafe { &mut *entry.pair.as_ptr() };
                break Some((&*key, val));
            }
        }
    }
}

impl<'map, K, V> fmt::Debug for IterMut<'map, K, V> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        match self.curr {
            Some(_) => fmtr.write_str("Some(_)"),
            None => fmtr.write_str("None"),
        }
    }
}
