/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::events::{event_types::Event, handler::EventHandler};
use std::sync::Arc;

#[derive(Default)]
pub struct CompositeEventHandler {
    handlers: Vec<Arc<dyn EventHandler>>,
}

impl CompositeEventHandler {
    pub fn new() -> Self {
        Self { handlers: Vec::new() }
    }

    pub fn add<T: EventHandler + 'static>(&mut self, handler: T) {
        self.handlers.push(Arc::new(handler));
    }

    pub fn add_arc(&mut self, handler: Arc<dyn EventHandler>) {
        self.handlers.push(handler);
    }

    pub fn from_vec(handlers: Vec<Arc<dyn EventHandler>>) -> Self {
        Self { handlers }
    }
}

#[async_trait::async_trait]
impl EventHandler for CompositeEventHandler {
    async fn put(&self, event: Event) -> anyhow::Result<()> {
        for handler in &self.handlers {
            let _ = handler.put(event.clone()).await;
        }
        Ok(())
    }
}
