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
    AddressSearchContext, DhtNode, DhtSearchPolicy, OverlayNode, OverlayNodesSearchContext,
};
use node::config::TonNodeGlobalConfigJson;
use std::{
    collections::HashMap,
    env,
    fs::File,
    io::BufReader,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    ops::Deref,
    sync::Arc,
};
use ton_block::{base64_encode, error, fail, KeyOption, Result};

include!("../../common/src/log.rs");

const DEFAULT_IP: &str = "0.0.0.0:4191";
const KEY_TAG: usize = 1;

fn scan(
    cfgfile: &str,
    bind_addr: &str,
    jsonl: bool,
    search_overlay: bool,
    use_workchain0: bool,
) -> Result<()> {
    let file = File::open(cfgfile)?;
    let reader = BufReader::new(file);
    let config: TonNodeGlobalConfigJson = serde_json::from_reader(reader)
        .map_err(|e| error!("Cannot read config from file {}: {}", cfgfile, e))?;
    let zero_state = config.zero_state()?;
    let zero_state = zero_state.file_hash;
    let dht_nodes = config.get_dht_nodes_configs()?;

    let mut rt = tokio::runtime::Runtime::new()?;
    let (_, config) =
        AdnlNodeConfig::with_ip_address_and_private_key_tags(bind_addr, vec![KEY_TAG])?;
    let adnl = rt.block_on(AdnlNode::with_config(config))?;
    let dht = DhtNode::with_adnl_node(adnl.clone(), KEY_TAG)?;
    let overlay = OverlayNode::with_params(adnl.clone(), zero_state.as_slice(), KEY_TAG)?;

    rt.block_on(adnl.start_over_udp(vec![dht.clone(), overlay.clone()]))?;

    let mut preset_nodes = Vec::new();
    for dht_node in dht_nodes.iter() {
        if let Some(key) = dht.add_peer(dht_node)? {
            preset_nodes.push(key)
        } else {
            fail!("Invalid DHT peer {:?}", dht_node)
        }
    }

    // Fetch signed address lists from preset nodes (picks up QUIC addresses)
    println!("Querying preset DHT nodes for signed address lists...");
    for node in preset_nodes.iter() {
        match rt.block_on(dht.get_signed_address_list(node)) {
            Ok(true) => println!("  {} - OK", node),
            Ok(false) => println!("  {} - no response", node),
            Err(e) => println!("  {} - error: {}", node, e),
        }
    }

    println!("Scanning DHT...");
    for node in preset_nodes.iter() {
        rt.block_on(dht.find_dht_nodes(node))?;
    }

    scan_overlay(&mut rt, &dht, preset_nodes.len(), &overlay, search_overlay, -1)?;
    if use_workchain0 {
        scan_overlay(&mut rt, &dht, preset_nodes.len(), &overlay, search_overlay, 0)?;
    }
    if search_overlay {
        return Ok(());
    }

    let mut count = 0;
    let mut quic_count = 0;
    let nodes = dht.get_known_nodes(5000)?;
    if !nodes.is_empty() {
        println!("---- Found DHT nodes:");
        for node in nodes {
            let key: Arc<dyn KeyOption> = (&node.id).try_into()?;
            match rt.block_on(dht.ping(key.id())) {
                Ok(true) => (),
                _ => continue,
            }
            let (adnl_addr, quic_addr) = AdnlNode::parse_address_list(&node.addr_list)?
                .ok_or_else(|| error!("Cannot parse address list {:?}", node.addr_list))?;
            let adr = adnl_addr.to_udp();

            // If not in the node record, try DHT value lookup for stored address
            // If no QUIC address from the node record, try DHT value lookup
            let quic_socket_addr = if let Some(q) = &quic_addr {
                Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(q.ip())), q.port()))
            } else {
                let mut ctx =
                    AddressSearchContext::with_params(key.id(), DhtSearchPolicy::FastSearch(3))?;
                if let Ok(Some((_, quic_addr, _))) = rt.block_on(dht.find_address(&mut ctx)) {
                    quic_addr.map(|q| SocketAddr::new(IpAddr::V4(Ipv4Addr::from(q.ip())), q.port()))
                } else {
                    None
                }
            };

            let mut addrs = vec![serde_json::json!({
                "@type": "adnl.address.udp",
                "ip": adr.ip,
                "port": adr.port
            })];
            if let Some(q) = quic_socket_addr {
                quic_count += 1;
                addrs.push(serde_json::json!({
                    "@type": "adnl.address.quic",
                    "ip": q.ip().to_string(),
                    "port": q.port()
                }));
            }
            let json = serde_json::json!(
                {
                    "@type": "dht.node",
                    "id": {
                        "@type": "pub.ed25519",
                        "key": base64_encode(key.pub_key()?)
                    },
                    "addr_list": {
                        "@type": "adnl.addressList",
                        "addrs": addrs,
                        "version": node.addr_list.version,
                        "reinit_date": node.addr_list.reinit_date,
                        "priority": node.addr_list.priority,
                        "expire_at": node.addr_list.expire_at
                    },
                    "version": node.version,
                    "signature": base64_encode(node.signature.deref())
                }
            );
            count += 1;
            println!(
                "{},",
                if jsonl {
                    serde_json::to_string(&json)?
                } else {
                    serde_json::to_string_pretty(&json)?
                }
            );
        }
        println!("Total: {} DHT nodes ({} with QUIC)", count, quic_count);
    } else {
        println!("---- No DHT nodes found via routing table");
    }

    // Also show QUIC addresses discovered from preset nodes via getSignedAddressList
    println!("\n---- Preset node QUIC addresses (from getSignedAddressList):");
    let local_key = adnl.key_by_tag(KEY_TAG)?.id().clone();
    for node in preset_nodes.iter() {
        let addrs = adnl.peer_ip_address(&local_key, node).ok().flatten();
        let (adnl_addr, quic_addr) = match addrs {
            Some((a, q)) => (Some(a), q),
            None => (None, None),
        };
        println!("  {} ADNL={:?} QUIC={:?}", node, adnl_addr, quic_addr);
    }

    Ok(())
}

fn scan_overlay(
    rt: &mut tokio::runtime::Runtime,
    dht: &Arc<DhtNode>,
    dht_presets: usize,
    overlay: &Arc<OverlayNode>,
    search_overlay: bool,
    workchain: i32,
) -> Result<()> {
    let overlay_id = overlay.calc_overlay_short_id(workchain, 0x8000000000000000u64 as i64)?;

    let mut ctx = OverlayNodesSearchContext::with_params(&overlay_id, DhtSearchPolicy::default())?;
    let mut overlays = HashMap::new();
    loop {
        let res = rt.block_on(dht.find_overlay_nodes(&mut ctx))?;
        let count = overlays.len();
        for (ip, node) in res {
            let key: Arc<dyn KeyOption> = node.id().try_into()?;
            overlays.insert(key.id().clone(), (ip, node));
        }
        if search_overlay {
            println!(
                "Found {} new OVERLAY nodes in {}({}), searching more...",
                overlays.len() - count,
                overlay_id,
                workchain
            )
        } else {
            println!(
                "Found {} new DHT nodes, searching more...",
                dht.get_known_nodes(5000)?.len() - dht_presets
            )
        }
        if !ctx.can_iterate() {
            break;
        }
    }

    if search_overlay {
        println!("---- Found OVERLAY nodes in {}({}):", overlay_id, workchain);
        for (ip, node) in overlays.iter() {
            println!("IP {}: {:?}", ip, node)
        }
        println!("---- Found {} OVERLAY nodes totally", overlays.len());
    }
    Ok(())
}

fn main() {
    let mut config = None;
    let mut jsonl = false;
    let mut overlay = false;
    let mut workchain0 = false;
    let mut bind_addr = DEFAULT_IP.to_string();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--jsonl" => jsonl = true,
            "--overlay" => overlay = true,
            "--workchain0" => workchain0 = true,
            "--bind" => {
                bind_addr = args.next().unwrap_or_else(|| {
                    eprintln!("--bind requires an argument (e.g. 127.0.0.1:4191)");
                    std::process::exit(1);
                });
            }
            _ => config = Some(arg),
        }
    }
    let config = if let Some(config) = config {
        config
    } else {
        println!("Usage: dhtscan [--jsonl] [--overlay] [--workchain0] [--bind ip:port] <path-to-global-config>");
        return;
    };
    init_log("./common/config/log_cfg.yml");
    scan(&config, &bind_addr, jsonl, overlay, workchain0)
        .unwrap_or_else(|e| println!("DHT scanning error: {}", e))
}
