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

    let c11 = create_cell(vec![], &[11, 0x80]).unwrap();
    let c10 = create_cell(vec![c11.clone()], &[10, 0x80]).unwrap();
    let c9 = create_cell(vec![], &[9, 0x80]).unwrap();
    let c8 = create_cell(vec![], &[8, 0x80]).unwrap();
    let c7 = create_cell(vec![], &[7, 0x80]).unwrap();
    let c6 = create_cell(vec![], &[6, 0x80]).unwrap();
    let c5 = create_cell(vec![c9.clone(), c10.clone()], &[5, 0x80]).unwrap();
    let c4 = create_cell(vec![c8.clone()], &[4, 0x80]).unwrap();
    let c3 = create_cell(vec![c6.clone(), c7.clone()], &[3, 0x80]).unwrap();
    let c2 = create_cell(vec![c4.clone(), c5.clone()], &[2, 0x80]).unwrap();
    let c1 = create_cell(vec![c2.clone(), c3.clone()], &[1, 0x80]).unwrap();

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
    let subvisited = ut.build_visited_subtree(&|h| h == &c4.hash(0)).unwrap();

    assert_eq!(subvisited.len(), 2);
    assert!(subvisited.contains(&c4.hash(0)));
    assert!(subvisited.contains(&c8.hash(0)));

    // create usage subtree with root in c5
    let subvisited = ut.build_visited_subtree(&|h| h == &c5.hash(0)).unwrap();

    assert_eq!(subvisited.len(), 3);
    assert!(subvisited.contains(&c5.hash(0)));
    assert!(subvisited.contains(&c10.hash(0)));
    assert!(subvisited.contains(&c11.hash(0)));
}

#[test]
fn test_cell_count_cells() {
    let cell = Cell::default();
    let mut builder = BuilderData::default();
    builder.checked_append_reference(cell.clone()).unwrap();
    builder.checked_append_reference(cell).unwrap();
    let cell = builder.into_cell().unwrap();
    assert_eq!(cell.count_cells(3).unwrap(), 3);
    cell.count_cells(1).expect_err("3 must exceeds 1");
    cell.count_cells(2).expect_err("3 must exceeds 2");
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
    assert_eq!(cell.repr_hash(), default_hash);

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
