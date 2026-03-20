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
    config_params::ConfigParamEnum, read_boc, write_read_and_assert, Block, BlockSignaturesSimplex,
    BlockSignaturesVariant, Cell, ShardIdent, SliceData, TopBlockDescr, UInt256,
};
use std::{fs::File, io::Read};

#[test]
fn test_crypto_signature_with() {
    let cs = CryptoSignature::with_r_s(&[1; 32], &[2; 32]);

    assert_ne!(cs, CryptoSignature::with_r_s(&[3; 32], &[2; 32]));
    write_read_and_assert(cs);
}

#[test]
fn test_crypto_signature_pair_new_default() {
    let vi = CryptoSignaturePair::new();
    let vi2 = CryptoSignaturePair::default();

    assert_eq!(vi, vi2);
    write_read_and_assert(vi);
}

#[test]
fn test_crypto_signature_pair_with() {
    let cs = CryptoSignature::with_r_s(&[1; 32], &[2; 32]);
    let csp = CryptoSignaturePair::with_params(UInt256::from([12; 32]), cs.clone());

    assert_ne!(csp, CryptoSignaturePair::with_params(UInt256::from([33; 32]), cs));

    write_read_and_assert(csp);
}

#[test]
fn test_crypto_sig_pub_keyr_with() {
    let keypair = Ed25519KeyOption::generate().unwrap();
    let spk = SigPubKey::from_bytes(keypair.pub_key().unwrap()).unwrap();
    write_read_and_assert(spk);
}

fn get_rand_vec() -> [u8; 32] {
    (0..32).map(|_| rand::random()).collect::<Vec<u8>>().try_into().unwrap()
}

fn get_rand_s() -> [u8; 32] {
    let mut v = (0..31).map(|_| rand::random()).collect::<Vec<u8>>();
    v.push(0);
    v.try_into().unwrap()
}

fn get_crypto_sig_pair() -> CryptoSignaturePair {
    CryptoSignaturePair::with_params(
        UInt256::rand(),
        CryptoSignature::with_r_s(&get_rand_vec(), &get_rand_s()),
    )
}

fn test_bsp() -> BlockSignaturesPure {
    let mut bsp = BlockSignaturesPure::with_weight(123);
    assert_eq!(bsp.count(), 0);
    assert_eq!(bsp.weight(), 123);

    for n in 1..100 {
        bsp.add_sigpair(get_crypto_sig_pair());
        assert_eq!(bsp.count(), n);
        write_read_and_assert(bsp.clone());
    }
    bsp
}

#[test]
fn test_crypto_block_signatures_pure() {
    let bsp = BlockSignaturesPure::default();
    let bsp1 = BlockSignaturesPure::default();
    assert_eq!(bsp, bsp1);

    write_read_and_assert(bsp);

    test_bsp();
}

#[test]
fn test_crypt_block_signatures() {
    let bs = BlockSignatures::new();
    let bs2 = BlockSignatures::default();
    assert_eq!(bs, bs2);

    write_read_and_assert(bs);

    let bs = BlockSignatures::with_params(ValidatorBaseInfo::with_params(12312, 4545), test_bsp());

    write_read_and_assert(bs);
}

#[test]
fn test_crypto_block_proof() {
    let bp = BlockProof::new();
    let bp2 = BlockProof::default();
    assert_eq!(bp, bp2);

    write_read_and_assert(bp);

    // Test with ordinary (catchain) signatures using convenience method
    let bs = BlockSignatures::with_params(ValidatorBaseInfo::with_params(12312, 4545), test_bsp());

    // Test with_ordinary_signatures convenience constructor
    let bp = BlockProof::with_ordinary_signatures(
        BlockIdExt::with_params(ShardIdent::default(), 43434, UInt256::rand(), UInt256::rand()),
        SliceData::new(vec![0x65, 0x08, 0x71, 0x36, 0x10, 0x00, 0x41, 0x00, 0x80])
            .into_cell()
            .unwrap(),
        Some(bs.clone()),
    );

    write_read_and_assert(bp);

    // Test with_params using BlockSignaturesVariant directly
    let bp2 = BlockProof::with_params(
        BlockIdExt::with_params(ShardIdent::default(), 43434, UInt256::rand(), UInt256::rand()),
        SliceData::new(vec![0x65, 0x08, 0x71, 0x36, 0x10, 0x00, 0x41, 0x00, 0x80])
            .into_cell()
            .unwrap(),
        Some(BlockSignaturesVariant::Ordinary(bs)),
    );

    write_read_and_assert(bp2);
}

#[test]
fn test_top_block_descr() {
    let b = BlockIdExt::with_params(
        ShardIdent::default(),
        3784685,
        UInt256::from([1; 32]),
        UInt256::from([2; 32]),
    );
    let bs = BlockSignatures::with_params(ValidatorBaseInfo::with_params(12312, 4545), test_bsp());
    let mut descr = TopBlockDescr::with_id_and_ordinary_signatures(b, bs);

    descr.append_proof(SliceData::new(vec![1, 0xF0]).into_cell().unwrap());
    descr.append_proof(SliceData::new(vec![2, 0xF0]).into_cell().unwrap());
    descr.append_proof(SliceData::new(vec![3, 0xF0]).into_cell().unwrap());

    write_read_and_assert(descr);
}

// ============= BlockSignaturesSimplex tests =============

#[test]
fn test_block_signatures_simplex_default() {
    let simplex = BlockSignaturesSimplex::default();
    assert_eq!(simplex.slot, 0);
    assert!(!simplex.is_final);
    // C++: approve/notar simplex signature sets must not be serialized into block proofs.
    // Default has is_final = false, so serialization must fail.
    assert!(simplex.serialize().is_err());
}

fn read_block(filename: &str) -> (Block, Cell, UInt256) {
    let mut f = File::open(filename).expect("Error open boc file");
    let mut data = Vec::new();
    f.read_to_end(&mut data).expect("Error read boc file");

    let root = read_boc(&data)
        .expect("Error deserializing boc file")
        .withdraw_single_root()
        .expect("Error deserializing boc file - expected one root");
    let block = Block::construct_from_cell(root.clone()).expect("error deserializing block");

    let hash = UInt256::calc_sha256(&data);

    (block, root, hash)
}

// ============================================================================
// BlockSignaturesSimplex tests
// ============================================================================

fn create_test_simplex_signatures() -> BlockSignaturesSimplex {
    let mut pure_sigs = BlockSignaturesPure::with_weight(1000);
    for _ in 0..3 {
        pure_sigs.add_sigpair(get_crypto_sig_pair());
    }

    // Create simple candidate_data bytes
    let candidate_data = vec![0x12, 0x34, 0x56, 0x78];
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap();

    BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(12345, 67890),
        pure_sigs,
        UInt256::rand(),
        42,
        data,
    )
}

#[test]
fn test_block_signatures_simplex_new_notarize() {
    let session_id = UInt256::rand();
    let candidate_data = vec![0xAB, 0xCD];
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap();

    let bs = BlockSignaturesSimplex::new_notarize(
        ValidatorBaseInfo::with_params(111, 222),
        BlockSignaturesPure::with_weight(500),
        session_id.clone(),
        99,
        data,
    );

    assert_eq!(bs.validator_info.validator_list_hash_short, 111);
    assert_eq!(bs.validator_info.catchain_seqno, 222);
    assert_eq!(bs.pure_signatures.weight(), 500);
    assert_eq!(bs.session_id, session_id);
    assert_eq!(bs.slot, 99);
    assert_eq!(bs.candidate_data_bytes().unwrap(), candidate_data);
    assert!(!bs.is_final);
}

#[test]
fn test_block_signatures_simplex_serialization() {
    let simplex = create_test_simplex_signatures();
    write_read_and_assert(simplex);
}

#[test]
fn test_block_signatures_simplex_is_final_flag() {
    let mut simplex = create_test_simplex_signatures();
    simplex.is_final = false;
    assert!(simplex.serialize().is_err());
}

// ============================================================================
// BlockSignaturesVariant tests
// ============================================================================

#[test]
fn test_block_signatures_variant_ordinary() {
    let ordinary =
        BlockSignatures::with_params(ValidatorBaseInfo::with_params(12312, 4545), test_bsp());
    let variant = BlockSignaturesVariant::Ordinary(ordinary.clone());

    assert!(variant.is_ordinary());
    assert!(!variant.is_simplex());
    assert_eq!(variant.validator_info(), &ordinary.validator_info);
    assert_eq!(variant.pure_signatures().weight(), ordinary.pure_signatures.weight());
}

#[test]
fn test_block_signatures_variant_simplex() {
    let simplex = create_test_simplex_signatures();
    let variant = BlockSignaturesVariant::Simplex(simplex.clone());

    assert!(!variant.is_ordinary());
    assert!(variant.is_simplex());
    assert_eq!(variant.validator_info(), &simplex.validator_info);
    assert_eq!(variant.pure_signatures().weight(), simplex.pure_signatures.weight());
}

#[test]
fn test_block_signatures_variant_from_ordinary() {
    let ordinary = BlockSignatures::with_params(
        ValidatorBaseInfo::with_params(111, 222),
        BlockSignaturesPure::with_weight(333),
    );

    let variant = BlockSignaturesVariant::from_ordinary(ordinary.clone());

    assert!(variant.is_ordinary());
    if let BlockSignaturesVariant::Ordinary(inner) = variant {
        assert_eq!(inner.validator_info, ordinary.validator_info);
    } else {
        panic!("Expected Ordinary variant");
    }
}

#[test]
fn test_block_signatures_variant_from_simplex() {
    let simplex = create_test_simplex_signatures();

    let variant = BlockSignaturesVariant::from_simplex(simplex.clone());

    assert!(variant.is_simplex());
    if let BlockSignaturesVariant::Simplex(inner) = variant {
        assert_eq!(inner.session_id, simplex.session_id);
        assert_eq!(inner.slot, simplex.slot);
    } else {
        panic!("Expected Simplex variant");
    }
}

#[test]
fn test_block_signatures_variant_ordinary_serialization() {
    let ordinary =
        BlockSignatures::with_params(ValidatorBaseInfo::with_params(12312, 4545), test_bsp());
    let variant = BlockSignaturesVariant::Ordinary(ordinary);

    write_read_and_assert(variant);
}

#[test]
fn test_block_signatures_variant_simplex_serialization() {
    let simplex = create_test_simplex_signatures();
    let variant = BlockSignaturesVariant::Simplex(simplex);

    write_read_and_assert(variant);
}

#[test]
fn test_block_signatures_variant_default_is_ordinary() {
    let variant = BlockSignaturesVariant::default();
    assert!(variant.is_ordinary());
}

#[test]
fn test_check_block_signature() {
    let block_bad_signatures = vec![
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "fff30bb892ac26faf24b3fcd2e6445813e53e96820f7419af069da8b5fa1cfff",
                "fc8c299052904d83df1a6a7477d883dd7f67e16d9767cc0bb56b39d961688d08",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "dcb3798da31297dd0b609aecdd15571e952de685515ffc9a4ecb795c4c4d36b8",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "b2f200c23402c51e08376a4965b66b06887ed3f0b016d500a0acaffaace4b545",
                "7cc5715516534c1fa98409dbe3b796222b3bd89c318edbe8f62693bba53f650a",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "57b180c641907c4f65e79238871dd8f690aa3b876433926af79105e0b1ffd859",
            )
            .unwrap(),
        },
    ];

    let block_signatures = vec![
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "fff31bb892ac26faf24b3fcd2e6445813e53e96820f7419af069da8b5fa1cfff",
                "fc8c299052904d83df1a6a7477d883dd7f67e16d9767cc0bb56b39d961688d08",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "dcb3798da31297dd0b609aecdd15571e952de685515ffc9a4ecb795c4c4d36b8",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "b2f200c23402c51e08376a4965b66b06887ed3f0b016d500a0acaffaace4b545",
                "7cc5715516534c1fa98409dbe3b790222b3bd89c318edbe8f62693bba53f650a",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "57b180c641907c4f65e79238871dd8f690aa3b876433926af79105e0b1ffd859",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "9150ae8eb892b8f1cf72d83987767794762000e297cbf0d40ed56376d58147d0",
                "c041e31b508ad1346f71b1364609580ab4a65de5e990efe262c809bda9275b0f",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "f9b100717cee6bb04541181db8657ca62ba040a56944c5bc70571a1e9ae1b58e",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "66ecf94fa4146b06bae2c41be52354bc65294d2f331d21a769df3e25ea0b6a10",
                "23accda06d4de7f09fa596b2ea232c7a4b87c83af1fb84060bb171691a34580f",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "506f978f0df1d268ffd1aa8eae5f205fcbc6b2e71c4994bf4670724b4ae45de3",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "45cca3042b51ac2b0e1075e1dda6595e17449dc11c85079b26999540daa2b2fd",
                "544923d65e21896ae8ed27dfb2c1c8550b1c2099ded126d799f7e6cf10a5a20e",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "0116a3162bba3b57704eea2c0456a3a33f69f0435b1826bc5feac8a1a635c418",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "d9a9a5424bf09f6c06c0c390d7d5b5374fd2e1f45ce6e32aad9984cb99f6769c",
                "8a921ed3bbdf8c84342357d67e31a60536b7ef793d800d5b2f2341d67ee66b0b",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "8bffc824f033a2ae0f27c75049a7ef75262d9788b01d678f8ce1f446d40b62e8",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "d1a8319425ad3bfbe29d0365372a21ec25269d3fe65dec2a44295cdd09a96858",
                "ddcdded6a93e98cc9fc5ba4b572d73bf651aac09e84f4e145192aeae24a0210c",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "9e0a5f6b95f90fc3d7321dbadd19765b1ef36b951091b35e00eb394f53b8f5f0",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "b4d1d4f50788ed9f58313b0d2f704deb841f6487203ff0fe7bcf86c0f7b08152",
                "68bafc59a643298c9f5ac2b4587c91e7c0cd9b7613d80208aaf1295536ac3b00",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "836c4d91152b257438adcabb0b659cd197e83e80ad2bd9f92bf627eea0d9ce00",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "bd549fc264576ccd2c7c312031286c3c58667d30412f9f5d4481fa3044e2d94d",
                "e09cf124ca2451c033e712a67e9624510dc2ab0736bee1b353fb3265cd9df804",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "b44798875f5c390ea9d405b653abb213fb25c108ddd316ccfbb10df2558d6e6c",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "3ef9613a7f36a2282b58232b49c1e2960b4e62db77f1a92e4a1bc41b5042c168",
                "2587baadbba9e1cc3536a20434d39a58e5c028ace47c470462d59c1919289a03",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "e832085c0ae28e711c290c521cae5939c5bae99330febf774b53599216283d6b",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "bf463c14fa0883e85baeadcd7c1341f687e924b960843126ef0160056788f943",
                "d623ad6f10ddf5a9db581059bc3a829e9406345f03b918edf325f6a37209650f",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "72dddc583866fa858bf11b31cd6ea7c7bd7c262b3ab9a387e3474ea2df39975d",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "cabef872450fcbdf06f24be8dd4644a00e8649fa69325736ff2f9d12c96bd285",
                "6f261985ed7400887a50f76e12bcdcdbc9ab03fccf41d6f01d2188e2ae357500",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "618a4a67c1d87435d8c485d5d49fcc4b037df84c30db17932324ffd823c41970",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "41f5348aa7d90694b72e97e29d4a96844359eee01f5416840b6479fd2f80cc4b",
                "dc0f10ff82d76a7b66f1cbae8eadb1f5f728c732abbfad4bcf4efd601eb4f902",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "933e6b659b0e6b69e50bf8d383cc8aead4614f6aa18091b311b0a3790ec69681",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "34b01a519c24d2e7140b6519472fd69bc2f6b46f59369ff0a911e960244db864",
                "ce89341d94ee5f5fa2a65fcb1ad9eedda1b03e41850c89dd31528da791c4ee05",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "c0d6649c96c0f14fc796dfbacd72f35dbf3b8b4e3f680a002b73caabe5e6f431",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "f5ef50b59cc5cd86f9b0ae1411b03823c46ac37df17b980692ff90061d81a0fe",
                "ff1665f347625860ed06bd2337e805bc283faa696fa25221d0ee91ceb2e68d07",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "2f8eef085a47f597b37fad66f802c2850ad67e2b1d2acb011373a8a0aad66244",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "dca4b1061a7c85e787cfc9bcd35185583ae523f0639fd9159a533d6c881faa5f",
                "11787412ab369446a241393dc60fa36624287368f2a20f486b947e5e9f222403",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "8607bd43a82a9f9ab17313e5f33360b440b6dcb0b677c244b31be8e78ea2b0db",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "94a0dbb33cd671d7c64a8fc47ff4bcf5d48de177f5e0eabcc86e5521e584d230",
                "99614f05cfecba3f3c869c7e8662d60c1e7473701ff51f45d4b26bb8628c7504",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "4333fcde9a1730b953841daad4c481adffc925da374ee71bf56a05902ee3ed96",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "c8cc4682fa8354b1fbd98d2e4104185e9a4feb6ab6d4b37b7cce12b453cdfec3",
                "b09e6a73a50af1369d611bcdc0a682ce84f95d72b5e6ffe9e0213a9bc09e740b",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "c30dbdd59cba5be576470fc266ffb3ef4e47028f765e8d278573946fccf01ca7",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "f3d494693f041047b4a815fcbff5306aa2d0b3850fee617966a0366f75b64e2a",
                "2742353b5e66c739114d45de08d18c0e71913ab64cb0755fc2d601e79063df0c",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "21914e7ab88c9247babd60103139b58eae4c7e158cfa76fcab003363ad5cdb85",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "d0360cf14f70b448f2c6c7d2d7c3e183c53351b6410be3feccdd234e377139af",
                "ea6e4d6fcbc33795b32d33823f4a13016ea05ac829b563359ac1b8f68e034c0d",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "312a2510024944e9e923252fc2fa74292c10824cb29351057fbee42c0d14cc24",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "c28ee03d92d207460d14768447fe3b7116750a54153ccff695c0a0e0d4adef77",
                "91293f795d1543b1cc69f470eb43dc1a9ead769f90555e6aaddf2d7b8513580a",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "6eeb0288ca77cba7f474ada177acc72dca8d0dbd3978282fd51fa94e53ff35fa",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "d5d0e860d2f469ef4c0a42eeef53486a608ba3102e2e3ff6dfe0da6d191d3d99",
                "2f546a6a0faa6ea5480074e47f61d31731d35ff2546a35a9a1af755dae48a40f",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "f70f09c410c0ea62e2a22899347a6aded73c6141e18ce123a578073c57245eb9",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "8a8cadd324832ba9b5c1f2578dae323ee9017fb22b0ce993dfc1798d1b0a5389",
                "73d8358fcf01ebd8a4e42b20cddafbb008571e41bc0272e4ddd4f54168f81e0e",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "f6175f79b86cf9107d351121012155e9721567d87aecfcd2c8652a241abab0f4",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "d56145375a0d9bc491c54ca09e4673bfb8b4754fb88ab2750cdf6fecb71f4730",
                "33c3df9e4a3c295895b21ce336e3cc0c71c516b5f8e90e054c6370e6f04be70f",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "bc3d7621b0057fcf23d31b71c4bf6c23dff17e84d1eebbb1ea31d2b26d3e1af9",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "23df386adce5f85d5970034a55201ae7422ee8890152a97bfe0eef113df812db",
                "5052811b9e04d4fe9d72f20790671c28043e187fc5323baee2ff45d59b6f0a0a",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "ac3fcc0213a98b92d49e735f72e2940673dc8bbaa2cbff65af77f5c75777d7a5",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "a8fa9700e8a2fae9659e73b774609754e079136d691f7011ed9227118ed4af1d",
                "9e77ddd222c396cbe3de2e58f3bbb03e0187fda11aa6b68a54f7c25a18a1c806",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "bc436714866ca5b208448cd4fd00b2a9d7b284293966b67f861bf18c4726bfc7",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "cd378ff7ea6d19b1d69b7edccd98c4668238297d32dfeb0f9001986d817ee997",
                "a12a36b1f09f33478c57ad0b6c1e1b6fcb688644cea97f1f2b38653dc5abf001",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "ca23e7624968267b04c05b31b3cf6a1d4a7a8ed19b5189f8e7f9d9e423841c0a",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "fd1d718d73c0dee29ea3a1091fb67721e88d27df2c32d886ca0d65b083fca293",
                "9385a68665f5da53996ed6944615a64113abb282bc588204d733ae81251f0602",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "8e4e5d778eed829c58d7d4d29877997f7ce78f8c0cd197f487126ba8f44e5eb1",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "6fe7b58b17ed037862a226c12550b2a49c2b6fe568928ef7d99a84fe53e842f9",
                "e1b274c6c18d657796f076e8b714f4b66111c69309e67856530a4122b9df4a04",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "a1c7175bac0efb62b007ae1b3cbb5ed86aa2aa884e73c7176148f9d4003ea1bc",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "5ab9447128450d335165694db2baaae73faac1cccede8972252df193f5d13fc7",
                "cdcc68a5396276c771384ce3396b65d045601c03ddea92705cb5e3cba865390c",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "fcab8d254a9250669bdd617c2ec55134dfdd98d33802c0142579538a39b170fc",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "38c159029faaf87dd2dc86795b674acfb34ec3e7f1be8fc36d1d87980c6fde73",
                "689f99299afe00799221a980e89a69589db8ef1399a2dac3c7dfeeea25083608",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "c36b3fd72ee80feb5002574355a7615df2a9c9589a93a5c81cec883c8e1b8695",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "cb681ce07a65c9afc8de6a74622c02f91f382f2045d93cd66957e9e444ab5009",
                "7e302862c989a2e64a30aaf4192b6d8dd261cbca012288857c03c5807b419b03",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "d6271b40cebcdb8eb13851164d1d266f802bef238418e1b6cea03b797f712016",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "fd063fa3f238296f03eb85e31dc3210558577b8c214ac5318e2b97d7949bc05a",
                "9804e722a360b8aae4b11e31f1752a75c4763bee3fd141d273710d88cfbc980f",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "d0d89de686d097e194d5a1ad10b3d1d68e9d2124e7acea735b66d3e1843e24ee",
            )
            .unwrap(),
        },
        CryptoSignaturePair {
            sign: CryptoSignature::from_r_s_str(
                "c998f9cbb729b0f4f5908a7b9fc2e8409cd0efe24a92e5bc0a3108fc915ccbed",
                "91a9b93158b7eb168ddae56bea04c362a5f6f4cc766fa1216e47586df7baa500",
            )
            .unwrap(),
            node_id_short: UInt256::from_str(
                "e382ca7174f5a6f68e43bb60495725b456279419897250974141b678a941df5f",
            )
            .unwrap(),
        },
    ];

    // read block
    let (_, block_root, file_hash) = read_block(
        "src/tests/data/E717C6051F28EC36FB7F612CC88380CACCBD16AEFB1F34456FC0C24F637E0020.boc",
    );

    // read key block & extract validator keys
    let (key_block, _, _) = read_block(
        "src/tests/data/6DD4CFAFD43CB7B38656379392764136A08CC260CBC9D00D8D92F4F3CDF9AB61.boc",
    );

    let cur_validators = key_block
        .read_extra()
        .unwrap()
        .read_custom()
        .unwrap()
        .unwrap()
        .config()
        .unwrap()
        .config(34)
        .unwrap()
        .unwrap();

    if let ConfigParamEnum::ConfigParam34(cur_validators) = cur_validators {
        let mut block_signatures_pure = BlockSignaturesPure::default();
        for sign in block_signatures {
            block_signatures_pure.add_sigpair(sign);
        }
        let mut block_bad_signatures_pure = BlockSignaturesPure::default();
        for sign in block_bad_signatures {
            block_bad_signatures_pure.add_sigpair(sign);
        }

        // check signatures

        let data = Block::build_data_for_sign(&block_root.repr_hash(), &file_hash);

        block_signatures_pure
            .check_signatures(cur_validators.cur_validators.list(), &data)
            .unwrap();

        let result =
            block_bad_signatures_pure.check_signatures(cur_validators.cur_validators.list(), &data);
        assert!(result.is_err());
    }
}

#[test]
fn test_block_proof_with_simplex_signatures() {
    // Create simplex signatures
    let validator_info = ValidatorBaseInfo::with_params(12345, 6789);
    let pure_sigs = test_bsp();
    let session_id = UInt256::rand();
    let slot = 42u32;
    let candidate_data = vec![0xAB, 0xCD, 0xEF];
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap();

    let simplex_sigs = BlockSignaturesSimplex::new_finalize(
        validator_info.clone(),
        pure_sigs.clone(),
        session_id.clone(),
        slot,
        data,
    );

    // Create BlockProof with simplex signatures
    let bp = BlockProof::with_params(
        BlockIdExt::with_params(ShardIdent::default(), 99999, UInt256::rand(), UInt256::rand()),
        SliceData::new(vec![0x11, 0x22, 0x33, 0x44, 0x80]).into_cell().unwrap(),
        Some(BlockSignaturesVariant::Simplex(simplex_sigs)),
    );

    // Test serialization roundtrip
    write_read_and_assert(bp.clone());

    // Verify the deserialized proof has the correct signature type
    let serialized = bp.serialize().unwrap();
    let deserialized = BlockProof::construct_from_cell(serialized).unwrap();

    match deserialized.signatures.as_ref().unwrap() {
        BlockSignaturesVariant::Simplex(s) => {
            assert_eq!(s.session_id, session_id);
            assert_eq!(s.slot, slot);
            assert!(s.is_final);
            assert_eq!(s.validator_info.catchain_seqno, 6789);
        }
        BlockSignaturesVariant::Ordinary(_) => {
            panic!("Expected Simplex signatures, got Ordinary");
        }
    }
}

#[test]
fn test_block_proof_without_signatures() {
    // Test BlockProof with no signatures (proof link)
    let bp = BlockProof::with_params(
        BlockIdExt::with_params(ShardIdent::default(), 12345, UInt256::rand(), UInt256::rand()),
        SliceData::new(vec![0xDE, 0xAD, 0xBE, 0xEF, 0x80]).into_cell().unwrap(),
        None,
    );

    write_read_and_assert(bp.clone());

    let serialized = bp.serialize().unwrap();
    let deserialized = BlockProof::construct_from_cell(serialized).unwrap();
    assert!(deserialized.signatures.is_none());
}

// ============================================================================
// BlockSignaturesSimplex - Extended coverage tests
// ============================================================================

#[test]
fn test_block_signatures_simplex_new_finalize() {
    let session_id = UInt256::rand();
    let candidate_data = vec![0x12, 0x34, 0x56, 0x78];
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap();

    let bs = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(111, 222),
        BlockSignaturesPure::with_weight(500),
        session_id.clone(),
        99,
        data,
    );

    assert_eq!(bs.validator_info.validator_list_hash_short, 111);
    assert_eq!(bs.validator_info.catchain_seqno, 222);
    assert_eq!(bs.pure_signatures.weight(), 500);
    assert_eq!(bs.session_id, session_id);
    assert_eq!(bs.slot, 99);
    assert_eq!(bs.candidate_data_bytes().unwrap(), candidate_data);
    assert!(bs.is_final); // Key difference from new_notarize
    assert!(bs.is_final()); // Test accessor method

    // Finalize signatures CAN be serialized
    assert!(bs.serialize().is_ok());
}

#[test]
fn test_block_signatures_simplex_with_params() {
    let session_id = UInt256::rand();
    let candidate_data = vec![0xAA, 0xBB, 0xCC];
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&candidate_data).unwrap();

    // Test with is_final = true
    let bs_final = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(100, 200),
        BlockSignaturesPure::with_weight(300),
        session_id.clone(),
        42,
        data.clone(),
    );

    assert!(bs_final.is_final);
    assert_eq!(bs_final.slot, 42);
    assert!(bs_final.serialize().is_ok());

    // Test with is_final = false
    let bs_notar = BlockSignaturesSimplex::new_notarize(
        ValidatorBaseInfo::with_params(100, 200),
        BlockSignaturesPure::with_weight(300),
        session_id.clone(),
        42,
        data,
    );

    assert!(!bs_notar.is_final);
    assert!(bs_notar.serialize().is_err()); // Cannot serialize non-final
}

#[test]
fn test_block_signatures_simplex_large_candidate_data() {
    // Test CellString serialization with data larger than one cell (127 bytes)
    // This tests the multi-cell chaining logic

    // Create data larger than MAX_BYTES_PER_CELL (127 bytes)
    let large_data: Vec<u8> = (0u8..=255).cycle().take(300).collect();
    assert!(large_data.len() > 127);
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&large_data).unwrap();

    let bs = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        UInt256::rand(),
        10,
        data,
    );

    // Verify roundtrip
    let extracted = bs.candidate_data_bytes().unwrap();
    assert_eq!(extracted, large_data);

    // Verify serialization/deserialization roundtrip
    write_read_and_assert(bs);
}

#[test]
fn test_block_signatures_simplex_very_large_candidate_data() {
    // Test with data spanning 3+ cells
    let very_large_data: Vec<u8> = (0u8..=255).cycle().take(500).collect();
    assert!(very_large_data.len() > 127 * 3);
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&very_large_data).unwrap();

    let bs = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        UInt256::rand(),
        10,
        data,
    );

    // Verify roundtrip
    let extracted = bs.candidate_data_bytes().unwrap();
    assert_eq!(extracted, very_large_data);

    // Verify full serialization roundtrip
    write_read_and_assert(bs);
}

#[test]
fn test_block_signatures_simplex_empty_candidate_data() {
    // Edge case: empty candidate_data
    let empty_data = Vec::new();
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&empty_data).unwrap();
    let bs = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        UInt256::rand(),
        10,
        data,
    );

    let extracted = bs.candidate_data_bytes().unwrap();
    assert_eq!(extracted, empty_data);

    // Verify hash of empty data
    let extracted_bytes = bs.candidate_data_bytes().unwrap();
    let hash = crate::sha256_digest(&extracted_bytes);
    let expected_empty_hash = crate::sha256_digest(&[]);
    assert_eq!(hash, expected_empty_hash);
}

#[test]
fn test_block_signatures_simplex_set_validator_info() {
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&[0x01]).unwrap();
    let mut bs = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        UInt256::rand(),
        10,
        data,
    );

    assert_eq!(bs.validator_info.validator_list_hash_short, 1);
    assert_eq!(bs.validator_info.catchain_seqno, 2);

    // Update validator info
    bs.set_validator_info(ValidatorBaseInfo::with_params(999, 888));

    assert_eq!(bs.validator_info.validator_list_hash_short, 999);
    assert_eq!(bs.validator_info.catchain_seqno, 888);
}

#[test]
fn test_block_signatures_simplex_pure_signatures_mut() {
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&[0x01]).unwrap();
    let mut bs = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        UInt256::rand(),
        10,
        data,
    );

    assert_eq!(bs.pure_signatures.count(), 0);

    // Add signature pair via mutable reference
    bs.pure_signatures_mut().add_sigpair(get_crypto_sig_pair());
    assert_eq!(bs.pure_signatures.count(), 1);
}

#[test]
fn test_block_signatures_simplex_slot_values() {
    // Test various slot values including edge cases
    for slot in [0u32, 1, 42, 1000, u32::MAX / 2, u32::MAX] {
        let data = BlockSignaturesSimplex::bytes_to_cell_tree(&[0x01]).unwrap();
        let bs = BlockSignaturesSimplex::new_finalize(
            ValidatorBaseInfo::with_params(1, 2),
            BlockSignaturesPure::with_weight(100),
            UInt256::rand(),
            slot,
            data,
        );

        assert_eq!(bs.slot, slot);

        // Verify roundtrip preserves slot
        let serialized = bs.serialize().unwrap();
        let deserialized = BlockSignaturesSimplex::construct_from_cell(serialized).unwrap();
        assert_eq!(deserialized.slot, slot);
    }
}

#[test]
fn test_block_signatures_simplex_session_id_preserved() {
    let session_id = UInt256::from([0xAB; 32]);
    let data = BlockSignaturesSimplex::bytes_to_cell_tree(&[0x01]).unwrap();
    let bs = BlockSignaturesSimplex::new_finalize(
        ValidatorBaseInfo::with_params(1, 2),
        BlockSignaturesPure::with_weight(100),
        session_id.clone(),
        10,
        data,
    );

    // Verify session_id is preserved in serialization
    let serialized = bs.serialize().unwrap();
    let deserialized = BlockSignaturesSimplex::construct_from_cell(serialized).unwrap();
    assert_eq!(deserialized.session_id, session_id);
}
