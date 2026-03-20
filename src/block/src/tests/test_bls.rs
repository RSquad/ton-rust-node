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
use crate::bls::*;
use std::{
    collections::{hash_map::DefaultHasher, HashSet},
    hash::{Hash, Hasher},
    time::Instant,
    vec,
};

#[test]
fn test_gen_bls_key_pair() {
    for _i in 0..100 {
        let now = Instant::now();
        let _key_pair = gen_bls_key_pair().unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        // println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by gen_bls_key_pair is: {:?}", duration);
    }
}

#[test]
fn test_gen_bls_key_pair_based_on_key_material() {
    let ikm = [0u8; BLS_KEY_MATERIAL_LEN];
    for _i in 0..100 {
        let now = Instant::now();
        let key_pair = gen_bls_key_pair_based_on_key_material(&ikm).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by gen_bls_key_pair_based_on_key_material is: {:?}", duration);
    }
}

#[test]
fn test_gen_public_key_based_on_secret_key() {
    for _i in 0..100 {
        let key_pair = gen_bls_key_pair().unwrap();
        let now = Instant::now();
        let pk = gen_public_key_based_on_secret_key(&key_pair.1).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by gen_public_key_based_on_secret_key is: {:?}", duration);
        assert_eq!(pk, key_pair.0);
    }
}

#[cfg(feature = "ci_run")]
#[test]
fn test_sign() {
    for _i in 0..100 {
        let key_pair = gen_bls_key_pair().unwrap();
        let msg = generate_random_msg_of_fixed_len(10000000);
        let now = Instant::now();
        let _sig = sign(&key_pair.1, &msg).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by sign is: {:?}", duration);
        // assert_eq!(pk, key_pair.0);
    }
}

#[test]
fn test_verify() {
    for _i in 0..100 {
        let key_pair = gen_bls_key_pair().unwrap();
        let msg = generate_random_msg_of_fixed_len(1000);
        let sig = sign(&key_pair.1, &msg).unwrap();
        let now = Instant::now();
        let res = verify(&sig, &msg, &key_pair.0).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by verify is: {:?}", duration);
        assert!(res);
    }
}

#[test]
fn test_add_node_info_to_sig() {
    let index = 100;
    let total_num_of_index = 10000;
    for _i in 0..100 {
        let key_pair = gen_bls_key_pair().unwrap();
        let msg = generate_random_msg_of_fixed_len(500000);
        let sig = sign(&key_pair.1, &msg).unwrap();
        let now = Instant::now();
        let _res = add_node_info_to_sig(sig, index, total_num_of_index).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by add_node_info_to_sig is: {:?}", duration);
    }
}

#[test]
fn test_sign_and_add_node_info() {
    let index = 100;
    let total_num_of_index = 1000;
    for _i in 0..100 {
        let key_pair = gen_bls_key_pair().unwrap();
        let msg = generate_random_msg_of_fixed_len(10000000);
        let now = Instant::now();
        let _res = sign_and_add_node_info(&key_pair.1, &msg, index, total_num_of_index).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by sign_and_add_node_info is: {:?}", duration);
    }
}

#[cfg(feature = "ci_run")]
#[test]
fn test_aggregate_public_keys() {
    let number_of_keys = 10000;
    for _i in 0..10 {
        let mut public_keys = Vec::new();
        for _j in 0..number_of_keys {
            let key_pair = gen_bls_key_pair().unwrap();
            public_keys.push(key_pair.0);
        }
        let public_keys_refs: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = public_keys.iter().collect();
        let now = Instant::now();
        let _res = aggregate_public_keys(&public_keys_refs).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by aggregate_public_keys is: {:?}", duration);
    }
}

#[test]
fn test_aggregate_public_keys_and_verify() {
    let number_of_keys = 10;
    let msg = generate_random_msg_of_fixed_len(1000);

    let mut public_keys = Vec::new();
    let mut sigs = Vec::new();
    for _j in 0..number_of_keys {
        let (pub_key, sig_key) = gen_bls_key_pair().unwrap();
        sigs.push(sign(&sig_key, &msg).unwrap());
        public_keys.push(pub_key);
    }

    let sigs_refs: Vec<&[u8; BLS_SIG_LEN]> =
        sigs.iter().map(|sig| sig.as_slice().try_into().unwrap()).collect();
    let sig_bytes = aggregate_pure_bls_signatures(&sigs_refs).unwrap();

    let keys_refs: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = public_keys.iter().collect();
    assert!(aggregate_public_keys_and_verify(&sig_bytes, &msg, &keys_refs).unwrap());
}

#[test]
fn test_aggregate_and_verify() {
    let number_of_keys = 10;
    let mut msgs = vec![];

    let mut public_keys = Vec::new();
    let mut sigs = Vec::new();
    for _j in 0..number_of_keys {
        let (pub_key, sig_key) = gen_bls_key_pair().unwrap();
        let msg = generate_random_msg_of_fixed_len(1000);
        sigs.push(sign(&sig_key, &msg).unwrap());
        msgs.push(msg);
        public_keys.push(pub_key);
    }

    let sigs_refs: Vec<&[u8; BLS_SIG_LEN]> =
        sigs.iter().map(|sig| sig.as_slice().try_into().unwrap()).collect();
    let sig_bytes = aggregate_pure_bls_signatures(&sigs_refs).unwrap();

    let keys_refs: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = public_keys.iter().collect();
    let msgs_refs: Vec<&[u8]> = msgs.iter().map(|msg| msg.as_slice()).collect();
    assert!(aggregate_and_verify(&sig_bytes, &msgs_refs, &keys_refs,).unwrap());

    let sig = P2Affine::new(&sig_bytes).unwrap();
    let mut pks = Vec::with_capacity(keys_refs.len());
    for bls_pk in keys_refs {
        pks.push(P1Affine::new(bls_pk).unwrap());
    }
    assert_eq!(
        aggregate_verify_nostd(&sig, true, &msgs_refs, &DST, &pks, true),
        blst::BLST_ERROR::BLST_SUCCESS
    )
}

#[cfg(feature = "ci_run")]
#[test]
fn test_aggregate_public_keys_based_on_nodes_info() {
    let total_num_of_nodes = 10000;
    for _i in 0..10 {
        let indexes: Vec<u16> = gen_signer_indexes(total_num_of_nodes, total_num_of_nodes * 2);
        let mut node_info_vec = Vec::new();
        for ind in &indexes {
            //println!("Node index = {}", ind);
            let nodes_info = NodesInfo::create_node_info(total_num_of_nodes, *ind).unwrap();
            node_info_vec.push(nodes_info)
        }
        let node_info_vec_refs: Vec<&NodesInfo> = node_info_vec.iter().collect();
        let info = NodesInfo::merge_multiple(&node_info_vec_refs).unwrap();
        println!("Node info size = {}", info.map.len());
        // info.print();

        let mut public_keys = Vec::new();
        for _j in 0..total_num_of_nodes {
            let key_pair = gen_bls_key_pair().unwrap();
            public_keys.push(key_pair.0);
        }
        let public_keys_refs: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = public_keys.iter().collect();
        let now = Instant::now();
        let _res = aggregate_public_keys_based_on_nodes_info(&public_keys_refs, &info.serialize())
            .unwrap();
        let duration = now.elapsed();

        println!("Time elapsed by aggregate_public_keys is: {:?}", duration);
    }
}

#[test]
fn test_aggregate_two_bls_signatures() {
    let number_of_keys = 100;
    for _i in 0..10 {
        let key_pair_1 = gen_bls_key_pair().unwrap();
        let key_pair_2 = gen_bls_key_pair().unwrap();
        let msg = generate_random_msg();
        let ind_1 = gen_random_index(number_of_keys);
        let ind_2 = gen_random_index(number_of_keys);
        let sig_1 = sign_and_add_node_info(&key_pair_1.1, &msg, ind_1, number_of_keys).unwrap();
        let sig_2 = sign_and_add_node_info(&key_pair_2.1, &msg, ind_2, number_of_keys).unwrap();
        let now = Instant::now();
        let _res = aggregate_two_bls_signatures(&sig_1, &sig_2).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by aggregate_two_bls_signatures is: {:?}", duration);
    }
}

#[test]
fn test_aggregate_two_bls_signatures_2() {
    let number_of_keys = 10000;
    for _i in 0..10 {
        let key_pair_1 = gen_bls_key_pair().unwrap();
        let key_pair_2 = gen_bls_key_pair().unwrap();
        let msg = generate_random_msg();

        let sig_1 = sign(&key_pair_1.1, &msg).unwrap();
        let sig_2 = sign(&key_pair_2.1, &msg).unwrap();
        let info_1 = create_random_nodes_info(number_of_keys, number_of_keys * 2);
        let info_2 = create_random_nodes_info(number_of_keys, number_of_keys * 2);
        println!("info_1 size: {:?}", &info_1.map.len());

        let bls_sig_1 = BlsSignature { sig_bytes: sig_1, nodes_info: info_1 }.serialize();

        println!("info_2 size: {:?}", &info_2.map.len());

        let bls_sig_2 = BlsSignature { sig_bytes: sig_2, nodes_info: info_2 }.serialize();

        let now = Instant::now();
        let _res = aggregate_two_bls_signatures(&bls_sig_1, &bls_sig_2).unwrap();
        let duration = now.elapsed();
        //  println!("Public key : {:?}", key_pair.0);
        //println!("Secret key : {:?}", key_pair.1);
        println!("Time elapsed by aggregate_two_bls_signatures is: {:?}", duration);
    }
}

#[cfg(feature = "ci_run")]
#[test]
fn test_aggregate_bls_signatures() {
    let number_of_keys = 10000;
    let number_of_signatures = 50;
    for _i in 0..10 {
        let mut sigs = Vec::new();
        let msg = generate_random_msg();
        for _j in 0..number_of_signatures {
            let key_pair = gen_bls_key_pair().unwrap();
            let sig = sign(&key_pair.1, &msg).unwrap();
            let info = create_random_nodes_info(number_of_keys, number_of_keys * 2);
            println!("info size: {:?}", &info.map.len());
            let bls_sig = BlsSignature { sig_bytes: sig, nodes_info: info }.serialize();
            sigs.push(bls_sig);
        }
        let sigs_refs: Vec<&[u8]> = sigs.iter().map(|sig| &sig[..]).collect();

        let now = Instant::now();
        let _res = aggregate_bls_signatures(&sigs_refs).unwrap();
        let duration = now.elapsed();

        println!("Time elapsed by aggregate_bls_signatures is: {:?}", duration);
    }
}

#[cfg(test)]
mod tests_aggregate {
    use super::*;

    /** zero split prevention and correct group checking **/

    #[test]
    //this is for zero public key
    fn test_aggregate_public_keys_fail_pk_is_infinity_point_for_min_pk_mode() {
        let _bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = kp_1.pk_bytes;

        // public key in compressed form in min_pk mode has size 381 bits
        // in reality we have array of length 384 bits,
        // where first three bits of first byte are reserved
        // 11000000, here first two bits shows that public key is in compressed form
        // and it's gonna be a point of infinity (zero vector)
        let a1 = [0xC0];
        // all other bits will be zero
        let a2 = [0; BLS_PUBLIC_KEY_LEN - 1];

        let key_2: Vec<u8> = a1.iter().chain(a2.iter()).copied().collect();
        let key_2: [u8; BLS_PUBLIC_KEY_LEN] = key_2.try_into().unwrap();
        println!("{:?}", key_2);
        // key_2 now is really zero public key for blst lib,
        // it will not throw bad encoding error and we can work with it
        // but to exclude zero split attack cases we setup
        // additional verification everywhere to exclude zero public key
        let keys = vec![&key_2, &key_1];
        let err = aggregate_public_keys(&keys).err();
        println!("{}", err.unwrap());
        //assert!(err.is_some());
    }

    #[test]
    //this is for zero public key
    fn test_infinity_pk_compressed_validate() {
        // public key in compressed form in min_pk mode has size 381 bits
        // in reality we have array of length 384 bits,
        // where first three bits of first byte are reserved
        // 11000000, here first two bits shows that public key is in compressed form
        // and it's gonna be a point of infinity (zero vector)
        let a1 = [0xC0];
        // all other bits will be zero
        let a2 = [0; BLS_PUBLIC_KEY_LEN - 1];
        let key_2: Vec<u8> = a1.iter().chain(a2.iter()).copied().collect();
        let key_2: [u8; BLS_PUBLIC_KEY_LEN] = key_2.try_into().unwrap();
        println!("{:?}", key_2);
        // key_2 now is really zero public key for blst lib,
        // it will not throw bad encoding error and we can work with it
        // but to exclude zero split attack cases we setup
        // additional verification everywhere to exclude zero public key
        let pkk = blst::min_pk::PublicKey::from_bytes(&key_2).unwrap();
        //let pkk = //convert_public_key_bytes_to_public_key(&key_2).unwrap();
        let err = pkk.validate().err();
        println!("ERROR {:?}", err);
    }

    #[test]
    fn test_infinity_pk_uncompressed_validate() {
        // 01000000, here first two bits shows that public key is in compressed form
        // and it's gonna be a point of infinity (zero vector)
        let a1 = [0x40];
        // all other bits will be zero
        let a2 = [0; 2 * BLS_PUBLIC_KEY_LEN - 1];
        let key_2: Vec<u8> = a1.iter().chain(a2.iter()).copied().collect();
        let key_2: [u8; 2 * BLS_PUBLIC_KEY_LEN] = key_2.try_into().unwrap();
        println!("{:?}", key_2);
        let pkk = blst::min_pk::PublicKey::from_bytes(&key_2).unwrap();
        let err = pkk.validate().err();
        println!("ERROR {:?}", err);
    }

    #[test]
    fn test_aggregate_public_keys_fail_pk_not_in_group() {
        let _bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = kp_1.pk_bytes;
        let key_2 = [
            130, 70, 150, 125, 169, 172, 192, 188, 9, 54, 153, 180, 207, 211, 148, 25, 5, 82, 202,
            176, 6, 166, 177, 79, 220, 204, 168, 36, 162, 159, 172, 63, 141, 16, 248, 139, 97, 73,
            38, 154, 188, 186, 72, 188, 75, 27, 199, 44,
        ];
        let keys = vec![&key_2, &key_1];
        let err = aggregate_public_keys(&keys).err();
        println!("{}", err.unwrap());
        //assert!(err.is_some());
    }

    #[test]
    fn test_infinity_sig_compressed_validate() {
        // 11000000, here first two bits shows that public key is in compressed form
        // and it's gonna be a point of infinity (zero vector)
        let a1 = [0xC0];
        // all other bits will be zero
        let a2 = [0; BLS_SIG_LEN - 1];

        let sig_2: Vec<u8> = a1.iter().chain(a2.iter()).copied().collect();
        let sig_2: [u8; BLS_SIG_LEN] = sig_2.try_into().unwrap();
        println!("{:?}", sig_2);

        let ss = blst::min_pk::Signature::from_bytes(&sig_2).unwrap();
        let err = ss.validate(true).err();
        println!("ERROR {:?}", err); /**/
        //assert!(err.is_some());
    }

    #[test]
    fn test_infinity_sig_uncompressed_validate() {
        // 11000000, here first two bits shows that public key is in compressed form
        // and it's gonna be a point of infinity (zero vector)
        let a1 = [0x40];
        // all other bits will be zero
        let a2 = [0; 2 * BLS_SIG_LEN - 1];

        let sig_2: Vec<u8> = a1.iter().chain(a2.iter()).copied().collect();
        let sig_2: [u8; 2 * BLS_SIG_LEN] = sig_2.try_into().unwrap();
        println!("{:?}", sig_2);

        let ss = blst::min_pk::Signature::from_bytes(&sig_2).unwrap();
        let err = ss.validate(true).err();
        println!("ERROR {:?}", err); /**/
        //assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_two_bls_signatures_sig_not_in_group() {
        let total = 3;
        let ind_1 = 0;
        let ind_2 = 2;
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_1.print();
        let msg = generate_random_msg();
        let agg_sig_1_bytes = BlsSignature::sign(&kp_1.sk_bytes, &msg, ind_1, total).unwrap();
        let sig_2: [u8; BLS_SIG_LEN] = [
            145, 159, 130, 216, 123, 12, 196, 4, 178, 40, 10, 4, 206, 211, 143, 207, 233, 217, 193,
            27, 251, 138, 210, 17, 189, 65, 10, 145, 47, 247, 82, 94, 15, 139, 219, 83, 9, 60, 251,
            70, 121, 176, 26, 94, 188, 188, 243, 225, 17, 176, 133, 133, 150, 81, 226, 69, 136, 52,
            209, 39, 19, 18, 110, 53, 61, 144, 227, 207, 190, 158, 54, 169, 113, 34, 57, 161, 90,
            110, 33, 46, 164, 236, 52, 251, 142, 236, 246, 173, 1, 183, 66, 238, 48, 140, 170, 141,
        ];
        let agg_sig_2_bytes = BlsSignature::add_node_info_to_sig(sig_2, ind_2, total).unwrap();
        let err = aggregate_two_bls_signatures(&agg_sig_2_bytes, &agg_sig_1_bytes).err();
        println!("{}", err.unwrap());
    }

    #[test]
    fn test_aggregate_bls_signatures_sig_not_in_group() {
        let total = 3;
        let ind_1 = 0;
        let ind_2 = 2;
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_1.print();
        let msg = generate_random_msg();
        let agg_sig_1_bytes = BlsSignature::sign(&kp_1.sk_bytes, &msg, ind_1, total).unwrap();
        let sig_2: [u8; BLS_SIG_LEN] = [
            145, 159, 130, 216, 123, 12, 196, 4, 178, 40, 10, 4, 206, 211, 143, 207, 233, 217, 193,
            27, 251, 138, 210, 17, 189, 65, 10, 145, 47, 247, 82, 94, 15, 139, 219, 83, 9, 60, 251,
            70, 121, 176, 26, 94, 188, 188, 243, 225, 17, 176, 133, 133, 150, 81, 226, 69, 136, 52,
            209, 39, 19, 18, 110, 53, 61, 144, 227, 207, 190, 158, 54, 169, 113, 34, 57, 161, 90,
            110, 33, 46, 164, 236, 52, 251, 142, 236, 246, 173, 1, 183, 66, 238, 48, 140, 170, 141,
        ];
        let agg_sig_2_bytes = BlsSignature::add_node_info_to_sig(sig_2, ind_2, total).unwrap();
        let sigs = vec![&agg_sig_2_bytes[..], &agg_sig_1_bytes[..]];
        let err = aggregate_bls_signatures(&sigs).err();
        println!("{}", err.unwrap());
    }

    /** aggregate_public_keys **/

    #[test]
    fn test_aggregate_public_keys_fail_input_empty() {
        let bls_pk_vec = Vec::new();
        let err = aggregate_public_keys(&bls_pk_vec).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_fail_to_aggregate_strange_public_keys() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        let key_2: [u8; BLS_PUBLIC_KEY_LEN] = [2; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        bls_pk_vec.push(&key_2);
        let err = aggregate_public_keys(&bls_pk_vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_fail_to_aggregate_strange_public_keys_2() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = kp_1.pk_bytes;
        let key_2: [u8; BLS_PUBLIC_KEY_LEN] = [0; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        bls_pk_vec.push(&key_2);
        let err = aggregate_public_keys(&bls_pk_vec).err();
        println!("{}", err.unwrap());
        //assert!(err.is_some());
    }

    /** aggregate_public_keys_based_on_nodes_info **/

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_info_fail_bls_pks_bytes_empty() {
        let bls_pk_vec = Vec::new();
        let total_num_of_nodes = 80;
        let new_info = HashMap::from([(1, 1)]);
        let node_info = NodesInfo::with_data(new_info, total_num_of_nodes).unwrap().serialize();
        let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_info_fail_node_info_len_too_small() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        let mut node_info = Vec::new();
        for _n in 0..6 {
            let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
            assert!(err.is_some());
            node_info.push(100);
        }
    }

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_info_fail_node_info_len_incorrect() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        let node_info = vec![0, 100, 0, 99, 0, 100, 3];
        let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_info_fail_zero_total_num_of_nodes() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        let node_info = vec![0, 0, 0, 5, 0, 99];
        let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_info_fail_index_bigger_than_total_num_of_nodes() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        let node_info = vec![0, 100, 0, 100, 0, 99];
        let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
        println!("{}", err.unwrap());
        //assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_info_fail_zero_number_of_occurrences() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        let node_info = vec![0, 100, 0, 66, 0, 0];
        let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
        //println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_fail_number_of_pks_not_equal_to_total_number_of_nodes(
    ) {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        let key_2: [u8; BLS_PUBLIC_KEY_LEN] = [2; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        bls_pk_vec.push(&key_2);
        let node_info = vec![0, 1, 0, 0, 0, 99];
        let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
        println!("{}", err.unwrap());
        //assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_public_keys_based_on_nodes_fail_strange_public_keys() {
        let mut bls_pk_vec: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let key_1: [u8; BLS_PUBLIC_KEY_LEN] = [1; BLS_PUBLIC_KEY_LEN];
        let key_2: [u8; BLS_PUBLIC_KEY_LEN] = [2; BLS_PUBLIC_KEY_LEN];
        bls_pk_vec.push(&key_1);
        bls_pk_vec.push(&key_2);
        let node_info = vec![0, 2, 0, 0, 0, 99];
        let err = aggregate_public_keys_based_on_nodes_info(&bls_pk_vec, &node_info).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    // aggregate_two_bls_signatures

    fn create_bls_sig() -> Vec<u8> {
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let msg = generate_random_msg();
        BlsSignature::sign(&kp.sk_bytes, &msg, 0, 100).unwrap()
    }

    #[test]
    fn test_aggregate_two_bls_signatures_fail_sig_bytes_len_too_small() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = Vec::new();
        for _n in 0..BLS_SIG_LEN + 6 {
            let err1 = aggregate_two_bls_signatures(&bls_sig_bytes, &vec).err();
            assert!(err1.is_some());
            // println!("{}",err1.unwrap().to_string());
            let err2 = aggregate_two_bls_signatures(&vec, &bls_sig_bytes).err();
            assert!(err2.is_some());
            //println!("{}",err2.unwrap().to_string());
            vec.push(100);
        }
    }

    #[test]
    fn test_aggregate_two_bls_signatures_fail_node_info_incorrect_len() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 100, 0, 99, 0, 100, 3];
        vec.append(&mut node_info_vec);
        let err1 = aggregate_two_bls_signatures(&bls_sig_bytes, &vec).err();
        assert!(err1.is_some());
        let err2 = aggregate_two_bls_signatures(&vec, &bls_sig_bytes).err();
        assert!(err2.is_some());
    }

    #[test]
    fn test_aggregate_two_bls_signatures_fail_zero_total_number_of_nodes() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 0, 0, 99, 0, 100];
        vec.append(&mut node_info_vec);
        let err1 = aggregate_two_bls_signatures(&bls_sig_bytes, &vec).err();
        assert!(err1.is_some());
        let err2 = aggregate_two_bls_signatures(&vec, &bls_sig_bytes).err();
        assert!(err2.is_some());
        //println!("{}",err2.unwrap().to_string());
    }

    #[test]
    fn test_aggregate_two_bls_signatures_fail_index_bigger_than_total_num_of_nodes() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 100, 0, 100, 0, 99];
        vec.append(&mut node_info_vec);
        let err1 = aggregate_two_bls_signatures(&bls_sig_bytes, &vec).err();
        assert!(err1.is_some());
        let err2 = aggregate_two_bls_signatures(&vec, &bls_sig_bytes).err();
        assert!(err2.is_some());
    }

    #[test]
    fn test_aggregate_two_bls_signatures_fail_zero_number_of_occurrences() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 100, 0, 99, 0, 0];
        vec.append(&mut node_info_vec);
        let err1 = aggregate_two_bls_signatures(&bls_sig_bytes, &vec).err();
        assert!(err1.is_some());
        let err2 = aggregate_two_bls_signatures(&vec, &bls_sig_bytes).err();
        assert!(err2.is_some());
        //  println!("{}",err2.unwrap().to_string());
    }

    #[test]
    fn test_aggregate_two_bls_signatures_fail_total_number_of_nodes_not_the_same() {
        let mut vec_1 = vec![1; BLS_SIG_LEN];
        let mut node_info_vec_1: Vec<u8> = vec![0, 103, 0, 99, 0, 1];
        vec_1.append(&mut node_info_vec_1);
        let mut vec_2 = vec![2; BLS_SIG_LEN];
        let mut node_info_vec_2: Vec<u8> = vec![0, 100, 0, 98, 0, 1];
        vec_2.append(&mut node_info_vec_2);
        let err = aggregate_two_bls_signatures(&vec_1, &vec_2).err();
        assert!(err.is_some());
        //println!("{}",err.unwrap().to_string());
    }

    #[test]
    fn test_aggregate_two_bls_signatures_fail_strange_sigs() {
        let mut vec_1 = vec![1; BLS_SIG_LEN];
        let mut node_info_vec_1: Vec<u8> = vec![0, 100, 0, 99, 0, 1];
        vec_1.append(&mut node_info_vec_1);
        let mut vec_2 = vec![2; BLS_SIG_LEN];
        let mut node_info_vec_2: Vec<u8> = vec![0, 100, 0, 98, 0, 1];
        vec_2.append(&mut node_info_vec_2);
        let err = aggregate_two_bls_signatures(&vec_1, &vec_2).err();
        assert!(err.is_some());
        //println!("{}",err.unwrap().to_string());
    }

    /** aggregate_bls_signatures **/

    #[test]
    fn test_aggregate_bls_signatures_fail_empty_input() {
        let vec = Vec::new();
        let err = aggregate_bls_signatures(&vec).err();
        assert!(err.is_some());
        //   println!("{}",err.unwrap().to_string());
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_sig_bytes_len_too_small() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = Vec::new();
        for _n in 0..BLS_SIG_LEN + 6 {
            let input = vec![&bls_sig_bytes[..], &vec[..]];
            let err = aggregate_bls_signatures(&input).err();
            assert!(err.is_some());
            //println!("{}",err.unwrap().to_string());
            vec.push(100);
        }
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_node_info_incorrect_len() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 100, 0, 99, 0, 100, 3];
        vec.append(&mut node_info_vec);
        let input = vec![&bls_sig_bytes[..], &vec[..]];
        let err = aggregate_bls_signatures(&input).err();
        //println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_zero_total_number_of_nodes() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 0, 0, 99, 0, 100];
        vec.append(&mut node_info_vec);
        let input = vec![&bls_sig_bytes[..], &vec[..]];
        let err = aggregate_bls_signatures(&input).err();
        assert!(err.is_some());
        // println!("{}",err.unwrap().to_string());
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_index_bigger_than_total_num_of_nodes() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 100, 0, 100, 0, 99];
        vec.append(&mut node_info_vec);
        let input = vec![&bls_sig_bytes[..], &vec[..]];
        let err = aggregate_bls_signatures(&input).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_zero_number_of_occurrences() {
        let bls_sig_bytes = create_bls_sig();
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut node_info_vec: Vec<u8> = vec![0, 100, 0, 99, 0, 0];
        vec.append(&mut node_info_vec);
        let input = vec![&bls_sig_bytes[..], &vec[..]];
        let err = aggregate_bls_signatures(&input).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_total_number_of_nodes_not_the_same() {
        let mut vec_1 = vec![1; BLS_SIG_LEN];
        let mut node_info_vec_1: Vec<u8> = vec![0, 103, 0, 99, 0, 1];
        vec_1.append(&mut node_info_vec_1);
        let mut vec_2 = vec![2; BLS_SIG_LEN];
        let mut node_info_vec_2: Vec<u8> = vec![0, 100, 0, 98, 0, 1];
        vec_2.append(&mut node_info_vec_2);
        let input = vec![&vec_1[..], &vec_2[..]];
        let err = aggregate_bls_signatures(&input).err();
        assert!(err.is_some());
        // println!("{}",err.unwrap().to_string());
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_strange_sigs() {
        let mut vec_1 = vec![1; BLS_SIG_LEN];
        let mut node_info_vec_1: Vec<u8> = vec![0, 100, 0, 99, 0, 1];
        vec_1.append(&mut node_info_vec_1);
        let mut vec_2 = vec![2; BLS_SIG_LEN];
        let mut node_info_vec_2: Vec<u8> = vec![0, 100, 0, 98, 0, 1];
        vec_2.append(&mut node_info_vec_2);
        let input = vec![&vec_1[..], &vec_2[..]];
        let err = aggregate_bls_signatures(&input).err();
        assert!(err.is_some());
        //println!("{}",err.unwrap().to_string());
    }

    #[test]
    fn test_aggregate_bls_signatures_fail_one_sig_not_enough() {
        let bls_sig_bytes = create_bls_sig();
        let vec = vec![&bls_sig_bytes[..]];
        let err = aggregate_bls_signatures(&vec).err();
        assert!(err.is_some());
        //println!("{}",err.unwrap().to_string());
    }

    /** other tests of correctness for aggregation and verification **/

    #[test]
    fn test_create_agg_sig_verify() {
        let total = 3;
        let ind_1 = 0;
        let ind_2 = 2;
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_1.print();
        let kp_2 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_2.print();
        let msg = generate_random_msg();
        let agg_sig_1_bytes = BlsSignature::sign(&kp_1.sk_bytes, &msg, ind_1, total).unwrap();
        let agg_sig_2_bytes = BlsSignature::sign(&kp_2.sk_bytes, &msg, ind_2, total).unwrap();

        let agg_sig_1 = BlsSignature::deserialize(&agg_sig_1_bytes).unwrap();
        let agg_sig_2 = BlsSignature::deserialize(&agg_sig_2_bytes).unwrap();

        assert_eq!(agg_sig_1.nodes_info.total_num_of_nodes, total);
        assert_eq!(agg_sig_1.nodes_info.map.len(), 1);
        assert!(agg_sig_1.nodes_info.map.contains_key(&ind_1));
        match agg_sig_1.nodes_info.map.get(&ind_1) {
            Some(number_of_occurrence) => assert_eq!(*number_of_occurrence, 1),
            None => panic!("Node index not found"),
        }
        assert_eq!(agg_sig_2.nodes_info.total_num_of_nodes, total);
        assert_eq!(agg_sig_2.nodes_info.map.len(), 1);
        assert!(agg_sig_2.nodes_info.map.contains_key(&ind_2));
        match agg_sig_2.nodes_info.map.get(&ind_2) {
            Some(number_of_occurrence) => assert_eq!(*number_of_occurrence, 1),
            None => panic!("Node index not found"),
        }

        let agg_sig_1_2_bytes =
            aggregate_two_bls_signatures(&agg_sig_1_bytes, &agg_sig_2_bytes).unwrap();
        let agg_sig_1_2 = BlsSignature::deserialize(&agg_sig_1_2_bytes).unwrap();
        agg_sig_1_2.print();

        let apks = vec![&kp_1.pk_bytes, &kp_2.pk_bytes];

        let apk_1_2 = aggregate_public_keys(&apks).unwrap();

        let res = BlsSignature::verify(&agg_sig_1_2_bytes, &apk_1_2, &msg).unwrap();
        println!("res = {}", res);
        assert!(res);
    }

    #[test]
    fn test_bls() {
        let msg = generate_random_msg();

        let total_num_of_nodes = 10;
        let indexes: Vec<u16> = gen_signer_indexes(total_num_of_nodes, 20);

        println!("Indexes = {:?}", indexes);

        let mut bls_sig_from_nodes: Vec<Vec<u8>> = Vec::new();
        let mut pk_from_nodes: Vec<[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
        let mut sk_from_nodes: Vec<[u8; BLS_SECRET_KEY_LEN]> = Vec::new();

        for i in 0..total_num_of_nodes {
            println!("Key pair # {}", i);
            let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
            kp.print();
            pk_from_nodes.push(kp.pk_bytes);
            sk_from_nodes.push(kp.sk_bytes);
            println!();
        }

        println!();
        println!("Signatures from nodes:");
        println!();

        for ind in &indexes {
            println!("Node index = {}", ind);
            let nodes_info = NodesInfo::create_node_info(total_num_of_nodes, *ind).unwrap();
            nodes_info.print();
            let sig =
                BlsSignature::sign(&sk_from_nodes[*ind as usize], &msg, *ind, total_num_of_nodes)
                    .unwrap();
            println!("sig = {:?}", &sig);
            println!("sig len = {}", &sig.len());
            bls_sig_from_nodes.push(sig);
        }

        let bls_sig_from_nodes_refs: Vec<&[u8]> =
            bls_sig_from_nodes.iter().map(|sig| &sig[..]).collect();
        let pk_from_nodes_refs: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = pk_from_nodes.iter().collect();

        let res_sig = aggregate_bls_signatures(&bls_sig_from_nodes_refs).unwrap();

        println!();
        println!("Aggregated Signature:");
        println!();

        println!("aggregated sig = {:?}", &res_sig);
        println!("aggregated sig len = {}", &res_sig.len());

        println!();
        println!("Deserialized Aggregated Signature:");
        println!();

        let agg_sig = BlsSignature::deserialize(&res_sig).unwrap();
        agg_sig.nodes_info.print();

        println!("aggregated sig bytes = {:?}", agg_sig.sig_bytes);
        println!("aggregated sig bytes len = {}", &agg_sig.sig_bytes.len());

        let len = agg_sig.nodes_info.map.keys().len();

        println!("len = {}", len);

        let res_pk = aggregate_public_keys_based_on_nodes_info(
            &pk_from_nodes_refs,
            &agg_sig.nodes_info.serialize(),
        )
        .unwrap();

        println!();
        println!("Aggregated public key:");
        println!();

        println!("aggregated pk = {:?}", &res_pk);
        println!("aggregated pk len = {}", &res_pk.len());

        let res = BlsSignature::verify(&res_sig, &res_pk, &msg).unwrap();

        println!("res = {}", res);
        assert!(res);
    }
}

#[cfg(test)]
mod tests_key_gen {
    use super::*;

    fn calculate_hash<T: Hash>(t: &T) -> u64 {
        let mut s = DefaultHasher::new();
        t.hash(&mut s);
        s.finish()
    }

    #[test]
    fn test_serialize_deserialize() {
        let key_pair = BlsKeyPair::gen_bls_key_pair().unwrap();
        let key_pair_data = key_pair.serialize();
        let key_pair_new = BlsKeyPair::deserialize(&key_pair_data).unwrap();
        let res = key_pair == key_pair_new;
        assert!(res);
    }

    #[test]
    fn test_serialize_deserialize_based_on_secret_key() {
        let key_pair = BlsKeyPair::gen_bls_key_pair().unwrap();
        let key_pair_data = key_pair.serialize();
        let key_pair_new = BlsKeyPair::deserialize_based_on_secret_key(&key_pair_data.1).unwrap();
        let res = key_pair == key_pair_new;
        assert!(res);
    }

    #[test]
    fn test_deserialize_fail() {
        let key_pair = BlsKeyPair::gen_bls_key_pair().unwrap();
        let key_pair_data = key_pair.serialize();
        let new_key_pair_data = ([1u8; BLS_PUBLIC_KEY_LEN], key_pair_data.1);
        let err = BlsKeyPair::deserialize(&new_key_pair_data).err();
        //println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_gen_key_randomness() {
        let mut hashes = HashSet::new();

        for _i in 0..1000 {
            let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
            hashes.insert(calculate_hash(&kp.sk_bytes));
            // println!("iter# {}", _i);
            // println!("key:");
            // println!("{:?}", &kp.sk_bytes);
        }
        assert_eq!(hashes.len(), 1000);
    }
}

#[cfg(test)]
mod sig_tests {
    use super::*;
    /** zero keys and incorrect subgroup checking **/

    #[test]
    fn test_pk_is_point_of_infinity_or_not_in_group() {
        let total_num_of_nodes = 100;
        let node_index = 2;
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let msg = generate_random_msg();
        let bls_sig_bytes =
            BlsSignature::sign(&kp.sk_bytes, &msg, node_index, total_num_of_nodes).unwrap();

        // 11000000, here first two bits shows that public key is in compressed form
        // and it's gonna be a point of infinity (zero vector)
        let a1 = [0xC0];
        // all other bits will be zero
        let a2 = [0; BLS_PUBLIC_KEY_LEN - 1];

        let key_2: Vec<u8> = a1.iter().chain(a2.iter()).copied().collect();
        let key_2: [u8; BLS_PUBLIC_KEY_LEN] = key_2.try_into().unwrap();
        println!("{:?}", key_2);

        let res = BlsSignature::verify(&bls_sig_bytes, &key_2, &msg).unwrap();
        println!("res = {}", res);
        assert!(!res);

        let key_3 = [
            130, 70, 150, 125, 169, 172, 192, 188, 9, 54, 153, 180, 207, 211, 148, 25, 5, 82, 202,
            176, 6, 166, 177, 79, 220, 204, 168, 36, 162, 159, 172, 63, 141, 16, 248, 139, 97, 73,
            38, 154, 188, 186, 72, 188, 75, 27, 199, 44,
        ];
        let err = BlsSignature::verify(&bls_sig_bytes, &key_3, &msg).err();
        println!("{}", err.unwrap());
        //assert!(err.is_some());
    }

    #[test]
    fn test_sig_not_in_group() {
        let sig: [u8; BLS_SIG_LEN] = [
            145, 159, 130, 216, 123, 12, 196, 4, 178, 40, 10, 4, 206, 211, 143, 207, 233, 217, 193,
            27, 251, 138, 210, 17, 189, 65, 10, 145, 47, 247, 82, 94, 15, 139, 219, 83, 9, 60, 251,
            70, 121, 176, 26, 94, 188, 188, 243, 225, 17, 176, 133, 133, 150, 81, 226, 69, 136, 52,
            209, 39, 19, 18, 110, 53, 61, 144, 227, 207, 190, 158, 54, 169, 113, 34, 57, 161, 90,
            110, 33, 46, 164, 236, 52, 251, 142, 236, 246, 173, 1, 183, 66, 238, 48, 140, 170, 141,
        ];
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        let msg = generate_random_msg();
        let _err = BlsSignature::simple_verify(&sig, &msg, &kp.pk_bytes).err();
        // println!("{}",err.unwrap().to_string());
        //assert!(err.is_some());
    }

    /** serialize/deserialize **/

    #[test]
    fn test_serialize_deserialize() {
        let sig_bytes = [1; BLS_SIG_LEN];
        let total_num_of_nodes = 100;
        let new_info = HashMap::from([(9, 1), (10, 4)]);
        let nodes_info = NodesInfo::with_data(new_info, total_num_of_nodes).unwrap();
        let bls_sig = BlsSignature { sig_bytes, nodes_info };
        let vec = bls_sig.serialize();
        assert_eq!(vec.len(), BLS_SIG_LEN + 10);
        let bls_sig_new = BlsSignature::deserialize(&vec).unwrap();
        assert_eq!(bls_sig.sig_bytes, bls_sig_new.sig_bytes);
        assert_eq!(bls_sig.sig_bytes, [1; BLS_SIG_LEN]);
        assert_eq!(bls_sig.nodes_info, bls_sig_new.nodes_info)
    }

    /** deserialize **/

    #[test]
    fn test_deserialize_fail_too_short_input() {
        let _vec: Vec<u8> = Vec::new();
        let mut vec = Vec::new();
        for _n in 0..(BLS_SIG_LEN + 6) {
            let err = BlsSignature::deserialize(&vec).err();
            //println!("{}",err.unwrap().to_string());
            assert!(err.is_some());
            vec.push(100);
        }
    }

    #[test]
    fn test_deserialize_fail_input_len_incorrect() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 99, 0, 100, 3];
        vec.append(&mut nodes_info);
        let err = BlsSignature::deserialize(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_deserialize_fail_index_bigger_than_total_num_of_nodes() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 100, 0, 99];
        vec.append(&mut nodes_info);
        let err = BlsSignature::deserialize(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_deserialize_fail_zero_total_num_of_nodes() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 0, 0, 100, 0, 99];
        vec.append(&mut nodes_info);
        let err = BlsSignature::deserialize(&vec).err();
        //println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_deserialize_fail_zero_number_of_occurrences() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 2, 0, 1, 0, 0];
        vec.append(&mut nodes_info);
        let err = BlsSignature::deserialize(&vec).err();
        //  println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_deserialize() {
        let mut vec = vec![1; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 99, 0, 120, 0, 70, 1, 0];
        vec.append(&mut nodes_info);
        let bls_sig = BlsSignature::deserialize(&vec).unwrap();
        bls_sig.print();
        assert_eq!(bls_sig.nodes_info.total_num_of_nodes, 100);
        let new_info: HashMap<u16, u16> = HashMap::from([(99, 120), (70, 256)]);
        assert_eq!(new_info, bls_sig.nodes_info.map);
        assert_eq!(vec![1; BLS_SIG_LEN], bls_sig.sig_bytes);
    }

    /** simple_sign/simple_verify **/

    #[test]
    fn test_simple_sign_fail_empty_msg() {
        let sk_bytes = [1; BLS_SECRET_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::simple_sign(&sk_bytes, &msg).err();
        //println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_simple_verify_fail_empty_msg() {
        let sig_bytes = [1; BLS_SIG_LEN];
        let pk_bytes = [1; BLS_PUBLIC_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::simple_verify(&sig_bytes, &msg, &pk_bytes).err();
        //println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_simple_sign_verify() {
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let msg = generate_random_msg();
        let sig = BlsSignature::simple_sign(&kp.sk_bytes, &msg).unwrap();
        println!("Signature:");
        println!("{:?}", sig);
        let res = BlsSignature::simple_verify(&sig, &msg, &kp.pk_bytes).unwrap();
        println!("res = {}", res);
        assert!(res);
    }

    #[test]
    fn test_simple_sign_verify_with_wrong_key() {
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_1.print();
        let kp_2 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_2.print();
        let msg = generate_random_msg();
        let sig = BlsSignature::simple_sign(&kp_1.sk_bytes, &msg).unwrap();
        println!("Signature:");
        println!("{:?}", sig);
        let res = BlsSignature::simple_verify(&sig, &msg, &kp_2.pk_bytes).unwrap();
        println!("res = {}", res);
        assert!(!res);
    }

    /** add_node_info_to_sig **/

    #[test]
    fn test_add_node_info_to_sig_fail_zero_total_num_of_nodes() {
        let total_num_of_nodes = 0;
        let node_index = 2;
        let sig_bytes: [u8; BLS_SIG_LEN] = [1; BLS_SIG_LEN];
        let err = add_node_info_to_sig(sig_bytes, node_index, total_num_of_nodes).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_add_node_info_to_sig_fail_node_index_bigger_than_total_num_of_nodes() {
        let total_num_of_nodes = 100;
        let node_index = 100;
        let sig_bytes: [u8; BLS_SIG_LEN] = [1; BLS_SIG_LEN];
        let err = add_node_info_to_sig(sig_bytes, node_index, total_num_of_nodes).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_add_node_info() {
        let total_num_of_nodes = 100;
        let node_index = 2;
        let sig_bytes: [u8; BLS_SIG_LEN] = [1; BLS_SIG_LEN];
        let bls_sig_bytes =
            add_node_info_to_sig(sig_bytes, node_index, total_num_of_nodes).unwrap();
        let bls_sig = BlsSignature::deserialize(&bls_sig_bytes).unwrap();
        assert_eq!(bls_sig.sig_bytes, [1; BLS_SIG_LEN]);
        assert_eq!(bls_sig.nodes_info.total_num_of_nodes, total_num_of_nodes);
        assert_eq!(bls_sig.nodes_info.map, HashMap::from([(2, 1)]));
    }

    /** sign ***/

    #[test]
    fn test_sign_fail_empty_msg() {
        let total_num_of_nodes = 100;
        let node_index = 2;
        let sk_bytes = [1; BLS_SECRET_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::sign(&sk_bytes, &msg, node_index, total_num_of_nodes).err();
        assert!(err.is_some());
    }

    #[test]
    fn test_sign_fail_zero_total_num_of_nodes() {
        let total_num_of_nodes = 0;
        let node_index = 2;
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let msg = generate_random_msg();
        let err = BlsSignature::sign(&kp.sk_bytes, &msg, node_index, total_num_of_nodes).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_sign_fail_node_index_bigger_than_total_num_of_nodes() {
        let total_num_of_nodes = 100;
        let node_index = 100;
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let msg = generate_random_msg();
        let err = BlsSignature::sign(&kp.sk_bytes, &msg, node_index, total_num_of_nodes).err();
        //println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_sign() {
        let total_num_of_nodes = 300;
        let node_index = 33;
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let msg = generate_random_msg();
        let sig_bytes = BlsSignature::simple_sign(&kp.sk_bytes, &msg).unwrap();
        let bls_sig_bytes =
            BlsSignature::sign(&kp.sk_bytes, &msg, node_index, total_num_of_nodes).unwrap();
        let bls_sig = BlsSignature::deserialize(&bls_sig_bytes).unwrap();
        assert_eq!(sig_bytes, bls_sig.sig_bytes);
        assert_eq!(bls_sig.nodes_info.total_num_of_nodes, total_num_of_nodes);
        assert_eq!(bls_sig.nodes_info.map, HashMap::from([(33, 1)]));
    }

    /** get_nodes_info_from_sig **/

    #[test]
    fn test_get_nodes_info_from_sig_fail_too_short_input() {
        let _vec: Vec<u8> = Vec::new();
        let mut vec = Vec::new();
        for _n in 0..(BLS_SIG_LEN + 6) {
            let err = BlsSignature::get_nodes_info_from_sig(&vec).err();
            //println!("{}",err.unwrap().to_string());
            assert!(err.is_some());
            vec.push(100);
        }
    }

    #[test]
    fn test_get_nodes_info_from_sig_fail_input_len_incorrect() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 99, 0, 100, 3];
        vec.append(&mut nodes_info);
        let err = BlsSignature::get_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_get_nodes_info_from_sig_fail_index_bigger_than_total_num_of_nodes() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 100, 0, 99];
        vec.append(&mut nodes_info);
        let err = BlsSignature::get_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_get_nodes_info_from_sig_fail_zero_total_num_of_nodes() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 0, 0, 1, 0, 99];
        vec.append(&mut nodes_info);
        let err = BlsSignature::get_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_get_nodes_info_from_sig_fail_zero_number_of_occurrences() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 7, 0, 1, 0, 0];
        vec.append(&mut nodes_info);
        let err = BlsSignature::get_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_get_nodes_info_from_sig() {
        let mut vec = vec![1; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 98, 0, 120, 0, 70, 1, 1];
        vec.append(&mut nodes_info);
        let nodes_info =
            NodesInfo::deserialize(&BlsSignature::get_nodes_info_from_sig(&vec).unwrap()).unwrap();
        nodes_info.print();
        assert_eq!(nodes_info.total_num_of_nodes, 100);
        let new_info: HashMap<u16, u16> = HashMap::from([(98, 120), (70, 257)]);
        assert_eq!(new_info, nodes_info.map);
    }

    /** truncate_nodes_info_from_sig **/

    #[test]
    fn test_truncate_nodes_info_from_sig_fail_too_short_input() {
        let _vec: Vec<u8> = Vec::new();
        let mut vec = Vec::new();
        for _n in 0..(BLS_SIG_LEN + 6) {
            let err = BlsSignature::truncate_nodes_info_from_sig(&vec).err();
            //println!("{}",err.unwrap().to_string());
            assert!(err.is_some());
            vec.push(100);
        }
    }

    #[test]
    fn test_truncate_nodes_info_from_sig_fail_input_len_incorrect() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 99, 0, 100, 3];
        vec.append(&mut nodes_info);
        let err = BlsSignature::truncate_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_truncate_nodes_info_from_sig_fail_index_bigger_than_total_num_of_nodes() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 100, 0, 99];
        vec.append(&mut nodes_info);
        let err = BlsSignature::truncate_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_truncate_nodes_info_from_sig_fail_zero_total_num_of_nodes() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 0, 0, 1, 0, 99];
        vec.append(&mut nodes_info);
        let err = BlsSignature::truncate_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_truncate_nodes_info_from_sig_fail_zero_number_of_occurrences() {
        let mut vec = vec![0; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 7, 0, 1, 0, 0];
        vec.append(&mut nodes_info);
        let err = BlsSignature::truncate_nodes_info_from_sig(&vec).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_truncate_nodes_info_from_sig_from_sig() {
        let mut vec = vec![10; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 98, 0, 120, 0, 70, 1, 1];
        vec.append(&mut nodes_info);
        let sig_bytes = &BlsSignature::truncate_nodes_info_from_sig(&vec).unwrap();
        assert_eq!(vec![10; BLS_SIG_LEN], sig_bytes);
    }

    /** verify **/

    #[test]
    fn test_verify_fail_empty_msg() {
        let mut bls_sig_bytes = vec![10; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 98, 0, 120, 0, 70, 1, 1];
        bls_sig_bytes.append(&mut nodes_info);
        let pk_bytes = [1; BLS_PUBLIC_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::verify(&bls_sig_bytes, &pk_bytes, &msg).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_verify_fail_too_short_input() {
        let pk_bytes = [1; BLS_PUBLIC_KEY_LEN];
        let msg = generate_random_msg();
        let _vec: Vec<u8> = Vec::new();
        let mut vec = Vec::new();
        for _n in 0..(BLS_SIG_LEN + 6) {
            let err = BlsSignature::verify(&vec, &pk_bytes, &msg).err();
            //println!("{}",err.unwrap().to_string());
            assert!(err.is_some());
            vec.push(100);
        }
    }

    #[test]
    fn test_verify_fail_input_len_incorrect() {
        let mut bls_sig_bytes = vec![10; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 99, 0, 100, 3];
        bls_sig_bytes.append(&mut nodes_info);
        let pk_bytes = [1; BLS_PUBLIC_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::verify(&bls_sig_bytes, &pk_bytes, &msg).err();
        //  println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_verify_fail_index_bigger_than_total_num_of_nodes() {
        let mut bls_sig_bytes = vec![10; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 100, 0, 100, 0, 99];
        bls_sig_bytes.append(&mut nodes_info);
        let pk_bytes = [1; BLS_PUBLIC_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::verify(&bls_sig_bytes, &pk_bytes, &msg).err();
        //  println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_verify_fail_zero_total_num_of_nodes() {
        let mut bls_sig_bytes = vec![10; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 0, 0, 100, 0, 99];
        bls_sig_bytes.append(&mut nodes_info);
        let pk_bytes = [1; BLS_PUBLIC_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::verify(&bls_sig_bytes, &pk_bytes, &msg).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    #[test]
    fn test_verify_fail_zero_number_of_occurrences() {
        let mut bls_sig_bytes = vec![10; BLS_SIG_LEN];
        let mut nodes_info: Vec<u8> = vec![0, 3, 0, 2, 0, 0];
        bls_sig_bytes.append(&mut nodes_info);
        let pk_bytes = [1; BLS_PUBLIC_KEY_LEN];
        let msg = Vec::new();
        let err = BlsSignature::verify(&bls_sig_bytes, &pk_bytes, &msg).err();
        // println!("{}",err.unwrap().to_string());
        assert!(err.is_some());
    }

    /** sign/verify **/

    #[test]
    fn test_sign_verify() {
        let total_num_of_nodes = 100;
        let node_index = 2;
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let msg = generate_random_msg();
        let bls_sig_bytes =
            BlsSignature::sign(&kp.sk_bytes, &msg, node_index, total_num_of_nodes).unwrap();
        let res = BlsSignature::verify(&bls_sig_bytes, &kp.pk_bytes, &msg).unwrap();
        println!("res = {}", res);
        assert!(res);
    }

    #[test]
    fn test_sign_verify_with_wrong_key() {
        let total_num_of_nodes = 100;
        let node_index = 2;
        let kp_1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_1.print();
        let kp_2 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp_2.print();
        let msg = generate_random_msg();
        let bls_sig_bytes =
            BlsSignature::sign(&kp_1.sk_bytes, &msg, node_index, total_num_of_nodes).unwrap();
        let res = BlsSignature::verify(&bls_sig_bytes, &kp_2.pk_bytes, &msg).unwrap();
        println!("res = {}", res);
        assert!(!res);
    }

    /** additional test for intersection **/

    #[test]
    fn test_agg_sig_intersection_issue() {
        let kp1 = BlsKeyPair::gen_key_pair().unwrap();
        let kp2 = BlsKeyPair::gen_key_pair().unwrap();
        let kp3 = BlsKeyPair::gen_key_pair().unwrap();

        println!("PK1 = {:?}", &kp1.pk.to_bytes());
        println!("PK2 = {:?}", &kp2.pk.to_bytes());
        println!("PK3 = {:?}", &kp3.pk.to_bytes());

        let pks_refs: Vec<&blst::min_pk::PublicKey> = vec![&kp1.pk, &kp2.pk, &kp3.pk];

        let agg_pk = match blst::min_pk::AggregatePublicKey::aggregate(&pks_refs, false) {
            Ok(agg_pk) => agg_pk,
            Err(err) => panic!("aggregate failure: {:?}", err),
        };
        let pk = agg_pk.to_public_key();

        println!("{:?}", &pk.to_bytes());
        println!("!the slice has {} elements", &pk.to_bytes().len());

        let msg = generate_random_msg();

        let sigs_from_nodes_part1: Vec<blst::min_pk::Signature> =
            vec![kp1.sk.sign(&msg, &DST, &[]), kp2.sk.sign(&msg, &DST, &[])];
        println!("sig len = {}", sigs_from_nodes_part1[0].to_bytes().len());
        println!("sig len = {}", sigs_from_nodes_part1[1].to_bytes().len());

        let mut sig_refs1: Vec<&blst::min_pk::Signature> = Vec::new();
        for sig in &sigs_from_nodes_part1 {
            sig_refs1.push(sig);
        }

        let mut agg_temp1 = blst::min_pk::AggregateSignature::from_signature(sig_refs1[0]);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp1, sig_refs1[1], false)
            .unwrap();

        let sigg1 = agg_temp1.to_signature();

        let sigs_from_nodes_part2: Vec<blst::min_pk::Signature> =
            vec![kp3.sk.sign(&msg, &DST, &[]), kp1.sk.sign(&msg, &DST, &[])];

        println!("sig len = {}", sigs_from_nodes_part2[0].to_bytes().len());
        println!("sig len = {}", sigs_from_nodes_part2[1].to_bytes().len());

        let mut sig_refs2: Vec<&blst::min_pk::Signature> = Vec::new();
        for sig in &sigs_from_nodes_part2 {
            sig_refs2.push(sig);
        }

        let mut agg_temp2 = blst::min_pk::AggregateSignature::from_signature(sig_refs2[0]);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp2, sig_refs2[1], false)
            .unwrap();

        let sigg2 = agg_temp2.to_signature();

        let mut agg_temp_last = blst::min_pk::AggregateSignature::from_signature(&sigg1);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp_last, &sigg2, false).unwrap();

        let agg_final = agg_temp_last.to_signature();
        println!("@@sig len = {}", agg_final.to_bytes().len());
        println!("{:?}", &agg_final.to_bytes());

        let res = BlsSignature::simple_verify(&agg_final.to_bytes(), &msg, &pk.to_bytes()).unwrap();

        println!("res = {}", res);

        assert!(!res);
    }

    #[test]
    fn test_agg_sig_intersection_issue2() {
        let kp1 = BlsKeyPair::gen_key_pair().unwrap();
        let kp2 = BlsKeyPair::gen_key_pair().unwrap();
        let kp3 = BlsKeyPair::gen_key_pair().unwrap();

        println!("PK1 = {:?}", &kp1.pk.to_bytes());
        println!("PK2 = {:?}", &kp2.pk.to_bytes());
        println!("PK3 = {:?}", &kp3.pk.to_bytes());

        let pks_refs1: Vec<&blst::min_pk::PublicKey> = vec![&kp1.pk, &kp2.pk];
        // pks_refs.push(&kp3.pk);

        let agg_pk1 = match blst::min_pk::AggregatePublicKey::aggregate(&pks_refs1, false) {
            Ok(agg_pk) => agg_pk,
            Err(err) => panic!("aggregate failure: {:?}", err),
        };

        let pk12 = agg_pk1.to_public_key();

        let pks_refs2: Vec<&blst::min_pk::PublicKey> = vec![&kp3.pk, &kp1.pk];

        let agg_pk2 = match blst::min_pk::AggregatePublicKey::aggregate(&pks_refs2, false) {
            Ok(agg_pk) => agg_pk,
            Err(err) => panic!("aggregate failure: {:?}", err),
        };

        let pk13 = agg_pk2.to_public_key();

        let mut agg_pk_final = blst::min_pk::AggregatePublicKey::from_public_key(&pk12);
        blst::min_pk::AggregatePublicKey::add_public_key(&mut agg_pk_final, &pk13, false).unwrap();

        let pk = agg_pk_final.to_public_key();

        println!("{:?}", &pk.to_bytes());
        println!("!the slice has {} elements", &pk.to_bytes().len());

        let msg = generate_random_msg();

        let sigs_from_nodes_part1: Vec<blst::min_pk::Signature> =
            vec![kp1.sk.sign(&msg, &DST, &[]), kp2.sk.sign(&msg, &DST, &[])];
        println!("sig len = {}", sigs_from_nodes_part1[0].to_bytes().len());
        println!("sig len = {}", sigs_from_nodes_part1[1].to_bytes().len());

        let mut sig_refs1: Vec<&blst::min_pk::Signature> = Vec::new();
        for sig in &sigs_from_nodes_part1 {
            sig_refs1.push(sig);
        }

        let mut agg_temp1 = blst::min_pk::AggregateSignature::from_signature(sig_refs1[0]);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp1, sig_refs1[1], false)
            .unwrap();

        let sigg1 = agg_temp1.to_signature();

        let sigs_from_nodes_part2: Vec<blst::min_pk::Signature> =
            vec![kp1.sk.sign(&msg, &DST, &[]), kp3.sk.sign(&msg, &DST, &[])];

        println!("sig len = {}", sigs_from_nodes_part2[0].to_bytes().len());
        println!("sig len = {}", sigs_from_nodes_part2[1].to_bytes().len());

        let mut sig_refs2: Vec<&blst::min_pk::Signature> = Vec::new();
        for sig in &sigs_from_nodes_part2 {
            sig_refs2.push(sig);
        }

        let mut agg_temp2 = blst::min_pk::AggregateSignature::from_signature(sig_refs2[0]);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp2, sig_refs2[1], false)
            .unwrap();

        let sigg2 = agg_temp2.to_signature();

        let mut agg_temp_last = blst::min_pk::AggregateSignature::from_signature(&sigg1);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp_last, &sigg2, false).unwrap();

        let agg_final = agg_temp_last.to_signature();
        println!("@@sig len = {}", agg_final.to_bytes().len());
        println!("{:?}", &agg_final.to_bytes());

        let res = BlsSignature::simple_verify(&agg_final.to_bytes(), &msg, &pk.to_bytes()).unwrap();

        println!("res = {}", res);

        assert!(res);

        //  assert_eq!(res, false);
    }

    #[test]
    fn test_agg_sig_intersection_issue3() {
        let kp1 = BlsKeyPair::gen_key_pair().unwrap();
        let kp2 = BlsKeyPair::gen_key_pair().unwrap();
        let kp3 = BlsKeyPair::gen_key_pair().unwrap();

        println!("PK1 = {:?}", &kp1.pk.to_bytes());
        println!("PK2 = {:?}", &kp2.pk.to_bytes());
        println!("PK3 = {:?}", &kp3.pk.to_bytes());

        let pks_refs1: Vec<&blst::min_pk::PublicKey> = vec![&kp1.pk, &kp2.pk, &kp3.pk, &kp1.pk];

        let agg_pk = match blst::min_pk::AggregatePublicKey::aggregate(&pks_refs1, false) {
            Ok(agg_pk) => agg_pk,
            Err(err) => panic!("aggregate failure: {:?}", err),
        };

        let pk = agg_pk.to_public_key();

        println!("{:?}", &pk.to_bytes());
        println!("!the slice has {} elements", &pk.to_bytes().len());

        let msg = generate_random_msg();

        let sigs_from_nodes_part1: Vec<blst::min_pk::Signature> =
            vec![kp1.sk.sign(&msg, &DST, &[]), kp2.sk.sign(&msg, &DST, &[])];
        println!("sig len = {}", sigs_from_nodes_part1[0].to_bytes().len());
        println!("sig len = {}", sigs_from_nodes_part1[1].to_bytes().len());

        let mut sig_refs1: Vec<&blst::min_pk::Signature> = Vec::new();
        for sig in &sigs_from_nodes_part1 {
            sig_refs1.push(sig);
        }

        let mut agg_temp1 = blst::min_pk::AggregateSignature::from_signature(sig_refs1[0]);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp1, sig_refs1[1], false)
            .unwrap();

        let sigg1 = agg_temp1.to_signature();

        let sigs_from_nodes_part2: Vec<blst::min_pk::Signature> =
            vec![kp1.sk.sign(&msg, &DST, &[]), kp3.sk.sign(&msg, &DST, &[])];

        println!("sig len = {}", sigs_from_nodes_part2[0].to_bytes().len());
        println!("sig len = {}", sigs_from_nodes_part2[1].to_bytes().len());

        let mut sig_refs2: Vec<&blst::min_pk::Signature> = Vec::new();
        for sig in &sigs_from_nodes_part2 {
            sig_refs2.push(sig);
        }

        let mut agg_temp2 = blst::min_pk::AggregateSignature::from_signature(sig_refs2[0]);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp2, sig_refs2[1], false)
            .unwrap();

        let sigg2 = agg_temp2.to_signature();

        let mut agg_temp_last = blst::min_pk::AggregateSignature::from_signature(&sigg1);
        blst::min_pk::AggregateSignature::add_signature(&mut agg_temp_last, &sigg2, false).unwrap();

        let agg_final = agg_temp_last.to_signature();
        println!("@@sig len = {}", agg_final.to_bytes().len());
        println!("{:?}", &agg_final.to_bytes());

        let res = BlsSignature::simple_verify(&agg_final.to_bytes(), &msg, &pk.to_bytes()).unwrap();

        println!("res = {}", res);

        assert!(res);
    }

    #[test]
    fn test() {
        let total_num_of_nodes = 100;
        let node_index = 3;
        let nodes_info = NodesInfo::create_node_info(total_num_of_nodes, node_index).unwrap();
        nodes_info.print();

        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();

        let msg = generate_random_msg();
        let sig_bytes = BlsSignature::simple_sign(&kp.sk_bytes, &msg).unwrap();

        println!("Signature:");
        println!("{:?}", sig_bytes);

        let acc_sig = BlsSignature { sig_bytes, nodes_info };
        let acc_sig_serialized = BlsSignature::serialize(&acc_sig);

        println!("acc_sig_serialized:");
        println!("{:?}", acc_sig_serialized);

        let acc_sig_new = BlsSignature::deserialize(&acc_sig_serialized).unwrap();

        assert_eq!(acc_sig_new.nodes_info, acc_sig.nodes_info);

        acc_sig_new.nodes_info.print();
        acc_sig.nodes_info.print();

        assert_eq!(acc_sig.sig_bytes, acc_sig_new.sig_bytes);

        let res = BlsSignature::simple_verify(&acc_sig_new.sig_bytes, &msg, &kp.pk_bytes).unwrap();

        println!("res = {}", res);
        assert!(res);
    }

    #[test]
    fn test_point_of_infinity_agg() {
        let kp = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp.print();
        let kp1 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp1.print();
        let kp2 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp2.print();
        let kp3 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp3.print();
        let kp4 = BlsKeyPair::gen_bls_key_pair().unwrap();
        kp4.print();

        let a1 = [0xC0];
        let a2 = [0; 47];

        let whole: Vec<u8> = a1.iter().chain(a2.iter()).copied().collect();
        let whole: [u8; 48] = whole.try_into().unwrap();
        println!("{:?}", whole);

        let pk = convert_public_key_bytes_to_public_key(&whole).unwrap();
        // println!("{}",err.unwrap().to_string());

        let pks: Vec<blst::min_pk::PublicKey> = vec![pk];

        let pk_refs: Vec<&blst::min_pk::PublicKey> = pks.iter().collect();
        let err = blst::min_pk::AggregatePublicKey::aggregate(&pk_refs, true).err();
        assert!(err.is_some());
    }
}

#[test]
fn test_bls_pairing() -> Result<()> {
    let p = generate_random_msg_of_fixed_len(BLS_G1_LEN as i32);
    let g1 = map_to_g1(p.as_slice().try_into()?);

    let p = generate_random_msg_of_fixed_len(BLS_G2_LEN as i32);
    let g2 = map_to_g2(p.as_slice().try_into()?);

    let s1 = generate_random_msg_of_fixed_len(BLS_SCALAR_LEN as i32);

    let m1 = g1_mul(&g1, &s1.as_slice().try_into()?)?;
    let g1_points = [&m1, &g1];

    let m2 = g2_mul(&g2, &s1.as_slice().try_into()?)?;
    let m2 = g2_neg(&m2)?;
    let g2_points = [&g2, &m2];

    assert!(pairing(&g1_points, &g2_points)?);

    Ok(())
}
