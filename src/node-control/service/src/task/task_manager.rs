/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::runtime_config::RuntimeConfig;
use common::{
    app_config::AppConfig,
    task_cancellation::{CancellationCtx, CancellationReason},
};
use std::sync::{Arc, Mutex};
use ton_block::UnixTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Stopped,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskStateView {
    pub enabled: bool,
    pub status: TaskStatus,
    pub updated_at: u64,
}

/// Trait for tasks managed by `TaskController`.
///
/// Uses `&self` (not `&mut self`) so that the task can be stored in `Arc`
/// and safely shared/restarted. If a task needs mutable internal state,
/// use interior mutability (`tokio::sync::Mutex`, `AtomicU64`, etc.)
/// inside the implementing struct.
#[async_trait::async_trait]
pub trait ServiceTask: Send + Sync {
    async fn run(
        &self,
        cancellation_ctx: CancellationCtx,
        app_config: Arc<AppConfig>,
    ) -> anyhow::Result<()>;
}

struct State {
    enabled: bool,
    status: TaskStatus,
    updated_at: u64,
    cancel: Option<CancellationCtx>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

pub struct TaskController {
    name: &'static str,
    task: Arc<dyn ServiceTask>,
    state: Mutex<State>,
    runtime_cfg: Arc<dyn RuntimeConfig>,
}

impl TaskController {
    pub fn new(
        name: &'static str,
        task: impl ServiceTask + 'static,
        runtime_cfg: Arc<dyn RuntimeConfig>,
    ) -> Self {
        Self {
            name,
            task: Arc::new(task),
            state: Mutex::new(State {
                enabled: true,
                status: TaskStatus::Stopped,
                updated_at: UnixTime::now(),
                cancel: None,
                handle: None,
            }),
            runtime_cfg,
        }
    }

    pub async fn status(&self) -> TaskStateView {
        let st = self.state.lock().expect("failed to lock state");
        TaskStateView { enabled: st.enabled, status: st.status, updated_at: st.updated_at }
    }

    pub async fn enable(&self) -> TaskStateView {
        let mut st = self.state.lock().expect("failed to lock state");

        st.enabled = true;

        if st.status == TaskStatus::Running {
            st.updated_at = UnixTime::now();
            return TaskStateView {
                enabled: st.enabled,
                status: st.status,
                updated_at: st.updated_at,
            };
        }

        let cancel_ctx = CancellationCtx::new();
        let task = self.task.clone();
        let name = self.name;

        tracing::debug!("starting {} task...", name);
        let ctx = cancel_ctx.clone();
        let app_config = self.runtime_cfg.get();
        let handle = tokio::spawn(async move {
            if let Err(e) = task.run(ctx, app_config).await {
                tracing::error!("{} task error: {:#}", name, e);
            }
        });
        tracing::info!("{} task started", name);

        st.cancel = Some(cancel_ctx);
        st.handle = Some(handle);
        st.status = TaskStatus::Running;
        st.updated_at = UnixTime::now();

        TaskStateView { enabled: st.enabled, status: st.status, updated_at: st.updated_at }
    }

    pub async fn disable(&self) -> TaskStateView {
        let handle_to_await = {
            let mut st = self.state.lock().expect("failed to lock state");
            st.enabled = false;

            if st.status == TaskStatus::Stopped {
                st.updated_at = UnixTime::now();
                return TaskStateView {
                    enabled: st.enabled,
                    status: st.status,
                    updated_at: st.updated_at,
                };
            }

            tracing::debug!("stopping {} task...", self.name);
            if let Some(mut ctx) = st.cancel.take() {
                ctx.cancel(CancellationReason::GracefullyShutdown());
            } else {
                tracing::warn!("{} task marked running but no cancellation ctx present", self.name);
            }

            st.handle.take()
        };

        if let Some(handle) = handle_to_await {
            tracing::debug!("await {} task join...", self.name);
            let _ = handle.await;
        }

        tracing::info!("{} task stopped", self.name);

        let mut st = self.state.lock().expect("failed to lock state");
        st.status = TaskStatus::Stopped;
        st.updated_at = UnixTime::now();

        TaskStateView { enabled: st.enabled, status: st.status, updated_at: st.updated_at }
    }

    pub async fn restart(&self) -> TaskStateView {
        let _ = self.disable().await;
        self.enable().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::app_config::{HttpConfig, TonHttpApiConfig};
    use contracts::{NominatorWrapper, TonWallet};
    use secrets_vault::vault::SecretVault;
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicBool, AtomicU32, Ordering},
    };
    use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

    struct NoopRuntimeConfig {
        app_config: Arc<AppConfig>,
    }

    impl RuntimeConfig for NoopRuntimeConfig {
        fn get(&self) -> Arc<AppConfig> {
            self.app_config.clone()
        }
        fn master_wallet(&self) -> Arc<dyn TonWallet> {
            unimplemented!()
        }
        fn pools(&self) -> Arc<HashMap<String, Arc<dyn NominatorWrapper>>> {
            unimplemented!()
        }
        fn wallets(&self) -> Arc<HashMap<String, Arc<dyn TonWallet>>> {
            unimplemented!()
        }
        fn rpc_client(&self) -> Arc<ClientJsonRpc> {
            unimplemented!()
        }
        fn vault(&self) -> Option<Arc<SecretVault>> {
            unimplemented!()
        }
        fn update_and_save(
            &self,
            _f: Box<dyn FnOnce(&mut AppConfig) + Send>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
    }

    fn runtime_config() -> Arc<dyn RuntimeConfig> {
        Arc::new(NoopRuntimeConfig {
            app_config: Arc::new(AppConfig {
                nodes: HashMap::new(),
                wallets: HashMap::new(),
                pools: HashMap::new(),
                bindings: HashMap::new(),
                ton_http_api: TonHttpApiConfig::default(),
                elections: None,
                voting: None,
                http: HttpConfig::default(),
                master_wallet: None,
                tick_interval: 30,
                log: None,
            }),
        })
    }

    struct WaitForeverTask {
        started: Arc<AtomicBool>,
    }

    impl WaitForeverTask {
        fn new() -> (Self, Arc<AtomicBool>) {
            let started = Arc::new(AtomicBool::new(false));
            (Self { started: started.clone() }, started)
        }
    }

    #[async_trait::async_trait]
    impl ServiceTask for WaitForeverTask {
        async fn run(
            &self,
            ctx: CancellationCtx,
            _app_config: Arc<AppConfig>,
        ) -> anyhow::Result<()> {
            self.started.store(true, Ordering::SeqCst);
            let mut rx = ctx.subscribe();
            let _ = rx.changed().await;
            Ok(())
        }
    }

    /// A task that completes immediately with `Ok(())`.
    struct ImmediateTask;

    #[async_trait::async_trait]
    impl ServiceTask for ImmediateTask {
        async fn run(
            &self,
            _ctx: CancellationCtx,
            _app_config: Arc<AppConfig>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// A task that always returns an error.
    struct FailingTask;

    #[async_trait::async_trait]
    impl ServiceTask for FailingTask {
        async fn run(
            &self,
            _ctx: CancellationCtx,
            _app_config: Arc<AppConfig>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("task failed on purpose")
        }
    }

    /// A task that counts how many times it has been started.
    struct CountingTask {
        count: Arc<AtomicU32>,
    }

    impl CountingTask {
        fn new() -> (Self, Arc<AtomicU32>) {
            let count = Arc::new(AtomicU32::new(0));
            (Self { count: count.clone() }, count)
        }
    }

    #[async_trait::async_trait]
    impl ServiceTask for CountingTask {
        async fn run(
            &self,
            ctx: CancellationCtx,
            _app_config: Arc<AppConfig>,
        ) -> anyhow::Result<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            let mut rx = ctx.subscribe();
            let _ = rx.changed().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn new_controller_is_stopped_and_enabled() {
        let ctrl = TaskController::new("test", ImmediateTask, runtime_config());
        let view = ctrl.status().await;

        assert!(view.enabled);
        assert_eq!(view.status, TaskStatus::Stopped);
    }

    #[tokio::test]
    async fn enable_starts_the_task() {
        let (task, started) = WaitForeverTask::new();
        let ctrl = TaskController::new("test", task, runtime_config());

        let view = ctrl.enable().await;

        assert!(view.enabled);
        assert_eq!(view.status, TaskStatus::Running);

        // Give the spawned task a moment to execute.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(started.load(Ordering::SeqCst), "task should have started");

        // Cleanup
        let view2 = ctrl.disable().await;
        assert_eq!(view2.status, TaskStatus::Stopped);
    }

    #[tokio::test]
    async fn enable_is_idempotent_when_already_running() {
        let (task, _started) = WaitForeverTask::new();
        let ctrl = TaskController::new("test", task, runtime_config());

        ctrl.enable().await;
        let view = ctrl.enable().await;

        assert!(view.enabled);
        assert_eq!(view.status, TaskStatus::Running);

        let view = ctrl.disable().await;
        assert_eq!(view.status, TaskStatus::Stopped);
    }

    #[tokio::test]
    async fn disable_stops_the_task() {
        let (task, _started) = WaitForeverTask::new();
        let ctrl = TaskController::new("test", task, runtime_config());

        ctrl.enable().await;
        let view = ctrl.disable().await;

        assert!(!view.enabled);
        assert_eq!(view.status, TaskStatus::Stopped);
    }

    #[tokio::test]
    async fn disable_is_idempotent_when_already_stopped() {
        let ctrl = TaskController::new("test", ImmediateTask, runtime_config());

        let view = ctrl.disable().await;
        assert!(!view.enabled);
        assert_eq!(view.status, TaskStatus::Stopped);
    }

    #[tokio::test]
    async fn disable_delivers_cancellation_signal() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = cancelled.clone();

        struct CancellationProbe {
            flag: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl ServiceTask for CancellationProbe {
            async fn run(
                &self,
                ctx: CancellationCtx,
                _app_config: Arc<AppConfig>,
            ) -> anyhow::Result<()> {
                let mut rx = ctx.subscribe();
                let _ = rx.changed().await;
                self.flag.store(true, Ordering::SeqCst);
                Ok(())
            }
        }

        let ctrl = TaskController::new(
            "test",
            CancellationProbe { flag: cancelled_clone },
            runtime_config(),
        );
        ctrl.enable().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        ctrl.disable().await;
        assert!(cancelled.load(Ordering::SeqCst), "task should have observed cancellation");
    }

    #[tokio::test]
    async fn restart_cycles_the_task() {
        let (task, count) = CountingTask::new();
        let ctrl = TaskController::new("test", task, runtime_config());

        ctrl.enable().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);

        let view = ctrl.restart().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(view.enabled);
        assert_eq!(view.status, TaskStatus::Running);
        assert_eq!(count.load(Ordering::SeqCst), 2, "task should have been started twice");

        ctrl.disable().await;
    }

    #[tokio::test]
    async fn failing_task_does_not_panic_controller() {
        let ctrl = TaskController::new("test", FailingTask, runtime_config());

        let view = ctrl.enable().await;
        assert_eq!(view.status, TaskStatus::Running);

        // Let the spawned task complete with its error.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Controller should still be operational; disable should not panic.
        let view = ctrl.disable().await;
        assert_eq!(view.status, TaskStatus::Stopped);
    }

    #[tokio::test]
    async fn status_reflects_enable_disable_cycle() {
        let (task, _) = WaitForeverTask::new();
        let ctrl = TaskController::new("test", task, runtime_config());

        let v1 = ctrl.status().await;
        assert_eq!(v1.status, TaskStatus::Stopped);
        assert!(v1.enabled);

        ctrl.enable().await;
        let v2 = ctrl.status().await;
        assert_eq!(v2.status, TaskStatus::Running);
        assert!(v2.enabled);

        ctrl.disable().await;
        let v3 = ctrl.status().await;
        assert_eq!(v3.status, TaskStatus::Stopped);
        assert!(!v3.enabled);
    }

    #[tokio::test]
    async fn task_status_as_str() {
        assert_eq!(TaskStatus::Running.as_str(), "running");
        assert_eq!(TaskStatus::Stopped.as_str(), "stopped");
    }

    // ── task! macro tests ───────────────────────────────────────────

    async fn dummy_run(
        _ctx: CancellationCtx,
        _app_config: Arc<AppConfig>,
        value: Arc<AtomicU32>,
        label: String,
    ) -> anyhow::Result<()> {
        value.fetch_add(1, Ordering::SeqCst);
        assert_eq!(label, "hello");
        Ok(())
    }

    crate::task!(DummyMacroTask, crate::task::task_manager::tests::dummy_run {
        value: Arc<AtomicU32>,
        label: String,
    });

    #[tokio::test]
    async fn macro_creates_struct_with_new() {
        let v = Arc::new(AtomicU32::new(0));
        let _task = DummyMacroTask::new(v, "hello".into());
    }

    #[tokio::test]
    async fn macro_task_implements_service_task() {
        let v = Arc::new(AtomicU32::new(0));
        let task = DummyMacroTask::new(v.clone(), "hello".into());

        let ctx = CancellationCtx::new();
        let runtime_cfg = runtime_config();
        task.run(ctx, runtime_cfg.get()).await.expect("task should succeed");

        assert_eq!(v.load(Ordering::SeqCst), 1, "run function should have been called");
    }

    #[tokio::test]
    async fn macro_task_clones_fields() {
        let v = Arc::new(AtomicU32::new(0));
        let task = DummyMacroTask::new(v.clone(), "hello".into());

        // Run twice to confirm fields are cloned each time (not moved).
        let ctx1 = CancellationCtx::new();
        let runtime_cfg = runtime_config();
        task.run(ctx1, runtime_cfg.get()).await.unwrap();
        let ctx2 = CancellationCtx::new();
        task.run(ctx2, runtime_cfg.get()).await.unwrap();

        assert_eq!(v.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn macro_task_works_with_controller() {
        let v = Arc::new(AtomicU32::new(0));
        let task = DummyMacroTask::new(v.clone(), "hello".into());
        let ctrl = TaskController::new("macro-test", task, runtime_config());

        ctrl.enable().await;
        // The dummy function completes immediately, give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(v.load(Ordering::SeqCst), 1);

        ctrl.disable().await;
        let view = ctrl.status().await;
        assert_eq!(view.status, TaskStatus::Stopped);
    }

    async fn no_fields_run(
        _ctx: CancellationCtx,
        _app_config: Arc<AppConfig>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    crate::task!(NoFieldsTask, crate::task::task_manager::tests::no_fields_run {});

    #[tokio::test]
    async fn macro_task_with_no_fields() {
        let task = NoFieldsTask::new();
        let ctx = CancellationCtx::new();
        let app_config = runtime_config().get();
        task.run(ctx, app_config).await.expect("zero-field task should work");
    }
}
