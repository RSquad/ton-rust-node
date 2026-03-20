/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Unit tests for RocksDB-based async key-value storage.

use crate::{AsyncKeyValueStorageOptions, ConsensusCommonFactory};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Creates a unique test database path inside target directory.
fn create_test_db_path(test_name: &str) -> PathBuf {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let random: u32 = rand::random();

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test_dbs")
        .join(format!("{}_{:016x}_{:08x}", test_name, timestamp, random));

    std::fs::create_dir_all(&path).unwrap();
    path
}

fn create_test_options() -> AsyncKeyValueStorageOptions {
    AsyncKeyValueStorageOptions { use_callback_thread: true }
}

fn create_test_options_no_callback_thread() -> AsyncKeyValueStorageOptions {
    AsyncKeyValueStorageOptions { use_callback_thread: false }
}

// ============================================================================
// Basic Operations Tests
// ============================================================================

#[test]
fn test_basic_set_and_get() {
    let path = create_test_db_path("test_basic_set_and_get");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test1",
        create_test_options(),
    )
    .unwrap();

    // Set value
    let set_result = storage.set(b"key1".to_vec(), b"value1".to_vec(), None);
    set_result.wait_timeout(Duration::from_secs(5)).expect("timeout").unwrap();

    // Get value
    let get_result = storage.get(b"key1".to_vec(), None);
    let value = get_result.wait_timeout(Duration::from_secs(5)).expect("timeout").unwrap();

    assert_eq!(value, Some(b"value1".to_vec()));

    storage.mark_for_destroy();
}

#[test]
fn test_get_nonexistent_key() {
    let path = create_test_db_path("test_get_nonexistent_key");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test2",
        create_test_options(),
    )
    .unwrap();

    let get_result = storage.get(b"nonexistent".to_vec(), None);
    let value = get_result.wait_timeout(Duration::from_secs(5)).expect("timeout").unwrap();

    assert_eq!(value, None);

    storage.mark_for_destroy();
}

#[test]
fn test_erase() {
    let path = create_test_db_path("test_erase");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test4",
        create_test_options(),
    )
    .unwrap();

    // Set then erase
    storage
        .set(b"key1".to_vec(), b"value1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    storage
        .erase(b"key1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    // Key should be gone
    let value = storage
        .get(b"key1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    assert_eq!(value, None);

    storage.mark_for_destroy();
}

#[test]
fn test_contains() {
    let path = create_test_db_path("test_contains");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test_contains",
        create_test_options(),
    )
    .unwrap();

    // Key doesn't exist
    let exists = storage
        .contains(b"key1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    assert!(!exists);

    // Set value
    storage
        .set(b"key1".to_vec(), b"value1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    // Key exists now
    let exists = storage
        .contains(b"key1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    assert!(exists);

    // Erase and check again
    storage
        .erase(b"key1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    let exists = storage
        .contains(b"key1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    assert!(!exists);

    storage.mark_for_destroy();
}

#[test]
fn test_overwrite() {
    let path = create_test_db_path("test_overwrite");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test5",
        create_test_options(),
    )
    .unwrap();

    // Set initial value
    storage
        .set(b"key1".to_vec(), b"value1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    // Overwrite
    storage
        .set(b"key1".to_vec(), b"value2".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    // Verify new value
    let value = storage
        .get(b"key1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    assert_eq!(value, Some(b"value2".to_vec()));

    storage.mark_for_destroy();
}

// ============================================================================
// Prefix Scan Tests
// ============================================================================

#[test]
fn test_prefix_scan() {
    let path = create_test_db_path("test_prefix_scan");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test6",
        create_test_options(),
    )
    .unwrap();

    // Set multiple keys with same prefix
    storage
        .set(b"prefix_a".to_vec(), b"value_a".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    storage
        .set(b"prefix_b".to_vec(), b"value_b".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    storage
        .set(b"prefix_c".to_vec(), b"value_c".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    storage
        .set(b"other_key".to_vec(), b"other_value".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    // Scan by prefix
    let results = storage
        .get_by_prefix(b"prefix_".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    assert_eq!(results.len(), 3);

    storage.mark_for_destroy();
}

#[test]
fn test_prefix_scan_u32() {
    let path = create_test_db_path("test_prefix_scan_u32");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test7",
        create_test_options(),
    )
    .unwrap();

    // Create keys with u32 TL-style prefix
    let prefix1: u32 = 0x12345678;
    let prefix2: u32 = 0x87654321;

    let mut key1 = prefix1.to_le_bytes().to_vec();
    key1.extend_from_slice(b"_suffix1");
    let mut key2 = prefix1.to_le_bytes().to_vec();
    key2.extend_from_slice(b"_suffix2");
    let mut key3 = prefix2.to_le_bytes().to_vec();
    key3.extend_from_slice(b"_other");

    storage
        .set(key1, b"value1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    storage
        .set(key2, b"value2".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();
    storage
        .set(key3, b"value3".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    // Scan by u32 prefix
    let results = storage
        .get_by_prefix_u32(prefix1, None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    assert_eq!(results.len(), 2);

    storage.mark_for_destroy();
}

// ============================================================================
// Callback Tests
// ============================================================================

#[test]
fn test_set_with_callback() {
    let path = create_test_db_path("test_set_with_callback");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test8",
        create_test_options(),
    )
    .unwrap();

    let callback_called = Arc::new(AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();

    let _ = storage.set(
        b"key1".to_vec(),
        b"value1".to_vec(),
        Some(Box::new(move |result| {
            assert!(result.is_ok());
            callback_called_clone.store(true, Ordering::SeqCst);
        })),
    );

    // Wait for callback
    let start = std::time::Instant::now();
    while !callback_called.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(callback_called.load(Ordering::SeqCst));

    storage.mark_for_destroy();
}

#[test]
fn test_get_with_callback() {
    let path = create_test_db_path("test_get_with_callback");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test9",
        create_test_options(),
    )
    .unwrap();

    // Set first
    storage
        .set(b"key1".to_vec(), b"value1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    let callback_called = Arc::new(AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();

    let _ = storage.get(
        b"key1".to_vec(),
        Some(Box::new(move |result| {
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), Some(b"value1".to_vec()));
            callback_called_clone.store(true, Ordering::SeqCst);
        })),
    );

    // Wait for callback
    let start = std::time::Instant::now();
    while !callback_called.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(callback_called.load(Ordering::SeqCst));

    storage.mark_for_destroy();
}

#[test]
fn test_erase_with_callback() {
    let path = create_test_db_path("test_erase_with_callback");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test10",
        create_test_options(),
    )
    .unwrap();

    // Set first
    storage
        .set(b"key1".to_vec(), b"value1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    let callback_called = Arc::new(AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();

    let _ = storage.erase(
        b"key1".to_vec(),
        Some(Box::new(move |result| {
            assert!(result.is_ok());
            callback_called_clone.store(true, Ordering::SeqCst);
        })),
    );

    // Wait for callback
    let start = std::time::Instant::now();
    while !callback_called.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(callback_called.load(Ordering::SeqCst));

    storage.mark_for_destroy();
}

// ============================================================================
// Async Result Tests
// ============================================================================

#[test]
fn test_async_result_is_ready() {
    let path = create_test_db_path("test_async_result_is_ready");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test11",
        create_test_options(),
    )
    .unwrap();

    let result = storage.set(b"key1".to_vec(), b"value1".to_vec(), None);

    // Wait a bit for the operation to complete
    std::thread::sleep(Duration::from_millis(100));

    // Should be ready
    assert!(result.is_ready());

    storage.mark_for_destroy();
}

#[test]
fn test_async_result_try_get() {
    let path = create_test_db_path("test_async_result_try_get");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test12",
        create_test_options(),
    )
    .unwrap();

    storage
        .set(b"key1".to_vec(), b"value1".to_vec(), None)
        .wait_timeout(Duration::from_secs(5))
        .expect("timeout")
        .unwrap();

    let result = storage.get(b"key1".to_vec(), None);

    // Wait for result
    result.wait_timeout(Duration::from_secs(5)).expect("timeout").unwrap();

    // try_get should return error (already taken)
    let try_result = result.try_get();
    assert!(try_result.is_some());
    assert!(try_result.unwrap().is_err());

    storage.mark_for_destroy();
}

// ============================================================================
// Sync Tests
// ============================================================================

#[test]
fn test_sync() {
    let path = create_test_db_path("test_sync");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test14",
        create_test_options(),
    )
    .unwrap();

    // Queue several writes
    for i in 0..10 {
        let _ =
            storage.set(format!("key{}", i).into_bytes(), format!("value{}", i).into_bytes(), None);
    }

    // Sync should wait for all ops to complete
    storage.sync(Some(Duration::from_secs(5))).unwrap();

    // All values should be readable
    for i in 0..10 {
        let value = storage
            .get(format!("key{}", i).into_bytes(), None)
            .wait_timeout(Duration::from_secs(1))
            .expect("timeout")
            .unwrap();
        assert_eq!(value, Some(format!("value{}", i).into_bytes()));
    }

    storage.mark_for_destroy();
}

#[test]
fn test_sync_with_timeout() {
    let path = create_test_db_path("test_sync_with_timeout");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test15",
        create_test_options(),
    )
    .unwrap();

    // Sync with timeout should complete
    let result = storage.sync(Some(Duration::from_secs(1)));
    assert!(result.is_ok());

    storage.mark_for_destroy();
}

// ============================================================================
// Lifecycle Tests
// ============================================================================

#[test]
fn test_storage_path() {
    let path = create_test_db_path("test_storage_path");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test16",
        create_test_options(),
    )
    .unwrap();

    assert_eq!(storage.get_path(), path.as_path());

    storage.mark_for_destroy();
}

#[test]
fn test_storage_id() {
    let path = create_test_db_path("test_storage_id");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "my_storage_id",
        create_test_options(),
    )
    .unwrap();

    assert_eq!(storage.get_storage_id(), "my_storage_id");

    storage.mark_for_destroy();
}

#[test]
fn test_pending_count() {
    let path = create_test_db_path("test_pending_count");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test17",
        create_test_options(),
    )
    .unwrap();

    // Initially zero
    assert_eq!(storage.pending_count(), 0);

    // Queue some operations (don't wait)
    for i in 0..5 {
        let _ = storage.set(format!("key{}", i).into_bytes(), b"value".to_vec(), None);
    }

    // Sync to clear pending
    storage.sync(Some(Duration::from_secs(5))).unwrap();

    // Should be zero again
    assert_eq!(storage.pending_count(), 0);

    storage.mark_for_destroy();
}

#[test]
fn test_mark_for_destroy() {
    let path = create_test_db_path("test_mark_for_destroy");
    let path_clone = path.clone();

    {
        let storage = ConsensusCommonFactory::create_async_key_value_storage(
            &path,
            "test19",
            create_test_options(),
        )
        .unwrap();

        storage
            .set(b"key1".to_vec(), b"value1".to_vec(), None)
            .wait_timeout(Duration::from_secs(5))
            .expect("timeout")
            .unwrap();

        storage.mark_for_destroy();
    } // Storage dropped here

    // Wait a bit for cleanup
    std::thread::sleep(Duration::from_millis(500));

    // Path should be deleted
    assert!(!path_clone.exists());
}

// ============================================================================
// No Callback Thread Tests
// ============================================================================

#[test]
fn test_no_callback_thread() {
    let path = create_test_db_path("test_no_callback_thread");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test20",
        create_test_options_no_callback_thread(),
    )
    .unwrap();

    let callback_called = Arc::new(AtomicBool::new(false));
    let callback_called_clone = callback_called.clone();

    // Callback should still work (executed in DB thread)
    let _ = storage.set(
        b"key1".to_vec(),
        b"value1".to_vec(),
        Some(Box::new(move |result| {
            assert!(result.is_ok());
            callback_called_clone.store(true, Ordering::SeqCst);
        })),
    );

    // Wait for callback
    let start = std::time::Instant::now();
    while !callback_called.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(callback_called.load(Ordering::SeqCst));

    storage.mark_for_destroy();
}

// ============================================================================
// Concurrent Access Tests
// ============================================================================

#[test]
fn test_concurrent_writes() {
    let path = create_test_db_path("test_concurrent_writes");
    let storage = Arc::new(
        ConsensusCommonFactory::create_async_key_value_storage(
            &path,
            "test21",
            create_test_options(),
        )
        .unwrap(),
    );

    let num_threads = 4;
    let writes_per_thread = 25;
    let counter = Arc::new(AtomicU32::new(0));

    let mut handles = vec![];

    for t in 0..num_threads {
        let storage_clone = storage.clone();
        let counter_clone = counter.clone();

        let handle = std::thread::spawn(move || {
            for i in 0..writes_per_thread {
                let key = format!("thread{}_key{}", t, i).into_bytes();
                let value = format!("value{}", i).into_bytes();

                storage_clone
                    .set(key, value, None)
                    .wait_timeout(Duration::from_secs(10))
                    .expect("timeout")
                    .unwrap();

                counter_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(counter.load(Ordering::SeqCst), num_threads * writes_per_thread);

    // Verify all values
    storage.sync(Some(Duration::from_secs(10))).unwrap();

    for t in 0..num_threads {
        for i in 0..writes_per_thread {
            let key = format!("thread{}_key{}", t, i).into_bytes();
            let value = storage
                .get(key, None)
                .wait_timeout(Duration::from_secs(5))
                .expect("timeout")
                .unwrap();
            assert!(value.is_some());
        }
    }

    storage.mark_for_destroy();
}

#[test]
fn test_concurrent_reads_and_writes() {
    let path = create_test_db_path("test_concurrent_reads_and_writes");
    let storage = Arc::new(
        ConsensusCommonFactory::create_async_key_value_storage(
            &path,
            "test22",
            create_test_options(),
        )
        .unwrap(),
    );

    // Pre-populate
    for i in 0..50 {
        storage
            .set(format!("key{}", i).into_bytes(), format!("value{}", i).into_bytes(), None)
            .wait_timeout(Duration::from_secs(5))
            .expect("timeout")
            .unwrap();
    }

    let read_count = Arc::new(AtomicU32::new(0));
    let write_count = Arc::new(AtomicU32::new(0));

    let mut handles = vec![];

    // Readers
    for _ in 0..2 {
        let storage_clone = storage.clone();
        let read_count_clone = read_count.clone();

        let handle = std::thread::spawn(move || {
            for i in 0..50 {
                let key = format!("key{}", i).into_bytes();
                let _ = storage_clone
                    .get(key, None)
                    .wait_timeout(Duration::from_secs(5))
                    .expect("timeout");
                read_count_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        handles.push(handle);
    }

    // Writers
    for t in 0..2 {
        let storage_clone = storage.clone();
        let write_count_clone = write_count.clone();

        let handle = std::thread::spawn(move || {
            for i in 0..25 {
                let key = format!("new_key_{}_{}", t, i).into_bytes();
                let value = format!("new_value{}", i).into_bytes();
                storage_clone
                    .set(key, value, None)
                    .wait_timeout(Duration::from_secs(5))
                    .expect("timeout")
                    .unwrap();
                write_count_clone.fetch_add(1, Ordering::SeqCst);
            }
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(read_count.load(Ordering::SeqCst), 100);
    assert_eq!(write_count.load(Ordering::SeqCst), 50);

    storage.mark_for_destroy();
}

// ============================================================================
// Sync Waits for Callbacks Test
// ============================================================================

#[test]
fn test_sync_waits_for_callbacks() {
    let path = create_test_db_path("test_sync_waits_for_callbacks");
    let storage = ConsensusCommonFactory::create_async_key_value_storage(
        &path,
        "test23",
        create_test_options(),
    )
    .unwrap();

    let callback_executed = Arc::new(AtomicBool::new(false));
    let callback_executed_clone = callback_executed.clone();

    // Post a write with a callback that takes time
    let _ = storage.set(
        b"key1".to_vec(),
        b"value1".to_vec(),
        Some(Box::new(move |_result| {
            // Simulate slow callback
            std::thread::sleep(Duration::from_millis(100));
            callback_executed_clone.store(true, Ordering::SeqCst);
        })),
    );

    // Sync should wait for callback to complete
    storage.sync(Some(Duration::from_secs(5))).unwrap();

    // Callback should have executed
    assert!(callback_executed.load(Ordering::SeqCst));

    storage.mark_for_destroy();
}
