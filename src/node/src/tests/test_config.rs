/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;

fn make_test_config_handler() -> NodeConfigHandler {
    let (sender, _reader) = tokio::sync::mpsc::unbounded_channel();
    NodeConfigHandler {
        runtime_handle: tokio::runtime::Handle::current(),
        sender,
        key_ring: Arc::new(lockfree::map::Map::new()),
        validator_keys: Arc::new(ValidatorKeys::new()),
    }
}

#[tokio::test]
async fn test_custom_overlays_serde() -> Result<()> {
    for _ in 0..100 {
        let mut overlays = Vec::new();
        for _ in 0..rand::random::<u32>() % 5 {
            let mut nodes = Vec::new();
            for _ in 0..rand::random::<u32>() % 12 {
                let adnl_id = UInt256::rand();
                let msg_sender = rand::random::<bool>();
                let msg_sender_priority = rand::random::<u32>();
                let block_sender = rand::random::<bool>();
                nodes.push(CustomOverlayNode {
                    adnl_id,
                    msg_sender: msg_sender.into(),
                    msg_sender_priority: msg_sender_priority as i32,
                    block_sender: block_sender.into(),
                });
            }
            let mut sender_shards = Vec::new();
            for _ in 0..(rand::random::<u32>() % 2) + 1 {
                let workchain = rand::random::<i32>();
                let shard = (rand::random::<u64>() & 0xFFFF_0000_0000_0000) | 0x0000_1000_0000_0000;
                sender_shards
                    .push(ton::ton_node::shardid::ShardId { workchain, shard: shard as i64 });
            }
            let name = format!("overlay_{}", UInt256::rand().to_hex_string());
            let skip_public_msg_send = rand::random::<bool>();
            let use_quic = rand::random::<bool>();
            let overlay = CustomOverlay {
                name,
                nodes,
                sender_shards,
                skip_public_msg_send: skip_public_msg_send.into(),
                use_quic: use_quic.into(),
            };
            overlays.push(overlay);
        }
        let coc = CustomOverlaysConfigBoxed::Engine_Validator_CustomOverlaysConfig(
            CustomOverlaysConfig { overlays },
        );

        let path = "../target/test_custom_overlays_serde.json";
        TonNodeConfig::save_custom_overlays_json(&coc, path).await?;
        let loaded_coc = TonNodeConfig::load_custom_overlays_json(path)?;
        let _ = std::fs::remove_file(path);
        assert_eq!(format!("{:?}", coc), format!("{:?}", loaded_coc));
    }
    Ok(())
}

#[tokio::test]
async fn test_get_actual_validator_key_ids_uses_validator_key_order() -> Result<()> {
    let config_handler = make_test_config_handler();

    let key_a = Ed25519KeyOption::<ZeroizingBytes>::generate()?;
    let key_b = Ed25519KeyOption::<ZeroizingBytes>::generate()?;
    let key_c = Ed25519KeyOption::<ZeroizingBytes>::generate()?;

    // Intentionally add keys in election order that differs from validator key order.
    config_handler.validator_keys.add(ValidatorKeysJson {
        expire_at: i32::MAX,
        election_id: 30,
        validator_key_id: base64_encode(key_b.id().data()),
        validator_adnl_key_id: None,
    })?;
    config_handler.validator_keys.add(ValidatorKeysJson {
        expire_at: i32::MAX,
        election_id: 10,
        validator_key_id: base64_encode(key_c.id().data()),
        validator_adnl_key_id: None,
    })?;
    config_handler.validator_keys.add(ValidatorKeysJson {
        expire_at: i32::MAX,
        election_id: 20,
        validator_key_id: base64_encode(key_a.id().data()),
        validator_adnl_key_id: None,
    })?;
    // Duplicate key in another election is still one local validator key.
    config_handler.validator_keys.add(ValidatorKeysJson {
        expire_at: i32::MAX,
        election_id: 40,
        validator_key_id: base64_encode(key_b.id().data()),
        validator_adnl_key_id: None,
    })?;

    let mut expected = vec![*key_a.id().data(), *key_b.id().data(), *key_c.id().data()];
    expected.sort_unstable();
    expected.dedup();

    let actual = config_handler.get_actual_validator_key_ids()?;
    let actual: Vec<[u8; 32]> = actual.iter().map(|key_id| *key_id.data()).collect();

    assert_eq!(actual, expected);
    Ok(())
}
