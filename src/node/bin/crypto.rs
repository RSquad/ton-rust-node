/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use clap::Parser;
use secrets_vault::vault_block::get_key_option_factory;
use std::{
    net::{IpAddr, SocketAddr},
    ops::Deref,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use ton_api::{
    ton::{
        adnl::{address::address::Udp, addresslist::AddressList},
        dht::node::Node,
        pk::privatekey::Ed25519 as PkEd25519,
        PublicKey,
    },
    Constructor, IntoBoxed, Signing,
};
use ton_block::{base64_decode, base64_encode, error, fail, KeyOption, Result};

#[derive(clap::Parser)]
#[command(name = "crypto", about = "TON cryptographic utilities")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Generate keys and network structures
    Gen {
        #[command(subcommand)]
        what: GenCommands,
    },
    /// Compute values from existing data
    Get {
        #[command(subcommand)]
        what: GetCommands,
    },
}

#[derive(clap::Subcommand)]
enum GenCommands {
    /// Generate a new Ed25519 keypair
    Key,
    /// Generate a signed DHT node entry
    Dht(GenDhtArgs),
}

#[derive(clap::Subcommand)]
enum GetCommands {
    /// Compute ADNL ID from an existing key
    AdnlId(GetAdnlIdArgs),
}

#[derive(clap::Args)]
struct GenDhtArgs {
    /// Socket address (e.g. 1.2.3.4:30303)
    #[arg(long)]
    addr: String,
    /// Base64-encoded private key
    #[arg(long)]
    key: String,
}

#[derive(clap::Args)]
struct GetAdnlIdArgs {
    /// Base64-encoded public key
    #[arg(long, group = "key_input")]
    public: Option<String>,
    /// Base64-encoded secret (private) key
    #[arg(long, group = "key_input")]
    secret: Option<String>,
}

fn decode_key_32(b64: &str) -> Result<[u8; 32]> {
    let bytes = base64_decode(b64)?;
    bytes.try_into().map_err(|_| error!("key must be exactly 32 bytes"))
}

fn now() -> i32 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i32
}

fn gen_key() -> Result<serde_json::Value> {
    let key = get_key_option_factory().generate()?;
    let pvt_key = key.pvt_key()?;
    let pvt_key_data: &[u8] = &pvt_key.lock()?;

    let mut secret_tl = Vec::with_capacity(36);
    secret_tl.extend_from_slice(&PkEd25519::constructor_const().to_le_bytes());
    secret_tl.extend_from_slice(pvt_key_data);

    Ok(serde_json::json!({
        "secret": base64_encode(pvt_key_data),
        "pubkey": base64_encode(key.pub_key()?),
        "adnlId": base64_encode(key.id().data()),
        "secretTl": base64_encode(&secret_tl),
    }))
}

fn gen_dht(addr: &str, key_b64: &str) -> Result<serde_json::Value> {
    let addr: SocketAddr = addr.parse().map_err(|e| error!("invalid address '{}': {}", addr, e))?;
    let ip = match addr.ip() {
        IpAddr::V4(v4) => i32::from_be_bytes(v4.octets()),
        _ => fail!("IPv6 not supported"),
    };
    let port = addr.port() as i32;

    let key = get_key_option_factory().from_private_key(&decode_key_32(key_b64)?)?;
    let version = now();

    let node = Node {
        id: PublicKey::try_from(&key)?,
        addr_list: AddressList {
            addrs: vec![Udp { ip, port }.into_boxed()].into(),
            version,
            reinit_date: version,
            priority: 0,
            expire_at: 0,
        },
        version,
        signature: Default::default(),
    };
    let signed = node.sign(&key)?;

    let pub_key: Arc<dyn KeyOption> = (&signed.id).try_into()?;
    Ok(serde_json::json!({
        "@type": "dht.node",
        "id": {
            "@type": "pub.ed25519",
            "key": base64_encode(pub_key.pub_key()?)
        },
        "addr_list": {
            "@type": "adnl.addressList",
            "addrs": [{
                "@type": "adnl.address.udp",
                "ip": ip,
                "port": port
            }],
            "version": signed.addr_list.version,
            "reinit_date": signed.addr_list.reinit_date,
            "priority": signed.addr_list.priority,
            "expire_at": signed.addr_list.expire_at
        },
        "version": signed.version,
        "signature": base64_encode(signed.signature.deref())
    }))
}

fn get_adnl_id(public: Option<&str>, secret: Option<&str>) -> Result<serde_json::Value> {
    let key: Arc<dyn KeyOption> = if let Some(public) = public {
        get_key_option_factory().from_public_key(&decode_key_32(public)?)
    } else if let Some(secret) = secret {
        get_key_option_factory().from_private_key(&decode_key_32(secret)?)?
    } else {
        fail!("either --public or --secret is required")
    };

    Ok(serde_json::json!({
        "adnlId": base64_encode(key.id().data()),
        "pubkey": base64_encode(key.pub_key()?),
    }))
}

fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Commands::Gen { what } => match what {
            GenCommands::Key => gen_key(),
            GenCommands::Dht(args) => gen_dht(&args.addr, &args.key),
        },
        Commands::Get { what } => match what {
            GetCommands::AdnlId(args) => {
                get_adnl_id(args.public.as_deref(), args.secret.as_deref())
            }
        },
    };
    match result {
        Ok(json) => println!("{}", serde_json::to_string_pretty(&json).unwrap()),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_api::{ton::dht::node::Node as TlNode, Serializer};
    use ton_block::{sha256_digest_slices, ED25519_KEY_TYPE};

    #[test]
    fn gen_key_output_has_all_fields() {
        let v = gen_key().unwrap();
        assert!(v["secret"].is_string());
        assert!(v["pubkey"].is_string());
        assert!(v["adnlId"].is_string());
        assert!(v["secretTl"].is_string());
    }

    #[test]
    fn gen_key_sizes() {
        let v = gen_key().unwrap();
        assert_eq!(base64_decode(v["secret"].as_str().unwrap()).unwrap().len(), 32);
        assert_eq!(base64_decode(v["pubkey"].as_str().unwrap()).unwrap().len(), 32);
        assert_eq!(base64_decode(v["adnlId"].as_str().unwrap()).unwrap().len(), 32);
        assert_eq!(base64_decode(v["secretTl"].as_str().unwrap()).unwrap().len(), 36);
    }

    #[test]
    fn gen_key_secret_tl_has_pk_ed25519_magic() {
        let v = gen_key().unwrap();
        let tl = base64_decode(v["secretTl"].as_str().unwrap()).unwrap();
        let magic = u32::from_le_bytes(tl[..4].try_into().unwrap());
        assert_eq!(magic, PkEd25519::constructor_const()); // 0x49682317
    }

    #[test]
    fn gen_key_secret_tl_contains_secret() {
        let v = gen_key().unwrap();
        let secret = base64_decode(v["secret"].as_str().unwrap()).unwrap();
        let tl = base64_decode(v["secretTl"].as_str().unwrap()).unwrap();
        assert_eq!(&tl[4..], secret.as_slice());
    }

    #[test]
    fn gen_key_adnl_id_is_sha256_of_type_and_pubkey() {
        let v = gen_key().unwrap();
        let pub_key = base64_decode(v["pubkey"].as_str().unwrap()).unwrap();
        let expected = sha256_digest_slices(&[&ED25519_KEY_TYPE.to_le_bytes(), &pub_key]);
        let adnl_id = base64_decode(v["adnlId"].as_str().unwrap()).unwrap();
        assert_eq!(adnl_id, expected.as_slice());
    }

    #[test]
    fn gen_key_derives_correct_pubkey_from_secret() {
        let v = gen_key().unwrap();
        let secret = v["secret"].as_str().unwrap();
        let expected_pubkey = v["pubkey"].as_str().unwrap();
        let key =
            get_key_option_factory().from_private_key(&decode_key_32(secret).unwrap()).unwrap();
        assert_eq!(base64_encode(key.pub_key().unwrap()), expected_pubkey);
    }

    #[test]
    fn get_adnl_id_from_public_matches_gen_key() {
        let v = gen_key().unwrap();
        let result = get_adnl_id(Some(v["pubkey"].as_str().unwrap()), None).unwrap();
        assert_eq!(result["adnlId"], v["adnlId"]);
        assert_eq!(result["pubkey"], v["pubkey"]);
    }

    #[test]
    fn get_adnl_id_from_secret_matches_gen_key() {
        let v = gen_key().unwrap();
        let result = get_adnl_id(None, Some(v["secret"].as_str().unwrap())).unwrap();
        assert_eq!(result["adnlId"], v["adnlId"]);
        assert_eq!(result["pubkey"], v["pubkey"]);
    }

    #[test]
    fn get_adnl_id_requires_key() {
        assert!(get_adnl_id(None, None).is_err());
    }

    #[test]
    fn gen_dht_output_structure() {
        let v = gen_key().unwrap();
        let dht = gen_dht("10.0.0.1:30303", v["secret"].as_str().unwrap()).unwrap();
        assert_eq!(dht["@type"], "dht.node");
        assert_eq!(dht["id"]["@type"], "pub.ed25519");
        assert_eq!(dht["addr_list"]["@type"], "adnl.addressList");
        assert_eq!(dht["addr_list"]["addrs"][0]["@type"], "adnl.address.udp");
        assert_eq!(dht["addr_list"]["addrs"][0]["ip"], 167772161); // 10.0.0.1
        assert_eq!(dht["addr_list"]["addrs"][0]["port"], 30303);
    }

    #[test]
    fn gen_dht_pubkey_matches_input() {
        let v = gen_key().unwrap();
        let dht = gen_dht("1.2.3.4:30303", v["secret"].as_str().unwrap()).unwrap();
        assert_eq!(dht["id"]["key"], v["pubkey"]);
    }

    #[test]
    fn gen_dht_signature_verifies() {
        let v = gen_key().unwrap();
        let dht = gen_dht("1.2.3.4:30303", v["secret"].as_str().unwrap()).unwrap();

        // Reconstruct the signed node from JSON and verify signature
        let pub_key_b64 = dht["id"]["key"].as_str().unwrap();
        let pub_key =
            get_key_option_factory().from_public_key(&decode_key_32(pub_key_b64).unwrap());

        let sig = base64_decode(dht["signature"].as_str().unwrap()).unwrap();
        let version = dht["version"].as_i64().unwrap() as i32;
        let ip = dht["addr_list"]["addrs"][0]["ip"].as_i64().unwrap() as i32;
        let port = dht["addr_list"]["addrs"][0]["port"].as_i64().unwrap() as i32;

        // Rebuild the TL node with empty signature and serialize
        let mut node = TlNode {
            id: PublicKey::try_from(&pub_key).unwrap(),
            addr_list: AddressList {
                addrs: vec![Udp { ip, port }.into_boxed()].into(),
                version,
                reinit_date: version,
                priority: 0,
                expire_at: 0,
            },
            version,
            signature: Default::default(),
        };

        // Serialize with empty signature (same as what Signing::sign does)
        let mut buf = Vec::new();
        Serializer::new(&mut buf).write_into_boxed(&node).unwrap();

        // Verify
        pub_key.verify(&buf, &sig).unwrap();

        // Also verify via Signing trait: set signature, then call verify
        node.signature = sig.into();
        node.verify(&pub_key).unwrap();
    }

    #[test]
    fn gen_dht_invalid_addr() {
        let v = gen_key().unwrap();
        assert!(gen_dht("not-an-addr", v["secret"].as_str().unwrap()).is_err());
    }

    #[test]
    fn decode_key_32_rejects_wrong_size() {
        assert!(decode_key_32(&base64_encode(&[0u8; 16])).is_err());
        assert!(decode_key_32(&base64_encode(&[0u8; 64])).is_err());
    }

    #[test]
    fn decode_key_32_accepts_correct_size() {
        assert!(decode_key_32(&base64_encode(&[0u8; 32])).is_ok());
    }
}
