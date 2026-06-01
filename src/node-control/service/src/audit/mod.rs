/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod enums;
pub mod event;
pub mod factory;
pub mod jsonl_log;
pub mod jsonl_writer;
pub mod log;
pub mod participant;

pub use common::app_config::AuditLogConfig;
pub use enums::{
    AuditActorKind, AuditEventPayload, AuditOutcome, AuditSeverity, AuditSource, AuditSubjectKind,
    StakeSkipReason,
};
pub use event::AuditEvent;
pub use factory::AuditLogFactory;
pub use jsonl_log::AuditInitError;
pub use log::{AuditLog, NoopAuditLog};
pub use participant::{AuditActor, AuditSubject};
