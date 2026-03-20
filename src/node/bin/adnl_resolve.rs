/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::{
    node::{AdnlNode, AdnlNodeConfig},
    AddressSearchContext, DhtNode, DhtSearchPolicy,
};
use node::config::TonNodeGlobalConfigJson;
use std::{convert::TryInto, env, fs::File, io::BufReader};
use ton_block::{base64_decode, error, fail, KeyId, Result};

include!("../../common/src/log.rs");

const IP: &str = "0.0.0.0:4191";
const KEY_TAG: usize = 1;

async fn scan(adnlid: &str, cfgfile: &str) -> Result<()> {
    let file = File::open(cfgfile)?;
    let reader = BufReader::new(file);
    let config: TonNodeGlobalConfigJson = serde_json::from_reader(reader)
        .map_err(|e| error!("Cannot read config from file {}: {}", cfgfile, e))?;
    let dht_nodes = config.get_dht_nodes_configs()?;
    let (_, config) = AdnlNodeConfig::with_ip_address_and_private_key_tags(IP, vec![KEY_TAG])?;
    let adnl = AdnlNode::with_config(config).await?;
    let dht = DhtNode::with_adnl_node(adnl.clone(), KEY_TAG)?;
    adnl.start_over_udp(vec![dht.clone()]).await?;

    let mut nodes = Vec::new();
    let mut bad_nodes = Vec::new();
    for dht_node in dht_nodes.iter() {
        if let Some(key) = dht.add_peer(dht_node)? {
            nodes.push(key)
        } else {
            fail!("Invalid DHT peer {:?}", dht_node)
        }
    }

    let keyid = KeyId::from_data((&base64_decode(adnlid)?[..32]).try_into()?);
    let mut context = AddressSearchContext::with_params(&keyid, DhtSearchPolicy::FastSearch(5))?;
    let mut index = 0;
    println!("Searching DHT for {}...", keyid);
    loop {
        if let Ok(Some((ip, key))) = dht.find_address(&mut context).await {
            println!("Found {} / {}", ip, key.id());
            return Ok(());
        }
        if index >= nodes.len() {
            nodes.clear();
            for dht_node in dht.get_known_nodes(10000)?.iter() {
                if let Some(key) = dht.add_peer(dht_node)? {
                    if !bad_nodes.contains(&key) {
                        nodes.push(key)
                    }
                }
            }
            if nodes.is_empty() {
                fail!("No good DHT peers")
            }
            index = 0;
        }
        println!(
            "Not found yet, scanning more DHT nodes from {} ({} of {}) ...",
            nodes[index],
            index,
            nodes.len()
        );
        if !dht.find_dht_nodes(&nodes[index]).await? {
            println!("DHT node {} is non-responsive", nodes[index]);
            bad_nodes.push(nodes.remove(index))
        } else {
            index += 1
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        println!("Usage: adnl_resolve <adnl-id> <path-to-global-config>");
        return;
    };
    // init_log("./common/config/log_cfg.yml");
    if let Err(e) = scan(args[1].as_str(), args[2].as_str()).await {
        println!("ADNL resolving error: {}", e)
    }
}
