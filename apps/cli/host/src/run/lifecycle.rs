use super::js_to_fixed_2;
use crate::control_plane::StdioControlPlane;
use crate::exit_codes;
use crate::init::Runtime;
use crate::output::OutputSink;
use platform_api::OrchestratorHandle;
use std::sync::Arc;

/// Claude Code 2.1.217 print-loop budget cleanup (`Wam` + `rcr`). After every
/// main turn, compare the cumulative cost directly with `--max-budget-usd` and
/// stop every running background local agent/workflow once the ceiling is
/// reached. This deliberately does not depend on the main turn returning
/// `error_max_budget_usd`: a natural `end_turn` can itself push the cumulative
/// cost over the ceiling.
pub(super) async fn stop_background_agents_at_budget(
    max_budget_usd: Option<f64>,
    orchestrator: &dyn OrchestratorHandle,
    task_registry: &dyn platform_api::task_registry::TaskRegistryHandle,
) -> usize {
    let Some(max_budget_usd) = max_budget_usd else {
        return 0;
    };
    let cost = orchestrator.snapshot_cost().await;
    if !budget_reached(max_budget_usd, cost.total_nano_usd) {
        return 0;
    }
    let notice = budget_halt_notice(cost.total_usd, max_budget_usd);
    let announce = || eprintln!("{notice}");
    let Ok(stopped) = task_registry
        .stop_background_agents_for_budget(&announce)
        .await
    else {
        return 0;
    };
    stopped
}

/// Print mode remains alive for delegated work and shells, then tears down
/// connection-owned monitors/parked workers before returning to its caller.
pub(super) async fn wind_down_print_tasks(
    runtime: &Runtime,
    max_budget_usd: Option<f64>,
    shutdown: tokio_util::sync::CancellationToken,
    control_plane: Option<&Arc<StdioControlPlane>>,
) -> Result<(), orchestrator::OrchestratorError> {
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        let cost = runtime.orchestrator.snapshot_cost().await;
        if max_budget_usd.is_some_and(|limit| budget_reached(limit, cost.total_nano_usd)) {
            break;
        }
        let records = platform_api::task_registry::TaskRegistryHandle::list(
            runtime.task_registry.as_ref(),
            platform_api::task_registry::TaskListFilter::default(),
        )
        .await
        .unwrap_or_default();
        let has_work = records.iter().any(print_task_keeps_session_alive);
        // Monitors are subscriptions, not jobs that can finish naturally. Stop
        // them before draining their final batch once no finite work remains.
        if !has_work {
            stop_print_subscriptions(runtime).await;
        }
        if runtime
            .task_registry
            .has_pending_task_notifications_for(None)
            .await
        {
            let cancel = shutdown.child_token();
            if let Some(plane) = control_plane {
                plane.set_active_turn(cancel.clone()).await;
            }
            let result = runtime
                .orchestrator
                .run_task_notification_rewake(runtime.task_registry.as_ref(), cancel)
                .await;
            if let Some(plane) = control_plane {
                plane.clear_active_turn().await;
            }
            match result {
                Err(error) => {
                    stop_print_tasks(runtime).await;
                    return Err(error);
                }
                Ok(orchestrator::conversation::TurnOutcome::Cancelled) => break,
                Ok(_) => {}
            }
            continue;
        }
        if !has_work {
            break;
        }
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
        }
    }
    stop_print_tasks(runtime).await;
    Ok(())
}

pub(super) fn print_task_keeps_session_alive(
    task: &platform_api::task_registry::TaskRecord,
) -> bool {
    platform_api::task_activity::is_active_delegated_task(task)
        || (platform_api::task_activity::is_live_shell_task(task)
            && task.kind.as_deref() != Some("monitor"))
}

pub(super) async fn stop_print_subscriptions(runtime: &Runtime) {
    stop_print_tasks_matching(runtime, true).await;
}

pub(super) async fn stop_print_tasks(runtime: &Runtime) {
    stop_print_tasks_matching(runtime, false).await;
}

pub(super) async fn stop_print_tasks_matching(runtime: &Runtime, subscriptions_only: bool) {
    if let Ok(records) = platform_api::task_registry::TaskRegistryHandle::list(
        runtime.task_registry.as_ref(),
        platform_api::task_registry::TaskListFilter::default(),
    )
    .await
    {
        for task in records.into_iter().filter(|task| {
            (matches!(
                task.status.as_str(),
                "running" | "pending" | "paused" | "queued"
            ) || task.is_parked)
                && (!subscriptions_only
                    || task.kind.as_deref() == Some("monitor")
                    || matches!(task.task_type.as_str(), "monitor_mcp" | "monitor_ws"))
        }) {
            let _ = runtime.task_registry.mark_notified(&task.task_id).await;
            let _ = runtime
                .task_registry
                .kill_with_reason(&task.task_id, "user")
                .await;
        }
    }
}

pub(super) fn budget_reached(max_budget_usd: f64, total_nano_usd: u64) -> bool {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let budget_nano_usd = (max_budget_usd.max(0.0) * 1_000_000_000.0) as u64;
    total_nano_usd >= budget_nano_usd
}

pub(super) fn budget_halt_notice(total_usd: f64, max_budget_usd: f64) -> String {
    let total_usd = js_to_fixed_2(total_usd);
    format!("Budget limit reached (${total_usd} of ${max_budget_usd}); stopping background agents.")
}

/// Enable process-tree tracking for print/SDK mode. Catchable signal handling
/// is awaited by the owning print future below; it must not call
/// `std::process::exit` from a detached task because that bypasses durable
/// response/outbox settlement.
pub(super) fn install_print_mode_process_cleanup() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    platform_posix::process::enable_print_mode_child_cleanup();
}

#[cfg(unix)]
pub(super) async fn print_shutdown_signal() -> i32 {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut term), Ok(mut hup), Ok(mut intr)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::hangup()),
        signal(SignalKind::interrupt()),
    ) else {
        return futures::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => nix::libc::SIGTERM,
        _ = hup.recv() => nix::libc::SIGHUP,
        _ = intr.recv() => nix::libc::SIGINT,
    }
}

#[cfg(not(unix))]
pub(super) async fn print_shutdown_signal() -> i32 {
    futures::future::pending().await
}

pub(super) async fn run_print_owned<F>(runtime: &Runtime, operation: F) -> i32
where
    F: std::future::Future<Output = i32>,
{
    run_print_owned_with_cleanup(runtime, operation, futures::future::ready(())).await
}

pub(super) async fn run_print_owned_with_cleanup<F, C>(
    runtime: &Runtime,
    operation: F,
    cleanup: C,
) -> i32
where
    F: std::future::Future<Output = i32>,
    C: std::future::Future<Output = ()>,
{
    install_print_mode_process_cleanup();
    let mut operation = Box::pin(operation);
    let outcome = tokio::select! {
        biased;
        signal = print_shutdown_signal() => Err(signal),
        code = &mut operation => Ok(code),
    };
    let code = match outcome {
        Ok(code) => code,
        Err(signal) => {
            // Drop the foreground turn first. Its response receipt transfers
            // any known usage to the retained settlement owner before the
            // lifecycle drain takes its FIFO fence.
            drop(operation);
            platform_posix::process::kill_all_active_children();
            runtime.orchestrator.request_exit().await;
            128 + signal
        }
    };
    cleanup.await;
    finish_oneshot_lifecycle(runtime, code).await
}

#[derive(Default)]
pub(super) struct PrintAuxTaskGroup {
    pub(super) tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl PrintAuxTaskGroup {
    pub(super) fn push(&self, task: tokio::task::JoinHandle<()>) {
        self.tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(task);
    }

    pub(super) async fn abort_and_join(&self) {
        let tasks = std::mem::take(
            &mut *self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    pub(super) async fn join(&self) {
        let tasks = std::mem::take(
            &mut *self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// Every branch that can invoke tools owns the same print shutdown boundary.
pub(super) async fn finish_print_branch(
    runtime: &Runtime,
    budget: Option<f64>,
    sink: &dyn OutputSink,
    code: i32,
) -> i32 {
    if code != exit_codes::SUCCESS {
        stop_print_tasks(runtime).await;
        return code;
    }
    match wind_down_print_tasks(
        runtime,
        budget,
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    {
        Ok(()) => code,
        Err(error) => {
            sink.error("runtime", &error.to_string()).await;
            exit_codes::RUNTIME_ERROR
        }
    }
}

pub(super) async fn finish_oneshot_lifecycle(runtime: &Runtime, code: i32) -> i32 {
    let report = runtime.session_lifecycle.shutdown_and_drain().await;
    for error in &report.errors {
        eprintln!("lingxi-cli: session shutdown persistence failed: {error}");
    }
    if code == exit_codes::SUCCESS && !report.errors.is_empty() {
        exit_codes::RUNTIME_ERROR
    } else {
        code
    }
}
