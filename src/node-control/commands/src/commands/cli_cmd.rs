/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::{
    nodectl::{
        auth_cmd::AuthCmd, automation_cmd::AutomationCmd, config_cmd::ConfigCmd,
        deploy_cmd::DeployCmd, key_cmd::KeyCmd, service_api_cmd::ApiCmd, service_cmd::ServiceCmd,
        vote_cmd::VoteCmd,
    },
    ton_http_api::get_config_param_cmd::GetConfigParamCmd,
};

#[derive(clap::Subcommand, Clone)]
pub enum Commands {
    // TON HTTP API
    #[command(name = "config-param")]
    GetConfigParam(GetConfigParamCmd),
    // REST API
    #[command(name = "api")]
    Api(ApiCmd),
    // Authentication user management (vault-backed)
    #[command(name = "auth")]
    Auth(AuthCmd),
    /// Contracts task automation (auto-deploy / auto-topup)
    #[command(name = "automation")]
    Automation(AutomationCmd),
    // Configuration management
    #[command(name = "config")]
    Config(ConfigCmd),
    // Deploy (wallets, contracts)
    #[command(name = "deploy")]
    Deploy(DeployCmd),
    // Key management
    #[command(name = "key")]
    Key(KeyCmd),
    // Start as service
    #[command(name = "service")]
    Service(ServiceCmd),
    // Config proposals voting
    #[command(name = "vote")]
    Vote(VoteCmd),
}
