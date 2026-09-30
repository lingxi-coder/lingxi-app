//! Host half of the supervised-shell ownership transaction.
use crate::background_launch::{self, ShellHandoffAck};
use lingxi_core::host::shell_handoff::ShellTaskHandoff;
use lingxi_core::host::task_registry::TaskRegistryHandle;
use std::path::Path;

fn ack(ids: Vec<String>, error: Option<String>) -> ShellHandoffAck {
    ShellHandoffAck {
        task_ids: ids,
        error,
        source: None,
    }
}

pub(crate) async fn finish_source(
    home: &Path,
    short: &str,
    registry: &dyn TaskRegistryHandle,
    exported: &[ShellTaskHandoff],
) -> Result<(), String> {
    if exported.is_empty() {
        return Ok(());
    }
    let prepared = background_launch::wait_shell_handoff_ack(home, short, "ready").await;
    let ready = match prepared {
        Ok(ready) if ready.error.is_none() => ready,
        result => {
            let error = match result {
                Ok(ack) => ack.error.unwrap(),
                Err(error) => error.to_string(),
            };
            abort_source(home, short, registry, exported, &error).await?;
            return Err(error);
        }
    };
    let expected: Vec<_> = exported.iter().map(|task| task.task_id.clone()).collect();
    if ready.task_ids != expected {
        let error = "destination shell preparation acknowledged a different roster".to_string();
        abort_source(home, short, registry, exported, &error).await?;
        return Err(error);
    }
    // Natural completion and ownership transfer serialize inside the registry.
    // Only ids accepted by that atomic operation belong to the destination.
    let accepted = match registry.commit_shell_handoff(&expected).await {
        Ok(accepted) => accepted,
        Err(error) => {
            let message = error.to_string();
            abort_source(home, short, registry, exported, &message).await?;
            return Err(message);
        }
    };
    let transferred: Vec<_> = exported
        .iter()
        .filter(|task| accepted.contains(&task.task_id))
        .cloned()
        .collect();
    if let Err(error) = background_launch::write_shell_handoff_ack(
        home,
        short,
        "commit",
        &ack(accepted.clone(), None),
    ) {
        // Atomic persistence can report an error after rename. Never reclaim
        // ownership if the worker might already have observed the commit.
        match background_launch::read_shell_handoff_ack(home, short, "commit") {
            Ok(Some(committed)) if committed.task_ids == accepted && committed.error.is_none() => {}
            Ok(None) => {
                background_launch::write_shell_handoff_ack(home, short, "abort", &ack(Vec::new(), Some(error.to_string())))
                    .map_err(|abort| format!("commit persistence failed ({error}); could not fence destination ({abort})"))?;
                registry.rollback_shell_handoff(exported).await.map_err(|rollback| format!("commit persistence failed ({error}); ownership recovery failed ({rollback})"))?;
                return Err(error.to_string());
            }
            _ => {
                return Err(format!(
                    "shell handoff ownership is uncertain after commit persistence failed: {error}"
                ))
            }
        }
    }
    // A shell that completed during preparation remains fenced until the
    // accepted subset is durable; only then may its source notification drain.
    let retained: Vec<_> = exported
        .iter()
        .filter(|task| !accepted.contains(&task.task_id))
        .cloned()
        .collect();
    registry
        .rollback_shell_handoff(&retained)
        .await
        .map_err(|error| error.to_string())?;
    // A durable commit is recoverable by this background job after a restart.
    // On timeout do not steal ownership back from a possibly active worker.
    let adopted = background_launch::wait_shell_handoff_ack(home, short, "adopted").await
        .map_err(|error| format!("background session {short} owns the shell handoff, but restoration has not acknowledged it: {error}"))?;
    if let Some(error) = adopted.error {
        // Batch adoption reports an error before publishing any destination row.
        background_launch::write_shell_handoff_ack(
            home,
            short,
            "abort",
            &ack(Vec::new(), Some(error.clone())),
        )
        .map_err(|abort| {
            format!("adoption failed ({error}); could not fence destination ({abort})")
        })?;
        registry
            .rollback_shell_handoff(&transferred)
            .await
            .map_err(|rollback| {
                format!("adoption failed ({error}); ownership recovery failed ({rollback})")
            })?;
        return Err(error);
    }
    if adopted.task_ids != accepted {
        return Err("destination shell adoption acknowledged a different roster".into());
    }
    Ok(())
}

async fn abort_source(
    home: &Path,
    short: &str,
    registry: &dyn TaskRegistryHandle,
    exported: &[ShellTaskHandoff],
    reason: &str,
) -> Result<(), String> {
    background_launch::write_shell_handoff_ack(
        home,
        short,
        "abort",
        &ack(Vec::new(), Some(reason.into())),
    )
    .map_err(|error| {
        format!(
            "could not fence failed handoff; source notification claims remain reserved: {error}"
        )
    })?;
    registry
        .rollback_shell_handoff(exported)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn restore_destination(
    home: &Path,
    short: &str,
    registry: &dyn TaskRegistryHandle,
    exported: &[ShellTaskHandoff],
) -> Result<(), String> {
    if exported.is_empty() {
        return Ok(());
    }
    let ids = exported.iter().map(|task| task.task_id.clone()).collect();
    if let Err(error) = registry.prepare_shell_handoff(exported).await {
        let message = error.to_string();
        let _ = background_launch::write_shell_handoff_ack(
            home,
            short,
            "ready",
            &ack(Vec::new(), Some(message.clone())),
        );
        return Err(message);
    }
    background_launch::write_shell_handoff_ack(home, short, "ready", &ack(ids, None))
        .map_err(|error| error.to_string())?;
    let committed =
        wait_for_source_decision(home, short, &crate::daemon_roster::SystemProbe).await?;
    if let Some(error) = committed.error {
        return Err(error);
    }
    if committed
        .task_ids
        .iter()
        .any(|id| !exported.iter().any(|task| &task.task_id == id))
    {
        return Err("source committed an unprepared shell identity".into());
    }
    let accepted: Vec<_> = exported
        .iter()
        .filter(|task| committed.task_ids.contains(&task.task_id))
        .cloned()
        .collect();
    if let Err(error) = registry.adopt_shell_handoff(&accepted).await {
        let message = error.to_string();
        persist_adoption_ack(|| {
            background_launch::write_shell_handoff_ack(
                home,
                short,
                "adopted",
                &ack(Vec::new(), Some(message.clone())),
            )
        })
        .await;
        return Err(message);
    }
    persist_adoption_ack(|| {
        background_launch::write_shell_handoff_ack(
            home,
            short,
            "adopted",
            &ack(committed.task_ids.clone(), None),
        )
    })
    .await;
    Ok(())
}

/// Once adoption has published observers, an ACK storage failure must never
/// drop the worker runtime. Keep ownership alive until its decision is durable.
async fn persist_adoption_ack(mut write: impl FnMut() -> std::io::Result<()>) {
    loop {
        match write() {
            Ok(()) => return,
            Err(error) => {
                tracing::warn!(%error, "shell adoption acknowledgement unavailable; retaining worker ownership")
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Persist before dispatching the worker: a crashed source cannot strand an
/// exported supervised shell between registry release and the final decision.
pub(crate) fn write_source_intent(
    home: &Path,
    short: &str,
    tasks: &[ShellTaskHandoff],
) -> std::io::Result<()> {
    if tasks.is_empty() {
        return Ok(());
    }
    let pid = i32::try_from(std::process::id())
        .map_err(|_| std::io::Error::other("source pid overflow"))?;
    let process_start = crate::daemon_roster::read_proc_start(pid)
        .ok_or_else(|| std::io::Error::other("source OS process identity unavailable"))?;
    let mut intent = ack(
        tasks.iter().map(|task| task.task_id.clone()).collect(),
        None,
    );
    intent.source = Some(background_launch::ShellHandoffSource { pid, process_start });
    background_launch::write_shell_handoff_ack(home, short, "intent", &intent)
}

fn source_is_gone(
    source: &background_launch::ShellHandoffSource,
    probe: &(dyn crate::daemon_roster::ProcProbe + Sync),
) -> bool {
    !probe.is_alive(source.pid)
        || probe
            .start_time(source.pid)
            .is_some_and(|start| start != source.process_start)
}

async fn wait_for_source_decision(
    home: &Path,
    short: &str,
    probe: &(dyn crate::daemon_roster::ProcProbe + Sync),
) -> Result<ShellHandoffAck, String> {
    loop {
        if let Some(abort) = background_launch::read_shell_handoff_ack(home, short, "abort")
            .map_err(|error| error.to_string())?
        {
            return Err(abort
                .error
                .unwrap_or_else(|| "source aborted shell handoff".into()));
        }
        if let Some(commit) = background_launch::read_shell_handoff_ack(home, short, "commit")
            .map_err(|error| error.to_string())?
        {
            return Ok(commit);
        }
        if let Some(intent) = background_launch::read_shell_handoff_ack(home, short, "intent")
            .map_err(|error| error.to_string())?
        {
            if intent
                .source
                .as_ref()
                .is_some_and(|source| source_is_gone(source, probe))
            {
                let recovered = ack(intent.task_ids, None);
                background_launch::write_shell_handoff_ack(home, short, "commit", &recovered)
                    .map_err(|error| error.to_string())?;
                tracing::warn!(short, task_ids = ?recovered.task_ids, "recovering prepared shells after source process exited before handoff decision");
                return Ok(recovered);
            }
        }
        // An alive source may still be completing its atomic registry commit.
        // Do not abandon prepared ownership solely because it is slow.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_roster::ProcProbe;

    struct Probe {
        alive: bool,
        start: Option<String>,
    }
    impl ProcProbe for Probe {
        fn is_alive(&self, _pid: i32) -> bool {
            self.alive
        }
        fn start_time(&self, _pid: i32) -> Option<String> {
            self.start.clone()
        }
    }
    fn prepared(home: &Path) {
        let mut intent = ack(vec!["bshell001".into()], None);
        intent.source = Some(background_launch::ShellHandoffSource {
            pid: 42,
            process_start: "original-start".into(),
        });
        background_launch::write_shell_handoff_ack(home, "feed1234", "intent", &intent).unwrap();
    }

    #[tokio::test]
    async fn source_crash_after_intent_before_decision_recovers_stable_shell_identity() {
        let home = tempfile::tempdir().unwrap();
        prepared(home.path());
        let recovered = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_source_decision(
                home.path(),
                "feed1234",
                &Probe {
                    alive: false,
                    start: None,
                },
            ),
        )
        .await
        .expect("source death must recover prepared ownership")
        .unwrap();
        assert_eq!(recovered.task_ids, vec!["bshell001"]);
        assert_eq!(
            background_launch::read_shell_handoff_ack(home.path(), "feed1234", "commit").unwrap(),
            Some(recovered)
        );
    }

    #[tokio::test]
    async fn live_or_unverifiable_source_is_not_stolen_and_reused_pid_recovers() {
        let home = tempfile::tempdir().unwrap();
        prepared(home.path());
        for start in [Some("original-start".into()), None] {
            let probe = Probe { alive: true, start };
            assert!(tokio::time::timeout(
                std::time::Duration::from_millis(20),
                wait_for_source_decision(home.path(), "feed1234", &probe)
            )
            .await
            .is_err());
            assert!(
                background_launch::read_shell_handoff_ack(home.path(), "feed1234", "commit")
                    .unwrap()
                    .is_none()
            );
        }
        let recovered = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_source_decision(
                home.path(),
                "feed1234",
                &Probe {
                    alive: true,
                    start: Some("replacement-start".into()),
                },
            ),
        )
        .await
        .expect("source death must recover prepared ownership")
        .unwrap();
        assert_eq!(recovered.task_ids, vec!["bshell001"]);
    }

    #[tokio::test]
    async fn adopted_ack_write_failure_keeps_owner_alive_until_retry_succeeds() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let write = || {
            if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                Err(std::io::Error::other("injected full filesystem"))
            } else {
                Ok(())
            }
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            persist_adoption_ack(write),
        )
        .await
        .unwrap();
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "first storage error cannot terminate the destination owner"
        );
    }
    use lingxi_core::host::task_registry::*;
    struct Registry {
        source: bool,
        fail_adopt: bool,
        ownership: std::sync::Arc<std::sync::Mutex<(bool, bool)>>,
    }
    #[async_trait::async_trait]
    impl TaskRegistryHandle for Registry {
        async fn create(&self, _: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn get(&self, _: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            unreachable!()
        }
        async fn list(&self, _: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            unreachable!()
        }
        async fn update(
            &self,
            _: &str,
            _: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn set_status(&self, _: &str, _: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn kill(&self, _: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn output(
            &self,
            _: &str,
            _: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            unreachable!()
        }
        async fn prepare_shell_handoff(
            &self,
            _: &[ShellTaskHandoff],
        ) -> Result<(), TaskRegistryError> {
            Ok(())
        }
        async fn commit_shell_handoff(
            &self,
            ids: &[String],
        ) -> Result<Vec<String>, TaskRegistryError> {
            assert!(self.source);
            let mut state = self.ownership.lock().unwrap();
            assert!(state.0 && !state.1);
            state.0 = false;
            Ok(ids.to_vec())
        }
        async fn adopt_shell_handoff(
            &self,
            _: &[ShellTaskHandoff],
        ) -> Result<(), TaskRegistryError> {
            if self.fail_adopt {
                return Err(TaskRegistryError::Internal(
                    "injected native attach failure".into(),
                ));
            }
            let mut state = self.ownership.lock().unwrap();
            assert!(
                !state.0 && !state.1,
                "destination cannot publish before source commit"
            );
            state.1 = true;
            Ok(())
        }
        async fn rollback_shell_handoff(
            &self,
            records: &[ShellTaskHandoff],
        ) -> Result<(), TaskRegistryError> {
            if records.is_empty() {
                return Ok(());
            }
            let mut state = self.ownership.lock().unwrap();
            assert!(
                !state.1,
                "rollback cannot steal acknowledged destination ownership"
            );
            state.0 = true;
            Ok(())
        }
    }
    fn shell() -> ShellTaskHandoff {
        ShellTaskHandoff {
            task_id: "bshell001".into(),
            command: "sleep 1".into(),
            description: "shell".into(),
            tool_use_id: None,
            creator_agent_id: None,
            cwd: None,
            caller: Some("turn".into()),
            output_offset: 12,
            process: lingxi_core::host::process::ShellProcessHandoff {
                supervisor_directory_identity: None,
                output_root_identity: None,
                output_file_identity: None,
                task_id: "bshell001".into(),
                pid: 123,
                supervisor_pid: 124,
                supervisor_start_identity: None,
                process_start_identity: None,
                owner: None,
                socket_path: "/private/supervisor/socket".into(),
                receipt_path: "/private/supervisor/receipt".into(),
                output_path: "/private/tasks/bshell001.output".into(),
                nonce: "fixture-capability".into(),
            },
        }
    }
    #[tokio::test]
    async fn shell_transfer_ack_commits_once_and_failed_adoption_restores_source() {
        for fail_adopt in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let ownership = std::sync::Arc::new(std::sync::Mutex::new((true, false)));
            let source = Registry {
                source: true,
                fail_adopt: false,
                ownership: ownership.clone(),
            };
            let destination = Registry {
                source: false,
                fail_adopt,
                ownership: ownership.clone(),
            };
            let exported = vec![shell()];
            write_source_intent(home.path(), "feed1234", &exported).unwrap();
            let (sent, received) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                tokio::join!(
                    finish_source(home.path(), "feed1234", &source, &exported),
                    restore_destination(home.path(), "feed1234", &destination, &exported)
                )
            })
            .await
            .expect("three-phase transfer must complete");
            assert_eq!(sent.is_ok(), !fail_adopt);
            assert_eq!(received.is_ok(), !fail_adopt);
            assert_eq!(*ownership.lock().unwrap(), (fail_adopt, !fail_adopt));
            if fail_adopt {
                assert!(background_launch::read_shell_handoff_ack(
                    home.path(),
                    "feed1234",
                    "abort"
                )
                .unwrap()
                .is_some());
            }
        }
    }
}
