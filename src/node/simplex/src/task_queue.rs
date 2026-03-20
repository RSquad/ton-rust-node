/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Task queue implementation for Simplex consensus
//!
//! Based on validator-session task queue implementation.

use crate::session_processor::SessionProcessor;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

/*
    Task types
*/

/// Task of main processing task queue
pub(crate) type TaskPtr = Box<dyn FnOnce(&mut SessionProcessor) + Send>;

/// Pointer to the main task queue
pub(crate) type TaskQueuePtr = Arc<dyn TaskQueue<TaskPtr>>;

/// Session callback task (for listener callbacks)
pub(crate) type CallbackTaskPtr = Box<dyn FnOnce() + Send>;

/// Pointer to session callback task queue
pub(crate) type CallbackTaskQueuePtr = Arc<dyn TaskQueue<CallbackTaskPtr>>;

/*
    TaskQueue trait
*/

/// Task queue interface
pub(crate) trait TaskQueue<FuncPtr: Send + 'static>: Send + Sync {
    /// Is queue overloaded
    fn is_overloaded(&self) -> bool;

    /// Is queue empty
    #[allow(dead_code)]
    fn is_empty(&self) -> bool;

    /// Post closure to queue
    fn post_closure(&self, task: FuncPtr);

    /// Pull closure from queue with timeout
    fn pull_closure(
        &self,
        timeout: Duration,
        last_warn_dump_time: &mut SystemTime,
    ) -> Option<FuncPtr>;

    /// Flush all pending tasks
    fn flush(&self);
}

/*
    Helper functions
*/

/// Post closure to be run in a main processing thread
pub(crate) fn post_closure<F>(queue: &TaskQueuePtr, task_fn: F)
where
    F: FnOnce(&mut SessionProcessor),
    F: Send + 'static,
{
    queue.post_closure(Box::new(task_fn));
}

/// Post closure to be run in a session callbacks processing thread
pub(crate) fn post_callback_closure<F>(queue: &CallbackTaskQueuePtr, task_fn: F)
where
    F: FnOnce(),
    F: Send + 'static,
{
    queue.post_closure(Box::new(task_fn));
}
