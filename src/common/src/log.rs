/*
 * Copyright 2018-2022 TON DEV SOLUTIONS LTD.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the Apache License, Version 2.0.
 * See the common/LICENSE file in this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[allow(dead_code)]
fn init_log(config: &str) {
    if !log::log_enabled!(log::Level::Error) {
        if let Err(e) = log4rs::init_file(config, Default::default()) {
            panic!("Cannot read logging configuration from {}: {}", config, e)
        }
    }
}

#[allow(dead_code)]
fn init_log_without_config(
    pattern: Option<&str>,
    log_level: log::LevelFilter,
    output_file: Option<&str>,
) {
    let pattern = pattern.unwrap_or("{h({l})}[{f}:{L}] {m}{n}");
    let encoder_boxed = Box::new(log4rs::encode::pattern::PatternEncoder::new(pattern));
    let config = if let Some(file) = output_file {
        let file = log4rs::append::file::FileAppender::builder()
            .encoder(encoder_boxed)
            .build(file)
            .unwrap();
        log4rs::config::Config::builder()
            .appender(log4rs::config::Appender::builder().build("file", Box::new(file)))
            .build(log4rs::config::Root::builder().appender("file").build(log_level))
            .unwrap()
    } else {
        let console =
            log4rs::append::console::ConsoleAppender::builder().encoder(encoder_boxed).build();
        log4rs::config::Config::builder()
            .appender(log4rs::config::Appender::builder().build("console", Box::new(console)))
            .build(log4rs::config::Root::builder().appender("console").build(log_level))
            .unwrap()
    };
    log4rs::init_config(config).ok();
}
