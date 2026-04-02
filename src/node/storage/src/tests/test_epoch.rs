/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;

#[tokio::test]
async fn test_epoch_router_validation() {
    let dir = tempfile::tempdir().unwrap();

    let config = ArchivalModeConfig {
        epoch_size: 0,
        new_epochs_path: dir.path().to_path_buf(),
        existing_epochs: vec![],
    };
    assert!(EpochRouter::new(&config).await.is_err());

    let config = ArchivalModeConfig {
        epoch_size: 10_000, // not a multiple of ARCHIVE_SLICE_SIZE
        new_epochs_path: dir.path().to_path_buf(),
        existing_epochs: vec![],
    };
    assert!(EpochRouter::new(&config).await.is_err());
}

#[tokio::test]
async fn test_epoch_router_resolve_and_create() {
    let dir = tempfile::tempdir().unwrap();
    let new_epochs_path = dir.path().join("new_epochs");

    let config = ArchivalModeConfig {
        epoch_size: 40_000,
        new_epochs_path: new_epochs_path.clone(),
        existing_epochs: vec![],
    };

    let router = EpochRouter::new(&config).await.unwrap();

    // No epochs exist yet
    assert!(router.resolve(0).is_none());
    assert!(router.resolve(39_999).is_none());

    // Create epoch for mc_seq_no 0
    let epoch = router.resolve_or_create(0).await.unwrap();
    assert_eq!(epoch.mc_seq_no_start(), 0);
    assert_eq!(epoch.mc_seq_no_end(), 39_999);
    assert!(epoch.path().starts_with(&new_epochs_path));

    // Resolve same epoch
    let epoch2 = router.resolve(20_000).unwrap();
    assert_eq!(epoch2.mc_seq_no_start(), 0);

    // Create second epoch
    let epoch3 = router.resolve_or_create(50_000).await.unwrap();
    assert_eq!(epoch3.mc_seq_no_start(), 40_000);
    assert_eq!(epoch3.mc_seq_no_end(), 79_999);

    // Verify both exist
    assert!(router.resolve(0).is_some());
    assert!(router.resolve(50_000).is_some());
    assert!(router.resolve(80_000).is_none());
}

#[tokio::test]
async fn test_epoch_router_with_existing_epochs() {
    let dir = tempfile::tempdir().unwrap();
    let epoch0_path = dir.path().join("epoch_0");
    let epoch1_path = dir.path().join("epoch_1");
    let new_epochs_path = dir.path().join("new_epochs");

    std::fs::create_dir_all(&epoch0_path).unwrap();
    std::fs::create_dir_all(&epoch1_path).unwrap();

    // Write metadata for existing epochs
    let meta0 = EpochMeta { mc_seq_no_start: 0, mc_seq_no_end: 39_999 };
    let meta1 = EpochMeta { mc_seq_no_start: 40_000, mc_seq_no_end: 79_999 };
    write_epoch_meta(&epoch0_path, &meta0).await.unwrap();
    write_epoch_meta(&epoch1_path, &meta1).await.unwrap();

    let config = ArchivalModeConfig {
        epoch_size: 40_000,
        new_epochs_path,
        existing_epochs: vec![EpochEntry { path: epoch0_path }, EpochEntry { path: epoch1_path }],
    };

    let router = EpochRouter::new(&config).await.unwrap();

    let e0 = router.resolve(0).unwrap();
    assert_eq!(e0.mc_seq_no_start(), 0);
    assert_eq!(e0.mc_seq_no_end(), 39_999);

    let e1 = router.resolve(40_000).unwrap();
    assert_eq!(e1.mc_seq_no_start(), 40_000);
    assert_eq!(e1.mc_seq_no_end(), 79_999);

    assert!(router.resolve(80_000).is_none());
}

#[tokio::test]
async fn test_epoch_router_rejects_misaligned_existing() {
    let dir = tempfile::tempdir().unwrap();
    let epoch_path = dir.path().join("bad_epoch");
    std::fs::create_dir_all(&epoch_path).unwrap();

    // Epoch with wrong size (60_000 != 40_000)
    let meta = EpochMeta { mc_seq_no_start: 0, mc_seq_no_end: 59_999 };
    write_epoch_meta(&epoch_path, &meta).await.unwrap();

    let config = ArchivalModeConfig {
        epoch_size: 40_000,
        new_epochs_path: dir.path().join("new_epochs"),
        existing_epochs: vec![EpochEntry { path: epoch_path }],
    };

    assert!(EpochRouter::new(&config).await.is_err());
}

#[tokio::test]
async fn test_epoch_router_discovers_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let new_epochs_path = dir.path().join("new_epochs");

    // First "run": create epochs dynamically
    let config = ArchivalModeConfig {
        epoch_size: 40_000,
        new_epochs_path: new_epochs_path.clone(),
        existing_epochs: vec![],
    };
    let router = EpochRouter::new(&config).await.unwrap();
    router.resolve_or_create(0).await.unwrap();
    router.resolve_or_create(50_000).await.unwrap();
    assert!(router.resolve(0).is_some());
    assert!(router.resolve(50_000).is_some());
    drop(router);

    // Second "run": new router should discover epochs from new_epochs_path
    let config2 = ArchivalModeConfig {
        epoch_size: 40_000,
        new_epochs_path: new_epochs_path.clone(),
        existing_epochs: vec![],
    };
    let router2 = EpochRouter::new(&config2).await.unwrap();
    let e0 = router2.resolve(0).unwrap();
    assert_eq!(e0.mc_seq_no_start(), 0);
    assert_eq!(e0.mc_seq_no_end(), 39_999);

    let e1 = router2.resolve(50_000).unwrap();
    assert_eq!(e1.mc_seq_no_start(), 40_000);
    assert_eq!(e1.mc_seq_no_end(), 79_999);

    assert!(router2.resolve(80_000).is_none());
}
