/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::memory::protected_memory::ProtectedMemory;

#[tokio::test]
async fn test_new_protected_data() {
    let data = ProtectedMemory::new(100).unwrap();
    assert_eq!(data.len().await, 100);
    assert!(!data.is_empty().await);
    assert!(data.allocated().await >= 100);
}

#[tokio::test]
async fn test_new_zero_size_ok() {
    let result = ProtectedMemory::new(0);
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_read_guard() {
    let data = ProtectedMemory::new(10).unwrap();
    let guard = data.lock().await.unwrap();
    assert_eq!(guard.len(), 10);
    let _slice: &[u8] = &guard;
}

#[tokio::test]
async fn test_write_guard() {
    let mut data = ProtectedMemory::new(10).unwrap();
    {
        let mut guard = data.lock_mut().await.unwrap();
        assert_eq!(guard.len(), 10);

        guard[0] = 1;
        guard[1] = 2;
    }

    let guard = data.lock().await.unwrap();
    assert_eq!(guard[0], 1);
    assert_eq!(guard[1], 2);
}

#[tokio::test]
async fn test_extend_from_slice_within_capacity() {
    let mut data = ProtectedMemory::new(10).unwrap();
    let initial_capacity = data.allocated().await;

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard[0] = 1;
        guard[1] = 2;
    }

    let additional = vec![3, 4, 5];
    data.lock_mut().await.unwrap().extend_from_slice(&additional).unwrap();

    assert_eq!(data.len().await, 13);
    assert_eq!(data.allocated().await, initial_capacity);

    let guard = data.lock().await.unwrap();
    assert_eq!(guard[0], 1);
    assert_eq!(guard[1], 2);
    assert_eq!(guard[10], 3);
    assert_eq!(guard[11], 4);
    assert_eq!(guard[12], 5);
}

#[tokio::test]
async fn test_extend_from_slice_requires_reallocation() {
    let mut data = ProtectedMemory::new(10).unwrap();
    let initial_capacity = data.allocated().await;

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard[0] = 1;
        guard[1] = 2;
    }

    let additional = vec![42; initial_capacity];
    data.lock_mut().await.unwrap().extend_from_slice(&additional).unwrap();

    assert_eq!(data.len().await, 10 + initial_capacity);
    assert!(data.allocated().await > initial_capacity);

    let guard = data.lock().await.unwrap();
    assert_eq!(guard[0], 1);
    assert_eq!(guard[1], 2);
    assert_eq!(guard[10], 42);
    assert_eq!(guard[guard.len() - 1], 42);
}

#[tokio::test]
async fn test_extend_from_slice_empty() {
    let mut data = ProtectedMemory::new(10).unwrap();
    let initial_len = data.len().await;
    let initial_capacity = data.allocated().await;

    data.lock_mut().await.unwrap().extend_from_slice(&[]).unwrap();

    assert_eq!(data.len().await, initial_len);
    assert_eq!(data.allocated().await, initial_capacity);
}

#[tokio::test]
async fn test_extend_multiple_times() {
    let mut data = ProtectedMemory::new(5).unwrap();

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard[0] = 0;
        guard[1] = 1;
        guard[2] = 2;
        guard[3] = 3;
        guard[4] = 4;

        guard.extend_from_slice(&[5, 6]).unwrap();
        guard.extend_from_slice(&[7, 8]).unwrap();
        guard.extend_from_slice(&[9, 10, 11]).unwrap();
    }

    assert_eq!(data.len().await, 12);

    let guard = data.lock().await.unwrap();
    assert_eq!(&guard[..], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
}

#[tokio::test]
async fn test_truncate() -> anyhow::Result<()> {
    let mut data = ProtectedMemory::new(10).unwrap();

    {
        let mut guard = data.lock_mut().await.unwrap();
        for i in 0..10 {
            guard[i] = i as u8;
        }

        guard.truncate(5)?;
    }

    assert_eq!(data.len().await, 5);

    let guard = data.lock().await.unwrap();
    assert_eq!(guard[0], 0);
    assert_eq!(guard[4], 4);

    Ok(())
}

#[tokio::test]
async fn test_truncate_to_zero() -> anyhow::Result<()> {
    let mut data = ProtectedMemory::new(10).unwrap();

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard[0] = 42;
        guard.truncate(0)?;
    }

    assert_eq!(data.len().await, 0);
    assert!(data.is_empty().await);

    Ok(())
}

#[tokio::test]
async fn test_truncate_larger_than_size_does_nothing() -> anyhow::Result<()> {
    let mut data = ProtectedMemory::new(10).unwrap();

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard[0] = 42;
        guard.truncate(20)?;
    }

    assert_eq!(data.len().await, 10);

    let guard = data.lock().await.unwrap();
    assert_eq!(guard[0], 42);

    Ok(())
}

#[tokio::test]
async fn test_truncate_same_size_does_nothing() -> anyhow::Result<()> {
    let mut data = ProtectedMemory::new(10).unwrap();

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.truncate(10)?;
    }

    assert_eq!(data.len().await, 10);

    Ok(())
}

#[tokio::test]
async fn test_extend_after_truncate() -> anyhow::Result<()> {
    let mut data = ProtectedMemory::new(10).unwrap();

    {
        let mut guard = data.lock_mut().await.unwrap();
        for i in 0..10 {
            guard[i] = i as u8;
        }
    }

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.truncate(5)?;
    }
    assert_eq!(data.len().await, 5);

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.extend_from_slice(&[10, 11, 12]).unwrap();
    }
    assert_eq!(data.len().await, 8);

    let guard = data.lock().await.unwrap();
    assert_eq!(guard[0], 0);
    assert_eq!(guard[4], 4);
    assert_eq!(guard[5], 10);
    assert_eq!(guard[6], 11);
    assert_eq!(guard[7], 12);

    Ok(())
}

#[tokio::test]
async fn test_large_allocation() {
    let size = 10000;
    let data = ProtectedMemory::new(size).unwrap();
    assert_eq!(data.len().await, size);
    assert!(data.allocated().await >= size);
}

#[tokio::test]
async fn test_extend_with_large_data() {
    let mut data = ProtectedMemory::new(100).unwrap();
    let large_data = vec![42; 10000];

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.extend_from_slice(&large_data).unwrap();
    }

    assert_eq!(data.len().await, 10100);

    let guard = data.lock().await.unwrap();
    assert_eq!(guard[100], 42);
    assert_eq!(guard[guard.len() - 1], 42);
}

#[tokio::test]
async fn test_sequential_operations() -> anyhow::Result<()> {
    let mut data = ProtectedMemory::new(5).unwrap();

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.copy_from_slice(&[1, 2, 3, 4, 5]);
    }

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.extend_from_slice(&[6, 7, 8]).unwrap();
    }
    assert_eq!(data.len().await, 8);

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.truncate(6)?;
    }
    assert_eq!(data.len().await, 6);

    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.extend_from_slice(&[9, 10]).unwrap();
    }
    assert_eq!(data.len().await, 8);

    let guard = data.lock().await.unwrap();
    assert_eq!(&guard[..], &[1, 2, 3, 4, 5, 6, 9, 10]);

    Ok(())
}

#[tokio::test]
async fn test_protection_restored_after_read() {
    let data = ProtectedMemory::new(10).unwrap();

    {
        let _guard = data.lock().await.unwrap();
    }

    let _guard2 = data.lock().await.unwrap();
}

#[tokio::test]
async fn test_protection_restored_after_write() {
    let mut data = ProtectedMemory::new(10).unwrap();

    {
        let mut _guard = data.lock_mut().await.unwrap();
    }

    let _guard2 = data.lock_mut().await.unwrap();
}

#[tokio::test]
async fn test_capacity_grows_appropriately() {
    let mut data = ProtectedMemory::new(1).unwrap();
    let initial_capacity = data.allocated().await;

    let extension = vec![1; initial_capacity * 2];
    {
        let mut guard = data.lock_mut().await.unwrap();
        guard.extend_from_slice(&extension).unwrap();
    }

    assert!(data.allocated().await >= initial_capacity * 2);
    assert_eq!(data.len().await, 1 + initial_capacity * 2);
}

#[tokio::test]
async fn test_concurrent_access() {
    use std::sync::Arc;

    let data = Arc::new(ProtectedMemory::new(64).unwrap());
    let mut handles = vec![];

    for _ in 0..10 {
        let data_clone = Arc::clone(&data);
        let handle = tokio::spawn(async move {
            let guard = data_clone.lock().await.unwrap();
            assert_eq!(guard.len(), 64);
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
}
