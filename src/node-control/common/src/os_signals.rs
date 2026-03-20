/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::task_cancellation::{CancellationCtx, CancellationReason};

fn signal_name(sig: libc::c_int) -> &'static str {
    match sig {
        libc::SIGINT => "SIGINT",
        libc::SIGTERM => "SIGTERM",
        #[cfg(unix)]
        libc::SIGQUIT => "SIGQUIT",
        _ => "UNKNOWN",
    }
}

#[cfg(unix)]
async fn wait_for_unix_signal() -> libc::c_int {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sig_int = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
    let mut sig_term = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
    let mut sig_quit = signal(SignalKind::quit()).expect("Failed to install SIGQUIT handler");

    tokio::select! {
        _ = sig_int.recv() => libc::SIGINT,
        _ = sig_term.recv() => libc::SIGTERM,
        _ = sig_quit.recv() => libc::SIGQUIT,
    }
}

#[cfg(not(unix))]
async fn wait_for_unix_signal() -> libc::c_int {
    tokio::signal::ctrl_c().await.expect("Failed to install Ctrl+C handler");
    libc::SIGINT
}

pub async fn wait(mut cancellation_ctx: CancellationCtx) {
    let mut cancellation_rx = cancellation_ctx.subscribe();

    tokio::select! {
        sig = wait_for_unix_signal() => {
            tracing::info!("received OS signal: {}({})", signal_name(sig), sig);
            cancellation_ctx.cancel(CancellationReason::OsSignal(sig));
        }

        _ = cancellation_rx.changed() => {
        }
    }
}
