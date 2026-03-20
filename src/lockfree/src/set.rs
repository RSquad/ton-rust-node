pub use crate::map::RandomState;
use crate::map::{
    Insertion as MapInsertion, IntoIter as MapIntoIter, Iter as MapIter, Map, Preview,
    ReadGuard as MapGuard, Removed as MapRemoved, SharedIncin as MapIncin,
};
use std::{
    borrow::Borrow,
    cmp::Ordering,
    fmt,
    hash::{BuildHasher, Hash},
    iter::FromIterator,
    ops::Deref,
};

/// A lock-free set. This is currently implemented on top of
/// [`Map`](::map::Map). To check more details about it, please see `Map` docs.
pub struct Set<T, H = RandomState> {
    inner: Map<T, (), H>,
}

impl<T> Set<T> {
    /// Creates a [`Set`] with the default hasher builder.
    pub fn new() -> Self {
        Self { inner: Map::new() }
    }

    /// Creates the [`Set`] using the given shared incinerator.
    pub fn with_incin(incin: SharedIncin<T>) -> Self {
        Self { inner: Map::with_incin(incin.inner) }
    }
}

impl<T, H> Set<T, H> {
    /// Creates an iterator over guarded references to the elements.
    pub fn iter(&self) -> Iter<'_, T> {
        self.into_iter()
    }
}

impl<T, H> Set<T, H>
where
    H: BuildHasher,
{
    /// Creates a [`Set`] with the given hasher builder.
    pub fn with_hasher(builder: H) -> Self {
        Self { inner: Map::with_hasher(builder) }
    }

    /// Creates the [`Set`] using the given hasher builder and shared
    /// incinerator.
    pub fn with_hasher_and_incin(builder: H, incin: SharedIncin<T>) -> Self {
        Self { inner: Map::with_hasher_and_incin(builder, incin.inner) }
    }

    /// The shared incinerator used by this `Map`.
    pub fn incin(&self) -> SharedIncin<T> {
        SharedIncin { inner: self.inner.incin() }
    }

    /// Returns the hasher builder used by this [`Set`].
    pub fn hasher(&self) -> &H {
        self.inner.hasher()
    }

    /// Tries to optimize space by removing unnecessary tables *without removing
    /// any element*. This method cannot be performed in a shared context.
    pub fn optimize_space(&mut self) {
        self.inner.optimize_space();
    }

    /// Removes all elements. This method cannot be performed in a shared
    /// context.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Tests if the given element is present on the [`Set`]. The method accepts
    /// a type resulted from borrowing the stored element. This method will
    /// only work correctly if [`Hash`] and [`Ord`] are implemented in the same
    /// way for the borrowed type and the stored type.
    pub fn contains<U>(&self, elem: &U) -> bool
    where
        U: Hash + Ord,
        T: Borrow<U>,
    {
        self.inner.get(elem).is_some()
    }

    /// Returns a guarded reference to the given element in the [`Set`]. This
    /// may be useful for types with additional metadata. The method accepts
    /// a type resulted from borrowing the stored element. This method will
    /// only work correctly if [`Hash`] and [`Ord`] are implemented in the same
    /// way for the borrowed type and the stored type. If the element is not
    /// found, [`None`] is obviously returned.
    pub fn get<'set, U>(&'set self, elem: &U) -> Option<ReadGuard<'set, T>>
    where
        U: Hash + Ord,
        T: Borrow<U>,
    {
        self.inner.get(elem).map(ReadGuard::new)
    }

    /// Inserts the element into the [`Set`]. If the element was already
    /// present, [`Err`]`(the_passed_value)` is returned.
    pub fn insert(&self, elem: T) -> Result<(), T>
    where
        T: Hash + Ord,
    {
        let result = self.inner.insert_with(elem, |_, _, stored| {
            if stored.is_some() {
                Preview::Discard
            } else {
                Preview::New(())
            }
        });
        match result {
            MapInsertion::Created => Ok(()),
            MapInsertion::Failed((elem, _)) => Err(elem),
            MapInsertion::Updated(_) => unreachable!(),
        }
    }

    /// Inserts _interactively_ the element into the [`Set`]. A passed closure
    /// tests if the insertion should proceed. The first argument of the
    /// closure is the element passed to `insert_with` and the second is the
    /// stored found element, if any. The closure returns whether the insertion
    /// should go on. This method is useful for types with metadata.
    pub fn insert_with<F>(&self, elem: T, mut interactive: F) -> Insertion<T, T>
    where
        F: FnMut(&T, Option<&T>) -> bool,
        T: Hash + Ord,
    {
        let result = self.inner.insert_with(elem, |elem, _, stored| {
            if interactive(elem, stored.map(|(elem, _)| elem)) {
                Preview::New(())
            } else {
                Preview::Discard
            }
        });

        match result {
            MapInsertion::Created => Insertion::Created,
            MapInsertion::Updated(old) => Insertion::Updated(Removed::new(old)),
            MapInsertion::Failed((elem, _)) => Insertion::Failed(elem),
        }
    }

    /// Tries to reinsert a previously removed element. The element must have
    /// been either:
    ///
    /// 1. Removed from this very [`Set`].
    /// 2. Removed from an already dead [`Set`].
    /// 3. Removed from a [`Set`] which has no sensitive reads active.
    ///
    /// If the removed element does not fit any category, the insertion will
    /// fail. Otherwise, insertion cannot fail.
    pub fn reinsert(&self, elem: Removed<T>) -> Result<(), Removed<T>>
    where
        T: Hash + Ord,
    {
        let result = self.inner.reinsert_with(elem.inner, |_, stored| stored.is_none());
        match result {
            MapInsertion::Created => Ok(()),
            MapInsertion::Failed(removed) => Err(Removed::new(removed)),
            MapInsertion::Updated(_) => unreachable!(),
        }
    }

    /// Tries to reinsert _interactively_ a previously removed element. A
    /// closure is passed to test if the reinsertion should proceed The first
    /// argument to the closure is a reference to the given element and the
    /// second is a reference to the found stored element, if any. The closure
    /// returns whether the reinsertion should go on. The removed element must
    /// have been either:
    ///
    /// 1. Removed from this very [`Set`].
    /// 2. Removed from an already dead [`Set`].
    /// 3. Removed from a [`Set`] which has no sensitive reads active.
    ///
    /// If the removed element does not fit any category, the insertion will
    /// fail. Otherwise, insertion cannot fail.
    pub fn reinsert_with<F>(&self, elem: Removed<T>, mut interactive: F) -> Insertion<T, Removed<T>>
    where
        F: FnMut(&T, Option<&T>) -> bool,
        T: Hash + Ord,
    {
        let result = self.inner.reinsert_with(elem.inner, |(elem, _), stored| {
            interactive(elem, stored.map(|(elem, _)| elem))
        });

        match result {
            MapInsertion::Created => Insertion::Created,
            MapInsertion::Updated(old) => Insertion::Updated(Removed::new(old)),
            MapInsertion::Failed(e) => Insertion::Failed(Removed::new(e)),
        }
    }

    /// Removes the given element inconditionally. The method accepts a
    /// type resulted from borrowing the stored element. This method will only
    /// work correctly if [`Hash`] and [`Ord`] are implemented in the same way
    /// for the borrowed type and the stored type.
    pub fn remove<U>(&self, elem: &U) -> Option<Removed<T>>
    where
        U: Hash + Ord,
        T: Borrow<U>,
    {
        self.inner.remove(elem).map(Removed::new)
    }

    /// Removes _interactively_ the given element. A closure is passed to
    /// validate the removal. The only argument passed to the closure is a
    /// reference to the found stored element. The return value is whether the
    /// removal should happen or not. The method accepts a type resulted from
    /// borrowing the stored element. This method will only work correctly
    /// if [`Hash`] and [`Ord`] are implemented in the same way for the borrowed
    /// type and the stored type.
    pub fn remove_with<U, F>(&self, elem: &U, mut interactive: F) -> Option<Removed<T>>
    where
        U: Hash + Ord,
        T: Borrow<U>,
        F: FnMut(&T) -> bool,
    {
        self.inner.remove_with(elem, |(elem, _)| interactive(elem)).map(Removed::new)
    }

    /// Acts just like [`Extend::extend`] but does not require mutability.
    #[allow(unused_must_use)]
    pub fn extend<I>(&self, iterable: I)
    where
        I: IntoIterator<Item = T>,
        T: Hash + Ord,
    {
        for val in iterable {
            self.insert(val);
        }
    }
}

impl<T, H> Default for Set<T, H>
where
    H: BuildHasher + Default,
{
    fn default() -> Self {
        Self { inner: Map::default() }
    }
}

impl<T, H> fmt::Debug for Set<T, H>
where
    H: fmt::Debug,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "Set {{ inner_map: {:?} }}", self.inner)
    }
}

impl<T, H> IntoIterator for Set<T, H> {
    type Item = T;

    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { inner: self.inner.into_iter() }
    }
}

impl<'set, T, H> IntoIterator for &'set Set<T, H> {
    type Item = ReadGuard<'set, T>;

    type IntoIter = Iter<'set, T>;

    fn into_iter(self) -> Self::IntoIter {
        Iter { inner: self.inner.iter() }
    }
}

impl<T, H> FromIterator<T> for Set<T, H>
where
    H: BuildHasher + Default,
    T: Hash + Ord,
{
    fn from_iter<I>(iterable: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        let this = Self::default();
        this.extend(iterable);
        this
    }
}

impl<T, H> Extend<T> for Set<T, H>
where
    H: BuildHasher,
    T: Hash + Ord,
{
    fn extend<I>(&mut self, iterable: I)
    where
        I: IntoIterator<Item = T>,
    {
        (*self).extend(iterable)
    }
}

/// An [`insert_with`](Set::insert_with) operation result.
#[derive(Debug, PartialEq, Eq)]
pub enum Insertion<T, E> {
    /// The element was created.
    Created,
    /// The element was updated and this was the old element.
    Updated(Removed<T>),
    /// The insertion failed and no operation was performed. Failure of an
    /// insertion might happen because the closure rejected the conditions.
    /// Another reason is that method-specific contract was not respected (such
    /// as the one of [`reinsert_with`](Set::reinsert_with)).
    Failed(E),
}

impl<T, E> Insertion<T, E> {
    /// Returns whether the insertion created an element.
    pub fn created(&self) -> bool {
        matches!(self, Insertion::Created)
    }

    /// Returns whether the insertion updated an element.
    pub fn updated(&self) -> Option<&Removed<T>> {
        match self {
            Insertion::Updated(pair) => Some(pair),
            _ => None,
        }
    }

    /// Tries to take the updated element of this insertion and encodes it as a
    /// [`Result`]. [`Ok`] is returned only if this insertion updated an
    /// element.
    pub fn take_updated(self) -> Result<Removed<T>, Self> {
        match self {
            Insertion::Updated(pair) => Ok(pair),
            this => Err(this),
        }
    }

    /// Returns whether the insertion failed.
    pub fn failed(&self) -> Option<&E> {
        match self {
            Insertion::Failed(err) => Some(err),
            _ => None,
        }
    }

    /// Tries to take the failure of this insertion and encodes it as a
    /// [`Result`]. [`Ok`] is returned only if this insertion has a failure.
    pub fn take_failed(self) -> Result<E, Self> {
        match self {
            Insertion::Failed(e) => Ok(e),
            this => Err(this),
        }
    }
}

/// A read-operation guard. This ensures no element allocation is
/// mutated or freed while potential reads are performed.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadGuard<'set, T>
where
    T: 'set,
{
    inner: MapGuard<'set, T, ()>,
}

impl<'set, T> ReadGuard<'set, T> {
    fn new(inner: MapGuard<'set, T, ()>) -> Self {
        Self { inner }
    }
}

impl<'set, T> Deref for ReadGuard<'set, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.inner.key()
    }
}

impl<'set, T> fmt::Debug for ReadGuard<'set, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        (**self).fmt(fmtr)
    }
}

impl<'set, T> fmt::Display for ReadGuard<'set, T>
where
    T: fmt::Display,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        (**self).fmt(fmtr)
    }
}

impl<'set, T> PartialEq<T> for ReadGuard<'set, T>
where
    T: PartialEq,
{
    fn eq(&self, other: &T) -> bool {
        **self == *other
    }
}

impl<'set, T> PartialOrd<T> for ReadGuard<'set, T>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &T) -> Option<Ordering> {
        (**self).partial_cmp(other)
    }
}

impl<'set, T> Borrow<T> for ReadGuard<'set, T> {
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<'set, T> AsRef<T> for ReadGuard<'set, T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

/// A removed element. It can be reinserted at the same [`Set`] it was removed.
/// It can also be inserted on another [`Set`], but only if either the [`Set`]
/// is dropped or there are no sensitive reads running on that [`Set`].
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Removed<T> {
    inner: MapRemoved<T, ()>,
}

impl<T> Removed<T> {
    fn new(inner: MapRemoved<T, ()>) -> Self {
        Self { inner }
    }

    /// Tries to acquire a mutable reference to the element. Succeeds only if
    /// either the original [`Set`] was dropped or no sensitive reads are being
    /// performed.
    pub fn try_as_mut(this: &mut Self) -> Option<&mut T> {
        MapRemoved::try_as_mut(&mut this.inner).map(|(elem, _)| elem)
    }

    /// Tries to convert this wrapper into the element. Succeeds only if either
    /// the original [`Set`] was dropped or no sensitive reads are being
    /// performed.
    pub fn try_into(this: Self) -> Result<T, Self> {
        match MapRemoved::try_into(this.inner) {
            Ok((elem, _)) => Ok(elem),
            Err(inner) => Err(Self::new(inner)),
        }
    }
}

impl<T> Deref for Removed<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.inner.key()
    }
}

impl<T> fmt::Debug for Removed<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        (**self).fmt(fmtr)
    }
}

impl<T> fmt::Display for Removed<T>
where
    T: fmt::Display,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        (**self).fmt(fmtr)
    }
}

impl<T> PartialEq<T> for Removed<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &T) -> bool {
        **self == *other
    }
}

impl<T> PartialOrd<T> for Removed<T>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &T) -> Option<Ordering> {
        (**self).partial_cmp(other)
    }
}

impl<T> Borrow<T> for Removed<T> {
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<T> AsRef<T> for Removed<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

/// An iterator over elements of a [`Set`]. The `Item` of this
/// iterator is a [`ReadGuard`].
#[derive(Debug)]
pub struct Iter<'set, T>
where
    T: 'set,
{
    inner: MapIter<'set, T, ()>,
}

impl<'set, T> Iterator for Iter<'set, T> {
    type Item = ReadGuard<'set, T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(ReadGuard::new)
    }
}

/// An iterator over owned elements of a [`Set`].
pub struct IntoIter<T> {
    inner: MapIntoIter<T, ()>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(elem, _)| elem)
    }
}

impl<T> fmt::Debug for IntoIter<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "IntoIter {{ inner: {:?} }}", self.inner)
    }
}

/// The shared incinerator used by [`Set`]. You may want to use this type
/// in order to reduce memory consumption of the minimal space required by the
/// incinerator. However, garbage items may be hold for longer time than they
/// would if no shared incinerator were used.
pub struct SharedIncin<T> {
    inner: MapIncin<T, ()>,
}

impl<T> SharedIncin<T> {
    /// Creates a new shared incinerator for [`Set`].
    pub fn new() -> Self {
        Self { inner: MapIncin::new() }
    }
}

impl<T> fmt::Debug for SharedIncin<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "{:?}", self.inner)
    }
}

impl<T> Default for SharedIncin<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for SharedIncin<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::{
        cmp::Ordering,
        hash::{Hash, Hasher},
    };

    #[derive(Debug, Clone, Copy)]
    struct EqI {
        i: usize,
        _j: usize,
    }

    impl PartialEq for EqI {
        fn eq(&self, other: &Self) -> bool {
            self.i == other.i
        }
    }

    impl Eq for EqI {}

    impl PartialOrd for EqI {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            self.i.partial_cmp(&other.i)
        }
    }

    impl Ord for EqI {
        fn cmp(&self, other: &Self) -> Ordering {
            self.i.cmp(&other.i)
        }
    }

    impl Hash for EqI {
        fn hash<H>(&self, hasher: &mut H)
        where
            H: Hasher,
        {
            self.i.hash(hasher)
        }
    }

    #[test]
    fn inserts_and_contains_checks() {
        let set = Set::new();
        assert!(!set.contains(&3));
        assert!(!set.contains(&5));
        set.insert(3).unwrap();
        assert!(set.contains(&3));
        assert!(!set.contains(&5));
        set.insert(3).unwrap_err();
        assert!(set.contains(&3));
        assert!(!set.contains(&5));
        set.insert(5).unwrap();
        assert!(set.contains(&3));
        assert!(set.contains(&5));
    }

    #[test]
    fn inserts_and_removes() {
        let set = Set::new();
        assert!(set.remove(&7).is_none());
        set.insert(7).unwrap();
        assert_eq!(set.remove(&7).unwrap(), 7);
        assert!(set.remove(&7).is_none());
        set.insert(3).unwrap();
        set.insert(5).unwrap();
        assert_eq!(set.remove(&5).unwrap(), 5);
        assert_eq!(set.remove(&3).unwrap(), 3);
        assert!(set.remove(&3).is_none());
        assert!(set.remove(&5).is_none());
    }

    #[test]
    fn inserts_and_reinserts() {
        let set = Set::new();
        set.insert(9).unwrap();
        set.insert(7).unwrap();
        set.insert(0).unwrap();
        let removed = set.remove(&9).unwrap();
        set.reinsert(removed).unwrap();
        set.insert(9).unwrap_err();
    }

    #[test]
    fn insert_with() {
        let set = Set::new();
        set.insert(EqI { i: 32, _j: 0 }).unwrap();
        set.insert(EqI { i: 34, _j: 10 }).unwrap();
        set.insert(EqI { i: 34, _j: 6 }).unwrap_err();
        set.insert_with(EqI { i: 34, _j: 6 }, |_, _| true).updated().unwrap();
        set.insert_with(EqI { i: 34, _j: 2 }, |_, _| false).failed().unwrap();
        assert!(set.insert_with(EqI { i: 33, _j: 2 }, |_, _| true).created());
        set.insert_with(EqI { i: 32, _j: 3 }, |_, _| true).updated().unwrap();
    }

    #[test]
    fn reinsert_with() {
        let set = Set::new();
        set.insert(EqI { i: 32, _j: 0 }).unwrap();
        set.insert(EqI { i: 34, _j: 10 }).unwrap();
        set.insert(EqI { i: 34, _j: 6 }).unwrap_err();
        let _34 = set.remove(&EqI { i: 34, _j: 325 }).unwrap();
        let _32 = set.remove(&EqI { i: 32, _j: 534 }).unwrap();

        set.insert(EqI { i: 34, _j: 6 }).unwrap();
        set.reinsert_with(_34, |_, _| true).updated().unwrap();
        let _32 = set.reinsert_with(_32, |_, _| false).take_failed().unwrap();
        assert!(set.reinsert_with(_32, |_, _| true).created());
    }
}
