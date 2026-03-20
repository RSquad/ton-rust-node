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
    ops::{Deref, DerefMut},
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
struct ProtectedMemoryInner {
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
            let num_pages = size.div_ceil(page_size);
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
            let num_pages = size.div_ceil(page_size);
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

    pub fn len(&self) -> usize {
        self.size_used
    }

    pub fn allocated(&self) -> usize {
        self.size_allocated
    }

    pub fn is_empty(&self) -> bool {
        self.size_used == 0
    }

    fn extend_from_slice(&mut self, other: &[u8]) -> anyhow::Result<()> {
        if other.is_empty() {
            return Ok(());
        }

        let new_size_used = self
            .size_used
            .checked_add(other.len())
            .ok_or_else(|| anyhow::anyhow!("size overflow"))?;

        if new_size_used <= self.size_allocated {
            Self::protect_page(self.ptr, true, true)?; // ReadWrite

            unsafe {
                copy_nonoverlapping(
                    other.as_ptr(),
                    (self.ptr.as_ptr() as *mut u8).add(self.size_used),
                    other.len(),
                );
            }

            self.size_used = new_size_used;
            Self::protect_page(self.ptr, false, false)?; // NoAccess

            return Ok(());
        }

        let (new_ptr, new_size_allocated) = Self::alloc_page(new_size_used)?;

        Self::protect_page(self.ptr, true, true)?; // ReadWrite
        Self::protect_page(new_ptr, true, true)?; // ReadWrite

        unsafe {
            copy_nonoverlapping(
                self.ptr.as_ptr() as *const u8,
                new_ptr.as_ptr() as *mut u8,
                self.size_used,
            );

            copy_nonoverlapping(
                other.as_ptr(),
                (new_ptr.as_ptr() as *mut u8).add(self.size_used),
                other.len(),
            );

            Self::memzero(self.ptr.as_ptr() as *mut u8, self.size_allocated);
        }

        Self::lock_page(new_ptr, new_size_allocated)?;
        Self::unlock_page(self.ptr, self.size_allocated).ok();
        Self::free_page(self.ptr);

        self.ptr = new_ptr;
        self.size_allocated = new_size_allocated;
        self.size_used = new_size_used;

        Self::protect_page(self.ptr, false, false)?; // NoAccess

        Ok(())
    }

    fn truncate(&mut self, len: usize) -> anyhow::Result<()> {
        if len >= self.size_used {
            return Ok(());
        }

        Self::protect_page(self.ptr, true, true /* ReadWrite */)?;
        unsafe {
            Self::memzero((self.ptr.as_ptr() as *mut u8).add(len), self.size_used - len);
        }
        Self::protect_page(self.ptr, false, false /* NoAccess */)?;

        self.size_used = len;
        Ok(())
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr() as *const u8, self.size_used) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr() as *mut u8, self.size_used) }
    }

    fn restore_protection(&self) -> anyhow::Result<()> {
        Self::protect_page(self.ptr, false, false) // NoAccess
    }
}

pub struct ProtectedMemory {
    inner: tokio::sync::Mutex<ProtectedMemoryInner>,
}

impl ProtectedMemory {
    pub fn new(size: usize) -> anyhow::Result<Self> {
        Ok(Self { inner: tokio::sync::Mutex::new(ProtectedMemoryInner::new(size)?) })
    }

    pub async fn from_slice(data: &[u8]) -> anyhow::Result<Self> {
        let mut protected_data = Self::new(0)?;

        {
            let mut lock = protected_data.lock_mut().await?;
            lock.extend_from_slice(data)?;
        }

        Ok(protected_data)
    }

    pub async fn generate_random(prng: &dyn Prng, size: usize) -> anyhow::Result<Self> {
        let mut data = Self::new(size)?;

        {
            let mut lock = data.lock_mut().await?;
            prng.fill_random(&mut lock).await?;
        }

        Ok(data)
    }

    pub async fn clone(&self) -> anyhow::Result<Self> {
        let mut data = ProtectedMemory::new(0)?;

        {
            let mut data_lock = data.lock_mut().await?;
            let this_lock = self.lock().await?;
            data_lock.extend_from_slice(&this_lock)?;
        }

        Ok(data)
    }

    pub async fn len(&self) -> usize {
        let guard = self.inner.lock().await;
        guard.len()
    }

    pub async fn allocated(&self) -> usize {
        let guard = self.inner.lock().await;
        guard.allocated()
    }

    pub async fn is_empty(&self) -> bool {
        let guard: tokio::sync::MutexGuard<'_, ProtectedMemoryInner> = self.inner.lock().await;
        guard.is_empty()
    }

    pub async fn lock(&self) -> anyhow::Result<ReadGuard<'_>> {
        let guard: tokio::sync::MutexGuard<'_, ProtectedMemoryInner> = self.inner.lock().await;
        ProtectedMemoryInner::protect_page(guard.ptr, false, true)?; // ReadOnly
        Ok(ReadGuard { guard })
    }

    pub async fn lock_mut(&mut self) -> anyhow::Result<WriteGuard<'_>> {
        let guard: tokio::sync::MutexGuard<'_, ProtectedMemoryInner> = self.inner.lock().await;
        ProtectedMemoryInner::protect_page(guard.ptr, true, true)?; // ReadWrite
        Ok(WriteGuard { guard })
    }

    pub async fn eq_pm(&self, other: &ProtectedMemory) -> anyhow::Result<bool> {
        if self.len().await != other.len().await {
            return Ok(false);
        }

        let d1: &[u8] = &self.lock().await?;
        let d2: &[u8] = &other.lock().await?;

        Ok(d1 == d2)
    }
}

unsafe impl Send for ProtectedMemoryInner {}
unsafe impl Send for ProtectedMemory {}
unsafe impl Sync for ProtectedMemory {}

impl Drop for ProtectedMemory {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.inner.try_lock() {
            ProtectedMemoryInner::protect_page(guard.ptr, true, true).ok(); // ReadWrite
            ProtectedMemoryInner::memzero(guard.ptr.as_ptr() as *mut u8, guard.size_allocated);
            ProtectedMemoryInner::free_page(guard.ptr);
            guard.size_allocated = 0;
            guard.size_used = 0;
        }
    }
}

impl Debug for ProtectedMemory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectedMemory").field("inner", &"[PROTECTED]").finish_non_exhaustive()
    }
}

pub struct ReadGuard<'a> {
    guard: tokio::sync::MutexGuard<'a, ProtectedMemoryInner>,
}

impl<'a> Deref for ReadGuard<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.guard.as_slice()
    }
}

impl<'a> AsRef<[u8]> for ReadGuard<'a> {
    fn as_ref(&self) -> &[u8] {
        self.guard.as_slice()
    }
}

impl<'a> Drop for ReadGuard<'a> {
    fn drop(&mut self) {
        self.guard.restore_protection().ok();
    }
}

impl<'a> Debug for ReadGuard<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadGuard").finish_non_exhaustive()
    }
}

pub struct WriteGuard<'a> {
    guard: tokio::sync::MutexGuard<'a, ProtectedMemoryInner>,
}

impl<'a> WriteGuard<'a> {
    pub fn extend_from_slice(&mut self, other: &[u8]) -> anyhow::Result<()> {
        self.guard.extend_from_slice(other)?;
        ProtectedMemoryInner::protect_page(self.guard.ptr, true, true) // ReadWrite
    }

    pub fn truncate(&mut self, len: usize) -> anyhow::Result<()> {
        self.guard.truncate(len)?;
        ProtectedMemoryInner::protect_page(self.guard.ptr, true, true /* ReadWrite */)
    }
}

impl<'a> Deref for WriteGuard<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.guard.as_slice()
    }
}

impl<'a> DerefMut for WriteGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut_slice()
    }
}

impl<'a> AsRef<[u8]> for WriteGuard<'a> {
    fn as_ref(&self) -> &[u8] {
        self.guard.as_slice()
    }
}

impl<'a> AsMut<[u8]> for WriteGuard<'a> {
    fn as_mut(&mut self) -> &mut [u8] {
        self.guard.as_mut_slice()
    }
}

impl<'a> Drop for WriteGuard<'a> {
    fn drop(&mut self) {
        self.guard.restore_protection().ok();
    }
}

impl<'a> Debug for WriteGuard<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteGuard").finish_non_exhaustive()
    }
}
