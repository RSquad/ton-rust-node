/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;

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
