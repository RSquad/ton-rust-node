/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{base64_decode, crc32_digest, BuilderData, IBitstring, SliceData, MAX_DEPTH};
use std::{
    fs::read,
    io::Cursor,
    path::Path,
    time::{Duration, Instant},
};

struct CellsHMStorage {
    cells: HashMap<u32, Cell>,
    loader: CellLoader,
}
impl CellsHMStorage {
    pub fn new() -> Self {
        Self {
            cells: HashMap::new(),
            loader: Arc::new(|_hash| fail!("Empty loader cannot load any cell")),
        }
    }
}
impl CellsTempStorage for CellsHMStorage {
    fn load_hash_and_depth(&self, index: u32) -> Result<(UInt256, u16)> {
        let cell = self.cells.get(&index).ok_or_else(|| error!("Cell #{} was not found", index))?;
        Ok((cell.repr_hash().clone(), cell.repr_depth()))
    }
    fn load_cell(&self, index: u32) -> Result<Cell> {
        self.cells.get(&index).cloned().ok_or_else(|| error!("Cell #{} was not found", index))
    }
    fn store_cell(&mut self, index: u32, cell: &Cell) -> Result<()> {
        self.cells.insert(index, cell.clone());
        Ok(())
    }
    fn cleanup(&mut self) -> Result<()> {
        Ok(())
    }
    fn loader(&self) -> &CellLoader {
        &self.loader
    }
}

fn build_tree_with_params(mut depth: u16, max_depth: u16, cells_count: &mut u32) -> Result<Cell> {
    let mut b = BuilderData::new();
    b.append_u32(rand::random::<u32>())?;
    b.append_u32(rand::random::<u32>())?;
    b.append_u32(rand::random::<u32>())?;
    b.append_u32(rand::random::<u32>())?;
    depth += 1;
    if depth < max_depth {
        b.checked_append_reference(build_tree_with_params(depth, max_depth, cells_count)?)?;
        b.checked_append_reference(build_tree_with_params(depth, max_depth, cells_count)?)?;
        b.checked_append_reference(build_tree_with_params(depth, max_depth, cells_count)?)?;
        b.checked_append_reference(build_tree_with_params(depth, max_depth, cells_count)?)?;
    }
    *cells_count += 1;
    b.into_cell()
}

/*
                                root0
        c1				c7				c12			c15
    c2	c4	c5		c8		c9			c13
    c3		c6				c10			c14
                            c11
*/

#[cfg(feature = "ci_run")]
fn build_tree() -> Cell {
    let mut root = BuilderData::new();
    let mut c1 = BuilderData::new();
    let mut c2 = BuilderData::new();
    let mut c3 = BuilderData::new();
    let mut c4 = BuilderData::new();
    let mut c5 = BuilderData::new();
    let mut c6 = BuilderData::new();
    let mut c7 = BuilderData::new();
    let mut c8 = BuilderData::new();
    let mut c9 = BuilderData::new();
    let mut c10 = BuilderData::new();
    let mut c11 = BuilderData::new();
    let mut c12 = BuilderData::new();
    let mut c13 = BuilderData::new();
    let mut c14 = BuilderData::new();
    let mut c15 = BuilderData::new();

    root.append_u8(0).unwrap();
    c1.append_u8(1).unwrap();
    c2.append_u8(2).unwrap();
    c3.append_u8(3).unwrap();
    c4.append_u8(4).unwrap();
    c5.append_u8(5).unwrap();
    c6.append_u8(6).unwrap();
    c7.append_u8(7).unwrap();
    c8.append_u8(8).unwrap();
    c9.append_u8(9).unwrap();
    c10.append_u8(10).unwrap();
    c11.append_u8(11).unwrap();
    c12.append_u8(12).unwrap();
    c13.append_u8(13).unwrap();
    c14.append_u8(14).unwrap();
    c15.append_u8(15).unwrap();

    c13.append_reference(c14);
    c12.append_reference(c13);
    c10.append_reference(c11);
    c9.append_reference(c10);
    c7.append_reference(c8);
    c7.append_reference(c9);
    c5.append_reference(c6);
    c2.append_reference(c3);
    c1.append_reference(c2);
    c1.append_reference(c4);
    c1.append_reference(c5);
    root.append_reference(c1);
    root.append_reference(c7);
    root.append_reference(c12);
    root.append_reference(c15);

    root.into_cell().unwrap()
}

/*
                root0
        c1				c4
    c2		c3

*/
#[cfg(feature = "ci_run")]
fn build_tree2(val: u8) -> Cell {
    let mut root = BuilderData::new();
    let mut c1 = BuilderData::new();
    let mut c2 = BuilderData::new();
    let mut c3 = BuilderData::new();
    let mut c4 = BuilderData::new();

    root.append_u8(val).unwrap();
    c1.append_u8(val + 1).unwrap();
    c2.append_u8(val + 2).unwrap();
    c3.append_u8(val + 3).unwrap();
    c4.append_u8(val + 4).unwrap();

    c1.append_reference(c2);
    c1.append_reference(c3);
    root.append_reference(c1);
    root.append_reference(c4);

    root.into_cell().unwrap()
}

/*
                root0
        c1				c4
    c2		c3

*/
fn build_tree3(val: u32) -> Cell {
    let mut root = BuilderData::new();
    let mut c1 = BuilderData::new();
    let mut c2 = BuilderData::new();
    let mut c3 = BuilderData::new();
    let mut c4 = BuilderData::new();

    root.append_u32(val).unwrap();
    c1.append_u32(val + 1).unwrap();
    c2.append_u32(val + 2).unwrap();
    c3.append_u32(val + 3).unwrap();
    c4.append_u32(val + 4).unwrap();

    c1.append_reference(c2);
    c1.append_reference(c3);
    root.append_reference(c1);
    root.append_reference(c4);

    root.into_cell().unwrap()
}

#[test]
fn test_many_bocs_in_one_file() -> Result<()> {
    // std::env::set_var("RUST_BACKTRACE", "full");

    let mut data = Vec::new();
    let mut roots = vec![];

    for i in 0..10 {
        let root = build_tree3(i);
        BocWriter::with_root(&root)?.write(&mut data)?;
        roots.push(root);
    }

    let mut cursor = Cursor::new(&data);
    for root in roots {
        let roots_restored = BocReader::new().stream_read(&mut cursor)?.roots;
        assert_eq!(root, roots_restored[0]);
    }
    Ok(())
}

#[cfg(feature = "ci_run")]
#[test]
fn test_tree_of_cells_serialization_deserialization() -> Result<()> {
    // std::env::set_var("RUST_BACKTRACE", "full");

    println!("one root");
    for flags in 0..32 {
        let boc_flags = BocFlags::from_bits(flags).unwrap();
        if boc_flags.contains(BocFlags::CacheBits) && !boc_flags.contains(BocFlags::Index) {
            continue;
        }
        println!("BOC flags {:?}", boc_flags);

        let root = build_tree();

        let mut data = Vec::new();
        BocWriter::with_flags([root.clone()], boc_flags)?.write(&mut data)?;

        let roots_restored = BocReader::new().read(&data)?.roots;
        assert_eq!(root, roots_restored[0].clone());

        let roots_restored_2 = BocReader::new().stream_read(&mut Cursor::new(&data))?.roots;
        assert_eq!(root, roots_restored_2[0].clone());

        let root_only = read_boc_root(&data)?;
        assert_eq!(root.data(), root_only.storage());

        let roots_restored_3 =
            BocReader::new().read_to_storage(&data, &mut CellsHMStorage::new())?.roots;
        assert_eq!(root, roots_restored_3[0].clone());
    }

    println!("many roots");
    for flags in 21..22 {
        let boc_flags = BocFlags::from_bits(flags).unwrap();
        if boc_flags.contains(BocFlags::CacheBits) && !boc_flags.contains(BocFlags::Index) {
            continue;
        }
        println!("BOC flags {:?}", boc_flags);

        let root0 = build_tree();
        let root1 = build_tree2(111);
        let root2 = build_tree2(222);

        let mut data = Vec::new();
        BocWriter::with_flags([root0.clone(), root1.clone(), root2.clone()], boc_flags)?
            .write(&mut data)?;
        let roots_restored = BocReader::new().read(&data)?.roots;

        assert_eq!(root0, roots_restored[0].clone());
        assert_eq!(root1, roots_restored[1].clone());
        assert_eq!(root2, roots_restored[2].clone());

        assert_ne!(root0, roots_restored[2].clone());
        assert_ne!(root1, roots_restored[0].clone());
        assert_ne!(root2, roots_restored[1].clone());

        let roots_restored_2 = BocReader::new().stream_read(&mut Cursor::new(&data))?.roots;

        assert_eq!(root0, roots_restored_2[0].clone());
        assert_eq!(root1, roots_restored_2[1].clone());
        assert_eq!(root2, roots_restored_2[2].clone());

        let roots_restored_3 =
            BocReader::new().read_to_storage(&data, &mut CellsHMStorage::new())?.roots;

        assert_eq!(root0, roots_restored_3[0].clone());
        assert_eq!(root1, roots_restored_3[1].clone());
        assert_eq!(root2, roots_restored_3[2].clone());
    }

    println!("so many roots");
    for flags in 0..32 {
        let boc_flags = BocFlags::from_bits(flags).unwrap();
        if boc_flags.contains(BocFlags::CacheBits) && !boc_flags.contains(BocFlags::Index) {
            continue;
        }
        println!("BOC flags {:?}", boc_flags);

        let len = 1024u32;
        let mut roots = vec![];
        for i in 0..len {
            roots.push(build_tree3(i * 100));
        }

        let mut data = Vec::new();
        BocWriter::with_flags(roots.clone(), boc_flags)?.write(&mut data)?;

        let roots_restored = BocReader::new().read(&data)?.roots;
        for i in 0..len {
            assert_eq!(&roots[i as usize], &roots_restored[i as usize]);
        }

        let roots_restored_2 = BocReader::new().stream_read(&mut Cursor::new(&data))?.roots;
        for i in 0..len {
            assert_eq!(&roots[i as usize], &roots_restored_2[i as usize]);
        }

        let roots_restored_3 =
            BocReader::new().read_to_storage(&data, &mut CellsHMStorage::new())?.roots;
        for i in 0..len {
            assert_eq!(&roots[i as usize], &roots_restored_3[i as usize]);
        }
    }

    Ok(())
}

/*
   root0    root1
    c2       c1
    c1       c0
    c0
*/
#[test]
fn test_roots_share_same_tree() -> Result<()> {
    let cell0 = Cell::default();
    let cell1 = BuilderData::with_raw_and_refs(vec![], 0, vec![cell0])?.into_cell()?;
    let cell2 = BuilderData::with_raw_and_refs(vec![], 0, vec![cell1.clone()])?.into_cell()?;
    let roots = vec![cell2, cell1];
    let mut output = vec![];
    let boc = BocWriter::with_roots(roots.clone())?;
    boc.write(&mut output)?;
    let res = BocReader::new().stream_read(&mut Cursor::new(&output))?;
    assert_eq!(roots, res.roots);
    Ok(())
}

// non-unique roots are banned
#[ignore]
#[test]
fn test_bug_serialization() -> Result<()> {
    let node0 = Cell::default();
    let node1 = BuilderData::with_raw(vec![0xff], 8)?.into_cell()?;
    let roots = vec![node0.clone(), node1, node0];
    let mut output = vec![];
    let boc = BocWriter::with_roots(roots.clone())?;
    boc.write(&mut output)?;
    let res = BocReader::new().stream_read(&mut Cursor::new(&output))?;
    assert_eq!(roots, res.roots);
    Ok(())
}

#[test]
fn test_number_of_bytes_to_fit() {
    assert_eq!(BocWriter::number_of_bytes_to_fit(255), 1);
    assert_eq!(BocWriter::number_of_bytes_to_fit(256), 2);
    assert_eq!(BocWriter::number_of_bytes_to_fit(200), 1);
    assert_eq!(BocWriter::number_of_bytes_to_fit(400), 2);
    assert_eq!(BocWriter::number_of_bytes_to_fit(333), 2);
    assert_eq!(BocWriter::number_of_bytes_to_fit(2000), 2);
    assert_eq!(BocWriter::number_of_bytes_to_fit(16000), 2);
    assert_eq!(BocWriter::number_of_bytes_to_fit(160000), 3);
    assert_eq!(BocWriter::number_of_bytes_to_fit(1073741823), 4);
}

#[test]
fn test_crc_pure() {
    // Some part of crc module's test from real ton sorces
    assert_eq!(crc32_digest([0; 32]), 0x8a9136aa);
    assert_eq!(crc32_digest([0xff; 32]), 0x62a8ab43);
    let data = [
        0x01, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00,
        0x00, 0x18, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00,
    ];
    assert_eq!(crc32_digest(data), 0xd9963a56);

    let mut digest = Crc32::new();
    digest.update(&data[..10]);
    digest.update(&data[10..]);
    assert_eq!(digest.finalize(), 0xd9963a56);
}

//#[ignore]
#[test]
fn test_crc_with_files() {
    let orig_bytes = read(Path::new(Path::new("src/tests/data/new-wallet-query.boc")))
        .expect("Error reading file");

    let crc1 = crc32_digest(&orig_bytes[..orig_bytes.len() - 4]);
    let crc2 = u32::from_le_bytes(
        orig_bytes[orig_bytes.len() - 4..].try_into().expect("incorrect length"),
    );

    println!("{:x}", crc1);
    println!("{:x}", crc2);

    assert_eq!(crc1, crc2);
}

#[test]
fn test_boc_write_crc() -> Result<()> {
    let mut bytes = Vec::new();
    BocWriter::with_params([Cell::default()], u16::MAX - 1, BocFlags::Crc32, &|| false)?
        .write(&mut bytes)?;

    let crc1 = crc32_digest(&bytes[..bytes.len() - 4]);
    let crc2 = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into()?);
    assert_eq!(crc1, crc2);
    Ok(())
}

#[test]
fn test_real_ton_boc2() -> Result<()> {
    // Compatibility checking

    let input = "B5EE9C7241040301000000004600024789FF86EE2B1CE113242F7CAE3511009B84F9E460D38773688AF808406AA75537991A11900102002C20DDA4F260F8005F04ED44D0D31F30A4C8CB1FC9ED540008000000005A785C4E";
    let orig_bytes = hex::decode(input)?;
    let rr = read_boc(&orig_bytes)?;

    let mut boc_flags = BocFlags::None;
    if rr.header.index_included {
        boc_flags |= BocFlags::Index;
    }
    if rr.header.has_crc {
        boc_flags |= BocFlags::Crc32;
    }
    if rr.header.has_cache_bits {
        boc_flags |= BocFlags::CacheBits;
    }

    let boc = BocWriter::with_flags(rr.roots.clone(), boc_flags)?;
    let mut bytes = Vec::new();
    boc.write(&mut bytes)?;
    let rr =
        BocReader::new().stream_read(&mut Cursor::new(&bytes)).expect("Error deserialising BOC");

    let root_only = read_boc_root(&bytes)?;
    assert_eq!(root_only.storage(), rr.roots[0].data());

    Ok(())
}

#[test]
fn test_boc_compatibility() -> Result<()> {
    fn test(path: &str) -> Result<()> {
        let data = std::fs::read(path)?;
        let read_result = read_boc(&data)?;

        let mut data2 = Vec::new();
        let flags = read_result.flags | BocFlags::TopHash;
        BocWriter::with_flags(read_result.roots.clone(), flags)?.write(&mut data2)?;
        // BocReader::new().read_inmem(Arc::new(data2.clone()))?;
        // assert_eq!(data, data2);

        // Write without flags to clean "has hashes" flags in all cells
        let mut data = Vec::new();
        BocWriter::with_flags(read_result.roots.clone(), BocFlags::None)?.write(&mut data)?;
        let read_result = read_boc(&data)?;

        let mut data2 = Vec::new();
        let flags = read_result.flags | BocFlags::TopHash;
        BocWriter::with_flags(read_result.roots.clone(), flags)?.write(&mut data2)?;
        // BocReader::new().read_inmem(Arc::new(data2.clone()))?;
        // assert_eq!(data, data2);

        let mut tcs = TestCellsStorage::with_flags(flags);
        for root in read_result.roots.iter() {
            tcs.add_root(root.clone());
        }
        let mut data3 = Vec::new();
        BigBocWriter::with_params(
            read_result.roots.clone(),
            MAX_SAFE_DEPTH,
            flags,
            &|| false,
            Arc::new(tcs),
        )?
        .write(&mut data3)?;
        assert_eq!(data, data3);

        Ok(())
    }

    test("src/tests/data/6A3BD5B96ABEA186BFEE202B70D510C29F85E126A522B08C1DCAD39F92CF5C51.boc")?;
    test("src/tests/data/45C9A4A6BCE4F9359298ED922B3C60C7EC521AAE0A2FE416CA85D336142F9B00.boc")?;

    Ok(())
}

#[test]
fn test_all_boc_modes() -> Result<()> {
    fn test(path: &str) -> Result<()> {
        let data = std::fs::read(path)?;
        let read_result = read_boc(&data)?;
        for flag in 0..32 {
            let boc_flags = BocFlags::from_bits(flag).unwrap();
            if boc_flags.contains(BocFlags::CacheBits) && !boc_flags.contains(BocFlags::Index) {
                continue;
            }
            println!("BOC flags {:?}", boc_flags);
            let mut data2 = Vec::new();
            BocWriter::with_flags(read_result.roots.clone(), boc_flags)?.write(&mut data2)?;
            let read_result2 = BocReader::new().stream_read(&mut Cursor::new(&data2))?;
            assert_eq!(read_result.roots, read_result2.roots);
        }
        Ok(())
    }

    test("src/tests/data/6A3BD5B96ABEA186BFEE202B70D510C29F85E126A522B08C1DCAD39F92CF5C51.boc")?;
    test("src/tests/data/45C9A4A6BCE4F9359298ED922B3C60C7EC521AAE0A2FE416CA85D336142F9B00.boc")?;

    Ok(())
}

fn build_tree_with_depth(depth: u16) -> Cell {
    let mut c = None;
    for _ in 0..=depth {
        let mut b = BuilderData::new();
        if let Some(c) = c {
            b.checked_append_reference(c).unwrap();
        }
        c = Some(b.finalize(depth).unwrap())
    }
    c.unwrap()
}

// #[cfg(release)] TODO: test overflows stack in debug mode
#[test]
fn test_default_max_safe_depth() {
    let handler = std::thread::Builder::new()
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            let c = build_tree_with_depth(2048);
            let b = write_boc(&c).unwrap();
            let c2 = BocReader::new()
                .stream_read(&mut std::io::Cursor::new(&b))
                .unwrap()
                .withdraw_single_root()
                .unwrap();
            assert_eq!(c, c2);
        })
        .unwrap();

    handler.join().unwrap();
}

// #[cfg(release)] TODO: test overflows stack in debug mode
#[cfg(feature = "ci_run")]
#[test]
fn test_max_depth() {
    std::thread::Builder::new()
        .stack_size(128 * 1024 * 1024)
        .spawn(|| {
            let depth = u16::MAX - 1;
            let c = build_tree_with_depth(depth);
            let mut b = vec![];

            BocWriter::with_params([c.clone()], depth, BocFlags::None, &|| false)
                .unwrap()
                .write(&mut b)
                .unwrap();

            let c2 = BocReader::new()
                .set_max_cell_depth(depth)
                .read(&b)
                .unwrap()
                .withdraw_single_root()
                .unwrap();
            assert_eq!(c, c2);

            let c4 = BocReader::new()
                .set_max_cell_depth(depth)
                .read_to_storage(&b, &mut CellsHMStorage::new())
                .unwrap()
                .withdraw_single_root()
                .unwrap();
            assert_eq!(c, c4);
        })
        .unwrap()
        .join()
        .unwrap();
}

pub struct TestCellsStorage {
    cells: HashMap<UInt256, Cell>,
    flags: BocFlags,
}

impl TestCellsStorage {
    pub fn new() -> Self {
        Self { cells: HashMap::new(), flags: BocFlags::None }
    }

    pub fn with_flags(flags: BocFlags) -> Self {
        let mut storage = Self::new();
        storage.flags = flags;
        storage
    }

    pub fn with_root(root_cell: Cell) -> Self {
        let mut storage = Self::new();
        storage.add_root(root_cell);
        storage
    }

    fn add_root(&mut self, root_cell: Cell) {
        self.add_cell(root_cell.clone());
        for i in 0..root_cell.references_count() {
            let ref_cell = root_cell.reference(i).unwrap();
            self.add_root(ref_cell);
        }
    }

    pub fn add_cell(&mut self, cell: Cell) {
        self.cells.entry(cell.repr_hash().clone()).or_insert(cell);
    }
}

impl CellsStorage for TestCellsStorage {
    fn load_cell(&self, hash: &UInt256) -> Result<Cell> {
        self.cells.get(hash).cloned().ok_or_else(|| error!("Can't find cell with hash {:x}", hash))
    }
    fn load_cell_data(
        &self,
        hash: &UInt256,
        write_hashes: bool,
        dest: &mut dyn Write,
    ) -> Result<()> {
        let cell = self.load_cell(hash)?;
        let cell_raw_data = cell.raw_data()?;
        let has_hashes = cell::store_hashes(cell_raw_data);
        match (write_hashes, has_hashes) {
            (true, true) | (false, false) => {
                // write as is
                dest.write_all(cell_raw_data)?;
            }
            (true, false) => {
                // repack with hashes
                let d1 = cell::calc_d1(
                    cell::level_mask(cell_raw_data),
                    true,
                    cell::cell_type(cell_raw_data),
                    cell::refs_count(cell_raw_data),
                );
                dest.write_all(&[d1])?;
                dest.write_all(&cell_raw_data[1..2])?; // D2
                for hash in cell.hashes() {
                    dest.write_all(hash.as_slice())?;
                }
                for depth in cell.depths() {
                    dest.write_all(&depth.to_be_bytes())?;
                }
                dest.write_all(&cell_raw_data[2..])?; // data
            }
            (false, true) => {
                // repack without hashes
                let d1 = cell::calc_d1(
                    cell::level_mask(cell_raw_data),
                    false,
                    cell::cell_type(cell_raw_data),
                    cell::refs_count(cell_raw_data),
                );
                dest.write_all(&[d1])?;
                dest.write_all(&cell_raw_data[1..2])?; // D2
                let hashes_len = (SHA256_SIZE + DEPTH_SIZE) * cell::hashes_count(cell_raw_data);
                dest.write_all(&cell_raw_data[2 + hashes_len..])?; // data
            }
        }
        Ok(())
    }
}

#[test]
fn test_boc_writer_stack() -> Result<()> {
    let mut cells_count = 0;
    let now = std::time::Instant::now();
    let root = build_tree_with_params(0, 10, &mut cells_count)?;
    let serialized_root = root.clone();
    let storage = TestCellsStorage::with_root(root.clone());
    let build_time = now.elapsed().as_millis();

    let now = std::time::Instant::now();
    let mut data = Vec::new();
    BocWriterStack::write(&mut data, Path::new("../target"), root, MAX_DEPTH, storage, &|| false)?;
    let serialize_time = now.elapsed().as_millis();

    println!("total cells {}", cells_count);
    println!("boc size {}", data.len());
    println!("build time {}ms  {}ms per cell", build_time, build_time as f64 / cells_count as f64);
    println!(
        "serialize time {}ms  {}ms per cell",
        serialize_time,
        serialize_time as f64 / cells_count as f64
    );

    let now = std::time::Instant::now();
    let deserialized_root =
        BocReader::new().stream_read(&mut Cursor::new(data))?.withdraw_single_root()?;
    let deserialize_time = now.elapsed().as_millis();
    println!(
        "deserialize time {}ms  {}ms per cell",
        deserialize_time,
        deserialize_time as f64 / cells_count as f64
    );

    assert_eq!(serialized_root, deserialized_root);

    Ok(())
}

#[test]
fn test_full_tree() -> Result<()> {
    let mut cells_count = 0;
    let now = std::time::Instant::now();
    let c = build_tree_with_params(0, 10, &mut cells_count)?;
    let build_time = now.elapsed().as_millis();

    let now = std::time::Instant::now();
    let b = write_boc(&c)?;
    let serialize_time = now.elapsed().as_millis();

    println!("total cells {}", cells_count);
    println!("boc size {}", b.len());
    println!("build time {}ms  {}ms per cell", build_time, build_time as f64 / cells_count as f64);
    println!(
        "serialize time {}ms  {}ms per cell",
        serialize_time,
        serialize_time as f64 / cells_count as f64
    );

    let now = std::time::Instant::now();
    let c2 = BocReader::new().stream_read(&mut Cursor::new(b))?.withdraw_single_root()?;
    let deserialize_time = now.elapsed().as_millis();
    println!(
        "deserialize time {}ms  {}ms per cell",
        deserialize_time,
        deserialize_time as f64 / cells_count as f64
    );

    assert_eq!(c, c2);

    Ok(())
}

fn c(bitstring: &str, children: impl AsRef<[Cell]>) -> Result<Cell> {
    let mut b = SliceData::from_string(bitstring)?.as_builder()?;
    for child in children.as_ref() {
        b.checked_append_reference(child.clone())?;
    }
    b.into_cell()
}

macro_rules! C {
    ($s:expr) => {
        c($s, &[])?
    };
    ($s:expr, $($x:expr),+ $(,)?) => {
        c($s, vec!($($x),+))?
    };
}

#[test]
fn test_boc_write_iterative() -> Result<()> {
    let root =
        C!("9023afe200000000000000000000000000000000000000000000000000000000000000000000000000000000002_",
            C!("00000000000000001_"),
            C!("dd45d21dba003_",
                C!("a0000020406080a0c0e10121416181a1c1e20222426282a2c2e30323436383a3c3f75174876e800800000000000000000000000000000000000000000000000000000000000000000000000000000004_",
                    C!("cec_",
                        C!("2_",
                            C!("50b24_"),
                            C!("2_",
                                C!("0391_"),
                                C!("040259_")
                            )
                        ),
                        C!("2_",
                            C!("2_",
                                C!("040321_"),
                                C!("0403e9_")
                            ),
                            C!("2_",
                                C!("0404b1_"),
                                C!("07312dc9_")
                            )
                        )
                    ),
                    C!("dc4c190b8000008101820283038404850586068707880889098a0a8b0b8c0c8d0d8e0e8f0f916407be01d6f34562de0000000000000000a2e90edd001ef7c_",
                        C!("cec_",
                            C!("2_",
                                C!("50b24_"),
                                C!("2_",
                                    C!("0391_"),
                                    C!("040259_")
                                )
                            ),
                            C!("2_",
                                C!("2_",
                                    C!("040321_"),
                                    C!("0403e9_")
                                ),
                                C!("2_",
                                    C!("0404b1_"),
                                    C!("07312dc9_")
                                )
                            )
                        ),
                        C!("3ffffffffffffff4_",
                            C!("3ffffffffffffff4_",
                                C!("3f3ffffffffffff4_",
                                    C!("0ffffffffffffff4_",
                                        C!("3fff3ffffffffff4_")
                                    )
                                )
                            )
                        ),
                        C!("3fff1ffffffffff4_"),
                        C!("a00f4172af42bd2799479d2d99695d9e4eb46e3144c7915d9455629fcdc3cc42e59",
                            C!("3ffffffffffffff4_")
                        )
                    )
                ),
                C!("cec_",
                    C!("2_",
                        C!("50b24_"),
                        C!("2_",
                            C!("0391_"),
                            C!("040259_")
                        )
                    ),
                    C!("2_",
                        C!("2_",
                            C!("040321_"),
                            C!("0403e9_")
                        ),
                        C!("2_",
                            C!("0404b1_"),
                            C!("07312dc9_")
                        )
                    )
                )
            ),
            C!("00000000000000000000000000000000000")
        );

    let data = write_boc(&root).unwrap();
    assert_eq!(read_single_root_boc(&data).unwrap(), root);

    let expected = hex::decode("b5ee9c7201021b010001a700035b9023afe20000000000000000000000000000000000000000000000000000000000000000000000000000000000200102030011000000000000000010020ddd45d21dba003004060023000000000000000000000000000000000008029fa0000020406080a0c0e10121416181a1c1e20222426282a2c2e30323436383a3c3f75174876e8008000000000000000000000000000000000000000000000000000000000000000000000000000000040605047ddc4c190b8000008101820283038404850586068707880889098a0a8b0b8c0c8d0d8e0e8f0f916407be01d6f34562de0000000000000000a2e90edd001ef7c0060708090203cec00f10010f3ffffffffffffff40a000f3fff1ffffffffff40143a00f4172af42bd2799479d2d99695d9e4eb46e3144c7915d9455629fcdc3cc42e5980e010f3ffffffffffffff40b010f3f3ffffffffffff40c010f0ffffffffffffff40d000f3fff3ffffffffff4000f3ffffffffffffff402012011120201201516000550b24002012013140003039100050402590201201718020120191a000504032100050403e900050404b1000707312dc9").unwrap();
    assert_eq!(expected, data);

    Ok(())
}

fn test_bad_boc(boc: Vec<u8>, read_root: bool) {
    match BocReader::new().stream_read(&mut std::io::Cursor::new(&boc)) {
        Ok(_) => panic!("BocReader::new().stream_read must panic"),
        Err(e) => println!("{:?}", e),
    }
    if read_root {
        match BocReader::new().read_root(&boc) {
            Ok(_) => panic!("BocReader::new().read_root must panic"),
            Err(e) => println!("{:?}", e),
        }
    }
    match BocReader::new().read(boc.as_slice()) {
        Ok(_) => panic!("BocReader::new().read must panic"),
        Err(e) => println!("{:?}", e),
    }
    match BocReader::new().read_to_storage(boc.as_slice(), &mut CellsHMStorage::new()) {
        Ok(_) => panic!("BocReader::new().read_to_storage must panic"),
        Err(e) => println!("{:?}", e),
    }
}

#[test]
fn test_bad_boc_1() {
    let mut bb = Vec::new();
    bb.extend_from_slice(&0xb5ee9c72_u32.to_be_bytes()); // magic
    bb.push(0b0000_0100); // flags
    bb.push(1); // offset size
    bb.extend_from_slice(&u32::MAX.to_be_bytes()); // cells
    bb.extend_from_slice(&u32::MAX.to_be_bytes()); // roots
    bb.extend_from_slice(&0u32.to_be_bytes()); // absent count
    bb.push(0); // tot_cells_size

    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_2() {
    let bb = base64_decode("aP9l8wIGAAAAAAAAAABo8w==").unwrap();
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_3() {
    let mut bb = Vec::new();
    bb.extend_from_slice(&0xb5ee9c72_u32.to_be_bytes()); // magic
    bb.push(0b0000_0100); // flags
    bb.push(1); // offset size
    bb.extend_from_slice(&0_u32.to_be_bytes()); // cells
    bb.extend_from_slice(&0_u32.to_be_bytes()); // roots
    bb.extend_from_slice(&0_u32.to_be_bytes()); // absent count
    bb.push(0); // tot_cells_size

    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_4() {
    let bb = base64_decode(
        "te6ccjEHBwIAEO6ccjAHBw0AAAAAJgAAAAEvAAAAAAEv8PDw8Cpk/wsAAAAAAAAAAAAAAAAAAv+s/8M=",
    )
    .unwrap();
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_5() {
    let bb = vec![
        0xb5u8, 0xee, 0x9c, 0x72, // magic
        1,    // flags ref size
        1,    // offset size
        1,    // cells count
        0,    // roots count
        0,    // absent count
        2,    // total cell size
        8,    // d1 <-- exotic cell
        0,    // d2 <-- empty data
    ];
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_6() {
    let bb =
        base64_decode("te6ccgEBBAEBIQEAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_7() {
    let bb = base64_decode("te6ccgEBAwEABgAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_8() {
    // 6GB allocation
    let bb = base64_decode("te6ccnQG+1d+m1sBAAEBAVsAAAaIm/YAGG4Anp6enhhuAJ6enp4A/yWGmwEBAQAABoib9gAYbgCenp76AKoBMAQlKv8=").unwrap();
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_9() {
    let bb = hex::decode("b5ee9c72010101000002080065").unwrap();
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_10() {
    let bb = base64_decode(
        "aP9l8wEBAgEAKQP/QQABSEgBAgAo//8A/wAo//8AAAAAAwAAAAMXeuR65P//////AP////8AAAADAAAAA+Q=",
    )
    .unwrap();
    test_bad_boc(bb, true);
}

#[test]
fn test_bad_boc_11() {
    // std::env::set_var("RUST_BACKTRACE", "full");

    let bb = base64_decode("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").unwrap();
    test_bad_boc(bb, false);
}

#[test]
fn test_chunked_vec() {
    use super::ChunkedVec;

    let mut chunked_vec = ChunkedVec::<u32, 3>::new();
    assert_eq!(chunked_vec.len(), 0);
    chunked_vec.push(1);
    assert_eq!(chunked_vec.len(), 1);
    chunked_vec.push(2);
    assert_eq!(chunked_vec.len(), 2);
    chunked_vec.push(3);
    assert_eq!(chunked_vec.len(), 3);
    chunked_vec.push(4);
    assert_eq!(chunked_vec.len(), 4);
    assert_eq!(chunked_vec.get(0), Some(&1));
    assert_eq!(chunked_vec[0], 1);
    assert_eq!(chunked_vec.get(1), Some(&2));
    assert_eq!(chunked_vec[1], 2);
    assert_eq!(chunked_vec.get(2), Some(&3));
    assert_eq!(chunked_vec[2], 3);
    assert_eq!(chunked_vec.get(3), Some(&4));
    assert_eq!(chunked_vec[3], 4);
    assert_eq!(chunked_vec.get(4), None);
    assert_eq!(chunked_vec.get(9), None);
}

#[ignore]
#[test]
fn test_bench_boc_write() -> Result<()> {
    let filename = "EE80F14E960B421D59A8DE8B3399699A2B20AA5EDA127C7B376DE2A16685D47D.boc";
    let data = std::fs::read(filename)?;
    let mut hash = [0; 32];

    let t = Instant::now();
    for _ in 0..1000 {
        // hash = crate::sha256_digest(&data);
        // let hash = sha256::digest(&data);
        hash = openssl::sha::sha256(&data);
    }
    let elapsed = t.elapsed();
    println!("HASH {:#?}", elapsed);
    println!("{}", hex::encode(&hash));

    let mut write_time = Duration::from_secs(0);
    let mut read_time = Duration::from_secs(0);
    let mut read_time_2 = Duration::from_secs(0);
    let mut total_bytes = 0;
    let mut header = Default::default();
    let n = 100;
    for _ in 0..n {
        let t = Instant::now();
        let read_result = BocReader::new().stream_read(&mut Cursor::new(&data))?;
        read_time += t.elapsed();
        header = read_result.header.clone();
        let root = read_result.withdraw_single_root()?;

        let t = Instant::now();
        let read_result = BocReader::new().read(&data)?;
        read_time_2 += t.elapsed();
        assert_eq!(root, read_result.withdraw_single_root()?);

        let t = Instant::now();
        let out_data = BocWriter::with_params([root], MAX_SAFE_DEPTH, BocFlags::all(), &|| false)?
            .write_to_vec()?;
        // drop(root);
        let elapsed = t.elapsed();
        // println!(
        //     "BOC write time: {:#?}, {} bytes/sec",
        //     elapsed,
        //     out_data.len() as f64 / elapsed.as_secs_f64()
        // );
        write_time += elapsed;
        total_bytes += out_data.len();
    }
    let avg_time = write_time / n;
    let avg_speed = total_bytes as f64 / write_time.as_secs_f64();
    println!("Average BOC write time: {:#?}, {} bytes/sec", avg_time, avg_speed);
    println!("Average BOC read time: {:#?}", read_time / n);
    println!("Average BOC read inmem time: {:#?}", read_time_2 / n);

    println!("{:?}", header);

    Ok(())
}
