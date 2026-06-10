/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod actor_builder;
pub mod enums;
pub mod event;
pub mod factory;
#[cfg(test)]
pub mod in_memory;
pub mod jsonl_log;
pub mod jsonl_writer;
pub mod log;
pub mod participant;

pub use actor_builder::{AuditActorBuilder, client_ip_from_headers};
pub use common::app_config::AuditLogConfig;
pub use enums::{
    AuditEventPayload, AuditOutcome, AuditSeverity, AuditSource, ConfigFieldChange, StakeSkipReason,
};
pub use event::{AuditEvent, AuditFileHeader, ElectionsStakeSubmittedParams};
pub use factory::AuditLogFactory;
#[cfg(test)]
pub use in_memory::InMemoryAuditLog;
pub use jsonl_log::AuditInitError;
pub use log::{AuditLog, NoopAuditLog};
pub use participant::{AuditActor, AuditTarget};
