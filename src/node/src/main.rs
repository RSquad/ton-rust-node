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
mod block;
mod block_proof;
mod boot;
mod collator_test_bundle;
mod config;
mod engine;
mod engine_operations;
mod engine_traits;
mod error;
mod full_node;
mod internal_db;
mod macros;
mod network;
mod rng;
mod rpc_server;
mod shard_state;
mod shard_states_keeper;
mod sync;
mod types;
mod validating_utils;
mod validator;

mod ext_messages;

#[cfg(not(feature = "xp25"))]
mod shard_blocks;
#[cfg(feature = "xp25")]
mod shard_blocks_intershard;
#[cfg(feature = "xp25")]
mod shard_blocks {
    pub use crate::shard_blocks_intershard::*;
}

use crate::{
    config::{SecretsVaultConfig, TonNodeConfig},
    engine::{Engine, EngineFlags, Stopper},
    internal_db::restore::set_graceful_termination,
    validating_utils::supported_version,
};
#[cfg(target_os = "linux")]
use std::os::raw::c_void;
use std::sync::Arc;
#[cfg(feature = "trace_alloc")]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::Duration,
};
#[cfg(feature = "trace_alloc_detail")]
use std::{
    fs::File,
    io::Write,
    mem::{self, MaybeUninit},
    sync::atomic::{AtomicIsize, AtomicUsize},
};
use ton_block::Result;
#[cfg(feature = "mirrornet")]
use ton_block::UnixTime;

#[cfg(test)]
#[path = "tests/test_helper.rs"]
pub mod test_helper;

#[cfg(target_os = "linux")]
#[link(name = "tcmalloc_minimal", kind = "dylib")]
extern "C" {
    pub fn tc_memalign(alignment: usize, size: usize) -> *mut c_void;
    pub fn tc_free(ptr: *mut c_void);
}

#[cfg(target_os = "linux")]
fn check_tcmalloc() {
    unsafe {
        let ptr = tc_memalign(10, 10);
        tc_free(ptr);
    }
}

#[cfg(feature = "trace_alloc")]
struct TracingAllocator {
    count: AtomicU64,
    allocated: AtomicU64,
    overhead: AtomicU64,
}

#[cfg(feature = "trace_alloc_detail")]
struct AllocDetail {
    start: AtomicUsize,
    size: AtomicIsize,
}

#[cfg(feature = "trace_alloc_detail")]
const SIZE_TRACEBUF: usize = 20000000;

#[cfg(feature = "trace_alloc_detail")]
lazy_static::lazy_static! {
    static ref TRACEBUF: [AllocDetail; SIZE_TRACEBUF] = {
        let mut data: [MaybeUninit<AllocDetail>; SIZE_TRACEBUF] = unsafe {
            MaybeUninit::uninit().assume_init()
        };
        for elem in &mut data[..] {
            elem.write(
                AllocDetail {
                    start: AtomicUsize::new(0),
                    size: AtomicIsize::new(0)
                }
            );
        }
        unsafe { mem::transmute::<_, [AllocDetail; SIZE_TRACEBUF]>(data) }
    };
    static ref TRACEBUF_HEAD: AtomicUsize = AtomicUsize::new(0);
    static ref TRACEBUF_TAIL: AtomicUsize = AtomicUsize::new(0);
}

#[cfg(feature = "trace_alloc")]
thread_local!(
    static NOCALC: AtomicBool = AtomicBool::new(false)
);

#[cfg(feature = "trace_alloc")]
unsafe impl GlobalAlloc for TracingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ret = System.alloc(layout);
        self.check_alloc(ret, layout.size());
        ret
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ret = System.alloc_zeroed(layout);
        self.check_alloc(ret, layout.size());
        ret
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.check_dealloc(ptr, layout.size());
        let ret = System.realloc(ptr, layout, new_size);
        self.check_alloc(ret, new_size);
        ret
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.check_dealloc(ptr, layout.size());
        System.dealloc(ptr, layout);
    }
}

#[cfg(feature = "trace_alloc")]
impl TracingAllocator {
    fn check_alloc(&self, _ptr: *mut u8, size: usize) {
        if NOCALC
            .with(|f| f.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_ok())
        {
            self.allocated.fetch_add(size as u64, Ordering::Relaxed);
            #[cfg(feature = "trace_alloc_detail")]
            Self::post_trace(_ptr as usize, size as isize);
            self.count.fetch_add(1, Ordering::Relaxed);
            NOCALC.with(|f| f.store(false, Ordering::Relaxed));
        } else {
            self.overhead.fetch_add(size as u64, Ordering::Relaxed);
        }
    }

    fn check_dealloc(&self, _ptr: *mut u8, size: usize) {
        if NOCALC
            .with(|f| f.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_ok())
        {
            self.allocated.fetch_sub(size as u64, Ordering::Relaxed);
            #[cfg(feature = "trace_alloc_detail")]
            Self::post_trace(_ptr as usize, -(size as isize));
            self.count.fetch_sub(1, Ordering::Relaxed);
            NOCALC.with(|f| f.store(false, Ordering::Relaxed));
        } else {
            self.overhead.fetch_sub(size as u64, Ordering::Relaxed);
        }
    }

    #[cfg(feature = "trace_alloc_detail")]
    fn post_trace(start: usize, size: isize) {
        loop {
            let this = TRACEBUF_HEAD.load(Ordering::Relaxed);
            let next = if this == SIZE_TRACEBUF - 1 { 0 } else { this + 1 };
            if next == TRACEBUF_TAIL.load(Ordering::Acquire) {
                thread::yield_now();
                continue;
            }
            if TRACEBUF_HEAD
                .compare_exchange(this, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
            {
                thread::yield_now();
                continue;
            }
            if start == 0 {
                panic!("ZEROADDR_WRITE")
            }
            TRACEBUF[this].start.store(start, Ordering::Relaxed);
            TRACEBUF[this].size.store(size, Ordering::Release);
            break;
        }
    }
}

#[cfg(feature = "trace_alloc")]
#[global_allocator]
static GLOBAL: TracingAllocator = TracingAllocator {
    count: AtomicU64::new(0),
    allocated: AtomicU64::new(0),
    overhead: AtomicU64::new(0),
};

fn init_logger<T: AsRef<std::path::Path>>(log_config_path: Option<T>) {
    if let Some(path) = log_config_path {
        if let Err(err) = log4rs::init_file(path, Default::default()) {
            println!("Error while initializing log by {}: {}", err, err);
        } else {
            return;
        }
    }

    let level = log::LevelFilter::Info;
    let stdout = log4rs::append::console::ConsoleAppender::builder()
        .target(log4rs::append::console::Target::Stdout)
        .build();

    let config = log4rs::config::Config::builder()
        .appender(
            log4rs::config::Appender::builder()
                .filter(Box::new(log4rs::filter::threshold::ThresholdFilter::new(level)))
                .build("stdout", Box::new(stdout)),
        )
        .build(log4rs::config::Root::builder().appender("stdout").build(log::LevelFilter::Info))
        .unwrap();

    let result = log4rs::init_config(config);
    if let Err(e) = result {
        println!("Error init log: {}", e);
    }
}

static NOT_SET_LABEL: &str = "Not set";

fn get_version() -> String {
    format!(
        "Execute {:?}\n\
        BLOCK_VERSION: {:?}\n\
        COMMIT_ID: {:?}\n\
        BUILD_DATE: {:?}\n\
        COMMIT_DATE: {:?}\n\
        GIT_BRANCH: {:?}\n\
        RUST_VERSION:{}\n",
        std::option_env!("CARGO_PKG_VERSION").unwrap_or(NOT_SET_LABEL),
        supported_version(),
        std::option_env!("BUILD_GIT_COMMIT").unwrap_or(NOT_SET_LABEL),
        std::option_env!("BUILD_TIME").unwrap_or(NOT_SET_LABEL),
        std::option_env!("BUILD_GIT_DATE").unwrap_or(NOT_SET_LABEL),
        std::option_env!("BUILD_GIT_BRANCH").unwrap_or(NOT_SET_LABEL),
        std::option_env!("BUILD_RUST_VERSION").unwrap_or(NOT_SET_LABEL),
    )
}

fn get_build_info() -> String {
    let mut info = String::new();
    info += &format!(
        "TON Rust Node, version {}\n\
        Rust: {}\n\
        NODE git commit:         {}",
        std::option_env!("CARGO_PKG_VERSION").unwrap_or(NOT_SET_LABEL),
        std::option_env!("BUILD_RUST_VERSION").unwrap_or(NOT_SET_LABEL),
        std::option_env!("BUILD_GIT_COMMIT").unwrap_or(NOT_SET_LABEL),
    );
    info
}

async fn start_engine(
    config: TonNodeConfig,
    zerostate_path: Option<&str>,
    validator_runtime: tokio::runtime::Handle,
    liteserver_runtime: tokio::runtime::Handle,
    flags: EngineFlags,
    stopper: Arc<Stopper>,
    metrics: Option<(std::net::SocketAddr, metrics_exporter_prometheus::PrometheusHandle)>,
) -> Result<(Arc<Engine>, tokio::task::JoinHandle<()>)> {
    crate::engine::run(
        config,
        zerostate_path,
        validator_runtime,
        liteserver_runtime,
        flags,
        stopper,
        metrics,
    )
    .await
}

const CONFIG_NAME: &str = "config.json";
const DEFAULT_CONFIG_NAME: &str = "default_config.json";

fn check_debug_build() {
    // check that node built with --release
    if cfg!(debug_assertions) {
        println!("!!! WARN: Node was built without --release\n");
    }
}

fn main() {
    check_debug_build();

    #[cfg(target_os = "linux")]
    check_tcmalloc();

    println!("{}", get_build_info());
    let version = get_version();
    println!("{}", version);

    let app = clap::Command::new("TON node")
        .arg(
            clap::Arg::new("zerostate")
                .short('z')
                .long("zerostate")
                .value_name("zerostate")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("config")
                .short('c')
                .long("configs")
                .value_name("config")
                .num_args(1)
                .default_value("./"),
        )
        .arg(
            clap::Arg::new("console_key")
                .short('k')
                .long("ckey")
                .value_name("console_key")
                .num_args(1)
                .help("use console key in json format"),
        )
        .arg(
            clap::Arg::new("initial_sync_disabled")
                .short('i')
                .long("initial-sync-disabled")
                .action(clap::ArgAction::SetTrue)
                .help("use this flag to sync from zero_state"),
        )
        .arg(
            clap::Arg::new("force_check_db")
                .short('f')
                .long("force-check-db")
                .action(clap::ArgAction::SetTrue)
                .help("start check & restore db process forcedly with refilling cells database"),
        )
        .arg(
            clap::Arg::new("process_conf_and_exit")
                .long("process-conf-and-exit")
                .action(clap::ArgAction::SetTrue)
                .help("finish node after config file processing (reading or generating)."),
        )
        .arg(
            clap::Arg::new("truncate_db")
                .long("truncate-db")
                .value_name("truncate_db")
                .help("truncate the database at the specified masterchain block sequence number"),
        );

    #[cfg(feature = "mirrornet")]
    let app = app.arg(
        clap::Arg::new("timeshift")
            .long("timeshift")
            .value_name("timeshift")
            .help("shift the node's time by the specified number of seconds (can be negative) for testing purposes"),
    );

    let matches = app.get_matches();

    let truncate_db = matches.get_one::<String>("truncate_db").map(|s| s.as_str()).map(|s| {
        s.parse::<u32>().unwrap_or_else(|e| {
            eprintln!("truncate_db value must be a valid u32: {}", e);
            std::process::exit(1);
        })
    });

    let flags = EngineFlags {
        initial_sync_disabled: matches.get_flag("initial_sync_disabled"),
        force_check_db: matches.get_flag("force_check_db"),
        truncate_db,
    };
    let process_conf_and_exit = matches.get_flag("process_conf_and_exit");

    let config_dir_path = match matches.get_one::<String>("config") {
        Some(config) => config.as_str(),
        None => {
            eprintln!("Can't load config: config dir is not set!");
            return;
        }
    };

    let console_key =
        matches.get_one::<String>("console_key").map(|console_key| console_key.to_string());

    let zerostate_path = matches.get_one::<String>("zerostate").map(String::as_str);
    let mut config = match TonNodeConfig::from_file(
        config_dir_path,
        CONFIG_NAME,
        None,
        DEFAULT_CONFIG_NAME,
        console_key,
    ) {
        Err(e) => {
            eprintln!("Can't load config: {e:?}");
            return;
        }
        Ok(c) => c,
    };

    if process_conf_and_exit {
        println!("Finish node because of --process-conf-and-exit flag is set");
        return;
    }

    init_logger(config.log_config_path());
    log::info!("{}", version);

    #[cfg(feature = "mirrornet")]
    if let Some(timeshift) = matches.get_one::<String>("timeshift") {
        let timeshift = timeshift.parse::<i64>().unwrap_or_else(|e| {
            log::error!("Invalid timeshift value {timeshift}: {e}");
            std::process::exit(1);
        });
        if let Err(e) = UnixTime::set_timeshift(timeshift) {
            log::error!("Failed to set timeshift: {e}");
            std::process::exit(1);
        }
        log::warn!("Node time is shifted by {} seconds", timeshift);
    }

    let metrics_handle = config
        .metrics()
        .expect("Bad metrics config")
        .map(|mc| (mc.address, engine::init_prometheus_recorder(&mc)));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .expect("Can't create Engine tokio runtime");
    let validator_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .expect("Can't create Validator tokio runtime");
    let liteserver_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .expect("Can't create Liteserver tokio runtime");

    // Load secrets from vault to config
    if let Err(e) = runtime.block_on(SecretsVaultConfig::on_load(&mut config)) {
        log::error!("Can't load secrets from the vault: {e:?}");
        eprintln!("Can't load secrets from the vault: {e:?}");
        return;
    }

    #[cfg(feature = "trace_alloc_detail")]
    thread::spawn(|| {
        let mut file = File::create("trace.bin").unwrap();
        loop {
            let this = TRACEBUF_TAIL.load(Ordering::Relaxed);
            let next = if this == SIZE_TRACEBUF - 1 { 0 } else { this + 1 };
            if this == TRACEBUF_HEAD.load(Ordering::Relaxed) {
                thread::yield_now();
                continue;
            }
            let size = TRACEBUF[this].size.load(Ordering::Acquire);
            if size == 0 {
                thread::yield_now();
                continue;
            }
            let start = TRACEBUF[this].start.load(Ordering::Relaxed);
            if start == 0 {
                panic!("ZEROADDR_READ")
            }
            file.write_all(&start.to_le_bytes()).ok();
            file.write_all(&size.to_le_bytes()).ok();
            TRACEBUF[this].size.store(0, Ordering::Release);
            TRACEBUF_TAIL.store(next, Ordering::Release);
        }
    });

    #[cfg(feature = "trace_alloc")]
    thread::spawn(|| loop {
        thread::sleep(Duration::from_millis(30000));
        let count = GLOBAL.count.load(Ordering::Relaxed);
        let allocated = GLOBAL.allocated.load(Ordering::Relaxed);
        let overhead = GLOBAL.overhead.load(Ordering::Relaxed);
        log::info!(
            "Allocated {} + {} = {} bytes, {} objects",
            allocated,
            overhead,
            allocated + overhead,
            count
        );
    });

    let stopper = Arc::new(Stopper::new());
    let stopper_ctrl_c = stopper.clone();
    ctrlc::set_handler(move || {
        log::warn!("Got SIGINT, starting node's safe stopping...");
        stopper_ctrl_c.set_stop();
    })
    .expect("Error setting termination signals handler");

    let validator_rt_handle = validator_runtime.handle().clone();
    let liteserver_rt_handle = liteserver_runtime.handle().clone();
    let db_dir = config.internal_db_path().to_string();
    runtime.block_on(async move {
        match start_engine(
            config,
            zerostate_path,
            validator_rt_handle,
            liteserver_rt_handle,
            flags,
            stopper.clone(),
            metrics_handle,
        )
        .await
        {
            Err(e) => {
                if stopper.check_stop() {
                    log::warn!("Node stopped ({})", e);
                    set_graceful_termination(&db_dir);
                } else {
                    log::error!("Can't start node's Engine: {e:?}");
                    eprintln!("Can't start node's Engine: {e:?}");
                }
            }
            Ok((engine, join_handle)) => {
                join_handle.await.ok();
                log::warn!("Still safe stopping node...");
                engine.wait_stop().await;
                log::warn!("Node stopped");
                set_graceful_termination(&db_dir);
            }
        }
    });
}
