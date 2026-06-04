/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::StorageTelemetry;
use crate::{
    archives::{
        archive_slice::ArchiveSlice,
        package::PKG_HEADER_SIZE,
        package_entry::PKG_ENTRY_HEADER_SIZE,
        package_entry_id::{GetFileName, PackageEntryId},
        package_id::PackageType,
    },
    block_handle_db::{BlockHandleStorage, FLAG_KEY_BLOCK},
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    tests::utils::create_block_handle_storage,
    types::BlockMeta,
    StorageAlloc,
};
use std::{future::Future, path::Path, pin::Pin, sync::Arc};
use ton_block::{error, AccountIdPrefixFull, BlockIdExt, Result, ShardIdent, UInt256};

const DB_PATH: &str = "../../target/test";

const ARCHIVE_PATH: &str = "archive/packages/arch0000/";
const ARCHIVE_00000_GOLD_PATH: &str = "src/tests/testdata/archive.00000.pack.gold";
const ARCHIVE_00100_GOLD_PATH: &str = "src/tests/testdata/archive.00100.pack.gold";
const ARCHIVE_00200_GOLD_PATH: &str = "src/tests/testdata/archive.00200.pack.gold";

struct TestContext {
    archive_slice: ArchiveSlice,
    block_handle_storage: BlockHandleStorage,
}

async fn prepare_test(
    name: &str,
    package_type: PackageType,
    shard_split_depth: u8,
    archive_id: u32,
) -> Result<(Arc<RocksDb>, TestContext)> {
    let db_root = Path::new(DB_PATH).join(name);
    let _ = std::fs::remove_dir_all(&db_root);
    let db = RocksDb::new(DB_PATH, name, None, AccessType::ReadWrite)?;
    let archive_slice = ArchiveSlice::new_empty(
        db.clone(),
        Arc::new(db_root),
        archive_id,
        package_type,
        shard_split_depth,
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )
    .await?;
    let (block_handle_storage, _) = create_block_handle_storage(db.clone());
    let test_context = TestContext { archive_slice, block_handle_storage };
    Ok((db, test_context))
}

async fn destroy_db(db: Arc<RocksDb>, name: &str) {
    drop(db);
    destroy_rocks_db(DB_PATH, name).await.unwrap();
}

type Pinned = Pin<Box<dyn Future<Output = Result<()>>>>;

async fn run_test(
    name: &str,
    package_type: PackageType,
    shard_split_depth: u8,
    archive_id: u32,
    scenario: impl Fn(TestContext) -> Pinned,
) -> Result<()> {
    let (db, test_context) =
        prepare_test(name, package_type, shard_split_depth, archive_id).await?;
    scenario(test_context).await?;
    destroy_db(db, name).await;
    Ok(())
}

#[tokio::test]
async fn test_scenario_gold() -> Result<()> {
    async fn scenario(mut test_context: TestContext) -> Result<()> {
        // Populating...

        let empty_id = PackageEntryId::<BlockIdExt>::Empty;
        let data = vec![1, 2, 3, 4, 5];
        for mc_seq_no in 0..250 {
            let block_id = BlockIdExt::with_params(
                ShardIdent::masterchain(),
                mc_seq_no,
                UInt256::with_array([mc_seq_no as u8; 32]),
                UInt256::default(),
            );
            let meta = BlockMeta::with_data(0, 0, 0, 0, 0);
            let handle = test_context
                .block_handle_storage
                .create_handle(block_id.clone(), meta, None)?
                .ok_or_else(|| error!("Cannot create handle for block {}", block_id))?;
            if mc_seq_no == 0 {
                // Prepend with Empty record
                test_context.archive_slice.add_file(&handle, &empty_id, vec![1, 2, 3]).await?;
            }
            let entry_id = PackageEntryId::Proof(&block_id);
            test_context.archive_slice.add_file(&handle, &entry_id, data.clone()).await?;
            let file = test_context
                .archive_slice
                .get_file(&handle, &entry_id)
                .await?
                .ok_or_else(|| error!("Cannot get file from archive for block {block_id}"))?;
            assert_eq!(file.filename(), &entry_id.filename());
            assert_eq!(file.data(), &data);
        }

        // Comparing...

        let archive_path = test_context.archive_slice.db_root_path.join(ARCHIVE_PATH);
        assert_eq!(
            tokio::fs::read(archive_path.join("archive.00000.pack")).await.unwrap(),
            tokio::fs::read(ARCHIVE_00000_GOLD_PATH).await.unwrap()
        );
        assert_eq!(
            tokio::fs::read(archive_path.join("archive.00100.pack")).await.unwrap(),
            tokio::fs::read(ARCHIVE_00100_GOLD_PATH).await.unwrap()
        );
        assert_eq!(
            tokio::fs::read(archive_path.join("archive.00200.pack")).await.unwrap(),
            tokio::fs::read(ARCHIVE_00200_GOLD_PATH).await.unwrap()
        );

        let hdr = PKG_HEADER_SIZE + PKG_ENTRY_HEADER_SIZE;
        let read = test_context
            .archive_slice
            .get_slice(0, (hdr + empty_id.filename().as_bytes().len()) as u64 + 1, 2)
            .await?;
        assert_eq!(read, vec![2, 3]);

        // Deleting...

        test_context.archive_slice.destroy().await?;
        assert!(!archive_path.join("archive.00000.pack").exists());
        assert!(!archive_path.join("archive.00100.pack").exists());
        assert!(!archive_path.join("archive.00200.pack").exists());
        drop(test_context);
        Ok(())
    }

    run_test("test_archive_slice_scenario_gold", PackageType::Blocks, 0, 0, |ctx| {
        Box::pin(scenario(ctx))
    })
    .await
}

#[tokio::test]
async fn test_key_blocks_slice() -> Result<()> {
    async fn scenario(test_context: TestContext) -> Result<()> {
        let data = vec![1, 2, 3, 4, 5];
        let key_blocks = vec![456, 777, 1976, 5456, 7324, 10345, 15822, 24054, 27000];
        for mc_seq_no in key_blocks {
            let block_id = BlockIdExt::with_params(
                ShardIdent::masterchain(),
                mc_seq_no,
                UInt256::rand(),
                UInt256::rand(),
            );
            let meta = BlockMeta::with_data(FLAG_KEY_BLOCK, 0, 0, 0, 0);
            let handle = test_context
                .block_handle_storage
                .create_handle(block_id.clone(), meta, None)?
                .ok_or_else(|| error!("Cannot create handle for block {}", block_id))?;
            let entry_id = PackageEntryId::Block(&block_id);
            test_context.archive_slice.add_file(&handle, &entry_id, data.clone()).await?;
            let file = test_context
                .archive_slice
                .get_file(&handle, &entry_id)
                .await?
                .ok_or_else(|| error!("Cannot get file from archive for block {}", block_id))?;
            assert_eq!(file.filename(), &entry_id.filename());
            assert_eq!(file.data(), &data);
        }
        drop(test_context);
        Ok(())
    }

    run_test("test_key_blocks_slice", PackageType::KeyBlocks, 0, 0, |ctx| Box::pin(scenario(ctx)))
        .await
}

#[tokio::test]
async fn test_lookup_proof_by_seqno() -> Result<()> {
    async fn scenario(test_context: TestContext) -> Result<()> {
        let proof_data = vec![7u8, 8, 9];
        let mc_seqno = 55u32;

        let block_id = BlockIdExt::with_params(
            ShardIdent::masterchain(),
            mc_seqno,
            UInt256::with_array([mc_seqno as u8; 32]),
            UInt256::default(),
        );
        let meta = BlockMeta::with_data(0, 1000, 100_000, mc_seqno, 0);
        let handle = test_context
            .block_handle_storage
            .create_handle(block_id.clone(), meta, None)?
            .ok_or_else(|| error!("Cannot create handle"))?;

        test_context
            .archive_slice
            .add_file(&handle, &PackageEntryId::Block(&block_id), vec![1, 2, 3])
            .await?;
        test_context
            .archive_slice
            .add_file(&handle, &PackageEntryId::Proof(&block_id), proof_data.clone())
            .await?;

        let prefix = AccountIdPrefixFull { workchain_id: -1, prefix: 0 };

        let result = test_context.archive_slice.lookup_proof_by_seqno(&prefix, mc_seqno).await?;
        let (found_id, found_data) = result.expect("proof should be found");
        assert_eq!(found_id, block_id);
        assert_eq!(found_data, proof_data);

        let result = test_context.archive_slice.lookup_proof_by_seqno(&prefix, 999).await?;
        assert!(result.is_none(), "lookup of non-existent seqno should return None");

        drop(test_context);
        Ok(())
    }

    run_test("test_lookup_proof_by_seqno", PackageType::Blocks, 0, 50, |ctx| {
        Box::pin(scenario(ctx))
    })
    .await
}

/// Regression for the truncate bug: re-importing into a package that already exists
/// (e.g. partially written by the node) must REFRESH the persisted entry size to the
/// new file size. Otherwise the stale (smaller) size would later truncate the full
/// .pack on reopen and drop the freshly imported tail.
#[tokio::test]
async fn test_import_package_entries_refreshes_existing_size() -> Result<()> {
    use crate::archives::package_entry_meta_db::PackageEntryInfo;

    let name = "test_import_refresh_size";
    let db_root = Path::new(DB_PATH).join(name);
    let _ = std::fs::remove_dir_all(&db_root);
    let db = RocksDb::new(DB_PATH, name, None, AccessType::ReadWrite)?;
    let archive_id = 0u32;
    let slice = ArchiveSlice::new_for_import(
        db.clone(),
        Arc::new(db_root),
        archive_id,
        PackageType::Blocks,
        0,
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )
    .await?;
    let shard = ShardIdent::masterchain();
    let entry = PackageEntryInfo { seqno: archive_id, shard: shard.clone() };

    // import_package_entries() expects the .pack file to already be on disk (placed by
    // import_package() before the call). Create a dummy one (header-sized is enough —
    // read-only open only checks the file length).
    let pack_dir = slice.db_root_path().join(ARCHIVE_PATH);
    std::fs::create_dir_all(&pack_dir)?;
    std::fs::write(pack_dir.join("archive.00000.pack"), vec![0u8; 64])?;

    // First import creates the package with size1.
    let size1 = 1000u64;
    slice.import_package_entries(archive_id, &shard, size1, &[]).await?;
    let pi = slice.get_package_by_entry(&entry).await.expect("package must be created");
    let meta1 = slice.entry_db.get_value(&pi.index().into())?;
    assert_eq!(meta1.entry_size(), size1, "size after first import");

    // Second import on the EXISTING package must refresh the persisted size to size2.
    let size2 = 5000u64;
    slice.import_package_entries(archive_id, &shard, size2, &[]).await?;
    let meta2 = slice.entry_db.get_value(&pi.index().into())?;
    assert_eq!(
        meta2.entry_size(),
        size2,
        "persisted size must be refreshed on existing package (truncate-bug regression)",
    );

    drop(slice);
    destroy_db(db, name).await;
    Ok(())
}
