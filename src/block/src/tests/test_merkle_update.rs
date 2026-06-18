/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{
    define_HashmapE, generate_test_account, read_single_root_boc, write_boc, AccountTestOptions,
    Block, BocWriter, CellLoader, CurrencyCollection, MerkleProof, ShardState, UsageTree,
};
use std::{
    fs::read,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
    time::Instant,
};

#[test]
fn test_merkle_update() {
    let mut acc = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let old_cell = acc.serialize().unwrap();
    let f = CurrencyCollection::with_coins(20);
    acc.add_funds(&f).unwrap();

    let mut data = SliceData::new(vec![
        0b00011111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    let data1 = SliceData::new(vec![
        0b00001111, 0b11111111, 0b11111111, 0b01110111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    let data2 = SliceData::new(vec![
        0b00111111, 0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    let data3 = SliceData::new(vec![
        0b00000111, 0b11111111, 0b01111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    let data4 = SliceData::new(vec![
        0b00111111, 0b00111111, 0b00111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    data.append_reference(data1).unwrap();
    data.append_reference(data2).unwrap();
    data.append_reference(data3).unwrap();
    data.append_reference(data4).unwrap();
    acc.set_data(data.into_cell().unwrap());

    let new_cell = acc.serialize().unwrap();
    assert_ne!(old_cell, new_cell);
    let mupd = MerkleUpdate::create(&old_cell, &new_cell).unwrap();
    let (updated_cell, _) = mupd.apply_for(&old_cell).unwrap();
    assert_eq!(new_cell, updated_cell);
}

#[test]
fn test_merkle_update_serialization() {
    let mut acc = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let old_cell = acc.serialize().unwrap();
    let f = CurrencyCollection::with_coins(20);
    acc.add_funds(&f).unwrap();
    let data = SliceData::new(vec![
        0b00011111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    acc.set_data(data.into_cell().unwrap());

    let new_cell = acc.serialize().unwrap();
    assert_ne!(old_cell, new_cell);
    let mupd = MerkleUpdate::create(&old_cell, &new_cell).unwrap();
    let mupd_bytes = write_boc(&mupd.serialize().unwrap()).unwrap();
    let mupd2 = MerkleUpdate::construct_from_bytes(&mupd_bytes).unwrap();
    let (updated_cell, _) = mupd2.apply_for(&old_cell).unwrap();
    assert_eq!(new_cell, updated_cell);
}

#[test]
fn test_empty_merkle_update() {
    let ss = ShardState::default();
    let cell = ss.serialize().unwrap();
    let mupd = MerkleUpdate::create(&cell, &cell).unwrap();
    let (cell2, _) = mupd.apply_for(&cell).unwrap();
    assert_eq!(cell, cell2);
}

#[test]
fn test_empty_merkle_update2() {
    let ss = ShardState::default();
    let cell1 = ss.serialize().unwrap();
    let cell2 = Cell::default();
    let mupd = MerkleUpdate::create(&cell1, &cell2).unwrap();
    let (cell3, _) = mupd.apply_for(&cell1).unwrap();
    assert_eq!(cell2, cell3);
}

#[test]
fn test_merkle_update_for_other_bags() {
    let cell1 = BuilderData::with_raw(vec![1, 2, 3, 0x80], 4).unwrap().into_cell().unwrap();
    let cell2 = BuilderData::with_raw(vec![5, 6, 7, 0x80], 4).unwrap().into_cell().unwrap();
    let mupd = MerkleUpdate::create(&cell1, &cell2).unwrap();
    let (cell3, _) = mupd.apply_for(&cell1).unwrap();
    assert_eq!(cell2, cell3);
}

#[test]
fn test_merkle_update_with_hasmaps() {
    define_HashmapE! {MerkleUpdates, 32, MerkleUpdate}
    let gen = |a: u32| {
        let mut acc = generate_test_account(true, AccountTestOptions::with_default_setup(true));
        let old_cell = acc.serialize().unwrap();
        let f = CurrencyCollection::with_coins(a as u64);
        acc.add_funds(&f).unwrap();
        let data = SliceData::new(vec![
            (a & 0xff) as u8,
            0b11111111,
            0b11111111,
            0b11111111,
            0b11111111,
            0b11111111,
            0b11111111,
            0b11110100,
        ]);
        acc.set_data(data.into_cell().unwrap());
        let new_cell = acc.serialize().unwrap();
        assert_ne!(old_cell, new_cell);
        MerkleUpdate::create(&old_cell, &new_cell).unwrap()
    };

    let _rng = rand::thread_rng();
    let mut map = MerkleUpdates::default();
    for _ in 0..100 {
        map.set(&rand::random::<u32>(), &gen(rand::random::<u32>())).unwrap();
    }
    let map_cell = map.serialize().unwrap();
    BocWriter::with_root(&map_cell).unwrap();
}

#[test]
fn test_merkle_update3() {
    let mut root1 = BuilderData::new();
    let mut a = BuilderData::new();
    let mut b = BuilderData::new();

    root1.append_raw(&[0], 8).unwrap();
    a.append_raw(&[1], 8).unwrap();
    b.append_raw(&[2], 8).unwrap();

    a.checked_append_reference(b.into_cell().unwrap()).unwrap();
    root1.checked_append_reference(a.into_cell().unwrap()).unwrap();

    let mut root2 = BuilderData::new();
    let mut a = BuilderData::new();
    let mut b = BuilderData::new();

    root2.append_raw(&[0], 8).unwrap();
    a.append_raw(&[1], 8).unwrap();
    b.append_raw(&[2], 8).unwrap();

    a.checked_append_reference(b.clone().into_cell().unwrap()).unwrap();
    root2.checked_append_reference(b.into_cell().unwrap()).unwrap();
    root2.checked_append_reference(a.into_cell().unwrap()).unwrap();

    let root1 = root1.into_cell().unwrap();
    let root2 = root2.into_cell().unwrap();

    let mupd = MerkleUpdate::create(&root1, &root2).unwrap();
    let (root3, _) = mupd.apply_for(&root1).unwrap();

    assert_eq!(root2, root3);
}

const PATH_TO_SS: &str = "src/tests/data/block_with_ss/shard-states/";
const PATH_TO_BLOCK: &str = "src/tests/data/block_with_ss/blocks/";

fn check_one_mu(index: u64) {
    let (block, _block_len) = block_from_file(&format!("{}{}", PATH_TO_BLOCK, index));
    let (shard_state, _ss_len) = ss_from_file(&format!("{}{}", PATH_TO_SS, index - 1));
    let (new_shard_state, _new_ss_len) = ss_from_file(&format!("{}{}", PATH_TO_SS, index));

    // apply update from block and compare result with new state
    let (updated_shard_state, _) =
        block.read_state_update().unwrap().apply_for(&shard_state).unwrap();
    assert_eq!(new_shard_state.repr_hash(), updated_shard_state.repr_hash());

    // calculate own mu, apply it and compare result with new state
    let mu = MerkleUpdate::create(&shard_state, &new_shard_state).unwrap();

    let (updated_shard_state_2, _) = mu.apply_for(&shard_state).unwrap();
    assert_eq!(new_shard_state.repr_hash(), updated_shard_state_2.repr_hash());
}

fn block_from_file(path: &str) -> (Block, usize) {
    let orig_bytes =
        read(Path::new(path)).unwrap_or_else(|_| panic!("Error reading file {:?}", path));

    let block = Block::construct_from_bytes(&orig_bytes).expect("Error deserializing Block");

    (block, orig_bytes.len())
}

fn ss_from_file(path: &str) -> (Cell, usize) {
    let orig_bytes =
        read(Path::new(path)).unwrap_or_else(|_| panic!("Error reading file {:?}", path));

    let root_cell = read_single_root_boc(&orig_bytes).expect("Error deserializing ShardState");
    (root_cell, orig_bytes.len())
}

#[test]
fn test_merkle_update_real_data() {
    for i in 2660..=2665
    /*2690*/
    {
        check_one_mu(i);
    }
    for i in 571525..=571527
    /*571555*/
    {
        check_one_mu(i);
    }
}

#[test]
fn test_merkle_update_create_fast() {
    for index in 2660..=2665 {
        let (shard_state, _ss_len) = ss_from_file(&format!("{}{}", PATH_TO_SS, index - 1));
        let (new_shard_state, _new_ss_len) = ss_from_file(&format!("{}{}", PATH_TO_SS, index));

        let usage_tree = UsageTree::with_root(shard_state.clone());

        // calculate MU regular way to fill usage tree
        MerkleUpdate::create(&shard_state, &new_shard_state).unwrap();

        let mu =
            MerkleUpdate::create_fast(&shard_state, &new_shard_state, |h| usage_tree.contains(h))
                .unwrap();

        let (updated_shard_state_2, _) = mu.apply_for(&shard_state).unwrap();
        assert_eq!(new_shard_state.repr_hash(), updated_shard_state_2.repr_hash());
    }
}

fn prepare_data_for_bench(
    root_path: &str,
    shard: &str,
    start_block: u32,
    blocks_count: u32,
) -> (Cell, Vec<MerkleUpdate>) {
    let (ss, _) = ss_from_file(&format!("{}/states/{}/{}", root_path, shard, start_block));
    let mut updates = vec![];
    for seqno in start_block + 1..=start_block + blocks_count {
        let (block, _) = block_from_file(&format!("{}/blocks/{}/{}", root_path, shard, seqno));
        updates.push(block.read_state_update().unwrap());
    }
    (ss, updates)
}

// To perform benchmark you should provide needed number of blocks (`blocks_count`)
// named by their seqno starting from `start_number` in the `root_path`/blocks dir,
// and shard state for start block in `root_path`/states dir (named like the start block)
#[ignore]
#[test]
fn merkle_update_apply_benchmark() {
    let max_threads = 4;
    let blocks_count = 300;
    let root_path = "/full-node-test";
    let shard = "0c00000000000000";
    let start_block = 4440457;

    for threads in 1..=max_threads {
        // Prepare
        let mut data = vec![];
        for _ in 0..threads {
            data.push(prepare_data_for_bench(root_path, shard, start_block, blocks_count));
        }

        // Go
        print!("\nmerkle_update_apply_benchmark {} thread(s): ", threads);
        let mut handles = vec![];
        for _ in 0..threads {
            let (mut ss, updates) = data.pop().unwrap();
            handles.push(std::thread::spawn(move || {
                let now = Instant::now();

                for update in updates {
                    ss = update.apply_for(&ss).unwrap().0;
                }

                print!("{} ", now.elapsed().as_millis());
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }
    println!();
}

#[test]
fn test_merkle_update4() {
    let mut root1 = BuilderData::new();
    root1.append_raw(&[0], 8).unwrap();

    for i in 0..1024 {
        let mut new_root = BuilderData::new();
        new_root.append_raw(&[i as u8], 8).unwrap();
        new_root.checked_append_reference(root1.clone().into_cell().unwrap()).unwrap();
        new_root.checked_append_reference(root1.into_cell().unwrap()).unwrap();
        root1 = new_root;
    }

    let mut root2 = BuilderData::new();
    let mut a = BuilderData::new();
    let mut b = BuilderData::new();

    root2.append_raw(&[0], 8).unwrap();
    a.append_raw(&[1], 8).unwrap();
    b.append_raw(&[2], 8).unwrap();

    a.checked_append_reference(b.clone().into_cell().unwrap()).unwrap();
    root2.checked_append_reference(b.into_cell().unwrap()).unwrap();
    root2.checked_append_reference(a.into_cell().unwrap()).unwrap();

    let root1 = root1.into_cell().unwrap();
    let root2 = root2.into_cell().unwrap();

    let usage_tree = UsageTree::with_root(root1.clone());
    let mut uc = usage_tree.root_cell();
    while let Ok(c) = uc.reference(1) {
        uc = c;
    }

    let mupd = MerkleUpdate::create_fast(&root1, &root2, |h| usage_tree.contains(h)).unwrap();
    let (root3, _) = mupd.apply_for(&root1).unwrap();

    assert_eq!(root2, root3);
}

#[test]
fn test_merkle_update5() {
    // std::env::set_var("RUST_BACKTRACE", "full");

    fn create_cell(bytes: &[u8], refs: &[&Cell]) -> Cell {
        let mut c = BuilderData::new();
        c.append_raw(bytes, bytes.len() * 8).unwrap();
        for child in refs {
            c.checked_append_reference((*child).clone()).unwrap();
        }
        c.into_cell().unwrap()
    }

    /* old tree
          root
      c5        c6
    c1  c2    c3  c4
              c1  c2
    */
    let c1 = create_cell(&[1, 1, 1], &[]);
    let c2 = create_cell(&[2, 2, 2], &[]);
    let c3 = create_cell(&[3, 3, 3], &[]);
    let c4 = create_cell(&[4, 4, 4], &[]);
    let c5 = create_cell(&[5, 5, 5], &[&c1, &c2]);
    let c6 = create_cell(&[6, 6, 6], &[&c3, &c4]);
    let old_tree = create_cell(&[1], &[&c5, &c6]);

    /* new tree
          root'
      c5        c6'
    c1  c2    c3'  c4'
              c1
    */
    let c3_ = create_cell(&[3, 3, 4], &[]);
    let c4_ = create_cell(&[4, 4, 5, 6], &[]);
    let c6_ = create_cell(&[6, 6, 6], &[&c3_, &c4_]);
    let new_tree = create_cell(&[1], &[&c5, &c6_]);

    // merkle proof of c6 subtree in old tree
    let cells = [
        old_tree.repr_hash().clone(),
        c6.repr_hash().clone(),
        c3.repr_hash().clone(),
        c4.repr_hash().clone(),
        c1.repr_hash().clone(),
        c2.repr_hash().clone(),
    ];
    let old_proof =
        MerkleProof::create(&old_tree, |h| cells.contains(h)).unwrap().serialize().unwrap();

    // merkle proof of c6' subtree in new tree
    let cells = [
        new_tree.repr_hash().clone(),
        c6_.repr_hash().clone(),
        c3_.repr_hash().clone(),
        c4_.repr_hash().clone(),
        c1.repr_hash().clone(),
    ];
    let new_proof =
        MerkleProof::create(&new_tree, |h| cells.contains(h)).unwrap().serialize().unwrap();

    for i in 0..2 {
        println!("old_proof\n{:#.100}", old_proof);
        println!("new_proof\n{:#.100}", new_proof);

        // merkle update old -> new proof
        let update = if i == 0 {
            // without optimisations
            let update = MerkleUpdate::create(&old_proof, &new_proof).unwrap();
            println!("update (without optimisations)\n{:#.100}", update.serialize().unwrap());
            update.serialize().unwrap()
        } else {
            // "fast"
            let cells = [
                old_tree.repr_hash().clone(),
                c6.repr_hash().clone(), /*c3.repr_hash(), c4.repr_hash(), c1.repr_hash()*/
            ];

            let update =
                MerkleUpdate::create_fast(&old_proof, &new_proof, |h| cells.contains(h)).unwrap();
            println!("update (fast)\n{:#.100}", update.serialize().unwrap());
            update.serialize().unwrap()
        };

        // merkle update as a subtree of big tree
        let b1 = create_cell(&[1], &[&update]);
        let b2 = create_cell(&[2], &[]);
        let b3 = create_cell(&[3], &[]);
        let b4 = create_cell(&[3], &[&b1, &b2, &b3]);
        let b5 = create_cell(&[3], &[&b4]);

        // merkle proof of merkle update in the big tree
        let mut cells =
            vec![b1.repr_hash().clone(), b4.repr_hash().clone(), b5.repr_hash().clone()];
        fn visit(c: &Cell, cells: &mut Vec<UInt256>) {
            cells.push(c.repr_hash().clone());
            for child in c.clone_references().unwrap() {
                visit(&child, cells);
            }
        }
        visit(&update, &mut cells);
        let proof = MerkleProof::create(&b5, |h| cells.contains(h)).unwrap();

        // ser-de
        let proof = MerkleProof::construct_from_bytes(&proof.write_to_bytes().unwrap()).unwrap();
        println!("proof\n{:#.100}", proof.serialize().unwrap());

        // apply merkle update from the last tree
        let block = proof.proof.clone().virtualize(1);

        let update = MerkleUpdate::construct_from_cell(
            block.reference(0).unwrap().reference(0).unwrap().reference(0).unwrap(),
        )
        .unwrap();

        let (new_proof_2, _) = update.apply_for(&old_proof).unwrap();
        assert_eq!(new_proof, new_proof_2);
    }
}

// ---------------------------------------------------------------------------
// apply_lazy_unchecked tests
// ---------------------------------------------------------------------------

/// Test factory: stores cells indexed by repr_hash, builds lazy cells whose
/// loader pulls from that map. Counts loader invocations so tests can assert
/// laziness.
struct TestLazyFactory {
    cells_by_hash: ahash::AHashMap<UInt256, Cell>,
    loader_calls: Arc<AtomicUsize>,
}

impl TestLazyFactory {
    fn new(root: &Cell) -> Arc<Self> {
        let mut cells_by_hash = ahash::AHashMap::new();
        Self::collect(root, &mut cells_by_hash);
        Arc::new(Self { cells_by_hash, loader_calls: Arc::new(AtomicUsize::new(0)) })
    }

    fn collect(cell: &Cell, out: &mut ahash::AHashMap<UInt256, Cell>) {
        if out.insert(cell.repr_hash().clone(), cell.clone()).is_some() {
            return;
        }
        for i in 0..cell.references_count() {
            let r = cell.reference(i).unwrap();
            Self::collect(&r, out);
        }
    }
}

impl CellsFactory for TestLazyFactory {
    fn create_cell(self: Arc<Self>, builder: BuilderData) -> Result<Cell> {
        builder.into_cell()
    }

    fn create_lazy_load_cell(self: Arc<Self>, pruned: &Cell, merkle_depth: u8) -> Result<Cell> {
        let cells = self.cells_by_hash.clone();
        let counter = Arc::clone(&self.loader_calls);
        let loader: CellLoader = Arc::new(move |hash: &UInt256| {
            counter.fetch_add(1, AtomicOrdering::SeqCst);
            cells
                .get(hash)
                .cloned()
                .ok_or_else(|| error!("Cell not found in test factory: {:x}", hash))
        });
        Cell::lazy_from_pruned(pruned, loader, merkle_depth)
    }
}

/// Build a small account-based old/new pair, like the existing
/// test_merkle_update setup but kept local.
fn build_account_update_pair() -> (Cell, Cell) {
    let mut acc = generate_test_account(true, AccountTestOptions::with_default_setup(true));
    let old_cell = acc.serialize().unwrap();
    acc.add_funds(&CurrencyCollection::with_coins(42)).unwrap();
    let data = SliceData::new(vec![
        0b00011111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111, 0b11111111,
        0b11110100,
    ]);
    acc.set_data(data.into_cell().unwrap());
    let new_cell = acc.serialize().unwrap();
    assert_ne!(old_cell.repr_hash(), new_cell.repr_hash());
    (old_cell, new_cell)
}

#[test]
fn test_apply_lazy_unchecked_matches_classic() {
    let (old_cell, new_cell) = build_account_update_pair();
    let mupd = MerkleUpdate::create(&old_cell, &new_cell).unwrap();

    let default_factory: Arc<dyn CellsFactory> = Arc::new(DefaultCellsFactory);
    let (classic_root, _) = mupd.apply_with_factory(&old_cell, &default_factory).unwrap();

    let lazy_factory: Arc<dyn CellsFactory> = TestLazyFactory::new(&old_cell);
    let (lazy_root, _) = mupd.apply_lazy_unchecked(&lazy_factory).unwrap();

    assert_eq!(lazy_root.repr_hash(), classic_root.repr_hash());
    assert_eq!(lazy_root.repr_hash(), new_cell.repr_hash());
    assert_eq!(lazy_root.repr_depth(), classic_root.repr_depth());
    assert_eq!(lazy_root.level_mask(), classic_root.level_mask());
}

#[test]
fn test_apply_lazy_unchecked_empty_update() {
    // new == old → MerkleUpdate.new is a single pruned branch over `old`.
    // apply_lazy_unchecked must hit the `self.new_hash == self.old_hash`
    // path on line ~432 and produce a lazy cell that mirrors `old`.
    let (old_cell, _) = build_account_update_pair();
    let mupd = MerkleUpdate::create(&old_cell, &old_cell).unwrap();
    assert_eq!(mupd.old_hash, mupd.new_hash);

    let factory = TestLazyFactory::new(&old_cell);
    let factory_dyn: Arc<dyn CellsFactory> = factory.clone();
    let (lazy_root, _) = mupd.apply_lazy_unchecked(&factory_dyn).unwrap();

    // Hash/depth/mask come from the pruned branch inline data — no loader
    // calls should be needed.
    assert_eq!(lazy_root.repr_hash(), old_cell.repr_hash());
    assert_eq!(lazy_root.repr_depth(), old_cell.repr_depth());
    assert_eq!(lazy_root.level_mask(), old_cell.level_mask());
    assert_eq!(
        factory.loader_calls.load(AtomicOrdering::SeqCst),
        0,
        "loader must not run while only hash/depth/mask are queried",
    );

    // Touching data triggers exactly one load.
    let _ = lazy_root.data();
    assert_eq!(factory.loader_calls.load(AtomicOrdering::SeqCst), 1);
    let _ = lazy_root.data();
    assert_eq!(factory.loader_calls.load(AtomicOrdering::SeqCst), 1, "load is memoised");
}

#[test]
fn test_apply_lazy_unchecked_lazy_loading() {
    // For a non-trivial update we expect hash queries on the resulting tree
    // to be answerable without loading every pruned subtree.
    let (old_cell, new_cell) = build_account_update_pair();
    let mupd = MerkleUpdate::create(&old_cell, &new_cell).unwrap();

    let factory = TestLazyFactory::new(&old_cell);
    let factory_dyn: Arc<dyn CellsFactory> = factory.clone();
    let (lazy_root, _) = mupd.apply_lazy_unchecked(&factory_dyn).unwrap();

    let before = factory.loader_calls.load(AtomicOrdering::SeqCst);
    let _ = lazy_root.repr_hash();
    let _ = lazy_root.repr_depth();
    let _ = lazy_root.level_mask();
    let after = factory.loader_calls.load(AtomicOrdering::SeqCst);
    assert_eq!(before, after, "repr_hash/repr_depth/level_mask must not trigger loader",);
}
