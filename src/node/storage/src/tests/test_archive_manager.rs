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
        archive_manager::ArchiveManager,
        package_entry_id::{GetFileName, PackageEntryId},
        ARCHIVE_PACKAGE_SIZE,
    },
    block_handle_db::{BlockHandleStorage, FLAG_KEY_BLOCK},
    db::rocksdb::{destroy_rocks_db, AccessType, RocksDb},
    tests::utils::create_block_handle_storage,
    types::BlockMeta,
    StorageAlloc,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{atomic::AtomicU8, Arc},
};
use ton_block::{error, AccountIdPrefixFull, BlockIdExt, Result, ShardIdent, UInt256};

const DB_PATH: &str = "../../target/test";

const TESTDATA_PATH: &str = "src/tests/testdata/";
const ARCHIVE_00000_GOLD: &str = "archive.00000-2.pack.gold";
const ARCHIVE_00100_GOLD: &str = "archive.00100.pack.gold";
const ARCHIVE_00200_GOLD: &str = "archive.00200.pack.gold";

async fn create_manager(
    root: &str,
    name: &str,
    cleanup: bool,
) -> Result<(ArchiveManager, Arc<RocksDb>)> {
    let path = Path::new(root).join(name);
    if cleanup {
        std::fs::remove_dir_all(&path).ok();
    }
    let db = RocksDb::new(root, name, None, AccessType::ReadWrite)?;
    let manager = ArchiveManager::with_data(
        db.clone(),
        Arc::new(path),
        0,
        Arc::new(AtomicU8::new(0)),
        #[cfg(feature = "telemetry")]
        Arc::new(StorageTelemetry::default()),
        Arc::new(StorageAlloc::default()),
    )
    .await?;
    Ok((manager, db))
}

async fn test_downloading(manager: &ArchiveManager, mc_seq_no: u32, gold_path: &str) -> Result<()> {
    let archive_id = manager.get_archive_id(mc_seq_no, &ShardIdent::masterchain()).await.unwrap();
    let mut data = Vec::new();
    let mut offset = 0;
    const ARCHIVE_PACKAGE_SIZE: u32 = 1024;
    loop {
        let slice = manager.get_archive_slice(archive_id, offset, ARCHIVE_PACKAGE_SIZE).await?;
        data.extend_from_slice(&slice[..]);
        if slice.len() < ARCHIVE_PACKAGE_SIZE as usize {
            break;
        }
        offset += slice.len() as u64;
    }
    assert_eq!(
        &data[..],
        tokio::fs::read(Path::new(TESTDATA_PATH).join(gold_path)).await.unwrap().as_slice()
    );
    Ok(())
}

#[tokio::test]
async fn test_scenario() -> Result<()> {
    const DB_NAME: &str = "test_archive_manager_scenario";

    let (manager, db) = create_manager(DB_PATH, DB_NAME, true).await?;
    let (block_handle_storage, _) = create_block_handle_storage(db.clone());

    let data = vec![1, 2, 3, 4, 5];
    for mc_seq_no in 0..250 {
        let block_id = BlockIdExt::with_params(
            ShardIdent::masterchain(),
            mc_seq_no,
            UInt256::with_array([mc_seq_no as u8; 32]),
            UInt256::default(),
        );
        let meta = BlockMeta::with_data(0, 0, 0, 0, 0);
        let handle = block_handle_storage
            .create_handle(block_id.clone(), meta, None)?
            .ok_or_else(|| error!("Cannot create handle for block {}", block_id))?;

        let entry_id = PackageEntryId::Proof(&block_id);
        manager.add_file(&entry_id, &data).await?;
        handle.set_proof();
        handle.set_block_applied();
        manager.move_to_archive(&handle, || Ok(())).await?;
    }

    let shard = ShardIdent::masterchain();
    for mc_seq_no in 0..300 {
        assert_eq!(
            manager.get_archive_id(mc_seq_no, &shard).await,
            Some(((mc_seq_no - mc_seq_no % ARCHIVE_PACKAGE_SIZE) as u64) << 32),
            "mc_seq_no = {}",
            mc_seq_no
        );
    }

    assert!(manager.get_archive_id(300, &shard).await.is_none());

    test_downloading(&manager, 0, ARCHIVE_00000_GOLD).await?;
    test_downloading(&manager, 100, ARCHIVE_00100_GOLD).await?;
    test_downloading(&manager, 222, ARCHIVE_00200_GOLD).await?;

    drop(block_handle_storage);
    drop(manager);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await?;
    Ok(())
}

#[tokio::test]
async fn test_scenario_keyblocks_10m() -> Result<()> {
    const DB_NAME: &str = "test_scenario_keyblocks_10m";

    let (manager, db) = create_manager(DB_PATH, DB_NAME, true).await?;
    let (block_handle_storage, _) = create_block_handle_storage(db.clone());

    let data = vec![1, 2, 3, 4, 5];
    let numbers = vec![7, 43, 560, 5480, 95480, 134354, 3548755, 10174254, 101574254, 1343548755];

    for mc_seq_no in numbers {
        let block_id = BlockIdExt::with_params(
            ShardIdent::masterchain(),
            mc_seq_no,
            UInt256::from_le_bytes(&mc_seq_no.to_le_bytes()),
            UInt256::default(),
        );
        let meta = BlockMeta::with_data(FLAG_KEY_BLOCK, 0, 0, 0, 0);

        let entry_id = PackageEntryId::Proof(&block_id);

        manager.add_file(&entry_id, &data).await?;

        let handle = block_handle_storage
            .create_handle(block_id.clone(), meta, None)?
            .ok_or_else(|| error!("Cannot create handle for block {}", block_id))?;

        handle.set_proof();
        handle.set_block_applied();
        manager.move_to_archive(&handle, || Ok(())).await?;
        handle.set_archived();
        assert!(handle.is_key_block()?);
        assert!(handle.has_proof());
        assert!(handle.is_archived());
        block_handle_storage.save_handle(&handle, None)?;

        tokio::time::sleep(std::time::Duration::from_millis(1)).await;

        let block_id = BlockIdExt::with_params(
            ShardIdent::masterchain(),
            mc_seq_no,
            UInt256::from_le_bytes(&mc_seq_no.to_le_bytes()),
            UInt256::default(),
        );
        let handle = block_handle_storage
            .load_handle_by_id(&block_id)?
            .ok_or_else(|| error!("Cannot load handle for block {}", block_id))?;
        assert!(handle.has_proof());
        assert!(handle.is_archived());
        assert!(handle.is_key_block()?);

        let entry_id = PackageEntryId::Proof(&block_id);
        manager.get_file(&handle, &entry_id).await?;
    }

    drop(block_handle_storage);
    drop(manager);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

fn generate_block_id(workchain_id: i32, shard_prefix_tagged: u64, seq_no: u32) -> BlockIdExt {
    let shard_id = ShardIdent::with_tagged_prefix(workchain_id, shard_prefix_tagged).unwrap();
    BlockIdExt::with_params(shard_id, seq_no, UInt256::rand(), UInt256::rand())
}

fn generate_short_filename(id: &BlockIdExt) -> String {
    PackageEntryId::ProofLink(id).filename_short()
}

#[tokio::test]
async fn test_clean_unapplied_files() -> Result<()> {
    const DB_NAME: &str = "clean_db";

    let (manager, db) = create_manager(DB_PATH, DB_NAME, true).await?;

    let id = generate_block_id(555, 0xF8000000_00000000, 100);
    let filename = generate_short_filename(&id);
    let path = manager.unapplied_files_path().join(filename);

    std::fs::write(&path, "test").unwrap();

    let id = generate_block_id(555, 0xF8000000_00000000, 99);
    manager.clean_unapplied_files(&[id]).await;

    assert!(path.exists(), "file {:?} must not be removed", path);

    let id = generate_block_id(333, 0xF8000000_00000000, 101);
    manager.clean_unapplied_files(&[id]).await;

    assert!(path.exists(), "file {:?} must not be removed", path);

    let id = generate_block_id(555, 0xE8000000_00000000, 101);
    manager.clean_unapplied_files(&[id]).await;

    assert!(path.exists(), "file {:?} must not be removed", path);

    let id = generate_block_id(555, 0xF8000000_00000000, 101);
    manager.clean_unapplied_files(&[id]).await;

    assert!(!path.exists(), "file {:?} must be removed", path);

    drop(manager);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}

#[allow(dead_code)]
struct TestArchiveManager {
    db: Arc<RocksDb>,
    archive_manager: ArchiveManager,
    block_handle_storage: BlockHandleStorage,
}

impl TestArchiveManager {
    async fn new(root: &str, name: &str) -> Result<Self> {
        let (archive_manager, db) = create_manager(root, name, true).await?;
        let (block_handle_storage, _) = create_block_handle_storage(db.clone());
        Ok(Self { db, archive_manager, block_handle_storage })
    }

    fn master_block_id(mc_seq_no: u32, version: u8) -> BlockIdExt {
        BlockIdExt::with_params(
            ShardIdent::masterchain(),
            mc_seq_no,
            UInt256::from_le_bytes(&mc_seq_no.to_le_bytes()),
            UInt256::from_le_bytes(&[version]),
        )
    }

    async fn add_entries(
        &self,
        range: impl IntoIterator<Item = u32>,
        version: u8,
        key_blocks: &[u32],
    ) {
        for mc_seq_no in range {
            let block_id = Self::master_block_id(mc_seq_no, version);
            let flags = if key_blocks.contains(&mc_seq_no) { FLAG_KEY_BLOCK } else { 0 };
            let meta = BlockMeta::with_data(flags, 0, 0, 0, 0);
            let handle = self
                .block_handle_storage
                .create_handle(block_id.clone(), meta, None)
                .unwrap()
                .unwrap();
            let entry_id = PackageEntryId::Block(&block_id);
            self.archive_manager.add_file(&entry_id, &[1, 2, 3, 4, 5]).await.unwrap();
            handle.set_data();
            let entry_id = PackageEntryId::Proof(&block_id);
            self.archive_manager.add_file(&entry_id, &[6, 7, 8, 9]).await.unwrap();
            handle.set_proof();
            handle.set_block_applied();
            self.archive_manager.move_to_archive(&handle, || Ok(())).await.unwrap();
            handle.set_archived();
            assert!(handle.is_archived());
            self.block_handle_storage.save_handle(&handle, None).unwrap();
        }
    }

    async fn trunc_with_check(&self, mc_seq_no: u32, version: u8) {
        let trunc_block_id = Self::master_block_id(mc_seq_no, version);
        println!("trunc on {}", trunc_block_id);
        self.archive_manager
            .trunc(&trunc_block_id, &|id: &BlockIdExt| id.seq_no() >= trunc_block_id.seq_no())
            .await
            .unwrap();

        let block_id = Self::master_block_id(mc_seq_no - 1, version);
        let handle = self.block_handle_storage.load_handle_by_id(&block_id).unwrap().unwrap();
        let entry_id = PackageEntryId::Block(&block_id);
        self.archive_manager.get_file(&handle, &entry_id).await.unwrap();
        let entry_id = PackageEntryId::Proof(&block_id);
        self.archive_manager.get_file(&handle, &entry_id).await.unwrap();

        let handle = self.block_handle_storage.load_handle_by_id(&trunc_block_id).unwrap().unwrap();
        let entry_id = PackageEntryId::Block(&trunc_block_id);
        self.archive_manager
            .get_file(&handle, &entry_id)
            .await
            .expect_err("block should not be read");
        let entry_id = PackageEntryId::Proof(&trunc_block_id);
        self.archive_manager
            .get_file(&handle, &entry_id)
            .await
            .expect_err("proof should not be read");

        let block_id = Self::master_block_id(mc_seq_no + 1, version);
        let handle = self.block_handle_storage.load_handle_by_id(&block_id).unwrap().unwrap();
        let entry_id = PackageEntryId::Block(&block_id);
        self.archive_manager
            .get_file(&handle, &entry_id)
            .await
            .expect_err("block should not be read");
        let entry_id = PackageEntryId::Proof(&block_id);
        self.archive_manager
            .get_file(&handle, &entry_id)
            .await
            .expect_err("proof should not be read");
    }
}

#[tokio::test]
async fn test_archive_truncate() {
    const DB_NAME: &str = "node_db_slices";
    destroy_rocks_db(DB_PATH, DB_NAME).await.ok();

    let manager = TestArchiveManager::new(DB_PATH, DB_NAME).await.unwrap();

    manager.add_entries(1..306, 0, &[53]).await;
    manager.trunc_with_check(54, 0).await;
    manager.trunc_with_check(53, 0).await;
    manager.trunc_with_check(52, 0).await;

    manager.add_entries(500..700, 1, &[500]).await;

    manager.add_entries(10_000_000..10_000_070, 1, &[10_000_000, 10_000_053]).await;
    manager.trunc_with_check(10_000_054, 1).await;
    manager.trunc_with_check(10_000_053, 1).await;
    manager.trunc_with_check(10_000_001, 1).await;

    drop(manager);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap()
}

#[tokio::test]
async fn test_block_index() -> Result<()> {
    // crate::tests::utils::init_test_log();
    // std::env::set_var("RUST_BACKTRACE", "full");

    fn make_data(id: &BlockIdExt) -> Vec<u8> {
        let mut data = id.shard().shard_prefix_with_tag().to_le_bytes().to_vec();
        data.extend_from_slice(&id.seq_no().to_le_bytes());
        data
    }

    const DB_NAME: &str = "test_block_index";

    let cleanup = true;
    let (manager, db) = create_manager(DB_PATH, DB_NAME, cleanup).await?;
    let (block_handle_storage, _) = create_block_handle_storage(db.clone());

    let total_mc_blocks = 41_000;
    let init_utime = 1760361100;
    let mut gen_utime = init_utime;
    let mut lt = 1_000_000;

    if cleanup {
        let mut shards_seqno = HashMap::new();
        shards_seqno.insert(ShardIdent::with_tagged_prefix(0, 0x4000_0000_0000_0000)?, 1);
        shards_seqno.insert(ShardIdent::with_tagged_prefix(0, 0xc000_0000_0000_0000)?, 1);
        for mc_seqno in 1..total_mc_blocks {
            let id = generate_block_id(-1, 0x8000_0000_0000_0000, mc_seqno);
            manager.add_file(&PackageEntryId::Block(&id), &make_data(&id)).await?;
            manager.add_file(&PackageEntryId::Proof(&id), &[1, 2, 3]).await?;
            let flags = if rand::random::<u16>() % 12345 == 0 { FLAG_KEY_BLOCK } else { 0 };
            let block_meta = BlockMeta::with_data(flags, gen_utime, lt, mc_seqno, 0);
            let handle = block_handle_storage.create_handle(id.clone(), block_meta, None)?.unwrap();
            handle.set_data();
            handle.set_proof();
            handle.set_state();
            manager.move_to_archive(&handle, || Ok(())).await?;
            handle.set_block_applied();
            block_handle_storage.save_handle(&handle, None)?;

            for (shard_id, seqno) in shards_seqno.iter_mut() {
                for i in 0..rand::random::<u8>() % 2 {
                    let id = generate_block_id(
                        shard_id.workchain_id(),
                        shard_id.shard_prefix_with_tag(),
                        *seqno,
                    );
                    manager.add_file(&PackageEntryId::Block(&id), &make_data(&id)).await?;
                    manager.add_file(&PackageEntryId::ProofLink(&id), &[1, 2, 3]).await?;
                    let block_meta =
                        BlockMeta::with_data(0, gen_utime, lt + i as u64 * 1_000_000, mc_seqno, 0);
                    let handle =
                        block_handle_storage.create_handle(id.clone(), block_meta, None)?.unwrap();
                    handle.set_data();
                    handle.set_proof_link();
                    handle.set_state();
                    manager.move_to_archive(&handle, || Ok(())).await?;
                    handle.set_block_applied();
                    block_handle_storage.save_handle(&handle, None)?;
                    *seqno += 1;
                }
                if rand::random::<u16>() % 3 != 0 {
                    gen_utime += 1;
                }
            }

            gen_utime += 1;
            lt += 4_000_000;

            if mc_seqno % 1_000 == 0 {
                println!("Added {} mc blocks", mc_seqno);
            }
        }
    } else {
        lt = 4_000_000 * (total_mc_blocks - 1) as u64;
        gen_utime = init_utime + (total_mc_blocks);
    }

    for _ in 0..20_000 {
        let lt = rand::random::<u64>() % (lt - 4_000_000);
        log::info!("lookup by lt {}", lt);
        let prefix = AccountIdPrefixFull { workchain_id: -1, prefix: rand::random::<u64>() };
        let (id, data) = manager.lookup_block_by_lt(&prefix, lt).await?.unwrap();
        assert_eq!(data, make_data(&id));

        let seqno = rand::random::<u32>() % (total_mc_blocks - 1) + 1;
        log::info!("lookup by seqno {}", seqno);
        let prefix = AccountIdPrefixFull { workchain_id: -1, prefix: rand::random::<u64>() };
        let (id, data) = manager.lookup_block_by_seqno(&prefix, seqno).await?.unwrap();
        assert_eq!(data, make_data(&id));

        let mut found = 0;
        let utime = init_utime + (rand::random::<u32>() % (gen_utime - init_utime)) - 100;
        log::info!("lookup by utime {}", utime);
        let prefix = AccountIdPrefixFull { workchain_id: 0, prefix: rand::random::<u64>() };
        manager
            .lookup_blocks_by_utime(
                &prefix,
                utime,
                Box::new(|id, data| {
                    assert_eq!(data, make_data(&id));
                    found += 1;
                    Ok(true)
                }),
            )
            .await?;
        assert!(found > 0);
    }

    drop(block_handle_storage);
    drop(manager);
    drop(db);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let (manager, db) = create_manager(DB_PATH, DB_NAME, false).await?;

    let lt = 1_000_000 + 19_998 * 4_000_000 + 100;
    log::info!("lookup by lt {}", lt);
    let prefix = AccountIdPrefixFull { workchain_id: -1, prefix: rand::random::<u64>() };
    let (id, data) = manager.lookup_block_by_lt(&prefix, lt).await?.unwrap();
    assert_eq!(data, make_data(&id));
    assert_eq!(id.seq_no(), 20_000);

    let lt = 1_000_000 + 19_999 * 4_000_000 - 100;
    log::info!("lookup by lt {}", lt);
    let prefix = AccountIdPrefixFull { workchain_id: -1, prefix: rand::random::<u64>() };
    let (id, data) = manager.lookup_block_by_lt(&prefix, lt).await?.unwrap();
    assert_eq!(data, make_data(&id));
    assert_eq!(id.seq_no(), 20_000);

    for _ in 0..20_000 {
        let lt = rand::random::<u64>() % lt;
        log::info!("lookup by lt {}", lt);
        let prefix = AccountIdPrefixFull { workchain_id: 0, prefix: rand::random::<u64>() };
        let (id, data) = manager.lookup_block_by_lt(&prefix, lt).await?.unwrap();
        assert_eq!(data, make_data(&id));

        let seqno = rand::random::<u32>() % (total_mc_blocks * 4);
        log::info!("lookup by seqno {}", seqno);
        let prefix = AccountIdPrefixFull { workchain_id: -1, prefix: rand::random::<u64>() };
        if let Some((id, data)) = manager.lookup_block_by_seqno(&prefix, seqno).await? {
            assert_eq!(data, make_data(&id));
        }

        let mut found = 0;
        let utime = init_utime + rand::random::<u32>() % (gen_utime - init_utime) - 100;
        log::info!("lookup by utime {}", utime);
        let prefix = AccountIdPrefixFull { workchain_id: -1, prefix: rand::random::<u64>() };
        manager
            .lookup_blocks_by_utime(
                &prefix,
                utime,
                Box::new(|id, data| {
                    assert_eq!(data, make_data(&id));
                    found += 1;
                    Ok(true)
                }),
            )
            .await?;
        assert!(found > 0);
    }

    drop(manager);
    drop(db);
    destroy_rocks_db(DB_PATH, DB_NAME).await.unwrap();
    Ok(())
}
