/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::events::event_types::Event;

#[async_trait::async_trait]
pub trait EventHandler: Send + Sync {
    async fn put(&self, event: Event) -> anyhow::Result<()>;
}
