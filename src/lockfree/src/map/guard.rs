use super::bucket::Garbage;
use crate::incin::{Incinerator, Pause};
use owned_alloc::OwnedAlloc;
use std::{
    borrow::Borrow,
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    mem::forget,
    ops::Deref,
    ptr::NonNull,
    sync::{Arc, Weak},
};

/// A read-operation guard. This ensures no entry allocation is
/// mutated or freed while potential reads are performed.
#[derive(Debug)]
pub struct ReadGuard<'map, K, V>
where
    K: 'map,
    V: 'map,
{
    pair: &'map (K, V),
    _pause: Pause<'map, Garbage<K, V>>,
}

impl<'map, K, V> ReadGuard<'map, K, V> {
    pub(super) fn new(pair: &'map (K, V), _pause: Pause<'map, Garbage<K, V>>) -> Self {
        Self { pair, _pause }
    }

    /// Utility method. Returns the key of this borrowed entry.
    // Shouldn't this be an associated function instead?
    pub fn key(&self) -> &K {
        let (k, _) = &**self;
        k
    }

    /// Utility method. Returns the value of this borrowed entry.
    // Shouldn't this be an associated function instead?
    pub fn val(&self) -> &V {
        let (_, v) = &**self;
        v
    }
}

impl<'map, K, V> Deref for ReadGuard<'map, K, V> {
    type Target = (K, V);

    fn deref(&self) -> &Self::Target {
        self.pair
    }
}

impl<'map, K, V> PartialEq for ReadGuard<'map, K, V>
where
    (K, V): PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<'map, K, V> PartialEq<(K, V)> for ReadGuard<'map, K, V>
where
    (K, V): PartialEq,
{
    fn eq(&self, other: &(K, V)) -> bool {
        **self == *other
    }
}

impl<'map, K, V> Eq for ReadGuard<'map, K, V> where (K, V): Eq {}

impl<'map, K, V> PartialOrd for ReadGuard<'map, K, V>
where
    (K, V): PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<'map, K, V> PartialOrd<(K, V)> for ReadGuard<'map, K, V>
where
    (K, V): PartialOrd,
{
    fn partial_cmp(&self, other: &(K, V)) -> Option<Ordering> {
        (**self).partial_cmp(other)
    }
}

impl<'map, K, V> Ord for ReadGuard<'map, K, V>
where
    (K, V): Ord,
{
    fn cmp(&self, other: &Self) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<'map, K, V> Hash for ReadGuard<'map, K, V>
where
    (K, V): Hash,
{
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        (**self).hash(hasher)
    }
}

impl<'map, K, V> AsRef<(K, V)> for ReadGuard<'map, K, V> {
    fn as_ref(&self) -> &(K, V) {
        self
    }
}

impl<'map, K, V> Borrow<(K, V)> for ReadGuard<'map, K, V> {
    fn borrow(&self) -> &(K, V) {
        self
    }
}

unsafe impl<'map, K, V> Send for ReadGuard<'map, K, V>
where
    K: Send,
    V: Send,
{
}

unsafe impl<'map, K, V> Sync for ReadGuard<'map, K, V>
where
    K: Sync,
    V: Sync,
{
}

/// A removed entry. It can be reinserted at the same [`Map`](super::Map) it was
/// removed. It can also be inserted on another [`Map`](super::Map), but only if
/// either the [`Map`](super::Map) is dropped, there are no sensitive reads
/// running on that [`Map`](super::Map) or both [`Map`](super::Map)s share the
/// same incinerator.
pub struct Removed<K, V> {
    nnptr: NonNull<(K, V)>,
    origin: Weak<Incinerator<Garbage<K, V>>>,
}

impl<K, V> Removed<K, V> {
    pub(super) fn new(alloc: OwnedAlloc<(K, V)>, origin: &Arc<Incinerator<Garbage<K, V>>>) -> Self {
        Self { nnptr: alloc.into_raw(), origin: Arc::downgrade(origin) }
    }

    pub(super) fn into_alloc(mut this: Self) -> OwnedAlloc<(K, V)> {
        // It is safe because we own the allocation.
        let alloc = unsafe { OwnedAlloc::from_raw(this.nnptr) };
        // There is no other way of dropping the weak and forgetting ourselves.
        // Rust does not let us move fields of a `Drop` struct.
        unsafe { (&mut this.origin as *mut Weak<_>).drop_in_place() }
        forget(this);
        alloc
    }

    pub(super) fn raw(this: &Self) -> NonNull<(K, V)> {
        this.nnptr
    }

    pub(super) fn is_usable_by(this: &mut Self, origin: &Arc<Incinerator<Garbage<K, V>>>) -> bool {
        match &this.origin.upgrade() {
            None => true,
            Some(arc) if Arc::ptr_eq(arc, origin) => true,
            Some(arc) => {
                if arc.try_clear() {
                    this.origin = Weak::new();
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Utility method. Returns the key of this removed entry.
    // Shouldn't this be an associated function instead?
    pub fn key(&self) -> &K {
        let (k, _) = &**self;
        k
    }

    /// Utility method. Returns the value of this removed entry.
    // Shouldn't this be an associated function instead?
    pub fn val(&self) -> &V {
        let (_, v) = &**self;
        v
    }

    /// Tries to acquire a mutable reference to the pair. Succeeds only if
    /// either the original [`Map`](super::Map) was dropped or no sensitive
    /// reads are being performed.
    pub fn try_as_mut(this: &mut Self) -> Option<&mut (K, V)> {
        let success = match this.origin.upgrade() {
            None => true,
            Some(arc) => {
                if arc.try_clear() {
                    this.origin = Weak::new();
                    true
                } else {
                    false
                }
            }
        };

        if success {
            // We own the allocation. This must be safe.
            Some(unsafe { this.nnptr.as_mut() })
        } else {
            None
        }
    }

    /// Tries to convert this wrapper into the pair. Succeeds only if either the
    /// original [`Map`](super::Map) was dropped or no sensitive reads are being
    /// performed.
    pub fn try_into(this: Self) -> Result<(K, V), Self> {
        let success = match this.origin.upgrade() {
            None => true,
            Some(arc) => arc.try_clear(),
        };

        if success {
            // We own the allocation. This must be safe.
            let (ret, _) = unsafe { OwnedAlloc::from_raw(this.nnptr) }.move_inner();
            forget(this);
            Ok(ret)
        } else {
            Err(this)
        }
    }
}

impl<K, V> Drop for Removed<K, V> {
    fn drop(&mut self) {
        // We own the allocation. This must be safe.
        let alloc = unsafe { OwnedAlloc::from_raw(self.nnptr) };
        if let Some(incin) = self.origin.upgrade() {
            incin.add(Garbage::Pair(alloc))
        }
    }
}

impl<K, V> Deref for Removed<K, V> {
    type Target = (K, V);

    fn deref(&self) -> &Self::Target {
        // We own the allocation. This must be safe.
        unsafe { self.nnptr.as_ref() }
    }
}

impl<K, V> fmt::Debug for Removed<K, V>
where
    (K, V): fmt::Debug,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "{:?}", **self)
    }
}

impl<K, V> PartialEq for Removed<K, V>
where
    (K, V): PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<K, V> PartialEq<(K, V)> for Removed<K, V>
where
    (K, V): PartialEq,
{
    fn eq(&self, other: &(K, V)) -> bool {
        **self == *other
    }
}

impl<K, V> Eq for Removed<K, V> where (K, V): Eq {}

impl<K, V> PartialOrd for Removed<K, V>
where
    (K, V): PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<K, V> PartialOrd<(K, V)> for Removed<K, V>
where
    (K, V): PartialOrd,
{
    fn partial_cmp(&self, other: &(K, V)) -> Option<Ordering> {
        (**self).partial_cmp(other)
    }
}

impl<K, V> Ord for Removed<K, V>
where
    (K, V): Ord,
{
    fn cmp(&self, other: &Self) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<K, V> Hash for Removed<K, V>
where
    (K, V): Hash,
{
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        (**self).hash(hasher)
    }
}

impl<K, V> AsRef<(K, V)> for Removed<K, V> {
    fn as_ref(&self) -> &(K, V) {
        self
    }
}

impl<K, V> Borrow<(K, V)> for Removed<K, V> {
    fn borrow(&self) -> &(K, V) {
        self
    }
}

unsafe impl<K, V> Send for Removed<K, V>
where
    K: Send,
    V: Send,
{
}

unsafe impl<K, V> Sync for Removed<K, V>
where
    K: Sync,
    V: Sync,
{
}
