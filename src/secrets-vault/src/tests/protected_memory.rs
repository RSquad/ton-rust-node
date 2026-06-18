/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use crate::memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner};

#[test]
fn test_new_protected_data() {
    let data: ProtectedMemory = ProtectedMemoryInner::new(100).unwrap().into();
    assert_eq!(data.len(), 100);
    assert!(!data.is_empty());
    assert!(data.allocated() >= 100);
}

#[test]
fn test_new_zero_size_ok() {
    let result = ProtectedMemoryInner::new(0);
    assert!(result.is_ok());
}

#[test]
fn test_read_guard() {
    let data: ProtectedMemory = ProtectedMemoryInner::new(10).unwrap().into();
    let guard = data.lock().unwrap();
    assert_eq!(guard.len(), 10);
    let _slice: &[u8] = &guard;
}

#[test]
fn test_write_handle() {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();
    {
        let mut handle = inner.write_handle().unwrap();
        assert_eq!(handle.len(), 10);

        handle[0] = 1;
        handle[1] = 2;
    }

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(guard[0], 1);
    assert_eq!(guard[1], 2);
}

#[test]
fn test_extend_from_slice_within_capacity() {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();
    let initial_capacity = inner.allocated();

    {
        let mut handle = inner.write_handle().unwrap();
        handle[0] = 1;
        handle[1] = 2;
    }

    inner.extend_from_slice(&[3, 4, 5]).unwrap();

    assert_eq!(inner.len(), 13);
    assert_eq!(inner.allocated(), initial_capacity);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(guard[0], 1);
    assert_eq!(guard[1], 2);
    assert_eq!(guard[10], 3);
    assert_eq!(guard[11], 4);
    assert_eq!(guard[12], 5);
}

#[test]
fn test_extend_from_slice_requires_reallocation() {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();
    let initial_capacity = inner.allocated();

    {
        let mut handle = inner.write_handle().unwrap();
        handle[0] = 1;
        handle[1] = 2;
    }

    let additional = vec![42; initial_capacity];
    inner.extend_from_slice(&additional).unwrap();

    assert_eq!(inner.len(), 10 + initial_capacity);
    assert!(inner.allocated() > initial_capacity);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(guard[0], 1);
    assert_eq!(guard[1], 2);
    assert_eq!(guard[10], 42);
    assert_eq!(guard[guard.len() - 1], 42);
}

#[test]
fn test_extend_from_slice_empty() {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();
    let initial_len = inner.len();
    let initial_capacity = inner.allocated();

    inner.extend_from_slice(&[]).unwrap();

    assert_eq!(inner.len(), initial_len);
    assert_eq!(inner.allocated(), initial_capacity);
}

#[test]
fn test_extend_multiple_times() {
    let mut inner = ProtectedMemoryInner::new(5).unwrap();

    {
        let mut handle = inner.write_handle().unwrap();
        handle[0] = 0;
        handle[1] = 1;
        handle[2] = 2;
        handle[3] = 3;
        handle[4] = 4;

        handle.extend_from_slice(&[5, 6]).unwrap();
        handle.extend_from_slice(&[7, 8]).unwrap();
        handle.extend_from_slice(&[9, 10, 11]).unwrap();
    }

    assert_eq!(inner.len(), 12);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(&guard[..], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
}

#[test]
fn test_truncate() -> anyhow::Result<()> {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();

    {
        let mut handle = inner.write_handle().unwrap();
        for i in 0..10 {
            handle[i] = i as u8;
        }

        handle.truncate(5)?;
    }

    assert_eq!(inner.len(), 5);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(guard[0], 0);
    assert_eq!(guard[4], 4);

    Ok(())
}

#[test]
fn test_truncate_to_zero() -> anyhow::Result<()> {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();

    {
        let mut handle = inner.write_handle().unwrap();
        handle[0] = 42;
        handle.truncate(0)?;
    }

    assert_eq!(inner.len(), 0);
    assert!(inner.is_empty());

    Ok(())
}

#[test]
fn test_truncate_larger_than_size_does_nothing() -> anyhow::Result<()> {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();

    {
        let mut handle = inner.write_handle().unwrap();
        handle[0] = 42;
        handle.truncate(20)?;
    }

    assert_eq!(inner.len(), 10);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(guard[0], 42);

    Ok(())
}

#[test]
fn test_truncate_same_size_does_nothing() -> anyhow::Result<()> {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();

    inner.truncate(10)?;

    assert_eq!(inner.len(), 10);

    Ok(())
}

#[test]
fn test_extend_after_truncate() -> anyhow::Result<()> {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();

    {
        let mut handle = inner.write_handle().unwrap();
        for i in 0..10 {
            handle[i] = i as u8;
        }
    }

    inner.truncate(5)?;
    assert_eq!(inner.len(), 5);

    inner.extend_from_slice(&[10, 11, 12])?;
    assert_eq!(inner.len(), 8);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(guard[0], 0);
    assert_eq!(guard[4], 4);
    assert_eq!(guard[5], 10);
    assert_eq!(guard[6], 11);
    assert_eq!(guard[7], 12);

    Ok(())
}

#[test]
fn test_large_allocation() {
    let size = 10000;
    let data: ProtectedMemory = ProtectedMemoryInner::new(size).unwrap().into();
    assert_eq!(data.len(), size);
    assert!(data.allocated() >= size);
}

#[test]
fn test_extend_with_large_data() {
    let mut inner = ProtectedMemoryInner::new(100).unwrap();
    let large_data = vec![42; 10000];

    inner.extend_from_slice(&large_data).unwrap();

    assert_eq!(inner.len(), 10100);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(guard[100], 42);
    assert_eq!(guard[guard.len() - 1], 42);
}

#[test]
fn test_sequential_operations() -> anyhow::Result<()> {
    let mut inner = ProtectedMemoryInner::new(5).unwrap();

    {
        let mut handle = inner.write_handle()?;
        handle.copy_from_slice(&[1, 2, 3, 4, 5]);
    }

    inner.extend_from_slice(&[6, 7, 8])?;
    assert_eq!(inner.len(), 8);

    inner.truncate(6)?;
    assert_eq!(inner.len(), 6);

    inner.extend_from_slice(&[9, 10])?;
    assert_eq!(inner.len(), 8);

    let data: ProtectedMemory = inner.into();
    let guard = data.lock().unwrap();
    assert_eq!(&guard[..], &[1, 2, 3, 4, 5, 6, 9, 10]);

    Ok(())
}

#[test]
fn test_protection_restored_after_read() {
    let data: ProtectedMemory = ProtectedMemoryInner::new(10).unwrap().into();

    {
        let _guard = data.lock().unwrap();
    }

    let _guard2 = data.lock().unwrap();
}

#[test]
fn test_protection_restored_after_write() {
    let mut inner = ProtectedMemoryInner::new(10).unwrap();

    {
        let mut _h = inner.write_handle().unwrap();
    }

    let _h2 = inner.write_handle().unwrap();
}

#[test]
fn test_capacity_grows_appropriately() {
    let mut inner = ProtectedMemoryInner::new(1).unwrap();
    let initial_capacity = inner.allocated();

    let extension = vec![1; initial_capacity * 2];
    inner.extend_from_slice(&extension).unwrap();

    assert!(inner.allocated() >= initial_capacity * 2);
    assert_eq!(inner.len(), 1 + initial_capacity * 2);
}

#[tokio::test]
async fn test_concurrent_access() {
    use std::sync::Arc;

    let data: Arc<ProtectedMemory> = Arc::new(ProtectedMemoryInner::new(64).unwrap().into());
    let mut handles = vec![];

    for _ in 0..10 {
        let data_clone = Arc::clone(&data);
        let handle = tokio::spawn(async move {
            {
                let guard = data_clone.lock().unwrap();
                assert_eq!(guard.len(), 64);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
}
