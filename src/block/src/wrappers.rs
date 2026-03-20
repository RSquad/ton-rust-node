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
use crate::{error, fail, Result};
use ctr::cipher::{KeyIvInit, StreamCipher};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use ed25519_dalek::{
    pkcs8::EncodePrivateKey, SecretKey, Signer, SigningKey, Verifier, VerifyingKey,
};
pub use ed25519_dalek::{
    PUBLIC_KEY_LENGTH as ED25519_PUBLIC_KEY_LENGTH, SECRET_KEY_LENGTH as ED25519_SECRET_KEY_LENGTH,
    SIGNATURE_LENGTH as ED25519_SIGNATURE_LENGTH,
};
use sha2::Digest;
use std::sync::LazyLock;
pub const P256_PUBLIC_KEY_LENGTH: usize = 33;
pub const P256_SIGNATURE_LENGTH: usize = 64;

// AES-CTR --------------------------------------------------------------

pub struct AesCtr {
    inner: ctr::Ctr128BE<aes::Aes256>,
}

impl AesCtr {
    pub fn with_params(key: &[u8], ctr: &[u8]) -> Self {
        Self {
            inner: ctr::Ctr128BE::<aes::Aes256>::new(
                aes::cipher::generic_array::GenericArray::from_slice(key),
                aes::cipher::generic_array::GenericArray::from_slice(ctr),
            ),
        }
    }

    pub fn apply_keystream(&mut self, buf: &mut [u8]) {
        self.inner.apply_keystream(buf)
    }
}

// Base-64 --------------------------------------------------------------

pub fn base64_decode(input: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    Ok(base64::decode(input)?)
}

pub fn base64_decode_url_safe(input: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    Ok(base64::decode_config(input, base64::URL_SAFE)?)
}

pub fn base64_decode_to_slice(input: impl AsRef<[u8]>, output: &mut [u8]) -> Result<()> {
    let config = base64::STANDARD;
    let result = base64::decode_config_slice(input, config, output)?;
    if output.len() != result {
        fail!("not enough bytes to decode only {}", result)
    }
    Ok(())
}

pub fn base64_encode(input: impl AsRef<[u8]>) -> String {
    base64::encode(input)
}

pub fn base64_encode_url_safe(input: impl AsRef<[u8]>) -> String {
    base64::encode_config(input, base64::URL_SAFE)
}

// Ed25519 --------------------------------------------------------------

pub struct Ed25519ExpandedPrivateKey {
    // Currently, ed25519_dalek::hazmat::ExpandedSecretKey can't be
    // converted back to bytes, so we have to have a raw slice here
    inner: [u8; 64],
}

impl Ed25519ExpandedPrivateKey {
    pub fn to_bytes(&self) -> [u8; 64] {
        self.inner
    }
}

pub struct Ed25519PrivateKey {
    inner: SecretKey,
}

impl Ed25519PrivateKey {
    pub fn to_bytes(&self) -> [u8; ED25519_SECRET_KEY_LENGTH] {
        self.inner
    }
    pub fn as_bytes(&self) -> &[u8; ED25519_SECRET_KEY_LENGTH] {
        &self.inner
    }
    pub fn sign(&self, data: &[u8]) -> [u8; ED25519_SIGNATURE_LENGTH] {
        let signing_key = SigningKey::from(&self.inner);
        signing_key.sign(data).to_bytes()
    }
    pub fn verifying_key(&self) -> [u8; ED25519_PUBLIC_KEY_LENGTH] {
        let signing_key = SigningKey::from(&self.inner);
        VerifyingKey::from(&signing_key).to_bytes()
    }
}

pub struct Ed25519PublicKey {
    inner: VerifyingKey,
}

impl Ed25519PublicKey {
    pub fn as_bytes(&self) -> &[u8; ED25519_PUBLIC_KEY_LENGTH] {
        self.inner.as_bytes()
    }
    pub fn to_bytes(&self) -> [u8; ED25519_PUBLIC_KEY_LENGTH] {
        self.inner.to_bytes()
    }
    pub fn from_bytes(bytes: &[u8; ED25519_PUBLIC_KEY_LENGTH]) -> Result<Self> {
        Ok(Self { inner: VerifyingKey::from_bytes(bytes)? })
    }
    pub fn verify(&self, data: &[u8], signature: &[u8; ED25519_SIGNATURE_LENGTH]) -> bool {
        self.inner.verify(data, &ed25519::Signature::from_bytes(signature)).is_ok()
    }
}

pub fn ed25519_create_expanded_private_key(src: &[u8]) -> Result<Ed25519ExpandedPrivateKey> {
    let ret = Ed25519ExpandedPrivateKey { inner: src.try_into()? };
    Ok(ret)
}

pub fn ed25519_create_private_key(src: &[u8]) -> Result<Ed25519PrivateKey> {
    let ret = Ed25519PrivateKey { inner: src.try_into()? };
    Ok(ret)
}

pub fn ed25519_create_public_key(src: &Ed25519ExpandedPrivateKey) -> Result<Ed25519PublicKey> {
    let exp_key = ed25519_dalek::hazmat::ExpandedSecretKey::from_bytes(&src.inner);
    let ret = Ed25519PublicKey { inner: VerifyingKey::from(&exp_key) };
    Ok(ret)
}

pub fn ed25519_encode_private_key_to_pkcs8(src: &[u8]) -> Result<Vec<u8>> {
    ed25519_dalek::SigningKey::from_bytes(src.try_into()?)
        .to_pkcs8_der()
        .map(|pkcs8| pkcs8.as_bytes().to_vec())
        .map_err(|e| error!("Cannot encode Ed25519 key to PKCS#8: {e}"))
}

pub fn ed25519_expand_private_key(src: &Ed25519PrivateKey) -> Result<Ed25519ExpandedPrivateKey> {
    let bytes = sha2::Sha512::default().chain_update(src.inner).finalize();
    let ret = Ed25519ExpandedPrivateKey { inner: bytes.into() };
    Ok(ret)
}

pub fn ed25519_generate_private_key() -> Result<Ed25519PrivateKey> {
    let ret = Ed25519PrivateKey { inner: SigningKey::generate(&mut rand::thread_rng()).to_bytes() };
    Ok(ret)
}

pub fn ed25519_sign_with_secret(
    secret_key: &[u8],
    data: &[u8],
) -> Result<[u8; ED25519_SIGNATURE_LENGTH]> {
    let signing_key = SigningKey::from_bytes(secret_key.try_into()?);
    Ok(signing_key.sign(data).to_bytes())
}

pub fn ed25519_sign(exp_pvt_key: &[u8], pub_key: Option<&[u8]>, data: &[u8]) -> Result<Vec<u8>> {
    let exp_key = ed25519_dalek::hazmat::ExpandedSecretKey::from_bytes(exp_pvt_key.try_into()?);
    let pub_key = if let Some(pub_key) = pub_key {
        VerifyingKey::from_bytes(pub_key.try_into()?)?
    } else {
        VerifyingKey::from(&exp_key)
    };
    Ok(ed25519_dalek::hazmat::raw_sign::<sha2::Sha512>(&exp_key, data, &pub_key).to_vec())
}

pub fn ed25519_verify(pub_key: &[u8], data: &[u8], signature: &[u8]) -> Result<()> {
    let pub_key = VerifyingKey::from_bytes(pub_key.try_into()?)?;
    pub_key.verify(data, &ed25519::Signature::from_bytes(signature.try_into()?))?;
    Ok(())
}

pub fn x25519_shared_secret(exp_pvt_key: &[u8], other_pub_key: &[u8]) -> Result<[u8; 32]> {
    let point = curve25519_dalek::edwards::CompressedEdwardsY(other_pub_key.try_into()?)
        .decompress()
        .ok_or_else(|| error!("Bad public key data"))?
        .to_montgomery()
        .to_bytes();
    Ok(x25519_dalek::x25519(exp_pvt_key[..32].try_into()?, point))
}

// P256 -----------------------------------------------------------------

pub fn p256_verify_signature(
    public_key: &[u8; P256_PUBLIC_KEY_LENGTH],
    data: &[u8],
    signature: &[u8; P256_SIGNATURE_LENGTH],
) -> Result<()> {
    let public_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key)?;
    let signature = p256::ecdsa::Signature::from_slice(signature)?;
    public_key.verify(data, &signature)?;
    Ok(())
}

// Secp256k1 ------------------------------------------------------------

static SECP256K1_VERIFY: LazyLock<secp256k1::Secp256k1<secp256k1::VerifyOnly>> =
    LazyLock::new(secp256k1::Secp256k1::verification_only);

pub fn secp256k1_recover_public_key(
    hash: [u8; 32],
    signature: &[u8; 64],
    recovery_id: u8,
) -> Result<[u8; 65]> {
    let recid = secp256k1::ecdsa::RecoveryId::try_from(recovery_id as i32)?;
    let signature = secp256k1::ecdsa::RecoverableSignature::from_compact(signature, recid)?;
    let public_key = signature.recover(&secp256k1::Message::from_digest(hash))?;
    Ok(public_key.serialize_uncompressed())
}

pub fn secp256k1_xonly_pubkey_tweak_add(key: &[u8; 32], tweak: [u8; 32]) -> Result<[u8; 65]> {
    let xonly_pubkey = secp256k1::XOnlyPublicKey::from_byte_array(key)?;
    let tweak = secp256k1::Scalar::from_be_bytes(tweak)?;

    // we have real public key inside `add_tweak`. maybe later API will be changed
    let (xonly_pubkey, parity) = xonly_pubkey.add_tweak(&SECP256K1_VERIFY, &tweak)?;
    let public_key = secp256k1::PublicKey::from_x_only_public_key(xonly_pubkey, parity);
    Ok(public_key.serialize_uncompressed())
}

// Ristretto 255 --------------------------------------------------------

pub fn ristretto_255_from_hash(hash: &[u8; 64]) -> [u8; 32] {
    let point = RistrettoPoint::from_uniform_bytes(hash);
    point.compress().to_bytes()
}

pub fn ristretto_255_from_compressed(point: [u8; 32]) -> Option<RistrettoPoint> {
    CompressedRistretto(point).decompress()
}

pub fn ristretto_255_add(x: [u8; 32], y: [u8; 32]) -> Option<[u8; 32]> {
    let r = CompressedRistretto(x).decompress()? + CompressedRistretto(y).decompress()?;
    Some(r.compress().to_bytes())
}

pub fn ristretto_255_sub(x: [u8; 32], y: [u8; 32]) -> Option<[u8; 32]> {
    let r = CompressedRistretto(x).decompress()? - CompressedRistretto(y).decompress()?;
    Some(r.compress().to_bytes())
}

pub fn ristretto_255_mul(x: [u8; 32], n: [u8; 32]) -> Option<[u8; 32]> {
    let r =
        CompressedRistretto(x).decompress()? * curve25519_dalek::Scalar::from_bytes_mod_order(n);
    Some(r.compress().to_bytes())
}

pub fn ristretto_255_mulbase(n: [u8; 32]) -> [u8; 32] {
    RistrettoPoint::mul_base(&curve25519_dalek::Scalar::from_bytes_mod_order(n))
        .compress()
        .to_bytes()
}

// SHA-2 ----------------------------------------------------------------

pub struct Sha256 {
    inner: sha2::Sha256,
}

impl Sha256 {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self { inner: sha2::Sha256::new() }
    }

    pub fn update(&mut self, data: impl AsRef<[u8]>) {
        self.inner.update(data)
    }

    pub fn finalize(self) -> [u8; 32] {
        self.inner.finalize().into()
    }
}

pub fn sha256_digest(data: impl AsRef<[u8]>) -> [u8; 32] {
    sha2::Sha256::digest(data).into()
}

pub fn sha256_digest_slices(data: &[&[u8]]) -> [u8; 32] {
    let mut digest = sha2::Sha256::new();
    for data in data {
        digest.update(data);
    }
    digest.finalize().into()
}

pub fn sha512_digest(data: impl AsRef<[u8]>) -> [u8; 64] {
    sha2::Sha512::digest(data).into()
}

// Blake ----------------------------------------------------------------

pub fn blake2b_digest(data: impl AsRef<[u8]>) -> [u8; 64] {
    *blake2b_simd::blake2b(data.as_ref()).as_array()
}

// Keccak ---------------------------------------------------------------

pub fn keccak256_digest(data: impl AsRef<[u8]>) -> [u8; 32] {
    let mut output = [0u8; 32];
    keccak_hash::keccak_256(data.as_ref(), &mut output);
    output
}

pub fn keccak512_digest(data: impl AsRef<[u8]>) -> [u8; 64] {
    let mut output = [0u8; 64];
    keccak_hash::keccak_512(data.as_ref(), &mut output);
    output
}

// CRC16 ----------------------------------------------------------------

pub fn ton_method_id(method_id: &str) -> u32 {
    (crc16::State::<crc16::XMODEM>::calculate(method_id.as_bytes()) as u32 & 0xffff) | 0x10000
}

pub fn address_crc(address: impl AsRef<[u8]>) -> u16 {
    crc16::State::<crc16::XMODEM>::calculate(address.as_ref())
}

// CRC32 ----------------------------------------------------------------

pub struct Crc32 {
    crc: u32,
}

impl Crc32 {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self { crc: 0 }
    }
    pub fn update(&mut self, data: impl AsRef<[u8]>) {
        self.crc = crc32c::crc32c_append(self.crc, data.as_ref())
    }
    pub fn finalize(self) -> u32 {
        self.crc
    }
}

pub fn crc32_digest(data: impl AsRef<[u8]>) -> u32 {
    crc32c::crc32c(data.as_ref())
}

// lz4 ------------------------------------------------------------

pub fn lz4_compress(data: impl AsRef<[u8]>, prepend_size: bool) -> Result<Vec<u8>> {
    let compressed = lz4::block::compress(
        data.as_ref(),
        Some(lz4::block::CompressionMode::DEFAULT),
        prepend_size,
    )?;
    Ok(compressed)
}

pub enum Lz4DecompressMode {
    WithPrependedSize,
    WithMaxSize(i32),
}

pub fn lz4_decompress(compressed: impl AsRef<[u8]>, mode: Lz4DecompressMode) -> Result<Vec<u8>> {
    let size = match mode {
        Lz4DecompressMode::WithPrependedSize => None,
        Lz4DecompressMode::WithMaxSize(max_size) => Some(max_size),
    };
    let decompressed = lz4::block::decompress(compressed.as_ref(), size)?;
    Ok(decompressed)
}

#[cfg(test)]
#[path = "tests/test_crypto.rs"]
mod tests;
