/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "telemetry")]
use crate::engine_traits::EngineTelemetry;
use crate::{engine_traits::EngineAlloc, error::NodeError};
use adnl::{
    common::{add_counted_object_to_map, CountedObject, Counter},
    declare_counted,
};
use std::{
    cmp::Ord,
    fmt::Display,
    hash::Hash,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use ton_block::{error, Result};

#[cfg(test)]
#[path = "tests/test_awaiters_pool.rs"]
mod tests;

declare_counted!(
    struct OperationAwaiters<R> {
        is_started: AtomicBool,
        tx: tokio::sync::watch::Sender<Option<Result<R>>>,
        rx: tokio::sync::watch::Receiver<Option<Result<R>>>,
    }
);

impl<R: Clone> OperationAwaiters<R> {
    fn new(
        is_started: bool,
        #[cfg(feature = "telemetry")] telemetry: &Arc<EngineTelemetry>,
        allocated: &Arc<EngineAlloc>,
    ) -> Arc<Self> {
        let (tx, rx) = tokio::sync::watch::channel(None);
        let ret = Self {
            is_started: AtomicBool::new(is_started),
            tx,
            rx,
            counter: allocated.awaiters.clone().into(),
        };
        #[cfg(feature = "telemetry")]
        telemetry.awaiters.update(allocated.awaiters.load(Ordering::Relaxed));
        Arc::new(ret)
    }
}

pub struct AwaitersPool<I, R> {
    ops_awaiters: lockfree::map::Map<I, Arc<OperationAwaiters<R>>>,
    description: &'static str,
    #[cfg(feature = "telemetry")]
    telemetry: Arc<EngineTelemetry>,
    allocated: Arc<EngineAlloc>,
}

impl<I, R> AwaitersPool<I, R>
where
    I: Ord + Hash + Clone + Display,
    R: Clone,
{
    pub fn new(
        description: &'static str,
        #[cfg(feature = "telemetry")] telemetry: Arc<EngineTelemetry>,
        allocated: Arc<EngineAlloc>,
    ) -> Self {
        Self {
            ops_awaiters: lockfree::map::Map::new(),
            description,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        }
    }

    pub async fn do_or_wait_with_owned_key(
        &self,
        id: I,
        wait_timeout_ms: Option<u64>,
        operation: impl futures::Future<Output = Result<R>>,
    ) -> Result<Option<R>> {
        self.do_or_wait(&id, wait_timeout_ms, operation).await
    }

    pub async fn do_or_wait(
        &self,
        id: &I,
        wait_timeout_ms: Option<u64>,
        operation: impl futures::Future<Output = Result<R>>,
    ) -> Result<Option<R>> {
        loop {
            if let Some(op_awaiters) = self.ops_awaiters.get(id) {
                if !op_awaiters.1.is_started.swap(true, Ordering::SeqCst) {
                    return Some(self.do_operation(id, operation, &op_awaiters.1).await)
                        .transpose();
                } else {
                    return self
                        .wait_operation(id, wait_timeout_ms, &op_awaiters.1, || Ok(false))
                        .await;
                }
            } else {
                let new_awaiters = OperationAwaiters::new(
                    true,
                    #[cfg(feature = "telemetry")]
                    &self.telemetry,
                    &self.allocated,
                );
                if add_counted_object_to_map(&self.ops_awaiters, id.clone(), || {
                    Ok(new_awaiters.clone())
                })? {
                    return Some(self.do_operation(id, operation, &new_awaiters).await).transpose();
                }
            }
        }
    }

    pub async fn wait(
        &self,
        id: &I,
        timeout_ms: Option<u64>,
        check_complete: impl Fn() -> Result<bool>,
    ) -> Result<Option<R>> {
        loop {
            if let Some(op_awaiters) = self.ops_awaiters.get(id) {
                return self.wait_operation(id, timeout_ms, &op_awaiters.1, check_complete).await;
            } else {
                let new_awaiters = OperationAwaiters::new(
                    false,
                    #[cfg(feature = "telemetry")]
                    &self.telemetry,
                    &self.allocated,
                );
                if add_counted_object_to_map(&self.ops_awaiters, id.clone(), || {
                    Ok(new_awaiters.clone())
                })? {
                    return self
                        .wait_operation(id, timeout_ms, &new_awaiters, check_complete)
                        .await;
                }
            }
        }
    }

    pub fn shunt(&self, id: &I, operation: impl Fn() -> Result<R>) -> Result<()> {
        if let Some(op_awaiters) = self.ops_awaiters.get(id) {
            let r = operation()?;
            let _ = op_awaiters.1.tx.send(Some(Ok(r)));
        }
        Ok(())
    }

    async fn wait_operation(
        &self,
        id: &I,
        timeout_ms: Option<u64>,
        op_awaiters: &OperationAwaiters<R>,
        check_complete: impl Fn() -> Result<bool>,
    ) -> Result<Option<R>> {
        let mut rx = op_awaiters.rx.clone();
        loop {
            log::trace!("{}: wait_operation: waiting... {}", self.description, id);

            let result = if let Ok(result) =
                tokio::time::timeout(Duration::from_millis(1), rx.changed()).await
            {
                result
            } else if check_complete()? {
                // Operation might be done before calling `wait_operation` - check it and return
                return Ok(None);
            } else if let Some(timeout_ms) = timeout_ms {
                tokio::time::timeout(Duration::from_millis(timeout_ms), rx.changed())
                    .await
                    .map_err(|_| {
                        NodeError::Timeout(format!("{}: timeout {}", self.description, id))
                    })?
            } else {
                rx.changed().await
            };
            if result.is_err() {
                return Ok(None);
            }

            let r = match &*rx.borrow() {
                Some(Ok(r)) => Ok(Some(r.clone())),
                Some(Err(e)) => Err(error!("{}", e)),
                None => continue,
            };
            log::trace!("{}: wait_operation: done {}", self.description, id);
            break r;
        }
    }

    async fn do_operation(
        &self,
        id: &I,
        operation: impl futures::Future<Output = Result<R>>,
        op_awaiters: &OperationAwaiters<R>,
    ) -> Result<R> {
        log::trace!("{}: do_operation: doing... {}", self.description, id);
        let mut rx = op_awaiters.rx.clone();
        tokio::select! {
            result = operation => {
                log::trace!("{}: do_operation: done {}", self.description, id);

                self.ops_awaiters.remove(id);

                let r = match result {
                    Ok(ref r) => Ok(r.clone()),
                    Err(ref e) => Err(error!("{}", e)), // The Error doesn't impl Clone,
                                                        // so it is impossible to clone full result
                };
                let _ = op_awaiters.tx.send(Some(r));
                return result;
            }
            _ = rx.changed() => {
                log::trace!("{}: do_operation: shunt {}", self.description, id);

                self.ops_awaiters.remove(id);

                match &*op_awaiters.rx.borrow() {
                    Some(Ok(r)) => return Ok(r.clone()),
                    Some(Err(e)) => return Err(error!("{}", e)),
                    None => return Err(error!("{}: do_operation: shunt: no result", self.description))
                }
            }
        }
    }
}
