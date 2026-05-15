/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;

#[test]
fn test_ed25519_signature() {
    let key = crate::Ed25519KeyOption::generate().unwrap();
    let data = [1, 2, 3, 4, 5];
    let s = key.sign(&data).unwrap();
    println!(
        "data {} pub key {}, signature {}",
        base64_encode(&data),
        base64_encode(key.pub_key().unwrap()),
        base64_encode(&s)
    );
}

#[test]
fn test_p256_signature() {
    use p256::ecdsa::signature::Signer;

    let msg = b"Hello, world!";
    println!("message: {}", hex::encode(msg));

    let secret_key = "f0243edb02866d956867947b003f2a73d2d698a7495b6d25d5a17e89ebd24484";
    let secret_key = hex::decode(secret_key).unwrap();
    let secret_key = p256::ecdsa::SigningKey::from_bytes(secret_key.as_slice().into()).unwrap();

    let public_key = p256::ecdsa::VerifyingKey::from(&secret_key);
    let encoded = public_key.to_encoded_point(true);
    let signature: p256::ecdsa::Signature = secret_key.sign(msg);

    println!("secret key: {}", hex::encode(secret_key.to_bytes()));
    println!("public key: {}", hex::encode(public_key.to_sec1_bytes()));
    println!("encoded public key: {}", hex::encode(encoded.as_bytes()));
    println!("signature: {}", hex::encode(signature.to_bytes()));

    let public_key =
        hex::decode("024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd").unwrap();
    let public_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key.as_slice()).unwrap();

    assert_eq!(
        hex::encode(public_key.to_sec1_bytes()),
        "044c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd299395343dea6a78f99d98294b877571e3cd6d16884c3e14dfb2edca59fcd948"
    );

    assert_eq!(
        hex::encode(encoded.as_bytes()),
        "024c0fb7f19c9879471f2eb3c57cc674d52483d8de673c566ccb6c13c17c16f3cd"
    );

    assert_eq!(
        hex::encode(signature.to_bytes()),
        "979e54eb9b3a942243effbd39ccdb309f4bd2973e77c31b92d04150c169e9e3597b666d6d2fd084c38e1eb1ed828e11c3e4fbdc74c18b0d466375e4304673be7"
    );

    let hash = sha256_digest(msg);
    println!("hash: {}", hex::encode(hash.as_slice()));
    let signature: p256::ecdsa::Signature = secret_key.sign(hash.as_slice());
    println!("signature: {}", hex::encode(signature.to_bytes()));

    public_key.verify(hash.as_slice(), &signature).unwrap();
}

#[test]
fn test_secp256k1_signature() {
    let msg = b"Hello, world!";
    let hash = sha256_digest(msg);
    assert_eq!(
        hex::encode(hash.as_slice()),
        "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
    );
    let msg = secp256k1::Message::from_digest(hash.into());
    let secret_key =
        hex::decode("f0243edb02866d956867947b003f2a73d2d698a7495b6d25d5a17e89ebd24484").unwrap();
    let secret_key = secp256k1::SecretKey::from_slice(&secret_key).unwrap();

    let signature = secret_key.sign_ecdsa(msg);
    let signature = signature.serialize_compact();
    assert_eq!(
        hex::encode(signature),
        "9f98fa24ca2128e412230d2679895cf93947a57a007583e5736660fff0aa860f1d5dabd0f349da42a2b0d26a0151b94317bd5eb53b2e9ed30e871252e4270f4d"
    );
    let public_keys = [
        "04951a1504d71666b700166cfdf5bafd29f7f0029d1fc76d84e3dd57832534aaa1873747a9230593f011126456cfabc2dc1b46346e3e6106046b094c7f354ddb5d",
        "042e96881915c8f5d99b1b5022e3e3e2863d083397c3f967a9e59ff951f32d76c7c8ee8fbc46cfaee597451e50fef89413a5376b53d28c07a992bbb1da5be1306a",
        "",
        "",
    ];
    for (recid, public_key) in public_keys.iter().enumerate() {
        let recid = secp256k1::ecdsa::RecoveryId::try_from(recid as i32).unwrap();
        let signature =
            secp256k1::ecdsa::RecoverableSignature::from_compact(&signature, recid).unwrap();
        if public_key.is_empty() {
            signature.recover(&msg).expect_err("should fail to recover");
        } else {
            assert_eq!(
                &hex::encode(signature.recover(&msg).unwrap().serialize_uncompressed()),
                public_key
            );
        }
    }
    let secp = secp256k1::Secp256k1::signing_only();
    let public_key = secret_key.public_key(&secp);
    assert_eq!(
        hex::encode(public_key.serialize_uncompressed()),
        "04951a1504d71666b700166cfdf5bafd29f7f0029d1fc76d84e3dd57832534aaa1873747a9230593f011126456cfabc2dc1b46346e3e6106046b094c7f354ddb5d"
    );
    let (xonly_pubkey, _parity) = public_key.x_only_public_key();
    assert_eq!(
        hex::encode(xonly_pubkey.serialize()),
        "951a1504d71666b700166cfdf5bafd29f7f0029d1fc76d84e3dd57832534aaa1"
    );
    let tweak = [0xA5; 32];
    assert_eq!(
        hex::encode(tweak),
        "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5"
    );
    let public_key = secp256k1_xonly_pubkey_tweak_add(&xonly_pubkey.serialize(), tweak).unwrap();
    assert_eq!(
        hex::encode(public_key),
        "048e68e631ac04cc14bd2988ffb20e840e7c2eb1b210cc8c900e6a810fcdb5b1ad54ed1ba2e3198b0f6a3bb7488b7213eb6e27830212be552e5be589e6fa50d105"
    );
    let tweak = secp256k1::Scalar::from_be_bytes(tweak).unwrap();
    let (xonly_pubkey, parity) = xonly_pubkey.add_tweak(&SECP256K1_VERIFY, &tweak).unwrap();
    assert_eq!(
        hex::encode(xonly_pubkey.serialize()),
        "8e68e631ac04cc14bd2988ffb20e840e7c2eb1b210cc8c900e6a810fcdb5b1ad"
    );
    assert_eq!(parity, secp256k1::Parity::Odd);
    let public_key = secp256k1::PublicKey::from_x_only_public_key(xonly_pubkey, parity);
    assert_eq!(
        hex::encode(public_key.serialize_uncompressed()),
        "048e68e631ac04cc14bd2988ffb20e840e7c2eb1b210cc8c900e6a810fcdb5b1ad54ed1ba2e3198b0f6a3bb7488b7213eb6e27830212be552e5be589e6fa50d105"
    );
}

#[test]
fn test_ristretto_255_from_hash() {
    let msg = "Ristretto points on curve";
    let hash = sha512_digest(msg.as_bytes());
    println!("{}", hex::encode(hash.as_slice()));
    let point = RistrettoPoint::from_uniform_bytes(hash.as_slice().try_into().unwrap());
    let compressed = point.compress().to_bytes();
    assert_eq!(ristretto_255_from_compressed(compressed), Some(point));
}

#[test]
fn test_lz4() {
    let data = b"Hello, world! This is a test for LZ4 compression and decompression.";

    let compressed = lz4_compress(data, true).unwrap();
    assert!(!compressed.is_empty());

    let decompressed = lz4_decompress(&compressed, Lz4DecompressMode::WithPrependedSize).unwrap();
    assert_eq!(decompressed, data);

    let compressed = lz4_compress(data, false).unwrap();
    assert!(!compressed.is_empty());

    let decompressed =
        lz4_decompress(&compressed, Lz4DecompressMode::WithMaxSize(data.len() as i32)).unwrap();
    assert_eq!(decompressed, data);

    let decompressed = lz4_decompress(&compressed, Lz4DecompressMode::WithMaxSize(16_000)).unwrap();
    assert_eq!(decompressed, data);

    let result = lz4_decompress(&compressed, Lz4DecompressMode::WithMaxSize(data.len() as i32 - 1));
    assert!(result.is_err());

    let invalid_data = b"Invalid compressed data";
    let result =
        lz4_decompress(invalid_data, Lz4DecompressMode::WithMaxSize(invalid_data.len() as i32));
    assert!(result.is_err());

    let empty_data = b"";
    let compressed = lz4_compress(empty_data, false).unwrap();
    assert!(!compressed.is_empty());

    let decompressed = lz4_decompress(&compressed, Lz4DecompressMode::WithMaxSize(0)).unwrap();
    assert_eq!(decompressed, empty_data);
}

#[test]
fn test_check_ton_method_id() {
    assert_eq!(ton_method_id("seqno"), 0x14C97);
}
