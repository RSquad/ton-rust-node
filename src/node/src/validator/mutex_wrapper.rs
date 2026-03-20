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
use adnl::telemetry::Metric;
use std::sync::Arc;
#[cfg(feature = "telemetry")]
use std::time::Instant;
use tokio::sync::OwnedMutexGuard;

pub struct MutexWrapper<T: Sized> {
    mutex: Arc<tokio::sync::Mutex<T>>,
    id: String,
    #[cfg(feature = "telemetry")]
    mutex_awaiting_metric: Option<Arc<Metric>>,
}

impl<T: Sized> MutexWrapper<T> {
    pub fn new(t: T, id: String) -> Self {
        MutexWrapper {
            mutex: Arc::new(tokio::sync::Mutex::new(t)),
            id,
            #[cfg(feature = "telemetry")]
            mutex_awaiting_metric: None,
        }
    }

    pub async fn execute_sync<Res, F>(&self, f: F) -> Res
    where
        F: FnOnce(&mut T) -> Res,
    {
        log::trace!(target: "validator", "Lock {} started acquire", self.id);

        let mut guard: OwnedMutexGuard<T>;
        #[cfg(feature = "telemetry")]
        {
            if let Some(metric) = &self.mutex_awaiting_metric {
                let started = Instant::now();
                guard = self.mutex.clone().lock_owned().await;
                metric.update(started.elapsed().as_micros() as u64);
            } else {
                guard = self.mutex.clone().lock_owned().await;
            }
        }
        #[cfg(not(feature = "telemetry"))]
        {
            guard = self.mutex.clone().lock_owned().await;
        }

        let guard_ref: &mut T = &mut guard;
        log::trace!(target: "validator", "Lock {} acquired", self.id);
        let res = f(guard_ref);
        log::trace!(target: "validator", "Lock {} released", self.id);
        res
    }
}
