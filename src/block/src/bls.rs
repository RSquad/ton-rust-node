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
use crate::{fail, Result};
use rand::{Rng, RngCore};
use std::collections::HashMap;

/*
    Constants
*/

pub const BLS_SECRET_KEY_LEN: usize = 32;
pub const BLS_PUBLIC_KEY_LEN_FOR_MIN_PK_MODE: usize = 48;
pub const BLS_PUBLIC_KEY_LEN_FOR_MIN_SIG_MODE: usize = 96;
pub const BLS_PUBLIC_KEY_LEN: usize = BLS_PUBLIC_KEY_LEN_FOR_MIN_PK_MODE;
pub const BLS_KEY_MATERIAL_LEN: usize = 32;
pub const BLS_SIG_LEN_FOR_MIN_PK_MODE: usize = 96;
pub const BLS_SIG_LEN_FOR_MIN_SIG_MODE: usize = 48;
pub const BLS_SIG_LEN: usize = BLS_SIG_LEN_FOR_MIN_PK_MODE;
pub const BLS_SEED_LEN: usize = 32;
pub const BLS_G1_LEN: usize = 48;
pub const BLS_FP_LEN: usize = 48;
pub const BLS_G2_LEN: usize = 96;
pub const BLS_FP2_LEN: usize = 96;
pub const BLS_SCALAR_LEN: usize = 32;

/*
    Utilities
*/

pub fn gen_bls_key_pair_based_on_key_material(
    ikm: &[u8; BLS_KEY_MATERIAL_LEN],
) -> Result<([u8; BLS_PUBLIC_KEY_LEN], [u8; BLS_SECRET_KEY_LEN])> {
    let key_pair = BlsKeyPair::gen_bls_key_pair_based_on_key_material(ikm)?;
    Ok(key_pair.serialize())
}

pub fn gen_bls_key_pair() -> Result<([u8; BLS_PUBLIC_KEY_LEN], [u8; BLS_SECRET_KEY_LEN])> {
    let key_pair = BlsKeyPair::gen_bls_key_pair()?;
    Ok(key_pair.serialize())
}

pub fn gen_public_key_based_on_secret_key(
    sk: &[u8; BLS_SECRET_KEY_LEN],
) -> Result<[u8; BLS_PUBLIC_KEY_LEN]> {
    let pk = BlsKeyPair::deserialize_based_on_secret_key(sk)?;
    Ok(pk.pk_bytes)
}

pub fn sign(sk_bytes: &[u8; BLS_SECRET_KEY_LEN], msg: &[u8]) -> Result<[u8; BLS_SIG_LEN]> {
    BlsSignature::simple_sign(sk_bytes, msg)
}

pub fn verify(
    sig_bytes: &[u8; BLS_SIG_LEN],
    msg: &[u8],
    pk_bytes: &[u8; BLS_PUBLIC_KEY_LEN],
) -> Result<bool> {
    BlsSignature::simple_verify(sig_bytes, msg, pk_bytes)
}

pub fn add_node_info_to_sig(
    sig_bytes: [u8; BLS_SIG_LEN],
    node_index: u16,
    total_num_of_nodes: u16,
) -> Result<Vec<u8>> {
    BlsSignature::add_node_info_to_sig(sig_bytes, node_index, total_num_of_nodes)
}

pub fn sign_and_add_node_info(
    sk_bytes: &[u8; BLS_SECRET_KEY_LEN],
    msg: &[u8],
    node_index: u16,
    total_num_of_nodes: u16,
) -> Result<Vec<u8>> {
    BlsSignature::sign(sk_bytes, msg, node_index, total_num_of_nodes)
}

pub fn truncate_nodes_info_from_sig(sig_bytes_with_nodes_info: &[u8]) -> Result<[u8; BLS_SIG_LEN]> {
    BlsSignature::truncate_nodes_info_from_sig(sig_bytes_with_nodes_info)
}

pub fn get_nodes_info_from_sig(sig_bytes_with_nodes_info: &[u8]) -> Result<Vec<u8>> {
    BlsSignature::get_nodes_info_from_sig(sig_bytes_with_nodes_info)
}

pub fn truncate_nodes_info_and_verify(
    sig_bytes_with_nodes_info: &[u8],
    pk_bytes: &[u8; BLS_PUBLIC_KEY_LEN],
    msg: &[u8],
) -> Result<bool> {
    BlsSignature::verify(sig_bytes_with_nodes_info, pk_bytes, msg)
}

/*
    Aggregation
*/

fn aggregate_public_keys_(
    bls_pks_bytes: &[&[u8; BLS_PUBLIC_KEY_LEN]],
) -> Result<blst::min_pk::PublicKey> {
    if bls_pks_bytes.is_empty() {
        fail!("Vector of public keys can not be empty!");
    }
    let mut pks: Vec<blst::min_pk::PublicKey> = Vec::new();
    for bls_pk in bls_pks_bytes {
        pks.push(convert_public_key_bytes_to_public_key(bls_pk)?);
    }
    let pk_refs: Vec<&blst::min_pk::PublicKey> = pks.iter().collect();
    let agg = match blst::min_pk::AggregatePublicKey::aggregate(&pk_refs, true) {
        Ok(agg) => agg,
        Err(err) => fail!("aggregate failure: {:?}", err),
    };
    Ok(agg.to_public_key())
}

pub fn aggregate_public_keys(
    bls_pks_bytes: &[&[u8; BLS_PUBLIC_KEY_LEN]],
) -> Result<[u8; BLS_PUBLIC_KEY_LEN]> {
    let aggr_pk = aggregate_public_keys_(bls_pks_bytes)?;
    Ok(aggr_pk.to_bytes())
}

pub fn aggregate_public_keys_and_verify(
    sig_bytes: &[u8; BLS_SIG_LEN],
    msg: &[u8],
    bls_pks_bytes: &[&[u8; BLS_PUBLIC_KEY_LEN]],
) -> Result<bool> {
    let aggr_pk = aggregate_public_keys_(bls_pks_bytes)?;
    let sig = convert_signature_bytes_to_signature(sig_bytes)?;
    let res = sig.verify(true, msg, &DST, &[], &aggr_pk, false);
    Ok(res == blst::BLST_ERROR::BLST_SUCCESS)
}

pub fn aggregate_and_verify(
    sig_bytes: &[u8; BLS_SIG_LEN],
    msgs: &[&[u8]],
    bls_pks_bytes: &[&[u8; BLS_PUBLIC_KEY_LEN]],
) -> Result<bool> {
    if msgs.len() != bls_pks_bytes.len() {
        fail!("Vector of messages and vector of public keys must have the same length!");
    }
    if msgs.is_empty() {
        fail!("Vector of messages can not be empty!");
    }

    #[cfg(feature = "std")]
    {
        let mut pks: Vec<blst::min_pk::PublicKey> = Vec::new();
        for bls_pk in bls_pks_bytes {
            pks.push(convert_public_key_bytes_to_public_key(bls_pk)?);
        }
        let pk_refs: Vec<&blst::min_pk::PublicKey> = pks.iter().collect();

        let sig = convert_signature_bytes_to_signature(sig_bytes)?;

        let res = sig.aggregate_verify(true, msgs, &DST, &pk_refs, true);
        Ok(res == blst::BLST_ERROR::BLST_SUCCESS)
    }
    #[cfg(not(feature = "std"))]
    {
        let sig = P2Affine::new(sig_bytes)?;
        let mut pks = Vec::with_capacity(bls_pks_bytes.len());
        for bls_pk in bls_pks_bytes {
            pks.push(P1Affine::new(bls_pk)?);
        }
        let res = aggregate_verify_nostd(&sig, true, msgs, &DST, &pks, true);
        Ok(res == blst::BLST_ERROR::BLST_SUCCESS)
    }
}

// The following code is a modified version of the `aggregate_verify` function from the `blst` crate.
// The original function is not available in the `no_std` environment,
// so we had to copy the code here and change treadpool calculations to a simple loop.
// Originally it returns only BLST_SUCCESS or BLST_VERIFY_FAIL, so we do the same here.
#[allow(dead_code)]
fn aggregate_verify_nostd(
    sig: &P2Affine,
    sig_groupcheck: bool,
    msgs: &[&[u8]],
    dst: &[u8],
    pks: &[P1Affine],
    pks_validate: bool,
) -> blst::BLST_ERROR {
    if sig_groupcheck && !sig.is_in_group() {
        return blst::BLST_ERROR::BLST_VERIFY_FAIL;
    }

    let mut acc = Option::<blst::Pairing>::None;
    for (msg, pk) in msgs.iter().zip(pks.iter()) {
        let mut pairing = blst::Pairing::new(true, dst);
        if pairing.aggregate(
            &pk.point,
            pks_validate,
            &unsafe { core::ptr::null::<blst::blst_p2_affine>().as_ref() },
            false,
            msg,
            &[],
        ) != blst::BLST_ERROR::BLST_SUCCESS
        {
            return blst::BLST_ERROR::BLST_VERIFY_FAIL;
        }
        pairing.commit();
        if let Some(acc) = acc.as_mut() {
            acc.merge(&pairing);
        } else {
            acc = Some(pairing);
        }
    }

    let mut gtsig = blst::blst_fp12::default();
    blst::Pairing::aggregated(&mut gtsig, &sig.point);

    if acc.map(|acc| acc.finalverify(Some(&gtsig))).unwrap_or(false) {
        blst::BLST_ERROR::BLST_SUCCESS
    } else {
        blst::BLST_ERROR::BLST_VERIFY_FAIL
    }
}

pub fn aggregate_public_keys_based_on_nodes_info(
    bls_pks_bytes: &[&[u8; BLS_PUBLIC_KEY_LEN]],
    nodes_info_bytes: &[u8],
) -> Result<[u8; BLS_PUBLIC_KEY_LEN]> {
    if bls_pks_bytes.is_empty() {
        fail!("Vector of public keys can not be empty!");
    }
    let nodes_info = NodesInfo::deserialize(nodes_info_bytes)?;
    if bls_pks_bytes.len() != nodes_info.total_num_of_nodes as usize {
        fail!("Vector of public keys is too short!");
    }
    let mut apk_pks_required_refs: Vec<&[u8; BLS_PUBLIC_KEY_LEN]> = Vec::new();
    for (index, number_of_occurrence) in &nodes_info.map {
        for _i in 0..*number_of_occurrence {
            apk_pks_required_refs.push(bls_pks_bytes[*index as usize]);
        }
    }
    // let now = Instant::now();
    let result = aggregate_public_keys(&apk_pks_required_refs)?;
    // let duration = now.elapsed();

    // println!("Time elapsed by !!!aggregate_public_keys is: {:?}", duration);
    Ok(result)
}

pub fn aggregate_two_bls_signatures(
    sig_bytes_with_nodes_info_1: &[u8],
    sig_bytes_with_nodes_info_2: &[u8],
) -> Result<Vec<u8>> {
    let bls_sig_1 = BlsSignature::deserialize(sig_bytes_with_nodes_info_1)?;
    let bls_sig_2 = BlsSignature::deserialize(sig_bytes_with_nodes_info_2)?;
    let new_nodes_info = NodesInfo::merge(&bls_sig_1.nodes_info, &bls_sig_2.nodes_info)?;
    let sig1 = convert_signature_bytes_to_signature(&bls_sig_1.sig_bytes)?;
    let sig2 = convert_signature_bytes_to_signature(&bls_sig_2.sig_bytes)?;
    let sig_validate_res = sig1.validate(false); //set true to exclude infinite point, i.e. zero sig
    if sig_validate_res.is_err() {
        fail!("Signature is not in group.");
    }
    let mut agg_sig = blst::min_pk::AggregateSignature::from_signature(&sig1);
    let res = blst::min_pk::AggregateSignature::add_signature(&mut agg_sig, &sig2, true);
    if res.is_err() {
        fail!("Failure while concatenate signatures");
    }
    let new_sig = agg_sig.to_signature();
    let new_agg_sig = BlsSignature { sig_bytes: new_sig.to_bytes(), nodes_info: new_nodes_info };
    let new_agg_sig_bytes = BlsSignature::serialize(&new_agg_sig);
    Ok(new_agg_sig_bytes)
}

pub fn aggregate_bls_signatures(sig_bytes_with_nodes_info_vec: &[&[u8]]) -> Result<Vec<u8>> {
    if sig_bytes_with_nodes_info_vec.is_empty() {
        fail!("Vector of signatures can not be empty!");
    }
    let mut bls_sigs: Vec<BlsSignature> = Vec::new();
    for bytes in sig_bytes_with_nodes_info_vec {
        let agg_sig = BlsSignature::deserialize(bytes)?;
        bls_sigs.push(agg_sig);
    }

    let bls_sigs_refs: Vec<&BlsSignature> = bls_sigs.iter().collect();
    let mut nodes_info_refs: Vec<&NodesInfo> = Vec::new();
    let mut sigs: Vec<blst::min_pk::Signature> = Vec::new();
    for sign in bls_sigs_refs {
        nodes_info_refs.push(&sign.nodes_info);
        let sig = convert_signature_bytes_to_signature(&sign.sig_bytes)?;
        // println!("{:?}", &sig.to_bytes());
        //return this part to exclude zero sig
        /* let res = sig.validate(true);
        if res.is_err() {
            fail!("Sig is point of infinity or does not belong to group.");
        }*/
        sigs.push(sig);
    }

    let new_nodes_info = NodesInfo::merge_multiple(&nodes_info_refs)?;

    let sig_refs: Vec<&blst::min_pk::Signature> = sigs.iter().collect();

    let agg = match blst::min_pk::AggregateSignature::aggregate(&sig_refs, true) {
        Ok(agg) => agg,
        Err(err) => fail!("aggregate failure: {:?}", err),
    };
    let new_sig = agg.to_signature();
    let new_sig_bytes = convert_signature_to_signature_bytes(new_sig);
    let new_agg_sig = BlsSignature { sig_bytes: new_sig_bytes, nodes_info: new_nodes_info };
    let new_agg_sig_bytes = BlsSignature::serialize(&new_agg_sig);
    Ok(new_agg_sig_bytes)
}

pub fn aggregate_pure_bls_signatures(
    sigs_bytes: &[&[u8; BLS_SIG_LEN]],
) -> Result<[u8; BLS_SIG_LEN]> {
    if sigs_bytes.is_empty() {
        fail!("Slice with signatures can not be empty!");
    }
    let sig = convert_signature_bytes_to_signature(sigs_bytes[0])?;
    let mut agg_sig = blst::min_pk::AggregateSignature::from_signature(&sig);
    for sig_bytes in sigs_bytes.iter().skip(1) {
        let sig = convert_signature_bytes_to_signature(sig_bytes)?;
        let res = blst::min_pk::AggregateSignature::add_signature(&mut agg_sig, &sig, true);
        if res.is_err() {
            fail!("Failure while aggregating signatures");
        }
    }
    Ok(agg_sig.to_signature().to_bytes())
}

/*
    Converter
*/

pub fn convert_secret_key_bytes_to_secret_key(
    sk_bytes: &[u8; BLS_SECRET_KEY_LEN],
) -> Result<blst::min_pk::SecretKey> {
    let sk = match blst::min_pk::SecretKey::from_bytes(sk_bytes) {
        Ok(sk) => sk,
        Err(err) => fail!("BLS secret key deserialize failure: {:?}", err),
    };
    Ok(sk)
}

pub fn convert_signature_bytes_to_signature(
    sig_bytes: &[u8; BLS_SIG_LEN],
) -> Result<blst::min_pk::Signature> {
    let sig = match blst::min_pk::Signature::from_bytes(sig_bytes) {
        Ok(sig) => sig,
        Err(err) => fail!("BLS signature deserialize failure: {:?}", err),
    };
    Ok(sig)
}

pub fn convert_public_key_bytes_to_public_key(
    pk_bytes: &[u8; BLS_PUBLIC_KEY_LEN],
) -> Result<blst::min_pk::PublicKey> {
    let pk = match blst::min_pk::PublicKey::from_bytes(pk_bytes) {
        Ok(pk) => pk,
        Err(err) => fail!("BLS public key deserialize failure: {:?}", err),
    };
    Ok(pk)
}

pub fn convert_signature_to_signature_bytes(sig: blst::min_pk::Signature) -> [u8; BLS_SIG_LEN] {
    sig.to_bytes()
}

/*
    Keygen
*/

pub struct KeyPair {
    pub sk: blst::min_pk::SecretKey,
    pub pk: blst::min_pk::PublicKey,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlsKeyPair {
    pub pk_bytes: [u8; BLS_PUBLIC_KEY_LEN],
    pub sk_bytes: [u8; BLS_SECRET_KEY_LEN],
}

impl BlsKeyPair {
    pub fn print_bls_public_key(bls_pk_bytes: &[u8]) {
        if bls_pk_bytes.len() != BLS_PUBLIC_KEY_LEN {
            panic!("Incorrect length of secret key byte array!")
        }
        println!("--------------------------------------------------");
        println!("Aggregated BLS public key");
        println!("--------------------------------------------------");
        println!("Public key bytes:");
        println!("{:?}", bls_pk_bytes);
        println!("--------------------------------------------------");
    }

    pub fn print(&self) {
        println!("--------------------------------------------------");
        println!("BLS key pair:");
        println!("--------------------------------------------------");
        println!("Secret key bytes:");
        println!("{:?}", &self.sk_bytes);
        println!("Secret key len: {}", &self.sk_bytes.len());
        println!("Public key bytes:");
        println!("{:?}", &self.pk_bytes);
        println!("Public key len: {}", &self.pk_bytes.len());
        println!("--------------------------------------------------");
    }

    pub fn serialize(&self) -> ([u8; BLS_PUBLIC_KEY_LEN], [u8; BLS_SECRET_KEY_LEN]) {
        (self.pk_bytes, self.sk_bytes)
    }

    pub fn deserialize(
        key_pair_data: &([u8; BLS_PUBLIC_KEY_LEN], [u8; BLS_SECRET_KEY_LEN]),
    ) -> Result<Self> {
        let sk = convert_secret_key_bytes_to_secret_key(&key_pair_data.1)?;
        let pk = sk.sk_to_pk();
        if key_pair_data.0 != pk.to_bytes() {
            fail!("Public key does not correspond to secret key!")
        }
        Ok(Self { pk_bytes: key_pair_data.0, sk_bytes: key_pair_data.1 })
    }

    pub fn deserialize_based_on_secret_key(sk_bytes: &[u8; BLS_SECRET_KEY_LEN]) -> Result<Self> {
        let sk = convert_secret_key_bytes_to_secret_key(sk_bytes)?;
        let pk = sk.sk_to_pk();
        Ok(Self { pk_bytes: pk.to_bytes(), sk_bytes: sk.to_bytes() })
    }

    pub fn gen_bls_key_pair_based_on_key_material(
        ikm: &[u8; BLS_KEY_MATERIAL_LEN],
    ) -> Result<Self> {
        let key_pair = BlsKeyPair::gen_key_pair_based_on_key_material(ikm)?;
        Ok(BlsKeyPair::convert_key_pair_to_bls_key_pair(key_pair))
    }

    pub fn gen_bls_key_pair() -> Result<Self> {
        let key_pair = BlsKeyPair::gen_key_pair()?;
        Ok(BlsKeyPair::convert_key_pair_to_bls_key_pair(key_pair))
    }

    pub fn gen_key_pair() -> Result<KeyPair> {
        let mut ikm = [0u8; BLS_KEY_MATERIAL_LEN];
        rand::thread_rng().fill_bytes(&mut ikm);
        BlsKeyPair::gen_key_pair_based_on_key_material(&ikm)
    }

    pub fn gen_key_pair_based_on_key_material(ikm: &[u8; BLS_KEY_MATERIAL_LEN]) -> Result<KeyPair> {
        if ikm.len() != BLS_KEY_MATERIAL_LEN {
            fail!("Incorrect length of key material byte array!")
        }
        if let Ok(sk) = blst::min_pk::SecretKey::key_gen(ikm, &[]) {
            let pk = sk.sk_to_pk();
            Ok(KeyPair { sk, pk })
        } else {
            fail!("Failed while generate key")
        }
    }

    fn convert_key_pair_to_bls_key_pair(key_pair: KeyPair) -> Self {
        BlsKeyPair { sk_bytes: key_pair.sk.to_bytes(), pk_bytes: key_pair.pk.to_bytes() }
    }
}

/*
    NodesInfo
*/

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NodesInfo {
    pub map: HashMap<u16, u16>,
    pub total_num_of_nodes: u16,
}

impl NodesInfo {
    pub fn create_node_info(total_num_of_nodes: u16, node_index: u16) -> Result<Self> {
        if total_num_of_nodes == 0 {
            fail!("Total number of nodes can not be zero!");
        }
        if node_index >= total_num_of_nodes {
            fail!("Index of node can not be greater than total number of nodes!");
        }
        let mut info = HashMap::new();
        let num_of_occurrences = 1;
        info.insert(node_index, num_of_occurrences);
        Ok(Self { map: info, total_num_of_nodes })
    }

    pub fn with_data(info: HashMap<u16, u16>, total_num_of_nodes: u16) -> Result<Self> {
        if total_num_of_nodes == 0 {
            fail!("Total number of nodes can not be zero!");
        }
        if info.is_empty() {
            fail!("Node info should not be empty!")
        }
        for (index, number_of_occurrence) in &info {
            if *index >= total_num_of_nodes {
                fail!("Index of node can not be greater than total number of nodes!")
            }
            if *number_of_occurrence == 0 {
                fail!("Number of occurrence for node can not be zero!")
            }
        }
        let nodes_info = NodesInfo { map: info, total_num_of_nodes };
        Ok(nodes_info)
    }

    pub fn print(&self) {
        println!("--------------------------------------------------");
        println!("Total number of nodes: {}", &self.total_num_of_nodes);
        println!("Indexes -- occurrences: ");
        for (index, number_of_occurrence) in &self.map {
            println!("{}: \"{}\"", index, number_of_occurrence);
        }
        println!("--------------------------------------------------");
        println!("--------------------------------------------------");
    }

    pub fn merge(info1: &NodesInfo, info2: &NodesInfo) -> Result<NodesInfo> {
        if info1.total_num_of_nodes != info2.total_num_of_nodes {
            fail!("Total number of nodes must be the same!");
        }
        let mut new_info = info1.map.clone();
        for (index, number_of_occurrence) in &info2.map {
            new_info.insert(
                *index,
                if new_info.contains_key(index) {
                    new_info[index] + *number_of_occurrence
                } else {
                    *number_of_occurrence
                },
            );
        }
        Ok(NodesInfo { map: new_info, total_num_of_nodes: info1.total_num_of_nodes })
    }

    pub fn merge_multiple(info_vec: &[&NodesInfo]) -> Result<NodesInfo> {
        if info_vec.len() <= 1 {
            fail!("Nodes info collection must have at least two elements!!")
        }
        let mut final_nodes_info = NodesInfo::merge(info_vec[0], info_vec[1])?;
        for info in info_vec.iter().skip(2) {
            final_nodes_info = NodesInfo::merge(&final_nodes_info, info)?;
        }
        Ok(final_nodes_info)
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut result_vec = Vec::new();
        let total_num_of_nodes = &self.total_num_of_nodes;
        let total_num_of_nodes_bytes = total_num_of_nodes.to_be_bytes();
        result_vec.extend_from_slice(&total_num_of_nodes_bytes);
        for (index, number_of_occurrence) in &self.map {
            let index_bytes = index.to_be_bytes();
            result_vec.extend_from_slice(&index_bytes);
            let number_of_occurrence_bytes = number_of_occurrence.to_be_bytes();
            result_vec.extend_from_slice(&number_of_occurrence_bytes);
        }
        result_vec
    }

    pub fn deserialize(info_bytes: &[u8]) -> Result<NodesInfo> {
        if info_bytes.len() <= 2 || (info_bytes.len() % 4) != 2 {
            fail!("node_info_bytes must have non zero length (> 2) being of form 4*k+2!");
        }
        let total_num_of_nodes = ((info_bytes[0] as u16) << 8) | info_bytes[1] as u16;
        if total_num_of_nodes == 0 {
            fail!("Total number of nodes can not be zero!");
        }
        let mut new_info = HashMap::new();
        for i in (2..info_bytes.len()).step_by(4) {
            let index = ((info_bytes[i] as u16) << 8) | info_bytes[i + 1] as u16;
            if index >= total_num_of_nodes {
                fail!("Index can not be greater than total_num_of_nodes!");
            }
            let number_of_occurrence = ((info_bytes[i + 2] as u16) << 8) | info_bytes[i + 3] as u16;
            new_info.insert(index, number_of_occurrence);
        }

        NodesInfo::with_data(new_info, total_num_of_nodes)
    }
}

/*
    Random helpers
*/

pub fn generate_random_msg() -> Vec<u8> {
    let msg_len = rand::thread_rng().gen_range(2..100);
    let mut msg = vec![0u8; msg_len as usize];
    rand::thread_rng().fill_bytes(&mut msg);
    msg
}

pub fn generate_random_msg_of_fixed_len(msg_len: i32) -> Vec<u8> {
    let mut msg = vec![0u8; msg_len as usize];
    rand::thread_rng().fill_bytes(&mut msg);
    msg
}

pub fn gen_signer_indexes(n: u16, k: u16) -> Vec<u16> {
    let mut rng = rand::thread_rng();

    loop {
        let mut indexes = Vec::new();

        for _i in 0..k {
            indexes.push(rng.gen_range(0..n));
        }

        if indexes.len() == (k as usize) {
            return indexes;
        }
    }
}

pub fn gen_random_index(n: u16) -> u16 {
    rand::thread_rng().gen_range(0..n)
}

pub fn create_random_nodes_info(total_num_of_nodes: u16, attempts: u16) -> NodesInfo {
    let indexes: Vec<u16> = gen_signer_indexes(total_num_of_nodes, attempts);
    let mut node_info_vec = Vec::new();
    for ind in &indexes {
        let nodes_info = NodesInfo::create_node_info(total_num_of_nodes, *ind).unwrap();
        node_info_vec.push(nodes_info)
    }
    let node_info_vec_refs: Vec<&NodesInfo> = node_info_vec.iter().collect();
    NodesInfo::merge_multiple(&node_info_vec_refs).unwrap()
}

/*
    Signing
*/

pub const DST: [u8; 43] = *b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlsSignature {
    pub sig_bytes: [u8; BLS_SIG_LEN],
    pub nodes_info: NodesInfo,
}

impl BlsSignature {
    pub fn serialize(&self) -> Vec<u8> {
        let mut vec = Vec::new();
        vec.extend_from_slice(&self.sig_bytes);
        let nodes_info_bytes = &self.nodes_info.serialize();
        vec.extend_from_slice(nodes_info_bytes);
        vec
    }

    pub fn deserialize(sig_bytes_with_nodes_info: &[u8]) -> Result<Self> {
        if sig_bytes_with_nodes_info.len() < BLS_SIG_LEN + 6 {
            fail!("Length of sig_bytes_with_nodes_info is too short!")
        }
        let mut sig_bytes: [u8; BLS_SIG_LEN] = [0; BLS_SIG_LEN];
        sig_bytes.copy_from_slice(&sig_bytes_with_nodes_info[0..BLS_SIG_LEN]);
        let len = sig_bytes_with_nodes_info.len() - BLS_SIG_LEN;
        let mut nodes_info_data = vec![0; len];
        nodes_info_data.copy_from_slice(&sig_bytes_with_nodes_info[BLS_SIG_LEN..]);
        let nodes_info = NodesInfo::deserialize(&nodes_info_data)?;
        Ok(Self { sig_bytes, nodes_info })
    }

    pub fn simple_sign(
        sk_bytes: &[u8; BLS_SECRET_KEY_LEN],
        msg: &[u8],
    ) -> Result<[u8; BLS_SIG_LEN]> {
        if msg.is_empty() {
            fail!("Msg to sign can not be empty!")
        }
        let sk = convert_secret_key_bytes_to_secret_key(sk_bytes)?;
        let sig = sk.sign(msg, &DST, &[]);
        Ok(sig.to_bytes())
    }

    pub fn simple_verify(
        sig_bytes: &[u8; BLS_SIG_LEN],
        msg: &[u8],
        pk_bytes: &[u8; BLS_PUBLIC_KEY_LEN],
    ) -> Result<bool> {
        if msg.is_empty() {
            fail!("Msg to sign can not be empty!")
        }
        let sig = convert_signature_bytes_to_signature(sig_bytes)?;
        let pk = convert_public_key_bytes_to_public_key(pk_bytes)?;
        let res = sig.verify(true, msg, &DST, &[], &pk, true);
        Ok(res == blst::BLST_ERROR::BLST_SUCCESS)
    }

    pub fn add_node_info_to_sig(
        sig_bytes: [u8; BLS_SIG_LEN],
        node_index: u16,
        total_num_of_nodes: u16,
    ) -> Result<Vec<u8>> {
        if total_num_of_nodes == 0 {
            fail!("Total number of nodes can not be zero!");
        }
        if node_index >= total_num_of_nodes {
            fail!("Index of node can not be greater than total number of nodes!");
        }
        let nodes_info = NodesInfo::create_node_info(total_num_of_nodes, node_index)?;
        let sig = Self { sig_bytes, nodes_info };
        let sig_bytes = BlsSignature::serialize(&sig);
        Ok(sig_bytes)
    }

    pub fn sign(
        sk_bytes: &[u8; BLS_SECRET_KEY_LEN],
        msg: &[u8],
        node_index: u16,
        total_num_of_nodes: u16,
    ) -> Result<Vec<u8>> {
        let sig = BlsSignature::simple_sign(sk_bytes, msg)?;
        add_node_info_to_sig(sig, node_index, total_num_of_nodes)
    }

    pub fn get_nodes_info_from_sig(sig_bytes_with_nodes_info: &[u8]) -> Result<Vec<u8>> {
        let bls_sig = BlsSignature::deserialize(sig_bytes_with_nodes_info)?;
        Ok(bls_sig.nodes_info.serialize())
    }

    pub fn truncate_nodes_info_from_sig(
        sig_bytes_with_nodes_info: &[u8],
    ) -> Result<[u8; BLS_SIG_LEN]> {
        let bls_sig = BlsSignature::deserialize(sig_bytes_with_nodes_info)?;
        Ok(bls_sig.sig_bytes)
    }

    pub fn verify(
        sig_bytes_with_nodes_info: &[u8],
        pk_bytes: &[u8; BLS_PUBLIC_KEY_LEN],
        msg: &[u8],
    ) -> Result<bool> {
        let sig_bytes = BlsSignature::truncate_nodes_info_from_sig(sig_bytes_with_nodes_info)?;
        let res = BlsSignature::simple_verify(&sig_bytes, msg, pk_bytes)?;
        Ok(res)
    }

    pub fn print_signature_bytes(sig_bytes: &[u8]) {
        if sig_bytes.len() != BLS_SIG_LEN {
            panic!("Incorrect length of signature byte array!")
        }
        println!("--------------------------------------------------");
        println!("BLS Signature bytes:");
        println!("--------------------------------------------------");
        println!("{:?}", sig_bytes);
        println!("--------------------------------------------------");
    }

    pub fn print_bls_signature(bls_sig_bytes: &[u8]) {
        let bls_sig = BlsSignature::deserialize(bls_sig_bytes).unwrap();
        bls_sig.print();
    }

    pub fn print(&self) {
        println!("--------------------------------------------------");
        println!("Aggregated BLS signature:");
        println!("--------------------------------------------------");
        println!("Signature bytes:");
        println!("{:?}", &self.sig_bytes);
        self.nodes_info.print();
        println!("--------------------------------------------------");
    }
}

/*
Fields G1 G2 arithmetic
*/

#[derive(Debug, Default, Copy, Clone)]
struct P1 {
    point: blst::blst_p1,
}

impl P1 {
    pub fn new(data: &[u8; BLS_G1_LEN]) -> Result<Self> {
        let a = P1Affine::new(data)?;
        let mut point = blst::blst_p1::default();
        unsafe {
            blst::blst_p1_from_affine(&mut point, &a.point);
        }
        Ok(Self { point })
    }

    pub fn aggregate(&mut self, with: &P1Affine) -> Result<&mut Self> {
        unsafe {
            if !blst::blst_p1_affine_in_g1(&with.point) {
                fail!("Point is not in G1")
            }
            let mut aggregated = blst::blst_p1::default();
            blst::blst_p1_add_or_double_affine(&mut aggregated, &self.point, &with.point);
            self.point = aggregated;
            Ok(self)
        }
    }

    pub fn compress(&self) -> [u8; BLS_G1_LEN] {
        let mut data = [0u8; BLS_G1_LEN];
        unsafe {
            blst::blst_p1_compress(data.as_mut_ptr(), &self.point);
        }
        data
    }

    pub fn neg(&mut self) -> Result<&mut Self> {
        unsafe {
            blst::blst_p1_cneg(&mut self.point, true);
        }
        Ok(self)
    }

    pub fn mul(&mut self, with: &[u8; BLS_SCALAR_LEN]) -> Result<&mut Self> {
        if with.iter().all(|&x| x == 0) {
            *self = Self::default();
        } else {
            unsafe {
                let mut scalar = blst::blst_scalar::default();
                blst::blst_scalar_from_bendian(&mut scalar, with.as_slice().as_ptr());
                blst::blst_p1_mult(
                    &mut self.point,
                    &self.point,
                    scalar.b.as_ptr(),
                    BLS_SCALAR_LEN * 8,
                );
            }
        }
        Ok(self)
    }

    pub fn is_in_group(&self) -> bool {
        unsafe { blst::blst_p1_in_g1(&self.point) }
    }
}

struct P1Affine {
    point: blst::blst_p1_affine,
}

impl P1Affine {
    pub fn new(data: &[u8; BLS_G1_LEN]) -> Result<Self> {
        let mut point = blst::blst_p1_affine::default();
        unsafe {
            match blst::blst_p1_deserialize(&mut point, data.as_ptr()) {
                blst::BLST_ERROR::BLST_SUCCESS => (),
                err => fail!("Failed to deserialize point (erorr code {})", err as u32),
            }
        }
        Ok(Self { point })
    }
}

// a + b
pub fn g1_add(a: &[u8; BLS_G1_LEN], b: &[u8; BLS_G1_LEN]) -> Result<[u8; BLS_G1_LEN]> {
    Ok(P1::new(a)?.aggregate(&P1Affine::new(b)?)?.compress())
}

// a - b = -b + a
pub fn g1_sub(a: &[u8; BLS_G1_LEN], b: &[u8; BLS_G1_LEN]) -> Result<[u8; BLS_G1_LEN]> {
    Ok(P1::new(b)?.neg()?.aggregate(&P1Affine::new(a)?)?.compress())
}

// -a
pub fn g1_neg(a: &[u8; BLS_G1_LEN]) -> Result<[u8; BLS_G1_LEN]> {
    Ok(P1::new(a)?.neg()?.compress())
}

pub fn map_to_g1(a: &[u8; BLS_G1_LEN]) -> [u8; BLS_G1_LEN] {
    unsafe {
        let mut fp = blst::blst_fp::default();
        blst::blst_fp_from_bendian(&mut fp, a.as_ptr());

        let mut p1 = blst::blst_p1::default();
        blst::blst_map_to_g1(&mut p1, &fp, std::ptr::null());

        let mut result = [0u8; BLS_G1_LEN];
        blst::blst_p1_compress(result.as_mut_ptr(), &p1);

        result
    }
}

// a * n
pub fn g1_mul(a: &[u8; BLS_G1_LEN], n: &[u8; BLS_SCALAR_LEN]) -> Result<[u8; BLS_G1_LEN]> {
    Ok(P1::new(a)?.mul(n)?.compress())
}

pub fn g1_multiexp(
    points: &[&[u8; BLS_G1_LEN]],
    scalars: &[&[u8; BLS_SCALAR_LEN]],
) -> Result<[u8; BLS_G1_LEN]> {
    if points.len() != scalars.len() {
        fail!("Points and scalars must have the same length!");
    }
    if points.is_empty() {
        return Ok(g1_zero());
    }
    let mut affine_points = Vec::with_capacity(points.len());
    for point in points {
        affine_points.push(P1Affine::new(point)?.point);
    }
    let sz = unsafe { blst::blst_p1s_mult_pippenger_scratch_sizeof(points.len()) };
    let mut scratch = vec![0 as blst::limb_t; sz / std::mem::size_of::<blst::limb_t>()];
    let points_ptrs =
        affine_points.iter().map(|p| p as *const blst::blst_p1_affine).collect::<Vec<_>>();
    let scalar_ptrs = scalars.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
    let mut ret = blst::blst_p1::default();
    unsafe {
        blst::blst_p1s_mult_pippenger(
            &mut ret,
            points_ptrs.as_ptr(),
            points.len(),
            scalar_ptrs.as_ptr(),
            BLS_SCALAR_LEN * 8,
            scratch.as_mut_ptr(),
        )
    }
    Ok(P1 { point: ret }.compress())
}

pub fn g1_zero() -> [u8; BLS_G1_LEN] {
    P1::default().compress()
}

pub fn g1_in_group(a: &[u8; BLS_G1_LEN]) -> bool {
    P1::new(a).is_ok_and(|p| p.is_in_group())
}

#[derive(Debug, Default, Copy, Clone)]
struct P2 {
    point: blst::blst_p2,
}

impl P2 {
    pub fn new(data: &[u8; BLS_G2_LEN]) -> Result<Self> {
        let a = P2Affine::new(data)?;
        let mut point = blst::blst_p2::default();
        unsafe {
            blst::blst_p2_from_affine(&mut point, &a.point);
        }
        Ok(Self { point })
    }

    pub fn aggregate(&mut self, with: &P2Affine) -> Result<&mut Self> {
        unsafe {
            if !blst::blst_p2_affine_in_g2(&with.point) {
                fail!("Point is not in G2")
            }
            let mut aggregated = blst::blst_p2::default();
            blst::blst_p2_add_or_double_affine(&mut aggregated, &self.point, &with.point);
            self.point = aggregated;
            Ok(self)
        }
    }

    pub fn compress(&self) -> [u8; BLS_G2_LEN] {
        let mut data = [0u8; BLS_G2_LEN];
        unsafe {
            blst::blst_p2_compress(data.as_mut_ptr(), &self.point);
        }
        data
    }

    pub fn neg(&mut self) -> Result<&mut Self> {
        unsafe {
            blst::blst_p2_cneg(&mut self.point, true);
        }
        Ok(self)
    }

    pub fn mul(&mut self, with: &[u8; BLS_SCALAR_LEN]) -> Result<&mut Self> {
        if with.iter().all(|&x| x == 0) {
            *self = Self::default();
        } else {
            unsafe {
                let mut scalar = blst::blst_scalar::default();
                blst::blst_scalar_from_bendian(&mut scalar, with.as_slice().as_ptr());
                blst::blst_p2_mult(
                    &mut self.point,
                    &self.point,
                    scalar.b.as_ptr(),
                    BLS_SCALAR_LEN * 8,
                );
            }
        }
        Ok(self)
    }

    pub fn is_in_group(&self) -> bool {
        unsafe { blst::blst_p2_in_g2(&self.point) }
    }
}

struct P2Affine {
    point: blst::blst_p2_affine,
}

impl P2Affine {
    pub fn new(data: &[u8; BLS_G2_LEN]) -> Result<Self> {
        let mut point = blst::blst_p2_affine::default();
        unsafe {
            match blst::blst_p2_deserialize(&mut point, data.as_ptr()) {
                blst::BLST_ERROR::BLST_SUCCESS => (),
                err => fail!("Failed to deserialize point (erorr code {})", err as u32),
            }
        }
        Ok(Self { point })
    }
    pub fn is_in_group(&self) -> bool {
        unsafe { blst::blst_p2_affine_in_g2(&self.point) }
    }
}

// a + b
pub fn g2_add(a: &[u8; BLS_G2_LEN], b: &[u8; BLS_G2_LEN]) -> Result<[u8; BLS_G2_LEN]> {
    Ok(P2::new(a)?.aggregate(&P2Affine::new(b)?)?.compress())
}

// a - b = -b + a
pub fn g2_sub(a: &[u8; BLS_G2_LEN], b: &[u8; BLS_G2_LEN]) -> Result<[u8; BLS_G2_LEN]> {
    Ok(P2::new(b)?.neg()?.aggregate(&P2Affine::new(a)?)?.compress())
}

// -a
pub fn g2_neg(a: &[u8; BLS_G2_LEN]) -> Result<[u8; BLS_G2_LEN]> {
    Ok(P2::new(a)?.neg()?.compress())
}

pub fn map_to_g2(a: &[u8; BLS_G2_LEN]) -> [u8; BLS_G2_LEN] {
    unsafe {
        let mut fp = blst::blst_fp2::default();
        blst::blst_fp_from_bendian(&mut fp.fp[0], a[..BLS_G1_LEN].as_ptr());
        blst::blst_fp_from_bendian(&mut fp.fp[1], a[BLS_G1_LEN..].as_ptr());

        let mut p2 = blst::blst_p2::default();
        blst::blst_map_to_g2(&mut p2, &fp, std::ptr::null());

        let mut result = [0u8; BLS_G2_LEN];
        blst::blst_p2_compress(result.as_mut_ptr(), &p2);

        result
    }
}

// a * n
pub fn g2_mul(a: &[u8; BLS_G2_LEN], n: &[u8; BLS_SCALAR_LEN]) -> Result<[u8; BLS_G2_LEN]> {
    Ok(P2::new(a)?.mul(n)?.compress())
}

pub fn g2_multiexp(
    points: &[&[u8; BLS_G2_LEN]],
    scalars: &[&[u8; BLS_SCALAR_LEN]],
) -> Result<[u8; BLS_G2_LEN]> {
    if points.len() != scalars.len() {
        fail!("Points and scalars must have the same length!");
    }
    if points.is_empty() {
        return Ok(g2_zero());
    }
    let mut affine_points = Vec::with_capacity(points.len());
    for point in points {
        affine_points.push(P2Affine::new(point)?.point);
    }
    let sz = unsafe { blst::blst_p2s_mult_pippenger_scratch_sizeof(points.len()) };
    let mut scratch = vec![0 as blst::limb_t; sz / std::mem::size_of::<blst::limb_t>()];
    let points_ptrs =
        affine_points.iter().map(|p| p as *const blst::blst_p2_affine).collect::<Vec<_>>();
    let scalar_ptrs = scalars.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
    let mut ret = blst::blst_p2::default();
    unsafe {
        blst::blst_p2s_mult_pippenger(
            &mut ret,
            points_ptrs.as_ptr(),
            points.len(),
            scalar_ptrs.as_ptr(),
            BLS_SCALAR_LEN * 8,
            scratch.as_mut_ptr(),
        )
    }
    Ok(P2 { point: ret }.compress())
}

pub fn g2_zero() -> [u8; BLS_G2_LEN] {
    P2::default().compress()
}

pub fn g2_in_group(a: &[u8; BLS_G2_LEN]) -> bool {
    P2::new(a).is_ok_and(|p| p.is_in_group())
}

pub fn pairing(x: &[&[u8; BLS_G1_LEN]], y: &[&[u8; BLS_G2_LEN]]) -> Result<bool> {
    const DST: [u8; 43] = *b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

    if x.len() != y.len() {
        fail!("Length of x and y must be the same!");
    }
    if x.is_empty() {
        return Ok(false);
    }
    unsafe {
        let mut pairing: Vec<u64> = vec![0; blst::blst_pairing_sizeof() / 8];
        let pairing_ptr = pairing.as_mut_ptr() as *mut blst::blst_pairing;
        blst::blst_pairing_init(pairing_ptr, true, DST.as_ptr(), DST.len());
        for (x, y) in x.iter().zip(y.iter()) {
            let xa = P1Affine::new(x)?;
            let ya = P2Affine::new(y)?;
            blst::blst_pairing_raw_aggregate(pairing_ptr, &ya.point, &xa.point);
        }
        blst::blst_pairing_commit(pairing_ptr);
        let res = blst::blst_pairing_finalverify(pairing_ptr, std::ptr::null());
        Ok(res)
    }
}

#[cfg(test)]
#[path = "tests/test_bls.rs"]
mod tests;
