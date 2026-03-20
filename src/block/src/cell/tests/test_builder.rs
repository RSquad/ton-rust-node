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

mod builder_append {
    use super::*;

    #[test]
    fn nothing_to_empty() {
        let builder = BuilderData::with_raw(vec![0xFF], 0).unwrap();
        assert!(builder.data().is_empty());
        assert_eq!(builder.length_in_bits(), 0);

        let builder = BuilderData::with_bitstring(vec![]).unwrap();
        assert!(builder.data().is_empty());
        assert_eq!(builder.length_in_bits(), 0);
    }

    #[test]
    fn one_byte_to_empty() {
        let result_table = [[0x80], [0xC0], [0xE0], [0xF0], [0xF8], [0xFC], [0xFE]];
        for x in 1..result_table.len() {
            let builder = BuilderData::with_raw(vec![0xFF], x).unwrap();
            assert_eq!(builder.data(), result_table[x - 1]);
            assert_eq!(builder.length_in_bits(), x);
        }

        let builder = BuilderData::with_raw(vec![0xFF], 8).unwrap();
        assert_eq!(builder.data(), &[0xFF]);
        assert_eq!(builder.length_in_bits(), 8);
    }

    #[test]
    fn two_bytes_to_empty() {
        let result_table = [
            [0xFF, 0x80],
            [0xFF, 0xC0],
            [0xFF, 0xE0],
            [0xFF, 0xF0],
            [0xFF, 0xF8],
            [0xFF, 0xFC],
            [0xFF, 0xFE],
        ];
        for x in 1..result_table.len() {
            let builder = BuilderData::with_raw(vec![0xFF, 0xFF], x + 8).unwrap();
            assert_eq!(builder.data(), result_table[x - 1]);
            assert_eq!(builder.length_in_bits(), x + 8);
        }

        let builder = BuilderData::with_raw(vec![0xFF, 0xFF], 16).unwrap();
        assert_eq!(builder.data(), &[0xFF, 0xFF]);
        assert_eq!(builder.length_in_bits(), 16);
    }

    #[test]
    fn one_byte_raw_by_bit() {
        let result_table = [[0x80], [0xC0], [0xE0], [0xF0], [0xF8], [0xFC], [0xFE]];
        let mut builder = BuilderData::new();
        for (x, v) in result_table.iter().enumerate() {
            builder.append_raw(&[0xFF], 1).unwrap();
            assert_eq!(builder.data(), *v);
            assert_eq!(builder.length_in_bits(), x + 1);
        }

        builder.append_raw(&[0xFF], 1).unwrap();
        assert_eq!(builder.data(), &[0xFF]);
        assert_eq!(builder.length_in_bits(), 8);
    }

    #[test]
    fn two_bytes_raw_by_bit() {
        let result_table = [
            [0xFF, 0x80],
            [0xFF, 0xC0],
            [0xFF, 0xE0],
            [0xFF, 0xF0],
            [0xFF, 0xF8],
            [0xFF, 0xFC],
            [0xFF, 0xFE],
        ];
        let mut builder = BuilderData::with_raw(vec![0xFF], 8).unwrap();

        for (x, v) in result_table.iter().enumerate() {
            builder.append_raw(&[0xFF], 1).unwrap();
            assert_eq!(builder.data(), *v);
            assert_eq!(builder.length_in_bits(), x + 9);
        }

        builder.append_raw(&[0xFF], 1).unwrap();
        assert_eq!(builder.data(), &[0xFF, 0xFF]);
        assert_eq!(builder.length_in_bits(), 16);
    }

    #[test]
    fn u8_to_empty() {
        let mut data = BuilderData::new();
        data.append_u8(100).expect("Builder was empty");
        assert_eq!(data.data(), &[100]);
        assert_eq!(data.length_in_bits(), 8);
    }

    #[test]
    fn u16_to_empty() {
        let mut data = BuilderData::new();
        data.append_u16(1123).expect("Builder was empty");
        assert_eq!(data.data(), &[0x4, 0x63]);
        assert_eq!(data.length_in_bits(), 16);
    }

    #[test]
    fn u32_to_empty() {
        let mut data = BuilderData::new();
        data.append_u32(1123).expect("Builder was empty");
        assert_eq!(data.data(), &[0x0, 0x0, 0x4, 0x63]);
        assert_eq!(data.length_in_bits(), 32);
    }

    #[test]
    fn u64_to_empty() {
        let mut data = BuilderData::new();
        data.append_u64(1123).expect("Builder was empty");
        assert_eq!(data.data(), &[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x4, 0x63]);
        assert_eq!(data.length_in_bits(), 64);
    }

    #[test]
    fn u128_to_empty() {
        let mut data = BuilderData::new();
        data.append_u128(1123).expect("Builder was empty");
        assert_eq!(
            data.data(),
            &[0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x4, 0x63]
        );
        assert_eq!(data.length_in_bits(), 128);
    }

    #[test]
    fn i32_few_times() {
        let mut data = BuilderData::new();
        for _i in 0..2 {
            data.append_i32(-1).expect("Builder was empty");
        }
        assert_eq!(data.data(), &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(data.length_in_bits(), 64);
    }

    #[test]
    fn i8_few_times() {
        let mut data = BuilderData::new();
        for i in 0..5 {
            data.append_i8(i).expect("Builder was empty");
        }
        assert_eq!(data.data(), &[0, 1, 2, 3, 4]);
        assert_eq!(data.length_in_bits(), 40);
    }

    #[test]
    fn one_byte_by_bit_bool() {
        let result_table = [[0x80], [0x80], [0xA0], [0xA0], [0xA8], [0xA8], [0xAA]];
        let mut builder = BuilderData::new();
        for (x, v) in result_table.iter().enumerate() {
            builder.append_bit_bool(x % 2 == 0).unwrap();
            assert_eq!(builder.data(), *v);
            assert_eq!(builder.length_in_bits(), x + 1);
        }
    }

    #[test]
    fn one_byte_by_one_and_zero() {
        let result_table = [[0x80], [0x80], [0xA0], [0xA0], [0xA8], [0xA8], [0xAA]];
        let mut builder = BuilderData::new();
        for (x, v) in result_table.iter().enumerate() {
            if x % 2 == 0 {
                builder.append_bit_one().unwrap();
            } else {
                builder.append_bit_zero().unwrap();
            }
            assert_eq!(builder.data(), *v);
            assert_eq!(builder.length_in_bits(), x + 1);
        }
    }

    #[test]
    fn to_builder() {
        let mut builder = BuilderData::with_raw(vec![0xFF], 7).unwrap();
        let builder2 = BuilderData::with_raw(vec![0xFF], 7).unwrap();

        builder.append_builder(&builder2).unwrap();
        assert_eq!(builder.data(), &[0xFF, 0xFC]);
        assert_eq!(builder.length_in_bits(), 14);

        let mut builder3 = BuilderData::with_raw(vec![0xAA], 6).unwrap();
        let builder4 = BuilderData::with_raw(vec![0xAA], 6).unwrap();

        builder3.append_builder(&builder4).unwrap();
        assert_eq!(builder3.data(), &[0xAA, 0xA0]);
        assert_eq!(builder3.length_in_bits(), 12);

        builder.append_builder(&builder3).unwrap();
        assert_eq!(builder.data(), &[0xFF, 0xFE, 0xAA, 0x80]);
        assert_eq!(builder.length_in_bits(), 26);
    }

    #[test]
    fn tagged_data() {
        let mut builder = BuilderData::with_bitstring(vec![0xFF, 0x80]).unwrap();
        assert_eq!(builder.length_in_bits(), 8);
        assert_eq!(builder.data(), &[0xFF]);
        builder.append_bitstring(&[0xFF, 0x80]).unwrap();
        assert_eq!(builder.length_in_bits(), 16);
        assert_eq!(builder.data(), &[0xFF, 0xFF]);
        builder.append_bitstring(&[0xF8]).unwrap();
        assert_eq!(builder.length_in_bits(), 20);
        assert_eq!(builder.data(), &[0xFF, 0xFF, 0xF0]);

        let mut builder2 = BuilderData::with_bitstring(vec![0xF8]).unwrap();
        assert_eq!(builder2.length_in_bits(), 4);
        assert_eq!(builder2.data(), &[0xF0]);
        builder2.append_bitstring(&[0xFF, 0xFF, 0xF8]).unwrap();
        assert_eq!(builder2.length_in_bits(), 24);
        assert_eq!(builder2.data(), &[0xFF, 0xFF, 0xFF]);
    }
}

mod builder_prepend {
    use super::*;

    #[test]
    fn to_builder() {
        let mut builder = BuilderData::with_raw(vec![0xFF], 7).unwrap();
        let builder2 = BuilderData::with_raw(vec![0xFF], 7).unwrap();

        builder.prepend_builder(&builder2).unwrap();
        assert_eq!(builder.data(), &[0xFF, 0xFC]);
        assert_eq!(builder.length_in_bits(), 14);

        let mut builder3 = BuilderData::with_raw(vec![0xAA], 6).unwrap();
        let builder4 = BuilderData::with_raw(vec![0xAA], 6).unwrap();

        builder3.prepend_builder(&builder4).unwrap();
        assert_eq!(builder3.data(), &[0xAA, 0xA0]);
        assert_eq!(builder3.length_in_bits(), 12);

        builder.prepend_builder(&builder3).unwrap();
        assert_eq!(builder.data(), &[0xAA, 0xAF, 0xFF, 0xC0]);
        assert_eq!(builder.length_in_bits(), 26);
    }

    #[test]
    fn tagged_data() {
        let mut builder = BuilderData::with_bitstring(vec![0xFF, 0x80]).unwrap();
        assert_eq!(builder.length_in_bits(), 8);
        assert_eq!(builder.data(), &[0xFF]);
        builder.prepend_bitstring(&[0xFF, 0x80]).unwrap();
        assert_eq!(builder.length_in_bits(), 16);
        assert_eq!(builder.data(), &[0xFF, 0xFF]);
        builder.prepend_bitstring(&[0xA8]).unwrap();
        assert_eq!(builder.length_in_bits(), 20);
        assert_eq!(builder.data(), &[0xAF, 0xFF, 0xF0]);

        let mut builder2 = BuilderData::with_bitstring(vec![0xF8]).unwrap();
        assert_eq!(builder2.length_in_bits(), 4);
        assert_eq!(builder2.data(), &[0xF0]);
        builder2.prepend_bitstring(&[0xAF, 0xFF, 0xF8]).unwrap();
        assert_eq!(builder2.length_in_bits(), 24);
        assert_eq!(builder2.data(), &[0xAF, 0xFF, 0xFF]);
    }
}

#[test]
fn test_bitstring_append() {
    let mut a = BuilderData::with_bitstring(vec![0x01, 0x80]).unwrap();
    let b = BuilderData::with_bitstring(vec![0x02, 0x80]).unwrap();
    a.append_builder(&b).unwrap();
    assert_eq!(&a, &BuilderData::with_bitstring(vec![0x01, 0x02, 0x80]).unwrap())
}

#[test]
fn test_bitstring_with_long_completion_tag() {
    let mut a = BuilderData::with_bitstring(vec![0x80, 0x00, 0x00]).unwrap();
    let b = BuilderData::with_bitstring(vec![0x02, 0x80, 0x00, 0x00]).unwrap();
    a.append_builder(&b).unwrap();
    assert_eq!(&a, &BuilderData::with_bitstring(vec![0x02, 0x80]).unwrap())
}
