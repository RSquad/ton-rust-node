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
use adnl::common::hash;
use std::{convert::TryInto, mem};
use ton_api::{
    deserialize_boxed,
    ton::{
        adnl::AddressList,
        dht::{key::Key as DhtKey, Node as DhtNode, ValueResult},
        pub_::publickey::Ed25519,
        rpc::dht::{FindValue, Store},
    },
    BoxedSerialize, IntoBoxed, Signing,
};
use ton_block::{base64_decode, base64_encode, Ed25519KeyOption, UInt256, ZeroizingBytes};

// TODO: will be refactored in ton_api
#[macro_export]
macro_rules! check_signature {
    ($value:expr, $public:ident) => {{
        let mut value = $value.clone();
        value.signature = Default::default();
        let data = value.into_boxed().boxed_serialized_bytes().unwrap();
        $public.verify(&data, &$value.signature).unwrap();
    }};
}

const NODE_PACKET: &str =
    "48325384c6b41348234479f44faca6e8a25f040af5bb1dafe71800b9b6bff1aa7ee168e8ef1dee790100\
     0000e7a60d67fc9712035f7600001000305e1000305e00000000000000000cd3325e402c62473f60f55a\
     534fb9d98d69507d9d6e178d3092692dbb4ea6df5140663baba57d22b4622bcd1e1a914e1d0375419944\
     1d5396bb21d59d172ee8af22bf4f0a000000";

const STORE_PACKET: &str =
    "124293345183afab3797d1f2881efcc0dab1362536b1cde232d2837d3b9451875df91e58076164647265\
     737300000000c6b4134854f739faf85978e18e220eeeaf98bcbb48e021b729e613a8971ecfa8a01893f1\
     f7319fcc407563ea5367b8e9be224948a4989427f8f221afc54289cc0663facdc650c20aac09260ab6aa\
     a44c387fe8a891610d129e4f4774a24b96b5c9ce14c9dce9eaf5040000002458e6272201000000e7a60d\
     67a5a585035f76000078dd2d5e86c02a5e00000000000000000000002462305e40e51646b2d5d6935458\
     38c50d10475d8c939ecfefc23abffff1198beedd7d45fd2dea451ef14507211dd864b1f728341c1c8ba4\
     56213349c93073eb28215a4302000000";

const FIND_VALUE1: &str =
    "11604baeb1e66593f1a898d0413b9d53dd2f2438d67f7ddefe3725b62023b1c5b5ba878606000000";
const FIND_VALUE2: &str =
    "11604bae21ec6dda1323710172d79b0a04162f9158c086505dbfe48997cee59ba730d1aa06000000";
const SOME: &str =
    "74f70ce4cb27ad90ff379c260213938c15fb383096f14fed42b86ef985253e76154ff5e909936ed7\
     076164647265737300000000c6b41348f3de543912ca8389a8bc9864703d0acd94d05fe933f63bc6\
     a1a93b5ea0920a7af7319fcc40411d147eb7a4db573cbe42d9b61c02894dba79fd71411b3bfa5d1e\
     1da83fc69dca3f54057625a0629ae34b037e82577c2fa4a621ad5da83585d92c36f1b0fa0f000000\
     2458e6272201000000e7a60d67b5f679035f760000c633305ec633305e0000000000000000000000\
     1462305e40fc47288a02a519372c1dbf54bb50afe78c1d9d0b0b295efdff69ebb7a2aced26a1b70f\
     3c025017a7ae746b309cc9be63d91feea09bd4602899a4194059262d0d000000";

fn get_node() -> DhtNode {
    let object = deserialize_boxed(hex::decode(NODE_PACKET).unwrap()).unwrap();
    let node = object.downcast::<DhtNode>().expect("It should be Node");
    println!("DHT {:?}", node);
    node
}

#[test]
fn test_real_dht_node_data() {
    let mut node = get_node().only();
    let public =
        Ed25519KeyOption::<ZeroizingBytes>::from_public_key(node.id.key().unwrap().as_slice());
    node.verify(&public).expect("signature must be verified ok");
}

#[test]
fn test_real_dht_store_data() {
    let data = hex::decode(STORE_PACKET).unwrap();
    let object = deserialize_boxed(data).unwrap();
    let mut value = object.downcast::<Store>().expect("It should be Store").value;
    let public =
        Ed25519KeyOption::<ZeroizingBytes>::from_public_key(value.key.id.key().unwrap().as_slice());
    value.verify(&public).expect("signature must be verified ok");
    value.key.verify(&public).expect("signature must be verified ok");
    println!("DHT {:?}", value);
    println!("key id: {}", base64_encode(&value.key.key.id.as_slice()));
    println!("key name: {}", String::from_utf8(value.key.key.name.to_vec()).unwrap());
    let object = deserialize_boxed(&value.value).unwrap();
    let list = object.downcast::<AddressList>().expect("It should be AddressList");
    println!("DHT {:?}", list);
}

#[test]
fn test_check_signature() {
    let mut node = get_node().only();
    // use public key from deserialized node struct to check signature
    let mut pub_key = [0; 32];
    pub_key.copy_from_slice(node.id.key().unwrap().as_slice());
    let keypair = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&pub_key);
    let signature = mem::take(&mut node.signature);
    let bytes = node.into_boxed().boxed_serialized_bytes().unwrap();
    assert!(keypair.verify(&bytes, &signature).is_ok());
}

#[test]
fn test_sign_and_check_signature() {
    let mut node = get_node().only();
    let keypair = Ed25519KeyOption::<ZeroizingBytes>::generate().unwrap();
    let key = UInt256::with_array(keypair.pub_key().unwrap().try_into().unwrap());
    node.id = Ed25519 { key }.into_boxed();
    node.signature = Default::default();
    let bytes = node.into_boxed().boxed_serialized_bytes().unwrap();
    let signature = keypair.sign(&bytes).unwrap();
    assert!(keypair.verify(&bytes, &signature).is_ok());
}

fn calc_key_id(key: &str) {
    println!("\nKey {}", key);
    let key = base64_decode(key).unwrap();
    let key = Ed25519KeyOption::<ZeroizingBytes>::from_public_key(&key.try_into().unwrap());
    println!("KeyId {} {:x?}", key.id(), key.id());
}

fn calc_key_id_from_private(key: &str) {
    println!("\nKey {}", key);
    let key = base64_decode(key).unwrap();
    //let key = ed25519_dalek::SecretKey::from_bytes(&key).unwrap();
    let key =
        Ed25519KeyOption::<ZeroizingBytes>::from_private_key(key.as_slice().try_into().unwrap())
            .unwrap();
    println!("KeyId {} key {}", key.id(), base64_encode(key.pub_key().unwrap()));
}

#[test]
fn test_dump_key() {
    calc_key_id_from_private("0SYrN/FZ85YpXS20aLTyUpYkTIzZBwJr5nK33q2QLPg=");
    //bbHSGFhXMIaujRgAp9pg4IEbxK+cmXfd/ymEc9Njk3Y=
}

#[test]
fn test_find_value() {
    /*
        calc_key_id("Q+0dOGQxfjjeLJD1KPjK5V1JEJ1MmKW9zapcvRLex0w=");
        calc_key_id("+Mkkr97O8hGWsw9GEYh2Lbm92ygaghEqM2ub9uWD3GE=");
        calc_key_id("V3WS+xH1sYOk52VnQW87w+0vYfIMmFV0XB6UIC9lPFE=");
        calc_key_id("wJveqOc7aOLcQ6cV5DGJKkWVTDaMcTlbefJAVG4rLnA=");
    */

    // Node 11
    calc_key_id("lJLCrLA58RYeckE/mlfJQGwMJH2cllXw6w+sa4CaCvA=");
    calc_key_id("cZZ2WlfTUD5G6ZJ+am440zpYbdO3vl0vxvTJR27gRW8=");
    calc_key_id("InXE32CSqNNqQ4YPu59ALLQNfupJNhPq62LItv8szO0=");
    calc_key_id("yBuBmx+xdlYpSEl4zwxLZqekd/0jV0e9j5pzmH3+tyQ=");
    calc_key_id("mbH/sC0QbAS7UwnyCSg/8LntJGvySru7Q2n5Ba25wwQ=");
    calc_key_id("e+ruzfxAaT8LSjA8m+kL77bi/grSCVChe/gR+jvHlIo=");

    let keyid = base64_decode("/1dWjhvLrzCUH22y1AaheSxVP4zEtHI2/IRPXcy6xkA=").unwrap();
    //"8Y6kpZBSKTpY6+vmNknfyzGRQTITiFQNOQbu0J+r110=").unwrap();
    let key = DhtKey {
        id: UInt256::with_array(keyid.try_into().unwrap()),
        idx: 0,
        name: "address".as_bytes().to_vec().into(),
    };
    let hash = hash(key);
    println!("\nDhtKeyId {:x?}", hash);

    let data = hex::decode(SOME).unwrap();
    let some = deserialize_boxed(data).unwrap().downcast::<ValueResult>().unwrap();
    println!("\nSome #1 {:?}", some);

    let data = hex::decode(FIND_VALUE1).unwrap();
    let find_value = deserialize_boxed(data).unwrap().downcast::<FindValue>().unwrap();
    println!(
        "\nFindValue #1 {:?}, key {}",
        find_value,
        base64_encode(&find_value.key.as_slice()[..])
    );
    let data = hex::decode(FIND_VALUE2).unwrap();
    let find_value = deserialize_boxed(data).unwrap().downcast::<FindValue>().unwrap();
    println!(
        "FindValue #2 {:?}, key {}",
        find_value,
        base64_encode(&find_value.key.as_slice()[..])
    );
}
