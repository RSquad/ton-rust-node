/*
 * Copyright 2018-2022 TON DEV SOLUTIONS LTD.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the Apache License, Version 2.0.
 * See the common/LICENSE file in this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::node::{AdnlNodeConfig, AdnlNodeConfigJson};
use std::{
    env,
    ffi::OsString,
    fs::{read_to_string, File},
    io::Write,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
use ton_block::{fail, sha256_digest, Result};

const ENV_VAR_PORT: &str = "BASE_PORT";

pub fn configure_ip(template: &str, default_port: u16) -> String {
    let port = env::var(ENV_VAR_PORT).unwrap_or_else(|_| format!("{}", default_port));
    let Some(pos) = template.find(":") else { panic!("Wrong IP template {}", template) };
    let port: u16 = port.parse().expect(&format!("Wrong port value {}", port));
    let offset: u16 =
        template[pos + 1..].parse().expect(&format!("Wrong port in template {}", template));
    let ret = format!("{}:{}", &template[..pos], port + offset);
    println!("\nUsing {} local address", ret);
    ret
}

pub async fn resolve_ip(ip: &str) -> Result<SocketAddr> {
    let mut ret = ip.parse::<SocketAddr>()?;
    if ret.ip().is_unspecified() {
        let ip = external_ip::ConsensusBuilder::new()
            .add_sources(external_ip::get_http_sources::<external_ip::Sources>())
            .build()
            .get_consensus()
            .await;
        if let Some(IpAddr::V4(ip)) = ip {
            ret.set_ip(IpAddr::V4(ip))
        } else {
            fail!("Cannot obtain own external IP address")
        }
    }
    Ok(ret)
}

pub fn get_test_config_path(prefix: &str, addr: &SocketAddr) -> Result<PathBuf> {
    let mut path = PathBuf::from(prefix);
    let mut file_name = if let Some(file_name) = path.file_name() {
        file_name.to_os_string()
    } else {
        OsString::new()
    };
    let parent = if let Some(parent) = path.parent() {
        if parent.as_os_str().is_empty() {
            None
        } else if !parent.exists() {
            fail!("Cannot generate config path: folder '{}' does not exist", parent.display())
        } else {
            Some(parent)
        }
    } else {
        None
    };
    if parent.is_none() {
        path = PathBuf::from("./target");
        if !path.exists() {
            path = PathBuf::from("../target");
            if !path.exists() {
                fail!("Cannot generate config path: no target folder exists")
            }
        };
        path.push(prefix);
    }
    let suffix = if let IpAddr::V4(ip) = addr.ip() {
        format!("_{}_{}.json", ip.to_string().as_str(), addr.port().to_string().as_str())
    } else {
        fail!("Cannot generate config path for IP address that is not V4")
    };
    file_name.push(suffix);
    path.set_file_name(file_name);
    Ok(path)
}

pub fn generate_adnl_configs(
    ip: &str,
    tags: Vec<usize>,
    addr: Option<SocketAddr>,
) -> Result<(AdnlNodeConfigJson, AdnlNodeConfig)> {
    if let Some(addr) = addr {
        let mut keys = Vec::new();
        let addr = addr.to_string();
        for tag in tags {
            let mut data = Vec::new();
            data.extend_from_slice(addr.as_bytes());
            data.extend_from_slice(&tag.to_be_bytes());
            let key = sha256_digest(&data);
            keys.push((key, tag));
        }
        AdnlNodeConfig::from_ip_address_and_private_keys(ip, keys)
    } else {
        AdnlNodeConfig::with_ip_address_and_private_key_tags(ip, tags)
    }
}

pub async fn get_adnl_config(
    prefix: &str,
    ip: &str,
    tags: Vec<usize>,
    deterministic: bool,
) -> Result<AdnlNodeConfig> {
    let resolved_ip = resolve_ip(ip).await?;
    let config = get_test_config_path(prefix, &resolved_ip)?;
    let config = if config.exists() {
        let config = read_to_string(config)?;
        AdnlNodeConfig::from_json(config.as_str())?
    } else {
        let resolved_ip = if deterministic { Some(resolved_ip) } else { None };
        let (json, bin) = generate_adnl_configs(ip, tags, resolved_ip)?;
        File::create(config)?.write_all(serde_json::to_string_pretty(&json)?.as_bytes())?;
        bin
    };
    Ok(config)
}
