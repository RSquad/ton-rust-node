use std::{
    mem::align_of,
    ptr::{null_mut, NonNull},
};

static _NON_NULL: u8 = /* dummy value */ 1;

// don't use with bit flags
#[inline(always)]
pub fn non_zero_null<T>() -> NonNull<T> {
    NonNull::from(&_NON_NULL).cast()
}

#[inline(always)]
pub unsafe fn bypass_null<T>(ptr: *mut T) -> NonNull<T> {
    debug_assert!(!ptr.is_null());
    NonNull::new_unchecked(ptr)
}

#[inline(always)]
pub fn check_null_align<T>() {
    debug_assert!((null_mut::<T>() as usize).is_multiple_of(align_of::<T>()));
}
