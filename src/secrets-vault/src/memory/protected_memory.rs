/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use crate::crypto::prng::Prng;
#[cfg(windows)]
use std::alloc::Layout;
use std::{
    fmt::{Debug, Formatter},
    ops::Deref,
    ptr::{copy_nonoverlapping, NonNull},
};
use zeroize::Zeroize;

trait PlatformMemory {
    fn alloc_page(size: usize) -> anyhow::Result<(NonNull<[u8]>, usize)>;
    fn free_page(ptr: NonNull<[u8]>);
    fn protect_page(ptr: NonNull<[u8]>, writable: bool, readable: bool) -> anyhow::Result<()>;
    fn lock_page(ptr: NonNull<[u8]>, size: usize) -> anyhow::Result<()>;
    fn unlock_page(ptr: NonNull<[u8]>, size: usize) -> anyhow::Result<()>;

    fn memzero(ptr: *mut u8, size: usize) {
        unsafe {
            let s = core::slice::from_raw_parts_mut(ptr, size);
            s.zeroize();
        }
    }
}

pub struct ProtectedMemoryInner {
    ptr: NonNull<[u8]>,
    size_allocated: usize,
    size_used: usize,
}

#[cfg(unix)]
impl PlatformMemory for ProtectedMemoryInner {
    fn alloc_page(size: usize) -> anyhow::Result<(NonNull<[u8]>, usize)> {
        unsafe {
            let page_size = libc::sysconf(libc::_SC_PAGESIZE);
            if page_size <= 0 {
                anyhow::bail!(std::io::Error::last_os_error())
            }
            let page_size = page_size as usize;
            let num_pages = size.div_ceil(page_size).max(1);
            let size_to_allocate = num_pages * page_size;
            match memsec::malloc_sized(size_to_allocate) {
                Some(ptr) => Ok((ptr, size_to_allocate)),
                None => anyhow::bail!("memsec malloc_sized failed"),
            }
        }
    }

    fn free_page(ptr: NonNull<[u8]>) {
        unsafe {
            memsec::free(ptr);
        }
    }

    fn protect_page(ptr: NonNull<[u8]>, writable: bool, readable: bool) -> anyhow::Result<()> {
        unsafe {
            let prot = match (readable, writable) {
                (false, false) => memsec::Prot::NoAccess,
                (true, false) => memsec::Prot::ReadOnly,
                (true, true) => memsec::Prot::ReadWrite,
                (false, true) => memsec::Prot::WriteOnly,
            };
            if !memsec::mprotect(ptr, prot) {
                anyhow::bail!("protect_page: memsec::mprotect failed")
            }
            Ok(())
        }
    }

    fn lock_page(ptr: NonNull<[u8]>, size: usize) -> anyhow::Result<()> {
        unsafe {
            if !memsec::mlock(ptr.as_ptr() as *mut u8, size) {
                anyhow::bail!("memsec::mlock failed")
            }
            Ok(())
        }
    }

    fn unlock_page(ptr: NonNull<[u8]>, size: usize) -> anyhow::Result<()> {
        unsafe {
            if !memsec::munlock(ptr.as_ptr() as *mut u8, size) {
                anyhow::bail!("memsec::munlock failed")
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
impl PlatformMemory for ProtectedMemoryInner {
    fn alloc_page(size: usize) -> anyhow::Result<(NonNull<[u8]>, usize)> {
        unsafe {
            let page_size = 4096;
            let num_pages = size.div_ceil(page_size).max(1);
            let size_to_allocate = num_pages * page_size;

            let layout = Layout::from_size_align(size_to_allocate, page_size)
                .map_err(|e| anyhow::anyhow!("Layout creation failed: {}", e))?;

            let ptr = std::alloc::alloc(layout);
            if ptr.is_null() {
                anyhow::bail!("Memory allocation failed");
            }

            let slice_ptr = std::ptr::slice_from_raw_parts_mut(ptr, size_to_allocate);
            let non_null = NonNull::new(slice_ptr)
                .ok_or_else(|| anyhow::anyhow!("Failed to create NonNull pointer"))?;

            Ok((non_null, size_to_allocate))
        }
    }

    fn free_page(ptr: NonNull<[u8]>) {
        unsafe {
            let size = ptr.len();
            let page_size = 4096;
            let layout = Layout::from_size_align_unchecked(size, page_size);
            std::alloc::dealloc(ptr.as_ptr() as *mut u8, layout);
        }
    }

    fn protect_page(_ptr: NonNull<[u8]>, _writable: bool, _readable: bool) -> anyhow::Result<()> {
        /*unsafe {
            let protect = match (readable, writable) {
                (false, false) => PAGE_NOACCESS,
                (true, false) => PAGE_READONLY,
                (true, true) => PAGE_READWRITE,
                (false, true) => PAGE_READWRITE, // Windows doesn't have write-only
            };

            let mut old_protect = 0u32;
            if VirtualProtect(ptr.as_ptr() as *mut _, ptr.len(), protect, &mut old_protect) == 0 {
                anyhow::bail!("protect_page: VirtualProtect failed: {}", io::Error::last_os_error())
            }
            Ok(())
        }*/
        Ok(())
    }

    fn lock_page(_ptr: NonNull<[u8]>, _size: usize) -> anyhow::Result<()> {
        /*
        unsafe {
            if VirtualLock(ptr.as_ptr() as *mut _, size) == 0 {
                anyhow::bail!("VirtualLock failed: {}", io::Error::last_os_error())
            }
            Ok(())
        }
        */
        Ok(())
    }

    fn unlock_page(_ptr: NonNull<[u8]>, _size: usize) -> anyhow::Result<()> {
        /*
        unsafe {
            if VirtualUnlock(ptr.as_ptr() as *mut _, size) == 0 {
                anyhow::bail!("VirtualUnlock failed: {}", io::Error::last_os_error())
            }
            Ok(())
        }
        */
        Ok(())
    }
}

impl ProtectedMemoryInner {
    pub fn new(size: usize) -> anyhow::Result<Self> {
        let (ptr, size_allocated) = Self::alloc_page(size)?;
        Self::lock_page(ptr, size_allocated)?;
        Self::protect_page(ptr, false, false)?; // NoAccess
        Ok(Self { ptr, size_allocated, size_used: size })
    }

    pub fn from_slice(data: &[u8]) -> anyhow::Result<Self> {
        let mut inner = Self::new(0)?;
        inner.extend_from_slice(data)?;
        Ok(inner)
    }

    pub fn generate_random(prng: &dyn Prng, size: usize) -> anyhow::Result<Self> {
        let mut inner = Self::new(size)?;
        {
            let mut handle = inner.write_handle()?;
            prng.fill_random(handle.as_mut())?;
        }
        Ok(inner)
    }

    pub fn try_clone(&self) -> anyhow::Result<Self> {
        Self::protect_page(self.ptr, false, true)?; // ReadOnly
        let read_slice =
            unsafe { std::slice::from_raw_parts(self.ptr.as_ptr() as *const u8, self.size_used) };
        let result = Self::from_slice(read_slice);
        let restore = Self::protect_page(self.ptr, false, false); // NoAccess
        let cloned = result?;
        restore?;
        Ok(cloned)
    }

    pub fn len(&self) -> usize {
        self.size_used
    }

    pub fn allocated(&self) -> usize {
        self.size_allocated
    }

    pub fn is_empty(&self) -> bool {
        self.size_used == 0
    }

    pub fn write_handle(&mut self) -> anyhow::Result<WriteHandle<'_>> {
        Self::protect_page(self.ptr, true, true)?; // ReadWrite
        Ok(WriteHandle { inner: self })
    }

    pub fn extend_from_slice(&mut self, other: &[u8]) -> anyhow::Result<()> {
        let mut handle = self.write_handle()?;
        handle.extend_from_slice(other)
    }

    pub fn truncate(&mut self, len: usize) -> anyhow::Result<()> {
        let mut handle = self.write_handle()?;
        handle.truncate(len)
    }
}

impl Drop for ProtectedMemoryInner {
    fn drop(&mut self) {
        Self::protect_page(self.ptr, true, true).ok(); // ReadWrite
        Self::memzero(self.ptr.as_ptr() as *mut u8, self.size_allocated);
        Self::free_page(self.ptr);
    }
}

unsafe impl Send for ProtectedMemoryInner {}

pub struct ProtectedMemory {
    inner: ProtectedMemoryInner,
    reader_count: parking_lot::Mutex<usize>,
}

impl From<ProtectedMemoryInner> for ProtectedMemory {
    fn from(inner: ProtectedMemoryInner) -> Self {
        Self { inner, reader_count: parking_lot::Mutex::new(0) }
    }
}

impl ProtectedMemory {
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn allocated(&self) -> usize {
        self.inner.allocated()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn lock(&self) -> anyhow::Result<ReadGuard<'_>> {
        {
            let mut count = self.reader_count.lock();
            if *count == 0 {
                ProtectedMemoryInner::protect_page(self.inner.ptr, false, true)?;
            }
            *count += 1;
        }
        Ok(ReadGuard { pm: self })
    }

    pub fn try_clone(&self) -> anyhow::Result<Self> {
        let read = self.lock()?;
        Ok(ProtectedMemoryInner::from_slice(&read)?.into())
    }

    pub fn eq_pm(&self, other: &ProtectedMemory) -> anyhow::Result<bool> {
        if self.len() != other.len() {
            return Ok(false);
        }

        let d1: &[u8] = &self.lock()?;
        let d2: &[u8] = &other.lock()?;

        Ok(d1 == d2)
    }
}

unsafe impl Send for ProtectedMemory {}
unsafe impl Sync for ProtectedMemory {}

impl Debug for ProtectedMemory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectedMemory").field("inner", &"[PROTECTED]").finish_non_exhaustive()
    }
}

pub struct ReadGuard<'a> {
    pm: &'a ProtectedMemory,
}

impl<'a> Deref for ReadGuard<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe {
            std::slice::from_raw_parts(
                self.pm.inner.ptr.as_ptr() as *const u8,
                self.pm.inner.size_used,
            )
        }
    }
}

impl<'a> AsRef<[u8]> for ReadGuard<'a> {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl<'a> Drop for ReadGuard<'a> {
    fn drop(&mut self) {
        let mut count = self.pm.reader_count.lock();
        *count -= 1;
        if *count == 0 {
            ProtectedMemoryInner::protect_page(self.pm.inner.ptr, false, false).ok();
        }
    }
}

impl<'a> Debug for ReadGuard<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadGuard").finish_non_exhaustive()
    }
}

pub struct WriteHandle<'a> {
    inner: &'a mut ProtectedMemoryInner,
}

impl<'a> WriteHandle<'a> {
    pub fn extend_from_slice(&mut self, other: &[u8]) -> anyhow::Result<()> {
        if other.is_empty() {
            return Ok(());
        }

        let new_size_used = self
            .inner
            .size_used
            .checked_add(other.len())
            .ok_or_else(|| anyhow::anyhow!("size overflow"))?;

        if new_size_used <= self.inner.size_allocated {
            unsafe {
                copy_nonoverlapping(
                    other.as_ptr(),
                    (self.inner.ptr.as_ptr() as *mut u8).add(self.inner.size_used),
                    other.len(),
                );
            }
            self.inner.size_used = new_size_used;
            return Ok(());
        }

        let (new_ptr, new_size_allocated) = ProtectedMemoryInner::alloc_page(new_size_used)?;

        let result: anyhow::Result<()> = (|| {
            ProtectedMemoryInner::protect_page(new_ptr, true, true)?; // ReadWrite
            ProtectedMemoryInner::lock_page(new_ptr, new_size_allocated)?;

            unsafe {
                copy_nonoverlapping(
                    self.inner.ptr.as_ptr() as *const u8,
                    new_ptr.as_ptr() as *mut u8,
                    self.inner.size_used,
                );

                copy_nonoverlapping(
                    other.as_ptr(),
                    (new_ptr.as_ptr() as *mut u8).add(self.inner.size_used),
                    other.len(),
                );

                ProtectedMemoryInner::memzero(
                    self.inner.ptr.as_ptr() as *mut u8,
                    self.inner.size_allocated,
                );
            }
            Ok(())
        })();

        if let Err(e) = result {
            ProtectedMemoryInner::free_page(new_ptr);
            return Err(e);
        }

        ProtectedMemoryInner::unlock_page(self.inner.ptr, self.inner.size_allocated).ok();
        ProtectedMemoryInner::free_page(self.inner.ptr);

        self.inner.ptr = new_ptr;
        self.inner.size_allocated = new_size_allocated;
        self.inner.size_used = new_size_used;

        Ok(())
    }

    pub fn truncate(&mut self, len: usize) -> anyhow::Result<()> {
        if len >= self.inner.size_used {
            return Ok(());
        }

        unsafe {
            ProtectedMemoryInner::memzero(
                (self.inner.ptr.as_ptr() as *mut u8).add(len),
                self.inner.size_used - len,
            );
        }

        self.inner.size_used = len;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.inner.size_used
    }

    pub fn is_empty(&self) -> bool {
        self.inner.size_used == 0
    }
}

impl<'a> Deref for WriteHandle<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe {
            std::slice::from_raw_parts(self.inner.ptr.as_ptr() as *const u8, self.inner.size_used)
        }
    }
}

impl<'a> std::ops::DerefMut for WriteHandle<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            std::slice::from_raw_parts_mut(self.inner.ptr.as_ptr() as *mut u8, self.inner.size_used)
        }
    }
}

impl<'a> AsRef<[u8]> for WriteHandle<'a> {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl<'a> AsMut<[u8]> for WriteHandle<'a> {
    fn as_mut(&mut self) -> &mut [u8] {
        self
    }
}

impl<'a> Drop for WriteHandle<'a> {
    fn drop(&mut self) {
        ProtectedMemoryInner::protect_page(self.inner.ptr, false, false).ok();
    }
}

impl<'a> Debug for WriteHandle<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteHandle").finish_non_exhaustive()
    }
}
