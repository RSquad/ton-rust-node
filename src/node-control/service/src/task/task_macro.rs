/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
/// Creates a task struct that implements [`ServiceTask`](task_manager::ServiceTask).
///
/// All fields are `.clone()`d and forwarded to the function:
///
/// ```ignore
/// task!(MyTask, path::to::run_fn {
///     field1: Type1,
///     field2: Type2,
/// });
/// ```
///
/// Expands to a `pub struct MyTask` with a `pub fn new(...)` constructor
/// and an `impl ServiceTask` whose `run` calls:
/// ```ignore
/// run_fn(cancellation_ctx, app_config, self.field1.clone(), self.field2.clone()).await;
/// ```
/// Note: the first two arguments are always `cancellation_ctx` and `app_config`.
/// `cancellation_ctx` is the cancellation context for the task.
/// `app_config` is the current application configuration.
#[macro_export]
macro_rules! task {
    ($name:ident, $fn_path:path { $($field:ident : $ty:ty),* $(,)? }) => {
        pub struct $name {
            $($field: $ty),*
        }

        impl $name {
            pub fn new($($field: $ty),*) -> Self {
                Self { $($field),* }
            }
        }

        #[async_trait::async_trait]
        impl $crate::task::task_manager::ServiceTask for $name {
            async fn run(
                &self,
                cancellation_ctx: common::task_cancellation::CancellationCtx,
                app_config: std::sync::Arc<common::app_config::AppConfig>,
            ) -> anyhow::Result<()> {
                $fn_path(cancellation_ctx, app_config, $(self.$field.clone()),*).await
            }
        }
    };
}
