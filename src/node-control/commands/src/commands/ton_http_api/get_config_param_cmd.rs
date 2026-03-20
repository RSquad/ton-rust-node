/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use clap::{Args, command};
use common::app_config::AppConfig;
use std::{path::Path, sync::Arc};
use ton_block::BuilderData;
use ton_block_json::{SerializationMode, serialize_known_config_param};
use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

#[derive(Args, Debug, Clone)]
#[command(about = "Get current config parameter from masterchain state (TON HTTP API)")]
pub struct GetConfigParamCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,

    #[arg(long_help = "config parameter id")]
    id: u32,
}

impl GetConfigParamCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        let app_cfg = Arc::new(match AppConfig::load(Path::new(&self.config)) {
            Ok(cfg) => cfg,
            Err(e) => anyhow::bail!("Failed to open config file: {}", e),
        });

        let rpc_client = ClientJsonRpc::connect_many(
            app_cfg.ton_http_api.resolved_endpoints(),
            app_cfg.ton_http_api.api_key.clone(),
        )?;

        let config = rpc_client
            .get_config_param(self.id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get config param: {}", e))?;
        let mut builder = BuilderData::new();
        config.write_to_cell(&mut builder)?;
        let json_param = serialize_known_config_param(
            self.id,
            builder.references()[0].clone(),
            SerializationMode::Standart,
        )?;
        let json_str = serde_json::to_string_pretty(&json_param)
            .map_err(|e| anyhow::anyhow!("config param {} serialization error: {}", self.id, e))?;
        println!("{}", json_str);
        Ok(())
    }
}
