/*
 * Copyright (C) 2019-2022 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;

#[test]
fn test_format_cell() {
    let mut root = BuilderData::new();
    let mut c1 = BuilderData::new();
    let mut c2 = BuilderData::new();
    let mut c3 = BuilderData::new();
    let mut c4 = BuilderData::new();

    root.append_u32(0xfff0).unwrap();
    c1.append_u32(0xfff1).unwrap();
    c2.append_u32(0xfff2).unwrap();
    c3.append_u32(0xfff3).unwrap();
    c4.append_u32(0xfff4).unwrap();

    c1.append_reference(c2);
    c1.append_reference(c3);
    root.append_reference(c1);
    root.append_reference(c4);

    let cell = root.into_cell().unwrap();

    assert_eq!(format!("{}", cell), "bits: 32   refs: 2   data: 0000fff0");

    assert_eq!(
        format!("{:#}", cell),
        r#"Ordinary   l: 000   bits: 32   refs: 2   data: 0000fff0
hashes: 2284543e5a1301a2f7035b17e5381479d56296760e745f284cc0ef6474c090ab
depths: 2"#
    );

    assert_eq!(
        format!("{:.1}", cell),
        r#"bits: 32   refs: 2   data: 0000fff0
 ├─bits: 32   refs: 2   data: 0000fff1
 └─bits: 32   refs: 0   data: 0000fff4"#
    );

    assert_eq!(
        format!("{:.2}", cell),
        r#"bits: 32   refs: 2   data: 0000fff0
 ├─bits: 32   refs: 2   data: 0000fff1
 │ ├─bits: 32   refs: 0   data: 0000fff2
 │ └─bits: 32   refs: 0   data: 0000fff3
 └─bits: 32   refs: 0   data: 0000fff4"#
    );

    assert_eq!(
        format!("{:#.1}", cell),
        r#"Ordinary   l: 000   bits: 32   refs: 2   data: 0000fff0
hashes: 2284543e5a1301a2f7035b17e5381479d56296760e745f284cc0ef6474c090ab
depths: 2
 ├─Ordinary   l: 000   bits: 32   refs: 2   data: 0000fff1
 │ hashes: 013a48b1f5da4a3ed97975185ef221aa031fb14c189d791db859a003bbde9c6a
 │ depths: 1
 └─Ordinary   l: 000   bits: 32   refs: 0   data: 0000fff4
   hashes: 8d7e5e5b7b0533c2bc2e18991c561d2f2a0a30225af61bb42ff55cd2fec2d3cc
   depths: 0"#
    );

    assert_eq!(
        format!("{:#.2}", cell),
        r#"Ordinary   l: 000   bits: 32   refs: 2   data: 0000fff0
hashes: 2284543e5a1301a2f7035b17e5381479d56296760e745f284cc0ef6474c090ab
depths: 2
 ├─Ordinary   l: 000   bits: 32   refs: 2   data: 0000fff1
 │ hashes: 013a48b1f5da4a3ed97975185ef221aa031fb14c189d791db859a003bbde9c6a
 │ depths: 1
 │ ├─Ordinary   l: 000   bits: 32   refs: 0   data: 0000fff2
 │ │ hashes: 89497332e5fc3e6a9256db6eca374b4c322b1f7e32da90d17cf5bcc78044dc67
 │ │ depths: 0
 │ └─Ordinary   l: 000   bits: 32   refs: 0   data: 0000fff3
 │   hashes: 138a0c065fa8178a3cb164a093bd0f9f2cdbfb824b5aae88fccd37e789d63da1
 │   depths: 0
 └─Ordinary   l: 000   bits: 32   refs: 0   data: 0000fff4
   hashes: 8d7e5e5b7b0533c2bc2e18991c561d2f2a0a30225af61bb42ff55cd2fec2d3cc
   depths: 0"#
    );

    let slice = SliceData::new(vec![0x0, 0x1, 0x80]);
    assert_eq!(format!("{:b}", slice.into_cell().unwrap()), "0000000000000001");

    let slice = SliceData::new(vec![0x0, 0x0, 0x80]);
    assert_eq!(format!("{:b}", slice.into_cell().unwrap()), "0000000000000000");

    let slice = SliceData::new(vec![0x0, 0x1, 0x20]);
    assert_eq!(format!("{:b}", slice.into_cell().unwrap()), "000000000000000100");

    let slice = SliceData::new(vec![0x0, 0x1, 0x02]);
    assert_eq!(format!("{:b}", slice.into_cell().unwrap()), "0000000000000001000000");

    let slice = SliceData::new(vec![0xff, 0x00, 0xfc]);
    assert_eq!(format!("{:b}", slice.into_cell().unwrap()), "111111110000000011111");

    let slice = SliceData::new(vec![0b10010111, 0b10001010, 0b10000010]);
    assert_eq!(format!("{:b}", slice.into_cell().unwrap()), "1001011110001010100000");
}

#[test]
fn test_format_slice() {
    let slice = SliceData::new(vec![0x25, 0x67]);
    assert_eq!(format!("{:x}", slice), r#"2567_"#);
    let slice = SliceData::new(vec![0x25, 0x68]);
    assert_eq!(format!("{:x}", slice), r#"256"#);
    let slice = SliceData::new(vec![0x25, 0x68, 0x80]);
    assert_eq!(format!("{:x}", slice), r#"2568"#);
    let slice = SliceData::new(vec![0x25, 0x68, 0x80]);
    assert_eq!(format!("{}", slice.into_cell().unwrap()), r#"bits: 16   refs: 0   data: 2568"#);
}

#[test]
fn test_compare_cells() {
    let builder1 = BuilderData::with_raw(vec![0xF0], 4).unwrap();
    let builder2 = BuilderData::with_bitstring(vec![0xF8]).unwrap();
    let builder3 = BuilderData::with_bitstring(vec![0xFF, 0x80]).unwrap();

    let cell1 = builder1.into_cell().unwrap();
    let cell2 = builder2.into_cell().unwrap();
    let cell3 = builder3.into_cell().unwrap();

    assert_eq!(cell1, cell2);
    assert_ne!(cell1, cell3);
    assert_ne!(cell2, cell3);
    assert_eq!(cell3.clone(), cell3);
}

#[test]
fn test_compare_slice_cells() {
    let cell1 = SliceData::new(vec![0x78]).into_cell().unwrap();
    let mut slice = SliceData::new(vec![0, 0x78]);
    slice.move_by(8).unwrap();
    let cell2 = slice.into_cell().unwrap();
    assert_eq!(
        SliceData::load_cell(cell1.clone()).unwrap(),
        SliceData::load_cell(cell2.clone()).unwrap()
    );
    assert_eq!(cell1, cell2);
}

#[test]
fn test_usage_cell() {
    /*
                      1
             2                  3
        4          5         6     7
        8        9   10
                     11
    */

    let c11 = create_cell(&[], &[11, 0x80]).unwrap();
    let c10 = create_cell(&[c11.clone()], &[10, 0x80]).unwrap();
    let c9 = create_cell(&[], &[9, 0x80]).unwrap();
    let c8 = create_cell(&[], &[8, 0x80]).unwrap();
    let c7 = create_cell(&[], &[7, 0x80]).unwrap();
    let c6 = create_cell(&[], &[6, 0x80]).unwrap();
    let c5 = create_cell(&[c9.clone(), c10.clone()], &[5, 0x80]).unwrap();
    let c4 = create_cell(&[c8.clone()], &[4, 0x80]).unwrap();
    let c3 = create_cell(&[c6.clone(), c7.clone()], &[3, 0x80]).unwrap();
    let c2 = create_cell(&[c4.clone(), c5.clone()], &[2, 0x80]).unwrap();
    let c1 = create_cell(&[c2.clone(), c3.clone()], &[1, 0x80]).unwrap();

    let ut = UsageTree::with_root(c1.clone());
    let mut c1_slice = SliceData::load_cell(ut.root_cell()).unwrap();

    let mut c2_slice = SliceData::load_cell(c1_slice.checked_drain_reference().unwrap()).unwrap();
    let _c3_slice = SliceData::load_cell(c1_slice.checked_drain_reference().unwrap()).unwrap();
    let mut c4_slice = SliceData::load_cell(c2_slice.checked_drain_reference().unwrap()).unwrap();
    let mut c5_slice = SliceData::load_cell(c2_slice.checked_drain_reference().unwrap()).unwrap();
    let _c9_slice = SliceData::load_cell(c5_slice.checked_drain_reference().unwrap()).unwrap();
    let mut c10_slice = SliceData::load_cell(c5_slice.checked_drain_reference().unwrap()).unwrap();
    let mut c11_slice = SliceData::load_cell(c10_slice.checked_drain_reference().unwrap()).unwrap();

    assert_eq!(11, c11_slice.get_next_byte().unwrap());

    for c in [&c1, &c2, &c5, &c10, &c11] {
        assert!(ut.contains(&c.hash(0)));
    }
    for c in [&c3, &c4, &c6, &c7, &c8, &c9] {
        assert!(!ut.contains(&c.hash(0)));
    }

    let mut c8_slice = SliceData::load_cell(c4_slice.checked_drain_reference().unwrap()).unwrap();
    assert_eq!(8, c8_slice.get_next_byte().unwrap());

    // create usage subtree with root in c4
    let subvisited = ut.build_visited_subtree(&|h| h == c4.hash(0)).unwrap();

    assert_eq!(subvisited.len(), 2);
    assert!(subvisited.contains(&c4.hash(0)));
    assert!(subvisited.contains(&c8.hash(0)));

    // create usage subtree with root in c5
    let subvisited = ut.build_visited_subtree(&|h| h == c5.hash(0)).unwrap();

    assert_eq!(subvisited.len(), 3);
    assert!(subvisited.contains(&c5.hash(0)));
    assert!(subvisited.contains(&c10.hash(0)));
    assert!(subvisited.contains(&c11.hash(0)));
}

#[test]
fn test_default_cell() {
    let default_hash = "96a296d224f285c67bee93c30f8a309157f0daa35dc5b87e410b78630a09cfc7";
    let default_hash: UInt256 = default_hash.parse().unwrap();
    assert_eq!(
        default_hash.as_slice(),
        crate::base64_decode("lqKW0iTyhcZ77pPDD4owkVfw2qNdxbh+QQt4YwoJz8c=").unwrap().as_slice()
    );
    let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::Ordinary, 0);
    let d2 = calc_d2(0);

    assert_eq!(UInt256::calc_file_hash(&[d1, d2]), default_hash);

    let cell = BuilderData::default().into_cell().unwrap();
    assert_eq!(*cell.repr_hash(), default_hash);

    assert_eq!(BuilderData::default(), BuilderData::from_cell(&Cell::default()).unwrap());
}

#[test]
fn test_div_ceil() {
    assert_eq!(0usize.div_ceil(8), 0);
    for i in 1..8usize {
        assert_eq!(i.div_ceil(8), 1);
        assert_eq!((8 + i).div_ceil(8), 2);
        assert_eq!((16 + i).div_ceil(8), 3);
    }
}

// ===== Cell::check_data tests =====

/// Build a valid ordinary cell raw buffer: [d1][d2][data_bytes]
/// tag byte for a non-byte-aligned `bits` count: the lowest set bit in the last data byte.
/// find_tag scans from LSB; for `bits % 8 == r` remaining bits, the tag is at position
/// `(8 - r - 1)` from LSB → `1u8 << (7 - r)`. For byte-aligned data, append 0x80 separately.
fn completion_tag(bits: usize) -> u8 {
    let r = bits % 8;
    if r == 0 {
        0x80
    } else {
        1u8 << (7 - r)
    }
}

fn make_ordinary(level_mask: u8, refs: usize, bits: usize) -> Vec<u8> {
    let lm = LevelMask::with_mask(level_mask);
    let d1 = calc_d1(lm, false, CellType::Ordinary, refs);
    let d2 = calc_d2(bits);
    let data_bytes = (bits / 8) + if bits % 8 != 0 { 1 } else { 0 };
    let mut buf = vec![d1, d2];
    if data_bytes > 0 {
        let mut data = vec![0u8; data_bytes];
        data[data_bytes - 1] = completion_tag(bits);
        buf.extend_from_slice(&data);
    }
    buf
}

fn pruned_branch_data(level_mask: u8) -> Vec<u8> {
    let lvl = LevelMask::with_mask(level_mask).level() as usize;
    let mut data = vec![0u8; 1 + 1 + lvl * (SHA256_SIZE + DEPTH_SIZE)];
    data[0] = u8::from(CellType::PrunedBranch); // wire value = 1
    data[1] = level_mask;
    // depths are 0, hashes are 0 — valid
    data
}

fn library_reference_data() -> Vec<u8> {
    let mut data = vec![0u8; 1 + SHA256_SIZE];
    data[0] = u8::from(CellType::LibraryReference); // wire value = 2
    data
}

fn merkle_proof_data() -> Vec<u8> {
    let mut data = vec![0u8; 1 + SHA256_SIZE + DEPTH_SIZE];
    data[0] = u8::from(CellType::MerkleProof); // wire value = 3
    data
}

fn merkle_update_data() -> Vec<u8> {
    let mut data = vec![0u8; 1 + 2 * (SHA256_SIZE + DEPTH_SIZE)];
    data[0] = u8::from(CellType::MerkleUpdate); // wire value = 4
    data
}

fn check_ok<'a>(raw: &'a [u8]) -> CellRawInfo<'a> {
    Cell::check_data(raw, false)
        .unwrap_or_else(|e| panic!("expected Ok for {:?}, got Err: {}", raw, e))
}

fn check_err(raw: &[u8]) {
    assert!(Cell::check_data(raw, false).is_err(), "expected Err for {:?}", raw);
}

// ----- valid cases -----

#[test]
fn test_check_data_empty_ordinary() {
    let raw = make_ordinary(0, 0, 0);
    let info = check_ok(&raw);
    assert_eq!(info.d1, raw[0]);
    assert_eq!(info.d2, 0);
    assert_eq!(info.data, &[] as &[u8]);
    assert_eq!(info.bit_len, 0);
}

#[test]
fn test_check_data_ordinary_8_bits() {
    let raw =
        vec![calc_d1(LevelMask::with_mask(0), false, CellType::Ordinary, 0), calc_d2(8), 0xAB];
    let info = check_ok(&raw);
    assert_eq!(info.bit_len, 8);
    assert_eq!(info.data, &[0xAB]);
}

#[test]
fn test_check_data_ordinary_non_byte_aligned() {
    // 5 bits: tag = 1 << (7-5) = 1 << 2 = 0x04; find_tag returns 8-3=5
    let raw = vec![
        calc_d1(LevelMask::with_mask(0), false, CellType::Ordinary, 0),
        calc_d2(5),
        completion_tag(5), // 0x04
    ];
    let info = check_ok(&raw);
    assert_eq!(info.bit_len, 5);
}

#[test]
fn test_check_data_ordinary_with_refs() {
    for rc in 1..=4 {
        let raw = make_ordinary(0, rc, 0);
        let info = check_ok(&raw);
        assert_eq!(refs_count(&[info.d1]), rc);
    }
}

#[test]
fn test_check_data_ordinary_with_level() {
    for lm in [1u8, 2, 3, 4, 5, 6, 7] {
        let raw = make_ordinary(lm, 0, 0);
        check_ok(&raw);
    }
}

#[test]
fn test_check_data_ordinary_max_data() {
    // 1023 bits → 128 bytes; 1023 % 8 = 7 → tag = 1 << (7-7) = 0x01
    let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::Ordinary, 0);
    let d2 = calc_d2(1023);
    let mut data = vec![0u8; 128];
    data[127] = completion_tag(1023); // 0x01
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    let info = check_ok(&raw);
    assert_eq!(info.bit_len, 1023);
}

#[test]
fn test_check_data_unbounded_allowed() {
    let mut raw = make_ordinary(0, 0, 0);
    raw.push(0xFF); // extra byte
                    // unbounded=false must fail
    assert!(Cell::check_data(&raw, false).is_err());
    // unbounded=true must succeed
    assert!(Cell::check_data(&raw, true).is_ok());
}

#[test]
fn test_check_data_store_hashes() {
    // ordinary, level 1, store_hashes=true → 2 hashes, 2 depths before data
    let lm = LevelMask::with_mask(1);
    let hc = lm.level() as usize + 1; // 2
    let d1 = calc_d1(lm, true, CellType::Ordinary, 0);
    let d2 = calc_d2(0);
    let mut raw = vec![d1, d2];
    raw.extend(std::iter::repeat(0u8).take(hc * (SHA256_SIZE + DEPTH_SIZE)));
    let info = check_ok(&raw);
    assert_eq!(info.bit_len, 0);
}

#[test]
fn test_check_data_pruned_branch() {
    for lm in [1u8, 2, 3, 5, 7] {
        let data = pruned_branch_data(lm);
        let bits = data.len() * 8;
        let lm_mask = LevelMask::with_mask(lm);
        let d1 = calc_d1(lm_mask, false, CellType::PrunedBranch, 0);
        let d2 = calc_d2(bits);
        let mut raw = vec![d1, d2];
        raw.extend_from_slice(&data);
        check_ok(&raw);
    }
}

#[test]
fn test_check_data_library_reference() {
    let data = library_reference_data();
    let bits = data.len() * 8;
    let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::LibraryReference, 0);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_ok(&raw);
}

#[test]
fn test_check_data_merkle_proof() {
    let data = merkle_proof_data();
    let bits = data.len() * 8;
    let d1 = calc_d1(LevelMask::with_mask(1), false, CellType::MerkleProof, 1);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_ok(&raw);
}

#[test]
fn test_check_data_merkle_update() {
    let data = merkle_update_data();
    let bits = data.len() * 8;
    let d1 = calc_d1(LevelMask::with_mask(3), false, CellType::MerkleUpdate, 2);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_ok(&raw);
}

// ----- invalid cases -----

#[test]
fn test_check_data_too_short() {
    check_err(&[]);
    check_err(&[0]);
}

#[test]
fn test_check_data_invalid_level_mask() {
    // level_mask bits are the top 3 bits of d1 — values 0-7 are all valid per LevelMask
    // but exotic bit combinations can still be invalid: test refs_count > 4
    let d1 = 5u8; // refs_count = 5
    let raw = vec![d1, 0x00];
    check_err(&raw);
}

#[test]
fn test_check_data_refs_count_too_many() {
    let d1 = 5u8; // 0b00000101 → refs=5, no exotic, level_mask=0
    check_err(&[d1, 0]);
}

#[test]
fn test_check_data_len_mismatch() {
    // d2=0x02 → 1 byte of data expected; provide 2
    let raw = vec![0x00u8, 0x02, 0xAB, 0xCD];
    check_err(&raw);
}

#[test]
fn test_check_data_truncated() {
    // d2=0x02 → 1 byte of data required; provide only header
    let raw = vec![0x00u8, 0x02];
    check_err(&raw);
}

#[test]
fn test_check_data_completion_tag_all_zero() {
    // non-byte-aligned (d2 odd) but data all zeros → invalid completion tag
    let d2 = calc_d2(5); // odd
    let raw = vec![0x00u8, d2, 0x00]; // completion tag = 0 is invalid
    check_err(&raw);
}

#[test]
fn test_check_data_ordinary_bit_len_too_large() {
    // MAX_DATA_BITS = 1023; build a 1024-bit cell (128 bytes + odd tag = 129 bytes)
    let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::Ordinary, 0);
    let d2 = calc_d2(1024); // 1024 bits → d2 = (128<<1) = 0x80
    let mut data = vec![0u8; 129];
    data[128] = 0x80; // completion tag
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw);
}

#[test]
fn test_check_data_exotic_empty_data() {
    // exotic flag set, but d2=0 → data is empty
    let d1 = EXOTIC_D1_FLAG; // exotic, level=0, refs=0
    let raw = vec![d1, 0x00];
    check_err(&raw);
}

#[test]
fn test_check_data_unknown_exotic_type() {
    let d1 = EXOTIC_D1_FLAG;
    let d2 = calc_d2(8);
    let raw = vec![d1, d2, 0x05]; // 0x05 is not a valid exotic type (valid: 1,2,3,4,0xff)
    check_err(&raw);
}

#[test]
fn test_check_data_pruned_nonzero_refs() {
    let lm = 1u8;
    let data = pruned_branch_data(lm);
    let bits = data.len() * 8;
    // refs=1 is invalid for pruned branch
    let d1 = calc_d1(LevelMask::with_mask(lm), false, CellType::PrunedBranch, 1);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw);
}

#[test]
fn test_check_data_pruned_zero_level() {
    // pruned branch with level_mask=0 is invalid (level must be non-zero)
    // but with level=0 and refs=0, hashes_count=1 (pruned detection uses level!=0)
    // Actually: exotic + refs=0 + level=0 means hashes_count=1 per standard, and
    // the pruned branch check triggers on exotic + refs_count=0 + level_mask.level()!=0
    // A pruned branch with level=0 won't be detected as pruned — it'll fall through
    // as "unknown exotic type" since data[0] = PrunedBranch but lvl==0 fails the lvl>0 check
    let data = vec![CellType::PrunedBranch as u8, 0x00, 0x80]; // tag + level_mask=0 + padding
    let bits = data.len() * 8;
    let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::PrunedBranch, 0);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw); // fails: pruned with level=0
}

#[test]
fn test_check_data_pruned_mask_mismatch() {
    let lm = 3u8;
    let mut data = pruned_branch_data(lm);
    data[1] = 5u8; // data[1] != level_mask
    let bits = data.len() * 8;
    let d1 = calc_d1(LevelMask::with_mask(lm), false, CellType::PrunedBranch, 0);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw);
}

#[test]
fn test_check_data_pruned_wrong_bit_len() {
    let lm = 1u8;
    let mut data = pruned_branch_data(lm);
    data.push(0xFF); // extra byte makes bit_len wrong
    let bits = data.len() * 8;
    let d1 = calc_d1(LevelMask::with_mask(lm), false, CellType::PrunedBranch, 0);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw);
}

#[test]
fn test_check_data_merkle_proof_wrong_refs() {
    let data = merkle_proof_data();
    let bits = data.len() * 8;
    // refs=2 is invalid (must be 1)
    let d1 = calc_d1(LevelMask::with_mask(1), false, CellType::MerkleProof, 2);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw);
}

#[test]
fn test_check_data_merkle_update_wrong_refs() {
    let data = merkle_update_data();
    let bits = data.len() * 8;
    // refs=1 instead of 2
    let d1 = calc_d1(LevelMask::with_mask(3), false, CellType::MerkleUpdate, 1);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw);
}

#[test]
fn test_check_data_library_reference_has_refs() {
    let data = library_reference_data();
    let bits = data.len() * 8;
    let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::LibraryReference, 1);
    let d2 = calc_d2(bits);
    let mut raw = vec![d1, d2];
    raw.extend_from_slice(&data);
    check_err(&raw);
}

#[test]
fn test_check_data_return_values() {
    // Verify CellRawInfo fields for a known cell
    let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::Ordinary, 2);
    let d2 = calc_d2(16);
    let raw = vec![d1, d2, 0xDE, 0xAD];
    let info = check_ok(&raw);
    assert_eq!(info.d1, d1);
    assert_eq!(info.d2, d2);
    assert_eq!(info.data, &[0xDE, 0xAD]);
    assert_eq!(info.bit_len, 16);
}

// ----- fuzz-style tests -----

/// Ensure check_data never panics on arbitrary 2-byte inputs.
#[test]
fn test_check_data_fuzz_two_bytes() {
    for d1 in 0u8..=255 {
        for d2 in 0u8..=255 {
            let _ = Cell::check_data(&[d1, d2], false);
            let _ = Cell::check_data(&[d1, d2], true);
        }
    }
}

/// Ensure check_data never panics on arbitrary 3-byte inputs.
#[test]
fn test_check_data_fuzz_three_bytes() {
    // All 3-byte combinations are too many (16M); sample systematically.
    for d1 in 0u8..=255 {
        for d2 in 0u8..=255 {
            for data in [0x00u8, 0x01, 0x80, 0xFF, 0x08, 0x55, 0xAA] {
                let _ = Cell::check_data(&[d1, d2, data], false);
                let _ = Cell::check_data(&[d1, d2, data], true);
            }
        }
    }
}

/// Bit-flip each byte of a valid cell and ensure no panics.
#[test]
fn test_check_data_fuzz_bit_flip() {
    let valid = {
        let d1 = calc_d1(LevelMask::with_mask(0), false, CellType::Ordinary, 0);
        vec![d1, calc_d2(8), 0xAB]
    };
    for byte_idx in 0..valid.len() {
        for bit in 0..8u8 {
            let mut mutated = valid.clone();
            mutated[byte_idx] ^= 1 << bit;
            let _ = Cell::check_data(&mutated, false);
            let _ = Cell::check_data(&mutated, true);
        }
    }
}

/// Truncate valid buffers of various sizes and ensure no panics.
#[test]
fn test_check_data_fuzz_truncation() {
    let valids: &[Vec<u8>] =
        &[make_ordinary(0, 0, 0), make_ordinary(0, 0, 8), make_ordinary(1, 0, 0), {
            let data = merkle_proof_data();
            let mut raw = vec![
                calc_d1(LevelMask::with_mask(1), false, CellType::MerkleProof, 1),
                calc_d2(data.len() * 8),
            ];
            raw.extend_from_slice(&data);
            raw
        }];
    for valid in valids {
        for len in 0..=valid.len() {
            let _ = Cell::check_data(&valid[..len], false);
            let _ = Cell::check_data(&valid[..len], true);
        }
    }
}

/// Random-ish inputs via LCG: ensure check_data is panic-free on arbitrary byte sequences.
#[test]
fn test_check_data_fuzz_pseudo_random() {
    let mut state = 0xDEAD_BEEFu64;
    let mut next = || -> u8 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };

    for _ in 0..100_000 {
        let len = next() as usize % 300;
        let buf: Vec<u8> = (0..len).map(|_| next()).collect();
        let _ = Cell::check_data(&buf, false);
        let _ = Cell::check_data(&buf, true);
    }
}

// DataCell tests

#[test]
fn test_data_cell_empty() {
    let cell = Cell::default();
    assert_eq!(cell.cell_type(), CellType::Ordinary);
    assert_eq!(cell.level(), 0);
    assert_eq!(cell.level_mask().mask(), 0);
    assert_eq!(cell.bit_length(), 0);
    assert_eq!(cell.references_count(), 0);
    assert_eq!(cell.data(), &[] as &[u8]);
    assert_eq!(cell.repr_depth(), 0);
    assert!(cell.store_hashes());
}

#[test]
fn test_data_cell_with_data() {
    let mut b = BuilderData::new();
    b.append_u64(0xDEAD_BEEF_CAFE_BABE).unwrap();
    let cell = b.into_cell().unwrap();

    assert_eq!(cell.cell_type(), CellType::Ordinary);
    assert_eq!(cell.level(), 0);
    assert_eq!(cell.bit_length(), 64);
    assert_eq!(cell.references_count(), 0);
    assert_eq!(cell.data(), &[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE]);
    assert_eq!(cell.repr_depth(), 0);
    assert_eq!(cell.hashes().len(), 1);
    assert_eq!(cell.depths().len(), 1);
}

#[test]
fn test_data_cell_with_refs() {
    let c1 = create_cell(&[], &[0xAA, 0x80]).unwrap();
    let c2 = create_cell(&[], &[0xBB, 0x80]).unwrap();
    let c3 = create_cell(&[], &[0xCC, 0x80]).unwrap();
    let parent = create_cell(&[c1.clone(), c2.clone(), c3.clone()], &[0xFF, 0x80]).unwrap();

    assert_eq!(parent.references_count(), 3);
    assert_eq!(parent.repr_depth(), 1);
    assert_eq!(*parent.reference(0).unwrap().repr_hash(), *c1.repr_hash());
    assert_eq!(*parent.reference(1).unwrap().repr_hash(), *c2.repr_hash());
    assert_eq!(*parent.reference(2).unwrap().repr_hash(), *c3.repr_hash());
    parent.reference(3).expect_err("out of bounds");
}

#[test]
fn test_data_cell_deep_tree() {
    let mut cell = create_cell(&[], &[0x80]).unwrap();
    for i in 1..=10u8 {
        cell = create_cell(&[cell], &[i, 0x80]).unwrap();
    }
    assert_eq!(cell.repr_depth(), 10);
    assert_eq!(cell.level(), 0);
    assert_eq!(cell.hashes().len(), 1);
}

#[test]
fn test_data_cell_4_refs() {
    let refs: Vec<Cell> = (0..4).map(|i| create_cell(&[], &[i, 0x80]).unwrap()).collect();
    let parent = create_cell(&refs, &[0xFF, 0x80]).unwrap();
    assert_eq!(parent.references_count(), 4);
    for i in 0..4 {
        assert_eq!(*parent.reference(i).unwrap().repr_hash(), *refs[i].repr_hash());
    }
}

#[test]
fn test_data_cell_non_byte_aligned() {
    // 5-bit data
    let mut b = BuilderData::new();
    b.append_raw(&[0b11010_000], 5).unwrap();
    let cell = b.into_cell().unwrap();
    assert_eq!(cell.bit_length(), 5);
    assert_eq!(cell.data(), &[0b11010_100]); // with completion tag
}

#[test]
fn test_data_cell_max_data() {
    let mut b = BuilderData::new();
    for _ in 0..127 {
        b.append_u8(0xAB).unwrap();
    }
    b.append_raw(&[0b1111111_0], 7).unwrap(); // 127*8 + 7 = 1023 bits
    let cell = b.into_cell().unwrap();
    assert_eq!(cell.bit_length(), 1023);
    assert_eq!(cell.data().len(), 128);
}

#[test]
fn test_data_cell_clone_eq() {
    let cell = create_cell(&[], &[0x42, 0x80]).unwrap();
    let cloned = cell.clone();
    assert_eq!(cell, cloned);
    assert_eq!(*cell.repr_hash(), *cloned.repr_hash());
    assert_eq!(cell.repr_depth(), cloned.repr_depth());
}

// Allocation mode tests

fn create_cell_in(
    references: &[Cell],
    data: &[u8],
    arena: Option<Arc<CellsArena>>,
) -> Result<Cell> {
    Cell::with_data_and_refs(
        &Cell::build_data(data, CellType::Ordinary, 0, references.len(), None)?,
        false,
        references,
        None,
        arena,
    )
}

#[test]
fn test_alloc_heap_basic() {
    let cell = create_cell_in(&[], &[0xAB, 0x80], None).unwrap();
    assert!(cell.is_heap());
    assert_eq!(cell.cell_type(), CellType::Ordinary);
    assert_eq!(cell.bit_length(), 8);
    assert_eq!(cell.data()[0], 0xAB);
    assert_eq!(cell.references_count(), 0);
}

#[test]
fn test_alloc_arena_basic() {
    // Arena cells don't hold a reference to the arena,
    // so we must keep it alive for the cell's lifetime.
    let arena = Arc::new(CellsArena::new(4096, 65536));
    let cell = create_cell_in(&[], &[0xAB, 0x80], Some(arena.clone())).unwrap();
    assert!(!cell.is_heap());
    assert_eq!(cell.tag() & !CELL_TYPE_BIT, CELL_ARENA);
    assert_eq!(cell.cell_type(), CellType::Ordinary);
    assert_eq!(cell.bit_length(), 8);
    assert_eq!(cell.data()[0], 0xAB);
    assert_eq!(cell.references_count(), 0);
    drop(cell);
    drop(arena);
}

#[test]
fn test_alloc_modes_produce_same_hashes() {
    let child_heap = create_cell(&[], &[0x01, 0x80]).unwrap();

    let arena = Arc::new(CellsArena::new(4096, 65536));
    let child_arena = create_cell_in(&[], &[0x01, 0x80], Some(arena.clone())).unwrap();

    assert_eq!(*child_heap.repr_hash(), *child_arena.repr_hash());
    assert_eq!(child_heap.repr_depth(), child_arena.repr_depth());
}

#[test]
fn test_alloc_modes_with_references() {
    let c1_heap = create_cell(&[], &[0x01, 0x80]).unwrap();
    let c2_heap = create_cell(&[], &[0x02, 0x80]).unwrap();

    let parent_heap =
        create_cell_in(&[c1_heap.clone(), c2_heap.clone()], &[0xFF, 0x80], None).unwrap();

    let arena = Arc::new(CellsArena::new(4096, 65536));
    let c1_arena = create_cell_in(&[], &[0x01, 0x80], Some(arena.clone())).unwrap();
    let c2_arena = create_cell_in(&[], &[0x02, 0x80], Some(arena.clone())).unwrap();
    let parent_arena =
        create_cell_in(&[c1_arena, c2_arena], &[0xFF, 0x80], Some(arena.clone())).unwrap();

    assert_eq!(*parent_heap.repr_hash(), *parent_arena.repr_hash());
    assert_eq!(parent_heap.references_count(), 2);
    assert_eq!(parent_arena.references_count(), 2);

    for i in 0..2 {
        assert_eq!(
            *parent_heap.reference(i).unwrap().repr_hash(),
            *parent_arena.reference(i).unwrap().repr_hash()
        );
    }
}

#[test]
fn test_alloc_heap_clone_shares_ptr() {
    // Heap cells are refcounted — clone shares the same allocation
    let cell = create_cell_in(&[], &[0x42, 0x80], None).unwrap();
    let cloned = cell.clone();
    assert!(cell.is_heap());
    assert!(cloned.is_heap());
    assert_eq!(*cell.repr_hash(), *cloned.repr_hash());
    assert_eq!(cell.raw_ptr(), cloned.raw_ptr());
}

#[test]
fn test_alloc_arena_multiple_cells() {
    let arena = Arc::new(CellsArena::new(4096, 65536));
    let mut cells = Vec::new();
    for i in 0u8..20 {
        let cell = create_cell_in(&[], &[i, 0x80], Some(arena.clone())).unwrap();
        assert_eq!(cell.tag() & !CELL_TYPE_BIT, CELL_ARENA);
        cells.push(cell);
    }
    for i in 0..cells.len() {
        for j in (i + 1)..cells.len() {
            assert_ne!(*cells[i].repr_hash(), *cells[j].repr_hash());
        }
    }
}

#[test]
fn test_alloc_arena_chunk_overflow() {
    // Minimal chunk size forces multiple chunks to be allocated
    let arena = Arc::new(CellsArena::new(CellsArena::MIN_CHUNK_SIZE, 65536));
    let mut cells = Vec::new();
    for i in 0u8..50 {
        let cell = create_cell_in(&[], &[i, 0x80], Some(arena.clone())).unwrap();
        cells.push(cell);
    }
    for (i, cell) in cells.iter().enumerate() {
        assert_eq!(cell.data()[0], i as u8);
    }
}

#[test]
fn test_alloc_modes_tree_hash_equivalence() {
    // Build a tree and verify root hashes match across allocation modes.
    let build_tree = |arena: Option<Arc<CellsArena>>| -> Cell {
        let leaf1 = create_cell_in(&[], &[0x01, 0x80], arena.clone()).unwrap();
        let leaf2 = create_cell_in(&[], &[0x02, 0x80], arena.clone()).unwrap();
        let leaf3 = create_cell_in(&[], &[0x03, 0x80], arena.clone()).unwrap();
        let mid = create_cell_in(&[leaf1, leaf2], &[0x10, 0x80], arena.clone()).unwrap();
        create_cell_in(&[mid, leaf3], &[0x20, 0x80], arena).unwrap()
    };

    let heap_root = build_tree(None);

    let arena = Arc::new(CellsArena::new(4096, 65536));
    let arena_root = build_tree(Some(arena.clone()));

    assert_eq!(*heap_root.repr_hash(), *arena_root.repr_hash());
    assert_eq!(heap_root.repr_depth(), arena_root.repr_depth());
}

#[test]
fn test_alloc_mixed_arena_references() {
    // Parent allocated on heap, children on arena
    let arena = Arc::new(CellsArena::new(4096, 65536));

    let child_heap = create_cell_in(&[], &[0x01, 0x80], None).unwrap();
    let child_arena = create_cell_in(&[], &[0x02, 0x80], Some(arena.clone())).unwrap();

    let parent =
        create_cell_in(&[child_heap.clone(), child_arena.clone()], &[0xFF, 0x80], None).unwrap();

    assert_eq!(parent.references_count(), 2);
    assert_eq!(*parent.reference(0).unwrap().repr_hash(), *child_heap.repr_hash());
    assert_eq!(*parent.reference(1).unwrap().repr_hash(), *child_arena.repr_hash());
}

#[test]
fn test_alloc_boc_round_trip() {
    // BOC round-trip: serialize and deserialize, verify hashes preserved.
    let child = create_cell_in(&[], &[0x42, 0x80], None).unwrap();
    let parent = create_cell_in(&[child], &[0xFF, 0x80], None).unwrap();

    let boc = crate::write_boc(&parent).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();

    assert_eq!(*parent.repr_hash(), *restored.repr_hash());
    assert_eq!(parent.repr_depth(), restored.repr_depth());
    assert_eq!(parent.references_count(), restored.references_count());
    assert_eq!(
        *parent.reference(0).unwrap().repr_hash(),
        *restored.reference(0).unwrap().repr_hash()
    );
}

#[test]
fn test_alloc_ownership_prefix_size() {
    let heap_cell = create_cell_in(&[], &[0x01, 0x80], None).unwrap();
    assert_eq!(heap_cell.ownership_prefix_size(), 8); // AtomicUsize refcount

    let arena = Arc::new(CellsArena::new(4096, 65536));
    let arena_cell = create_cell_in(&[], &[0x01, 0x80], Some(arena.clone())).unwrap();
    assert_eq!(arena_cell.ownership_prefix_size(), 0); // no prefix
}

#[test]
fn test_alloc_arena_rejects_heap_child() {
    let heap_child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let arena = Arc::new(CellsArena::new(4096, 65536));
    create_cell_in(&[heap_child], &[0xFF, 0x80], Some(arena.clone()))
        .expect_err("heap child in arena parent");
}

#[test]
fn test_alloc_arena_rejects_foreign_arena_child() {
    let arena1 = Arc::new(CellsArena::new(4096, 65536));
    let arena2 = Arc::new(CellsArena::new(4096, 65536));
    let child = create_cell_in(&[], &[0x01, 0x80], Some(arena1.clone())).unwrap();
    create_cell_in(&[child], &[0xFF, 0x80], Some(arena2.clone())).expect_err("foreign arena child");
}

#[test]
fn test_alloc_arena_accepts_same_arena_child() {
    let arena = Arc::new(CellsArena::new(4096, 65536));
    let child = create_cell_in(&[], &[0x01, 0x80], Some(arena.clone())).unwrap();
    let parent = create_cell_in(&[child], &[0xFF, 0x80], Some(arena.clone())).unwrap();
    assert_eq!(parent.references_count(), 1);
}

// Merkle proof cell tests

fn build_merkle_proof(child: &Cell) -> Cell {
    let child_hash = child.repr_hash();
    let child_depth = child.repr_depth();
    // cell data: [type_byte=3][child_repr_hash:32][child_repr_depth:2] + completion tag
    let mut data = vec![u8::from(CellType::MerkleProof)];
    data.extend_from_slice(child_hash.as_slice());
    data.extend_from_slice(&child_depth.to_be_bytes());
    data.push(0x80); // completion tag for byte-aligned data

    let child_lm = child.level_mask().mask();
    let lm = child_lm | (1 << child.level());

    let raw = Cell::build_data(&data, CellType::MerkleProof, lm, 1, None).unwrap();
    Cell::with_data_and_refs(&raw, false, &[child.clone()], None, None).unwrap()
}

#[test]
fn test_merkle_proof_level0_child() {
    let child = create_cell(&[], &[0xAB, 0x80]).unwrap();
    assert_eq!(child.level(), 0);

    let proof = build_merkle_proof(&child);
    assert_eq!(proof.cell_type(), CellType::MerkleProof);
    assert_eq!(proof.level(), 1);
    assert_eq!(proof.level_mask().mask(), 1);
    assert_eq!(proof.references_count(), 1);

    // hash(0) is proof's own hash at level 0 (computed from d1/d2/data/child hashes)
    // hash(1) = proof's repr_hash (highest level)
    assert_eq!(*proof.hash(1), *proof.repr_hash());
    assert_ne!(*proof.hash(0), *proof.hash(1));
    assert_eq!(proof.depth(0), child.repr_depth() + 1);

    assert_eq!(proof.hashes().len(), 2);
    assert_eq!(proof.depths().len(), 2);

    // BOC round-trip preserves hashes
    let boc = crate::write_boc(&proof).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();
    assert_eq!(*restored.hash(0), *proof.hash(0));
    assert_eq!(*restored.repr_hash(), *proof.repr_hash());
}

#[test]
fn test_merkle_proof_nested() {
    // child -> proof1 (level 1) -> proof2 (level 2)
    let child = create_cell(&[], &[0x42, 0x80]).unwrap();
    let proof1 = build_merkle_proof(&child);
    assert_eq!(proof1.level(), 1);
    assert_eq!(proof1.level_mask().mask(), 1);

    let proof2 = build_merkle_proof(&proof1);
    assert_eq!(proof2.level(), 2);
    assert_eq!(proof2.level_mask().mask(), 3);
    assert_eq!(proof2.hashes().len(), 3); // levels 0, 1, 2
    assert_eq!(proof2.depths().len(), 3);

    // All three hash levels are distinct
    assert_ne!(*proof2.hash(0), *proof2.hash(1));
    assert_ne!(*proof2.hash(1), *proof2.hash(2));
    assert_eq!(*proof2.hash(2), *proof2.repr_hash());
}

#[test]
fn test_merkle_proof_triple_nested() {
    // child -> p1 (level 1) -> p2 (level 2) -> p3 (level 3)
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let p1 = build_merkle_proof(&child);
    let p2 = build_merkle_proof(&p1);
    let p3 = build_merkle_proof(&p2);

    assert_eq!(p3.level(), 3);
    assert_eq!(p3.level_mask().mask(), 7);
    assert_eq!(p3.hashes().len(), 4);
    assert_eq!(p3.depths().len(), 4);

    // All four hash levels are distinct; hash(3) = repr_hash
    assert_eq!(*p3.hash(3), *p3.repr_hash());
    assert_ne!(*p3.hash(0), *p3.hash(1));
    assert_ne!(*p3.hash(1), *p3.hash(2));
    assert_ne!(*p3.hash(2), *p3.hash(3));
}

// Merkle update cell tests

fn build_merkle_update(old_child: &Cell, new_child: &Cell) -> Cell {
    let old_hash = old_child.repr_hash();
    let old_depth = old_child.repr_depth();
    let new_hash = new_child.repr_hash();
    let new_depth = new_child.repr_depth();

    let mut data = vec![u8::from(CellType::MerkleUpdate)];
    data.extend_from_slice(old_hash.as_slice());
    data.extend_from_slice(&old_depth.to_be_bytes());
    data.extend_from_slice(new_hash.as_slice());
    data.extend_from_slice(&new_depth.to_be_bytes());
    data.push(0x80); // completion tag

    let child_lm = old_child.level_mask() | new_child.level_mask();
    let max_level = child_lm.level();
    let lm = child_lm.mask() | (1 << max_level);

    let raw = Cell::build_data(&data, CellType::MerkleUpdate, lm, 2, None).unwrap();
    Cell::with_data_and_refs(&raw, false, &[old_child.clone(), new_child.clone()], None, None)
        .unwrap()
}

#[test]
fn test_merkle_update_level0_children() {
    let old = create_cell(&[], &[0xAA, 0x80]).unwrap();
    let new = create_cell(&[], &[0xBB, 0x80]).unwrap();

    let update = build_merkle_update(&old, &new);
    assert_eq!(update.cell_type(), CellType::MerkleUpdate);
    assert_eq!(update.level(), 1);
    assert_eq!(update.level_mask().mask(), 1);
    assert_eq!(update.references_count(), 2);
    assert_eq!(update.hashes().len(), 2);
    assert_eq!(update.depths().len(), 2);

    assert_eq!(*update.reference(0).unwrap().repr_hash(), *old.repr_hash());
    assert_eq!(*update.reference(1).unwrap().repr_hash(), *new.repr_hash());
}

#[test]
fn test_merkle_update_with_proof_children() {
    let leaf1 = create_cell(&[], &[0x01, 0x80]).unwrap();
    let leaf2 = create_cell(&[], &[0x02, 0x80]).unwrap();
    let old = build_merkle_proof(&leaf1); // level 1
    let new = build_merkle_proof(&leaf2); // level 1

    let update = build_merkle_update(&old, &new);
    assert_eq!(update.level(), 2);
    assert_eq!(update.level_mask().mask(), 3);
    assert_eq!(update.hashes().len(), 3);
}

// Pruned branch cell tests

fn build_pruned_branch(level_mask: u8, hashes: &[[u8; 32]], depths: &[u16]) -> Cell {
    let lm = LevelMask::with_mask(level_mask);
    let lvl = lm.level() as usize;
    assert_eq!(hashes.len(), lvl);
    assert_eq!(depths.len(), lvl);

    let mut data = vec![u8::from(CellType::PrunedBranch), level_mask];
    for h in hashes {
        data.extend_from_slice(h);
    }
    for &d in depths {
        data.extend_from_slice(&d.to_be_bytes());
    }
    data.push(0x80); // completion tag

    let raw = Cell::build_data(&data, CellType::PrunedBranch, level_mask, 0, None).unwrap();
    Cell::with_data_and_refs(&raw, false, &[], None, None).unwrap()
}

#[test]
fn test_pruned_branch_level1() {
    let hash = [0xAB; 32];
    let pruned = build_pruned_branch(1, &[hash], &[5]);

    assert_eq!(pruned.cell_type(), CellType::PrunedBranch);
    assert_eq!(pruned.level(), 1);
    assert_eq!(pruned.level_mask().mask(), 1);
    assert_eq!(pruned.references_count(), 0);
    assert_eq!(pruned.hashes().len(), 2); // 1 stored in data + 1 repr

    // hash(0) comes from pruned data
    assert_eq!(pruned.hash(0).as_slice(), &hash);
    // depth(0) comes from pruned data
    assert_eq!(pruned.depth(0), 5);

    // hash(1) = repr_hash (computed)
    assert_ne!(pruned.hash(1).as_slice(), &hash);
}

#[test]
fn test_pruned_branch_level2() {
    let h1 = [0x11; 32];
    let h2 = [0x22; 32];
    let pruned = build_pruned_branch(3, &[h1, h2], &[10, 20]);

    assert_eq!(pruned.level(), 2);
    assert_eq!(pruned.level_mask().mask(), 3);
    assert_eq!(pruned.hashes().len(), 3);
    assert_eq!(pruned.depths().len(), 3);

    assert_eq!(pruned.hash(0).as_slice(), &h1);
    assert_eq!(pruned.hash(1).as_slice(), &h2);
    assert_eq!(pruned.depth(0), 10);
    assert_eq!(pruned.depth(1), 20);
}

#[test]
fn test_pruned_branch_level3() {
    let h1 = [0x11; 32];
    let h2 = [0x22; 32];
    let h3 = [0x33; 32];
    let pruned = build_pruned_branch(7, &[h1, h2, h3], &[1, 2, 3]);

    assert_eq!(pruned.level(), 3);
    assert_eq!(pruned.level_mask().mask(), 7);
    assert_eq!(pruned.hashes().len(), 4);
    assert_eq!(pruned.depths().len(), 4);

    assert_eq!(pruned.hash(0).as_slice(), &h1);
    assert_eq!(pruned.hash(1).as_slice(), &h2);
    assert_eq!(pruned.hash(2).as_slice(), &h3);
    assert_eq!(pruned.depth(0), 1);
    assert_eq!(pruned.depth(1), 2);
    assert_eq!(pruned.depth(2), 3);
}

#[test]
fn test_pruned_branch_non_contiguous_mask() {
    // level_mask=5 (0b101): levels 0 and 2 set, level 1 not set → level=2, 2 hashes
    let h1 = [0xAA; 32];
    let h2 = [0xBB; 32];
    let pruned = build_pruned_branch(5, &[h1, h2], &[7, 8]);

    assert_eq!(pruned.level(), 2);
    assert_eq!(pruned.level_mask().mask(), 5);
    assert_eq!(pruned.hashes().len(), 3); // pruned stores 2 + 1 repr

    // hash(0) → array_index via calc_hash_index(0) on mask 5 = level of (5 & 1) = 1 → index 1
    // Actually: calc_hash_index(0) = level of (mask & with_level(0)) = level of (5 & 0) = 0
    // For pruned branch, array_index 0 != level(2), so it reads from pruned data
    assert_eq!(pruned.hash(0).as_slice(), &h1);
    assert_eq!(pruned.depth(0), 7);
}

// Library reference cell tests

#[test]
fn test_library_reference() {
    let lib_hash = [0x42; 32];
    let mut data = vec![u8::from(CellType::LibraryReference)];
    data.extend_from_slice(&lib_hash);
    data.push(0x80); // completion tag

    let raw = Cell::build_data(&data, CellType::LibraryReference, 0, 0, None).unwrap();
    let cell = Cell::with_data_and_refs(&raw, false, &[], None, None).unwrap();

    assert_eq!(cell.cell_type(), CellType::LibraryReference);
    assert_eq!(cell.level(), 0);
    assert_eq!(cell.references_count(), 0);
    assert_eq!(cell.hashes().len(), 1);
    assert_eq!(cell.bit_length(), (1 + 32) * 8);
}

// VirtualCell tests

#[test]
fn test_virtual_cell_level0_noop() {
    let cell = create_cell(&[], &[0xAB, 0x80]).unwrap();
    let hash_before = cell.repr_hash().clone();
    let vcell = cell.virtualize(1);
    // level-0 cell: virtualize is a no-op
    assert_eq!(*vcell.repr_hash(), hash_before);
    assert_eq!(vcell.level(), 0);
}

#[test]
fn test_virtual_cell_merkle_proof() {
    let child = create_cell(&[], &[0x42, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    assert_eq!(proof.level(), 1);
    assert_eq!(proof.level_mask().mask(), 1);

    let vproof = proof.clone().virtualize(1);
    assert_eq!(vproof.level(), 0);
    assert_eq!(vproof.level_mask().mask(), 0);

    // After virtualization with offset=1, virtual index 0 maps to proof's hash(0)
    assert_eq!(*vproof.hash(0), *proof.hash(0));
    assert_eq!(vproof.depth(0), proof.depth(0));
}

#[test]
fn test_virtual_cell_nested_proof() {
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let p1 = build_merkle_proof(&child);
    let p2 = build_merkle_proof(&p1);
    assert_eq!(p2.level(), 2);
    assert_eq!(p2.level_mask().mask(), 3);

    // Virtualize by 1: level drops from 2 to 1
    let v1 = p2.clone().virtualize(1);
    assert_eq!(v1.level(), 1);
    assert_eq!(v1.level_mask().mask(), 1);
    // Virtual index maps through to inner: v1.hash(i) = p2.hash(i)
    assert_eq!(*v1.hash(0), *p2.hash(0));
    assert_eq!(*v1.hash(1), *p2.hash(1));

    // Virtualize by 2: level drops to 0
    let v2 = p2.clone().virtualize(2);
    assert_eq!(v2.level(), 0);
    assert_eq!(*v2.hash(0), *p2.hash(0));
}

#[test]
fn test_virtual_cell_level3() {
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let p1 = build_merkle_proof(&child);
    let p2 = build_merkle_proof(&p1);
    let p3 = build_merkle_proof(&p2);
    assert_eq!(p3.level(), 3);
    assert_eq!(p3.level_mask().mask(), 7);
    assert_eq!(p3.hashes().len(), 4);

    for offset in 1..=3u8 {
        let v = p3.clone().virtualize(offset);
        let expected_level = 3 - offset;
        assert_eq!(v.level(), expected_level);
    }
}

#[test]
fn test_virtual_cell_reference_access() {
    let child = create_cell(&[], &[0xAB, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    let vproof = proof.clone().virtualize(1);

    // Reference from virtualized cell should also be virtualized
    let vref = vproof.reference(0).unwrap();
    assert_eq!(vref.level(), 0);
    assert_eq!(*vref.repr_hash(), *child.repr_hash());
}

// UsageCell tests

#[test]
fn test_usage_cell_visit_on_load() {
    let cell = create_cell(&[], &[0xAB, 0x80]).unwrap();
    let ut = UsageTree::with_params(cell.clone(), true);

    // visit_on_load=true: root is visited immediately
    assert!(ut.contains(&cell.repr_hash()));
}

#[test]
fn test_usage_cell_visit_on_access() {
    let child = create_cell(&[], &[0xBB, 0x80]).unwrap();
    let parent = create_cell(&[child.clone()], &[0xAA, 0x80]).unwrap();
    let ut = UsageTree::with_root(parent.clone());
    let root = ut.root_cell();

    // Only root is visited (visit_on_load=false by default in with_root)
    assert!(!ut.contains(&parent.repr_hash()));

    // Access data → marks visited
    let _ = root.data();
    assert!(ut.contains(&parent.repr_hash()));
    assert!(!ut.contains(&child.repr_hash()));

    // Access reference → marks child visited
    let child_ref = root.reference(0).unwrap();
    let _ = child_ref.data();
    assert!(ut.contains(&child.repr_hash()));
}

#[test]
fn test_usage_cell_hash_depth_passthrough() {
    let cell = create_cell(&[], &[0x42, 0x80]).unwrap();
    let ut = UsageTree::with_root(cell.clone());
    let usage = ut.root_cell();

    assert_eq!(*usage.repr_hash(), *cell.repr_hash());
    assert_eq!(usage.repr_depth(), cell.repr_depth());
    assert_eq!(usage.level(), cell.level());
    assert_eq!(usage.bit_length(), cell.bit_length());
    assert_eq!(usage.cell_type(), cell.cell_type());
    assert_eq!(usage.references_count(), cell.references_count());
}

#[test]
fn test_usage_cell_with_merkle_proof() {
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    let ut = UsageTree::with_root(proof.clone());
    let usage = ut.root_cell();

    assert_eq!(usage.level(), 1);
    assert_eq!(*usage.hash(0), *proof.hash(0));
    assert_eq!(*usage.hash(1), *proof.hash(1));
}

// VirtualCell wrapped in UsageCell tests

#[test]
fn test_usage_wraps_virtual_level_and_hash() {
    // VirtualCell wrapping MerkleProof (level 1), then wrapped in UsageCell.
    // level/hash/depth must correctly pass through both wrappers.
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    assert_eq!(proof.level(), 1);

    let vcell = proof.clone().virtualize(1);
    assert_eq!(vcell.level(), 0);

    let ut = UsageTree::with_root(vcell.clone());
    let usage_of_virtual = ut.root_cell();

    assert_eq!(usage_of_virtual.level(), 0);
    assert_eq!(usage_of_virtual.level_mask().mask(), 0);
    assert_eq!(usage_of_virtual.hashes_count(), 1);
    assert_eq!(*usage_of_virtual.repr_hash(), *vcell.repr_hash());
    assert_eq!(usage_of_virtual.repr_depth(), vcell.repr_depth());
}

#[test]
fn test_usage_wraps_virtual_data_triggers_visit() {
    // Accessing data() on UsageCell(VirtualCell) must mark the cell as visited.
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    let vcell = proof.clone().virtualize(1);

    let ut = UsageTree::with_root(vcell.clone());
    let usage = ut.root_cell();

    assert!(!ut.contains(&vcell.repr_hash()), "must not be visited before access");
    let _ = usage.data();
    assert!(ut.contains(&vcell.repr_hash()), "must become visited after data()");
}

#[test]
fn test_usage_wraps_virtual_visit_on_load() {
    // visit_on_load=true: UsageCell(VirtualCell) is marked visited immediately on creation.
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    let vcell = proof.clone().virtualize(1);

    let ut = UsageTree::with_params(vcell.clone(), true);

    assert!(ut.contains(&vcell.repr_hash()), "must be visited immediately with visit_on_load=true");
}

#[test]
fn test_usage_wraps_virtual_reference_is_usage_of_virtual() {
    // reference() on UsageCell(VirtualCell) must return a UsageCell(VirtualCell).
    // Child cell is virtualized with the same offset and tracked via the same UsageTree.
    let grandchild = create_cell(&[], &[0xAB, 0x80]).unwrap();
    let child = create_cell(&[grandchild.clone()], &[0x01, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    assert_eq!(proof.level(), 1);

    let vcell = proof.clone().virtualize(1);
    assert_eq!(vcell.level(), 0);

    let ut = UsageTree::with_root(vcell.clone());
    let usage = ut.root_cell();

    // Access data first to register visit
    let _ = usage.data();

    // reference(0) must return a cell with the correct repr_hash
    let child_via_usage = usage.reference(0).unwrap();
    // Child cell through virtualization must match the original
    assert_eq!(*child_via_usage.repr_hash(), *child.repr_hash());
    assert_eq!(child_via_usage.level(), child.level());

    // Accessing child cell's data must register it in the UsageTree
    assert!(!ut.contains(&child.repr_hash()));
    let _ = child_via_usage.data();
    assert!(ut.contains(&child.repr_hash()));
}

#[test]
fn test_usage_wraps_virtual_nested_proof_level2() {
    // Level-2 VirtualCell wrapped in UsageCell.
    // hash(i) must correspond to the virtualized values.
    let leaf = create_cell(&[], &[0x01, 0x80]).unwrap();
    let p1 = build_merkle_proof(&leaf);
    let p2 = build_merkle_proof(&p1);
    assert_eq!(p2.level(), 2);

    let vcell = p2.clone().virtualize(1);
    assert_eq!(vcell.level(), 1);

    let ut = UsageTree::with_root(vcell.clone());
    let usage = ut.root_cell();

    assert_eq!(usage.level(), 1);
    assert_eq!(usage.level_mask().mask(), vcell.level_mask().mask());
    assert_eq!(usage.hashes_count(), 2);
    assert_eq!(*usage.hash(0), *vcell.hash(0));
    assert_eq!(*usage.hash(1), *vcell.hash(1));
    assert_eq!(*usage.repr_hash(), *vcell.repr_hash());
}

#[test]
fn test_usage_wraps_virtual_data_matches_inner() {
    // data() of UsageCell(VirtualCell) must match data() of VirtualCell itself
    // (which in turn reads from the inner DataCell).
    let child = create_cell(&[], &[0x42, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);
    let vcell = proof.clone().virtualize(1);

    let ut = UsageTree::with_root(vcell.clone());
    let usage = ut.root_cell();

    assert_eq!(usage.data(), vcell.data());
    assert_eq!(usage.bit_length(), vcell.bit_length());
    assert_eq!(usage.cell_type(), vcell.cell_type());
    assert_eq!(usage.references_count(), vcell.references_count());
}

// LoadedCell tests (via BOC round-trip)

#[test]
fn test_loaded_cell_via_boc() {
    let child = create_cell(&[], &[0xBB, 0x80]).unwrap();
    let parent = create_cell(&[child.clone()], &[0xAA, 0x80]).unwrap();

    let boc = crate::write_boc(&parent).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();

    assert_eq!(*restored.repr_hash(), *parent.repr_hash());
    assert_eq!(restored.repr_depth(), parent.repr_depth());
    assert_eq!(restored.bit_length(), parent.bit_length());
    assert_eq!(restored.level(), parent.level());
    assert_eq!(restored.references_count(), parent.references_count());
    assert_eq!(*restored.reference(0).unwrap().repr_hash(), *child.repr_hash());
}

#[test]
fn test_loaded_cell_merkle_proof_via_boc() {
    let child = create_cell(&[], &[0x42, 0x80]).unwrap();
    let proof = build_merkle_proof(&child);

    let boc = crate::write_boc(&proof).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();

    assert_eq!(*restored.repr_hash(), *proof.repr_hash());
    assert_eq!(restored.level(), 1);
    assert_eq!(restored.level_mask().mask(), 1);
    assert_eq!(restored.hashes().len(), 2);
    assert_eq!(*restored.hash(0), *proof.hash(0));
    assert_eq!(*restored.hash(1), *proof.hash(1));
    assert_eq!(restored.depth(0), proof.depth(0));
    assert_eq!(restored.depth(1), proof.depth(1));
}

#[test]
fn test_loaded_cell_nested_merkle_via_boc() {
    let leaf = create_cell(&[], &[0x01, 0x80]).unwrap();
    let p1 = build_merkle_proof(&leaf);
    let p2 = build_merkle_proof(&p1);

    let boc = crate::write_boc(&p2).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();

    assert_eq!(restored.level(), 2);
    assert_eq!(restored.level_mask().mask(), 3);
    assert_eq!(restored.hashes().len(), 3);
    for i in 0..3 {
        assert_eq!(*restored.hash(i), *p2.hash(i));
        assert_eq!(restored.depth(i), p2.depth(i));
    }
}

#[test]
fn test_boc_round_trip_deep_tree() {
    let mut cell = create_cell(&[], &[0x80]).unwrap();
    for i in 1..=5u8 {
        cell = create_cell(&[cell], &[i, 0x80]).unwrap();
    }
    let original_hash = cell.repr_hash().clone();
    let original_depth = cell.repr_depth();

    let boc = crate::write_boc(&cell).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();

    assert_eq!(*restored.repr_hash(), original_hash);
    assert_eq!(restored.repr_depth(), original_depth);

    // Walk down the tree
    let mut current = restored;
    for i in (1..=5u8).rev() {
        assert_eq!(current.data()[0], i);
        current = current.reference(0).unwrap();
    }
    assert_eq!(current.references_count(), 0);
}

// Level mask tests across cell types

#[test]
fn test_level_mask_ordinary_all_masks() {
    // Ordinary cells can have any level_mask (0..7), but hashes are computed
    // for each significant level. Test that hashes/depths counts are correct.
    for mask in 0u8..=7 {
        let raw = Cell::build_data(&[0xAB], CellType::Ordinary, mask, 0, None).unwrap();
        let cell = Cell::with_data_and_refs(&raw, false, &[], None, None).unwrap();
        let expected_level = LevelMask::with_mask(mask).level();
        assert_eq!(cell.level(), expected_level, "mask={}", mask);
        assert_eq!(cell.hashes().len(), expected_level as usize + 1, "mask={}", mask);
        assert_eq!(cell.depths().len(), expected_level as usize + 1, "mask={}", mask);
    }
}

#[test]
fn test_level_mask_pruned_branch_all_valid() {
    // Pruned branch valid masks: any non-zero mask
    for mask in [1u8, 2, 3, 4, 5, 6, 7] {
        let lvl = LevelMask::with_mask(mask).level() as usize;
        let hashes: Vec<[u8; 32]> = (0..lvl).map(|i| [i as u8 + 1; 32]).collect();
        let depths: Vec<u16> = (0..lvl).map(|i| i as u16 + 1).collect();
        let pruned = build_pruned_branch(mask, &hashes, &depths);

        assert_eq!(pruned.level_mask().mask(), mask, "mask={}", mask);
        assert_eq!(pruned.level(), lvl as u8, "mask={}", mask);
        // Pruned branch always stores 1 hash (repr) + level hashes from data
        assert_eq!(pruned.hashes().len(), lvl + 1, "mask={}", mask);
    }
}

#[test]
fn test_level_mask_merkle_proof_all_child_levels() {
    // Build merkle proofs over children of different levels
    let base = create_cell(&[], &[0x01, 0x80]).unwrap();
    assert_eq!(base.level(), 0);

    // Proof over level-0 child → proof level 1
    let p1 = build_merkle_proof(&base);
    assert_eq!(p1.level_mask().mask(), 1);

    // Proof over level-1 child → proof level 2
    let p2 = build_merkle_proof(&p1);
    assert_eq!(p2.level_mask().mask(), 3);

    // Proof over level-2 child → proof level 3
    let p3 = build_merkle_proof(&p2);
    assert_eq!(p3.level_mask().mask(), 7);
    assert_eq!(p3.hashes().len(), 4);
    assert_eq!(p3.depths().len(), 4);
}

#[test]
fn test_hash_consistency_across_cell_types() {
    // Build a cell, serialize/deserialize, and check that hashes match
    // between DataCell (original) and the restored cell
    let child = create_cell(&[], &[0xCC, 0x80]).unwrap();
    let parent = create_cell(&[child.clone()], &[0xDD, 0x80]).unwrap();

    let boc = crate::write_boc(&parent).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();

    // Compare all hashes and depths
    for i in 0..=parent.level() as usize {
        assert_eq!(*restored.hash(i), *parent.hash(i), "hash mismatch at index {}", i);
        assert_eq!(restored.depth(i), parent.depth(i), "depth mismatch at index {}", i);
    }
}

#[test]
fn test_merkle_proof_hash_consistency_boc() {
    let leaf = create_cell(&[], &[0x01, 0x80]).unwrap();
    let proof = build_merkle_proof(&leaf);

    let boc = crate::write_boc(&proof).unwrap();
    let restored = crate::read_single_root_boc(boc).unwrap();

    for i in 0..=proof.level() as usize {
        assert_eq!(*restored.hash(i), *proof.hash(i), "hash mismatch at level {}", i);
        assert_eq!(restored.depth(i), proof.depth(i), "depth mismatch at level {}", i);
    }
}

// LoadedCell weak loader tests

fn make_loaded_cell_with_loader(
    data: &[u8],
    ref_hashes: &[UInt256],
    ref_depths: &[u16],
    loader: Arc<dyn Fn(&UInt256) -> crate::Result<Cell> + Send + Sync>,
    arena: Option<Arc<CellsArena>>,
) -> Cell {
    // Build raw data with store_hashes set. Level 0, ordinary, hashes_depths = hash(0) + depth(0).
    let _hc = 1; // hashes_count for level 0
    let rc = ref_hashes.len();
    let d1 =
        crate::cell::calc_d1(crate::cell::LevelMask::with_mask(0), true, CellType::Ordinary, rc);
    let d2 = crate::cell::calc_d2(crate::cell::find_tag(data));
    let data_len = ((d2 >> 1) + (d2 & 1)) as usize;

    // We need the hash and depth, but for a loaded cell we can provide dummy ones.
    // The real hash would be computed from data+refs, but for testing loader behavior
    // we just need a valid layout.
    let dummy_hash = [0xABu8; 32];
    let dummy_depth: u16 = 0;

    let mut raw = Vec::new();
    raw.push(d1);
    raw.push(d2);
    // hashes (hc=1 hash)
    raw.extend_from_slice(&dummy_hash);
    // depths (hc=1 depth)
    raw.extend_from_slice(&dummy_depth.to_be_bytes());
    // cell data
    raw.extend_from_slice(&data[..data_len]);

    Cell::with_data_and_loader(&raw, false, ref_hashes, ref_depths, &loader, arena).unwrap()
}

#[test]
fn test_loaded_cell_heap_loader_works() {
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let child_hash = child.repr_hash().clone();
    let child_clone = child.clone();

    let loader: Arc<dyn Fn(&UInt256) -> crate::Result<Cell> + Send + Sync> =
        Arc::new(move |hash: &UInt256| {
            if *hash == child_hash {
                Ok(child_clone.clone())
            } else {
                crate::fail!("unknown hash")
            }
        });

    let loaded = make_loaded_cell_with_loader(
        &[0xFF, 0x80],
        &[child.repr_hash().clone()],
        &[child.repr_depth().to_be_bytes()[0] as u16],
        loader,
        None,
    );

    assert!(loaded.is_heap());
    let ref0 = loaded.reference(0).unwrap();
    assert_eq!(*ref0.repr_hash(), *child.repr_hash());
}

#[test]
fn test_loaded_cell_arena_weak_loader_works() {
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let child_hash = child.repr_hash().clone();
    let child_clone = child.clone();

    let loader: Arc<dyn Fn(&UInt256) -> crate::Result<Cell> + Send + Sync> =
        Arc::new(move |hash: &UInt256| {
            if *hash == child_hash {
                Ok(child_clone.clone())
            } else {
                crate::fail!("unknown hash")
            }
        });

    let arena = Arc::new(CellsArena::new(4096, 65536));
    let loaded = make_loaded_cell_with_loader(
        &[0xFF, 0x80],
        &[child.repr_hash().clone()],
        &[child.repr_depth()],
        loader.clone(),
        Some(arena.clone()),
    );

    assert!(!loaded.is_heap());
    // Loader Arc is still alive, so Weak can upgrade
    let ref0 = loaded.reference(0).unwrap();
    assert_eq!(*ref0.repr_hash(), *child.repr_hash());
}

#[test]
fn test_loaded_cell_arena_weak_loader_fails_after_drop() {
    let child = create_cell(&[], &[0x01, 0x80]).unwrap();
    let child_hash = child.repr_hash().clone();
    let child_clone = child.clone();

    let loader: Arc<dyn Fn(&UInt256) -> crate::Result<Cell> + Send + Sync> =
        Arc::new(move |hash: &UInt256| {
            if *hash == child_hash {
                Ok(child_clone.clone())
            } else {
                crate::fail!("unknown hash")
            }
        });

    let arena = Arc::new(CellsArena::new(4096, 65536));
    let loaded = make_loaded_cell_with_loader(
        &[0xFF, 0x80],
        &[child.repr_hash().clone()],
        &[child.repr_depth()],
        loader.clone(),
        Some(arena.clone()),
    );

    // Drop the only strong reference to the loader
    drop(loader);

    // Now the Weak inside the arena cell can't upgrade
    let result = loaded.reference(0);
    assert!(result.is_err(), "should fail after loader is dropped");
}

#[test]
fn test_loaded_cell_arena_no_loader_leak() {
    let loader: Arc<dyn Fn(&UInt256) -> crate::Result<Cell> + Send + Sync> =
        Arc::new(|_| crate::fail!("not called"));

    let weak_check = Arc::downgrade(&loader);

    let arena = Arc::new(CellsArena::new(4096, 65536));
    let _loaded =
        make_loaded_cell_with_loader(&[0x01, 0x80], &[], &[], loader.clone(), Some(arena.clone()));

    // Drop our strong ref — arena cell holds only Weak
    drop(loader);

    // The Weak should not be upgradeable (no leak)
    assert!(weak_check.upgrade().is_none(), "loader Arc should have been dropped");
}

// Concurrent arena tests

#[test]
fn test_arena_concurrent_alloc_no_overlap() {
    // Multiple threads allocate cells from the same arena.
    // Verify that all cells have valid, non-overlapping data.
    let arena = Arc::new(CellsArena::new(4096, 1 << 20));
    let cells_per_thread = 200;
    let num_threads = 8;

    let handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let arena = arena.clone();
            std::thread::spawn(move || {
                let mut cells = Vec::with_capacity(cells_per_thread);
                for i in 0..cells_per_thread {
                    let byte = ((t * cells_per_thread + i) & 0xFF) as u8;
                    let cell = create_cell_in(&[], &[byte, 0x80], Some(arena.clone())).unwrap();
                    assert!(arena.contains(cell.raw_ptr()));
                    cells.push((byte, cell));
                }
                cells
            })
        })
        .collect();

    let all_cells: Vec<_> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    assert_eq!(all_cells.len(), num_threads * cells_per_thread);

    // Every cell must still read back its data correctly
    for (expected_byte, cell) in &all_cells {
        assert_eq!(cell.data()[0], *expected_byte);
    }

    // All raw pointers must be distinct (no overlap)
    let mut ptrs: Vec<_> = all_cells.iter().map(|(_, c)| c.raw_ptr() as usize).collect();
    ptrs.sort();
    ptrs.dedup();
    assert_eq!(ptrs.len(), all_cells.len(), "some cells share the same pointer");
}

#[test]
fn test_arena_concurrent_alloc_with_chunk_growth() {
    // Small chunk_size forces frequent chunk growth under contention.
    let arena = Arc::new(CellsArena::new(CellsArena::MIN_CHUNK_SIZE, 1 << 20));
    let cells_per_thread = 100;
    let num_threads = 8;

    let handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let arena = arena.clone();
            std::thread::spawn(move || {
                let mut cells = Vec::with_capacity(cells_per_thread);
                for i in 0..cells_per_thread {
                    let byte = ((t * 100 + i) & 0xFF) as u8;
                    let cell = create_cell_in(&[], &[byte, 0x80], Some(arena.clone())).unwrap();
                    cells.push((byte, cell));
                }
                cells
            })
        })
        .collect();

    let all_cells: Vec<_> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    assert_eq!(all_cells.len(), num_threads * cells_per_thread);

    for (expected_byte, cell) in &all_cells {
        assert_eq!(cell.data()[0], *expected_byte);
        assert!(arena.contains(cell.raw_ptr()));
    }
}

#[test]
fn test_arena_concurrent_trees() {
    // Each thread builds a small tree in the shared arena,
    // then we verify all root hashes match the heap-built equivalent.
    let arena = Arc::new(CellsArena::new(4096, 1 << 20));
    let num_threads = 8;

    let handles: Vec<_> = (0..num_threads as u8)
        .map(|t| {
            let arena = arena.clone();
            std::thread::spawn(move || {
                let leaf1 = create_cell_in(&[], &[t, 0x80], Some(arena.clone())).unwrap();
                let leaf2 =
                    create_cell_in(&[], &[t.wrapping_add(1), 0x80], Some(arena.clone())).unwrap();
                let root = create_cell_in(
                    &[leaf1, leaf2],
                    &[t.wrapping_add(2), 0x80],
                    Some(arena.clone()),
                )
                .unwrap();

                // Build the same tree on heap for hash comparison
                let h_leaf1 = create_cell(&[], &[t, 0x80]).unwrap();
                let h_leaf2 = create_cell(&[], &[t.wrapping_add(1), 0x80]).unwrap();
                let h_root = create_cell(&[h_leaf1, h_leaf2], &[t.wrapping_add(2), 0x80]).unwrap();

                assert_eq!(
                    *root.repr_hash(),
                    *h_root.repr_hash(),
                    "hash mismatch for thread {}",
                    t
                );
                root
            })
        })
        .collect();

    let roots: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(roots.len(), num_threads);

    for root in &roots {
        assert!(arena.contains(root.raw_ptr()));
        assert_eq!(root.references_count(), 2);
    }
}

#[test]
fn test_arena_concurrent_contains() {
    // Allocate cells from multiple threads, then verify contains()
    // returns correct results from all threads simultaneously.
    let arena = Arc::new(CellsArena::new(4096, 1 << 20));
    let other_arena = Arc::new(CellsArena::new(4096, 65536));

    // Pre-allocate some cells
    let arena_cells: Vec<_> =
        (0..100u8).map(|i| create_cell_in(&[], &[i, 0x80], Some(arena.clone())).unwrap()).collect();
    let other_cell = create_cell_in(&[], &[0xFF, 0x80], Some(other_arena.clone())).unwrap();
    let heap_cell = create_cell(&[], &[0xAA, 0x80]).unwrap();

    let arena_cells = Arc::new(arena_cells);
    let other_cell = Arc::new(other_cell);
    let heap_cell = Arc::new(heap_cell);

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let arena = arena.clone();
            let arena_cells = arena_cells.clone();
            let other_cell = other_cell.clone();
            let heap_cell = heap_cell.clone();
            std::thread::spawn(move || {
                for cell in arena_cells.iter() {
                    assert!(arena.contains(cell.raw_ptr()));
                }
                assert!(!arena.contains(other_cell.raw_ptr()));
                assert!(!arena.contains(heap_cell.raw_ptr()));
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}
