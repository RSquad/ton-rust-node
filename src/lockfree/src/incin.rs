use crate::tls::ThreadLocal;
use std::{
    cell::Cell,
    fmt,
    marker::PhantomData,
    sync::atomic::{AtomicUsize, Ordering::*},
};

/// The incinerator. It is an API used to solve the infamous ABA problem. It
/// basically consists of a counter and a list of garbage. Before a thread
/// begins a suffering-from-ABA operation, it should start a new pause, and keep
/// the incinerator paused while it is performing the operation.
///
/// When a thread wants to drop an allocation that might affect other threads
/// with ABA problem or uses-after-free, it should `add` it to the incinerator's
/// garbage list. The incinerator will only execute the `drop` of its type `T`
/// when the pause counter is zero.
///
/// When the incinerator is dropped, all the garbage is automatically dropped
/// too.
///
/// C11 Implementation: <https://gitlab.com/bzim/c11-incinerator/>
///
/// # Example
/// ```rust
/// extern crate lockfree;
///
/// use lockfree::incin::Incinerator;
/// use std::{
///     ptr::{null_mut, NonNull},
///     sync::{
///         atomic::{AtomicPtr, Ordering::*},
///         Arc,
///     },
///     thread,
/// };
///
/// let incin = Arc::new(Incinerator::<Box<u128>>::new());
/// let ptr = Box::into_raw(Box::new(55u128));
/// let dummy_state = Arc::new(AtomicPtr::new(ptr));
///
/// let mut threads = Vec::with_capacity(16);
///
/// for i in 0 .. 16 {
///     let state = dummy_state.clone();
///     let incin = incin.clone();
///     threads.push(thread::spawn(move || {
///         let ptr = incin.pause_with(|_| {
///             let loaded = state.load(SeqCst);
///             let new = unsafe { *loaded + i };
///             state.swap(Box::into_raw(Box::new(new)), SeqCst)
///         });
///
///         // dropping
///         incin.add(unsafe { Box::from_raw(ptr) })
///     }));
/// }
///
/// for thread in threads {
///     thread.join().unwrap();
/// }
///
/// let boxed = unsafe { Box::from_raw(dummy_state.load(SeqCst)) };
/// assert!(*boxed <= 15 * 15);
/// ```
#[derive(Debug)]
pub struct Incinerator<T> {
    counter: AtomicUsize,
    tls_list: ThreadLocal<GarbageList<T>>,
}

impl<T> Incinerator<T> {
    /// Creates a new incinerator, with no pauses and empty garbage list.
    pub fn new() -> Self {
        Self { counter: AtomicUsize::new(0), tls_list: ThreadLocal::new() }
    }

    /// Increments the pause counter and creates a pause associated with this
    /// incinerator. Only after creating the pause you should perform atomic
    /// operations such as `load` and any other operation affected by ABA
    /// problem. This operation performs [`AcqRel`] on the pause counter.
    pub fn pause(&self) -> Pause<'_, T> {
        let mut count = self.counter.load(Relaxed);
        loop {
            // Sanity check.
            if count == usize::MAX {
                panic!("Too many pauses");
            }
            // Simply try to increment it. This will be decremented at
            // `Pause::drop`. Nobody will be able to drop stuff while this is
            // not 0.
            match self.counter.compare_exchange(count, count + 1, AcqRel, Relaxed) {
                Ok(_) => {
                    break Pause {
                        incin: self,
                        had_list: self.tls_list.get().is_some(),
                        _unsync: PhantomData,
                    };
                }

                Err(new) => count = new,
            }
        }
    }

    /// Creates a pause before executing the given closure and resumes the
    /// incinerator only after executing the closure. You should execute the
    /// whole ABA-problem-suffering cycle of `load` and `compare_exchange`
    /// inside the closure. See documentation for [`Incinerator::pause`] and
    /// `Pause::resume` for more details.
    pub fn pause_with<F, A>(&self, exec: F) -> A
    where
        F: FnOnce(&Pause<T>) -> A,
    {
        let pause = self.pause();
        let ret = exec(&pause);
        pause.resume();
        ret
    }

    /// Adds the given value to the garbage list. The value is only dropped when
    /// the counter is zero. If the counter is zero when the method is called,
    /// the value is immediately dropped and the garbage list is cleared. You
    /// must remove the resource from shared context before calling this method.
    /// This operation performs [`Acquire`] on the pause counter.
    pub fn add(&self, val: T) {
        if self.counter.load(Acquire) == 0 {
            // Safe to drop it all. Note that we check the counter after the
            // resource was removed from shared context. Since we use Thread
            // Local Storage, nobody can add something to the list meanwhile
            // besides us.
            self.tls_list.get().map(GarbageList::clear);
            drop(val);
        } else {
            // Not safe to drop. We have to save the value in the garbage list.
            self.tls_list.with_init(GarbageList::new).add(val);
        }
    }

    /// Tries to delete the garbage list associated with this thread. The
    /// garbage list is only cleared if the counter is zero. In case of success,
    /// `true` is returned. This operation performs [`Acquire`] on the pause
    /// counter.
    pub fn try_clear(&self) -> bool {
        if self.counter.load(Acquire) == 0 {
            // It is only safe to drop if there are no active pauses. Remember
            // nobody can add something to this specific list besides us because
            // it is thread local.
            self.tls_list.get().map(GarbageList::clear);
            true
        } else {
            false
        }
    }

    /// Clears everything that is in the inicinerator regardless of pauses.
    /// Exclusive reference is required.
    pub fn clear(&mut self) {
        self.tls_list.clear();
    }
}

impl<T> Default for Incinerator<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// An active incinerator pause. When a value of this type is alive, no
/// sensitive data is dropped in the incinerator. When a value of this type is
/// dropped, the incinerator counter is decremented.
#[derive(Debug)]
pub struct Pause<'incin, T>
where
    T: 'incin,
{
    incin: &'incin Incinerator<T>,
    had_list: bool,
    _unsync: PhantomData<*mut ()>,
}

impl<'incin, T> Pause<'incin, T> {
    /// Returns the incinerator on which this pause acts.
    pub fn incin(&self) -> &Incinerator<T> {
        self.incin
    }

    /// Adds the given value to the garbage list of the incinerator but if the
    /// counter is `1` (i.e. this is the only active pause) data is immediately
    /// dropped. See documention for [`Incinerator::add`] for more. This
    /// operation performs [`Acquire`] on the pause counter.
    pub fn add_to_incin(&self, val: T) {
        if self.incin.counter.load(Acquire) == 1 {
            // We are the only pause active in this case.
            //
            // Safe to drop it all. Note that we check the counter after the
            // resource was removed from shared context. Since we use Thread
            // Local Storage, nobody can add something to the list meanwhile
            // besides us.
            if self.had_list {
                self.incin.tls_list.get().map(GarbageList::clear);
            }
            drop(val);
        } else {
            // Not safe to drop. We have to save the value in the garbage list.
            self.incin.tls_list.with_init(GarbageList::new).add(val);
        }
    }

    /// Forces drop and decrements the incinerator counter. If the counter
    /// becomes 0, the list associated with this thread is cleared. This method
    /// does not need to be called because the incinerator counter is
    /// decremented when the pause is dropped. This operation performs
    /// [`AcqRel`] on the pause counter.
    pub fn resume(self) {}
}

impl<'incin, T> Drop for Pause<'incin, T> {
    fn drop(&mut self) {
        if self.incin.counter.fetch_sub(1, AcqRel) == 1 {
            // If the previous value was 1, this means now it is 0 and... we can
            // delete our local list.
            self.incin.tls_list.get().map(GarbageList::clear);
        }
    }
}

impl<'incin, T> Clone for Pause<'incin, T> {
    fn clone(&self) -> Self {
        self.incin.pause()
    }
}

unsafe impl<'incin, T> Send for Pause<'incin, T> where T: Send {}

struct GarbageList<T> {
    list: Cell<Vec<T>>,
}

impl<T> GarbageList<T> {
    fn new() -> Self {
        Self { list: Cell::new(Vec::new()) }
    }

    fn add(&self, val: T) {
        let mut list = self.list.replace(Vec::new());
        list.push(val);
        self.list.replace(list);
    }

    fn clear(&self) {
        self.list.replace(Vec::new());
    }
}

impl<T> fmt::Debug for GarbageList<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, fmtr: &mut fmt::Formatter) -> fmt::Result {
        let list = self.list.replace(Vec::new());
        write!(fmtr, "{:?}", list)?;

        let mut tmp = self.list.replace(list);

        // A totally weird corner case, but we have to handle it.
        if !tmp.is_empty() {
            let mut list = self.list.replace(Vec::new());
            list.append(&mut tmp);
            self.list.replace(list);
        }
        Ok(())
    }
}

macro_rules! doc {
    ($doc:expr ; $($target:tt)*) => {
        #[doc = $doc]
        $($target)*
    };
}

macro_rules! make_shared_incin {
    (
        { $target:expr }
        $(#[$meta:meta])*
        $vis:vis $name:ident<$($params:ident),*> of $garbage:ty
    ) => {
        doc! {
            concat!("The shared incinerator used by ", $target, ". You may \
                     want to use this type in order to reduce memory \
                     consumption of the minimal space required by the \
                     incinerator. However, garbage items may be hold for \
                     longer time than they would if no shared incinerator \
                     were used.");
            $(#[$meta])*
            $vis struct $name<$($params),*> {
                inner: ::std::sync::Arc<crate::incin::Incinerator<$garbage>>,
            }
        }

        impl<$($params),*> $name<$($params),*> {
            doc! {
                concat!("Creates a new shared incinerator for ", $target, ".");
                $vis fn new() -> Self {
                    use std::sync::Arc;
                    use crate::incin::Incinerator;
                    Self {
                        inner: Arc::new(Incinerator::new()),
                    }
                }
            }

            doc! {
                concat!("Tries to clear the incinerator garbage list in the \
                         best possible way given the runtime status of this \
                         incinerator.");
                $vis fn clear(&mut self) {
                    use std::{
                        mem::replace,
                        sync::Arc,
                    };
                    // use incin::Incinerator;

                    // I know this sounds weird. This is because Arc::get_mut
                    // locks stuff. We don't want that.
                    let arc = replace(&mut self.inner, Arc::new(Default::default()));

                    match Arc::try_unwrap(arc) {
                        Ok(mut incin) => {
                            incin.clear();
                            self.inner = Arc::new(incin);
                        },

                        Err(arc) => {
                            arc.try_clear();
                            self.inner = arc;
                        }
                    }
                }
            }
        }

        impl<$($params),*> Default for $name<$($params),*> {
            fn default() -> Self {
                Self::new()
            }
        }

        impl<$($params),*> Clone for $name<$($params),*> {
            fn clone(&self) -> Self {
                Self {
                    inner: self.inner.clone(),
                }
            }
        }
    };
}
