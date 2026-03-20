/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::app_config::{AppConfig, LogConfig, LogOutput, LogRotation};
use tracing_subscriber::{Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt};

pub fn setup_log(
    config: Option<&AppConfig>,
) -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let default_log_config = LogConfig::default();
    let log_config = config.and_then(|c| c.log.as_ref()).unwrap_or(&default_log_config);
    let level = log_config.level;

    let make_filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level.as_str()))
    };

    let make_layer = || {
        tracing_subscriber::fmt::layer()
            .with_file(false)
            .with_line_number(false)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_target(true)
            .with_level(true)
    };

    let make_file_appender = || -> anyhow::Result<(tracing_appender::non_blocking::NonBlocking, tracing_appender::non_blocking::WorkerGuard)> {
        let log_file = log_config.path.as_ref().ok_or(anyhow::anyhow!("log.path is required when output is 'file' or 'all'"))?;
        let log_dir = log_file.parent().unwrap_or(std::path::Path::new("."));
        let log_name = log_file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("nodectl.log");

        let mut condition = rolling_file::RollingConditionBasic::new()
            .max_size(log_config.max_size_mb * 1_000_000);

        condition = match log_config.rotation {
            LogRotation::Hourly => condition.hourly(),
            LogRotation::Never => condition,
            LogRotation::Daily => condition.daily(),
        };

        let file_appender = rolling_file::BasicRollingFileAppender::new(
            log_dir.join(log_name),
            condition,
            log_config.max_files,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create log file appender: {e}"))?;

        Ok(tracing_appender::non_blocking(file_appender))
    };

    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();
    let guard: Option<tracing_appender::non_blocking::WorkerGuard> = match &log_config.output {
        LogOutput::Console => {
            layers.push(make_layer().with_ansi(true).with_filter(make_filter()).boxed());
            None
        }
        LogOutput::File => {
            let (non_blocking, guard) = make_file_appender()?;
            layers.push(
                make_layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_filter(make_filter())
                    .boxed(),
            );
            Some(guard)
        }
        LogOutput::All => {
            let (non_blocking, guard) = make_file_appender()?;
            layers.push(make_layer().with_ansi(true).with_filter(make_filter()).boxed());
            layers.push(
                make_layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_filter(make_filter())
                    .boxed(),
            );
            Some(guard)
        }
    };

    match tracing_subscriber::registry().with(layers).try_init() {
        Ok(()) => Ok(guard),
        Err(e) => {
            tracing::warn!("Global subscriber already set, skipping re-init: {e}");
            Ok(guard)
        }
    }
}
