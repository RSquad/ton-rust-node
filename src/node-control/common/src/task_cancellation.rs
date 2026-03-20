/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::{
    error::Error as StdError,
    fmt::{Display, Formatter},
    sync::Arc,
};

#[derive(Debug)]
pub enum CancellationReason {
    GracefullyShutdown(),
    OsSignal(i32),
    Error(Arc<anyhow::Error>),
}

impl Clone for CancellationReason {
    fn clone(&self) -> Self {
        match self {
            CancellationReason::GracefullyShutdown() => CancellationReason::GracefullyShutdown(),
            CancellationReason::OsSignal(s) => CancellationReason::OsSignal(*s),
            CancellationReason::Error(e) => CancellationReason::Error(Arc::clone(e)),
        }
    }
}

impl Display for CancellationReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CancellationReason::GracefullyShutdown() => {
                write!(f, "Gracefully shutdown (exit code EXIT_OK)")
            }
            CancellationReason::OsSignal(sig) => write!(f, "terminated by signal {sig}"),
            CancellationReason::Error(err) => {
                if f.alternate() {
                    write!(f, "{:#}", err)
                } else {
                    write!(f, "{err}")
                }
            }
        }
    }
}

impl std::error::Error for CancellationReason {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            CancellationReason::Error(err) => err.source(),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for CancellationReason {
    fn from(err: anyhow::Error) -> Self {
        CancellationReason::Error(Arc::new(err))
    }
}

#[derive(Clone)]
pub struct CancellationCtx {
    tx: tokio::sync::watch::Sender<Option<CancellationReason>>,
    rx: tokio::sync::watch::Receiver<Option<CancellationReason>>,
}

impl Default for CancellationCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationCtx {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel::<Option<CancellationReason>>(None);

        CancellationCtx { tx, rx }
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<CancellationReason>> {
        self.rx.clone()
    }

    pub fn cancel(&mut self, reason: CancellationReason) {
        match self.tx.send(Some(reason.clone())) {
            Ok(_) => tracing::debug!("cancel signal: {}", reason),
            Err(e) => tracing::error!("failed to send cancel: {}", e),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.rx.borrow().is_some()
    }
}
