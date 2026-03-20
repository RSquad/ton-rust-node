use std::{
    fmt,
    mem::{replace, MaybeUninit},
    sync::atomic::{
        AtomicBool,
        Ordering::{self, *},
    },
};

/// A shared removable value. You can only take values from this type (no
/// insertion allowed). No extra allocation is necessary. It may be useful for
/// things like shared `thread::JoinHandle`s.
pub struct Removable<T> {
    item: MaybeUninit<T>,
    present: AtomicBool,
}

impl<T> Removable<T> {
    /// Creates a removable item with the passed argument as a present value.
    pub fn new(val: T) -> Self {
        Self { item: MaybeUninit::new(val), present: AtomicBool::new(true) }
    }

    /// Creates a removable item with no present value.
    pub fn empty() -> Self {
        Self {
            // This is safe because we will only read from the item if present
            // is true. Present will only be true if we write to it.
            item: MaybeUninit::uninit(),
            present: AtomicBool::new(false),
        }
    }

    /// Replaces the stored value with a given one and returns the old value.
    /// Requires a mutable reference since the type of the value might not be
    /// atomic.
    pub fn replace(&mut self, val: Option<T>) -> Option<T> {
        let present = self.present.get_mut();

        match val {
            Some(val) => {
                if *present {
                    // Safe because if present was true, the memory was initialized. All
                    // and present will only be false if item is uninitialized.
                    unsafe { Some(replace(&mut self.item, MaybeUninit::new(val)).assume_init()) }
                } else {
                    *present = true;
                    self.item = MaybeUninit::new(val);
                    None
                }
            }

            None if *present => {
                // Safe because we get the pointer from a valid reference
                // and present will only be false if item is uninitialized.
                *present = false;
                Some(unsafe { replace(&mut self.item, MaybeUninit::uninit()).assume_init() })
            }

            None => None,
        }
    }

    /// Tries to get a mutable reference to the stored value. If the value was
    /// not present, `None` is returned.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        if *self.present.get_mut() {
            Some(unsafe { &mut *self.item.as_mut_ptr() })
        } else {
            None
        }
    }

    /// Tests if the stored value is present. Note that there are no guarantees
    /// that `take` will be successful if this method returns `true` because
    /// some other thread could take the value meanwhile.
    pub fn is_present(&self, ordering: Ordering) -> bool {
        self.present.load(ordering)
    }

    /// Tries to take the value. If no value was present in first place, `None`
    /// is returned. In terms of memory ordering, `AcqRel` should be enough.
    pub fn take(&self, ordering: Ordering) -> Option<T> {
        if self.present.swap(false, ordering) {
            // Safe because if present was true, the memory was initialized. All
            // other reads won't happen because we set present to false.
            Some(unsafe { core::ptr::read(self.item.as_ptr()) })
        } else {
            None
        }
    }
}

impl<T> fmt::Debug for Removable<T> {
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        write!(fmtr, "Removable {{ present: {:?} }}", self.is_present(Relaxed))
    }
}

impl<T> Default for Removable<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> Drop for Removable<T> {
    fn drop(&mut self) {
        if *self.present.get_mut() {
            // Safe because present will only be true when the memory is
            // initialized. And now we are at drop.
            // it will be automatically dropped
            unsafe {
                core::ptr::read(self.item.as_ptr());
            }
        }
    }
}

impl<T> From<Option<T>> for Removable<T> {
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(item) => Self::new(item),
            None => Self::empty(),
        }
    }
}

unsafe impl<T> Send for Removable<T> where T: Send {}
unsafe impl<T> Sync for Removable<T> where T: Send {}
