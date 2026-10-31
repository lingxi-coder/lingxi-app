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

/// Retained source authority for one admitted shell handoff. The lifecycle
/// retries this object after its producer has stopped, including after a panic.
pub(crate) struct SourceHandoffRecovery {
    home: std::path::PathBuf,
    session_id: String,
    registry: std::sync::Arc<dyn TaskRegistryHandle>,
    progress: std::sync::Mutex<SourceProgress>,
}

#[derive(Default)]
struct SourceProgress {
    // None means export has not returned. The production registry's export
    // guard releases every fence on error or unwind before returning.
    exported: Option<Vec<ShellTaskHandoff>>,
    short: Option<String>,
    finish_started: bool,
    restored: std::collections::HashSet<String>,
}

impl SourceHandoffRecovery {
    pub(crate) fn new(
        home: std::path::PathBuf,
        session_id: String,
        registry: std::sync::Arc<dyn TaskRegistryHandle>,
    ) -> Self {
        Self {
            home,
            session_id,
            registry,
            progress: std::sync::Mutex::new(SourceProgress::default()),
        }
    }

    pub(crate) fn record_export(&self, exported: Vec<ShellTaskHandoff>) {
        self.progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .exported = Some(exported);
    }

    pub(crate) async fn export(&self) -> Result<Vec<ShellTaskHandoff>, String> {
        self.registry
            .export_shell_handoff()
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) fn record_short(&self, short: &str) {
        self.progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .short = Some(short.to_owned());
    }

    fn snapshot(&self) -> (Option<String>, Option<Vec<ShellTaskHandoff>>, bool) {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            progress.short.clone(),
            progress.exported.clone(),
            progress.finish_started,
        )
    }

    /// Remember successful rollback before any further await. A completed row
    /// may be notified and evicted, so recovery must not adopt it again.
    async fn rollback(&self, records: &[ShellTaskHandoff]) -> Result<(), String> {
        let pending: Vec<_> = {
            let progress = self
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            records
                .iter()
                .filter(|record| !progress.restored.contains(&record.task_id))
                .cloned()
                .collect()
        };
        if pending.is_empty() {
            return Ok(());
        }
        self.registry
            .rollback_shell_handoff(&pending)
            .await
            .map_err(|error| error.to_string())?;
        let mut progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        progress
            .restored
            .extend(pending.into_iter().map(|record| record.task_id));
        Ok(())
    }

    /// Fence the destination before reclaiming anything. Even a failed atomic
    /// write can have renamed its file, so verify the durable result explicitly.
    fn fence(&self, short: &str, reason: &str) -> Result<(), String> {
        let valid = |ack: &ShellHandoffAck| {
            ack.task_ids.is_empty() && ack.error.is_some() && ack.source.is_none()
        };
        match background_launch::read_shell_handoff_ack(&self.home, short, "abort") {
            Ok(Some(existing)) if valid(&existing) => return Ok(()),
            Ok(Some(_)) => return Err("invalid shell handoff abort acknowledgement".into()),
            Ok(None) => {}
            Err(error) => return Err(format!("could not inspect shell handoff abort: {error}")),
        }
        let written = background_launch::write_shell_handoff_ack(
            &self.home,
            short,
            "abort",
            &ack(Vec::new(), Some(reason.into())),
        );
        match background_launch::read_shell_handoff_ack(&self.home, short, "abort") {
            Ok(Some(existing)) if valid(&existing) => Ok(()),
            observed => Err(format!(
                "could not confirm destination fence: write={written:?}, read={observed:?}"
            )),
        }
    }

    async fn abort_source(
        &self,
        short: &str,
        exported: &[ShellTaskHandoff],
        reason: &str,
    ) -> Result<(), String> {
        self.fence(short, reason)
            .map_err(|error| format!("{reason}; destination fence failed: {error}"))?;
        self.rollback(exported)
            .await
            .map_err(|error| format!("{reason}; shell recovery failed: {error}"))
    }

    pub(crate) async fn finish_source(&self) -> Result<(), String> {
        self.progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish_started = true;
        let (short, exported, _) = self.snapshot();
        let exported = exported.ok_or_else(|| "shell export did not complete".to_string())?;
        if exported.is_empty() {
            return Ok(());
        }
        let short = short.ok_or_else(|| "background launch identity is unavailable".to_string())?;
        let prepared = background_launch::wait_shell_handoff_ack(&self.home, &short, "ready").await;
        let ready = match prepared {
            Ok(ready) if ready.error.is_none() => ready,
            result => {
                let error = match result {
                    Ok(ack) => ack.error.unwrap(),
                    Err(error) => error.to_string(),
                };
                self.abort_source(&short, &exported, &error).await?;
                return Err(error);
            }
        };
        let expected: Vec<_> = exported.iter().map(|task| task.task_id.clone()).collect();
        if ready.source.is_some() || ready.task_ids != expected {
            let error = "destination shell preparation acknowledged a different roster".to_string();
            self.abort_source(&short, &exported, &error).await?;
            return Err(error);
        }
        let accepted = match self.registry.commit_shell_handoff(&expected).await {
            Ok(accepted) => accepted,
            Err(error) => {
                let message = error.to_string();
                self.abort_source(&short, &exported, &message).await?;
                return Err(message);
            }
        };
        let transferred: Vec<_> = exported
            .iter()
            .filter(|task| accepted.contains(&task.task_id))
            .cloned()
            .collect();
        if let Err(error) = background_launch::write_shell_handoff_ack(
            &self.home,
            &short,
            "commit",
            &ack(accepted.clone(), None),
        ) {
            match background_launch::read_shell_handoff_ack(&self.home, &short, "commit") {
                Ok(Some(committed))
                    if committed.task_ids == accepted
                        && committed.error.is_none()
                        && committed.source.is_none() => {}
                Ok(None) => {
                    self.abort_source(&short, &exported, &error.to_string())
                        .await?;
                    return Err(error.to_string());
                }
                _ => {
                    return Err(format!(
                        "shell handoff ownership is uncertain after commit persistence failed: {error}"
                    ));
                }
            }
        }
        let retained: Vec<_> = exported
            .iter()
            .filter(|task| !accepted.contains(&task.task_id))
            .cloned()
            .collect();
        self.rollback(&retained)
            .await
            .map_err(|error| format!("could not restore retained source shells: {error}"))?;
        let adopted = background_launch::wait_shell_handoff_ack(&self.home, &short, "adopted").await
            .map_err(|error| format!("background session {short} owns the shell handoff, but restoration has not acknowledged it: {error}"))?;
        if let Some(error) = adopted.error {
            // The destination reports batch adoption failure before publishing rows.
            if !adopted.task_ids.is_empty() || adopted.source.is_some() {
                return Err("invalid failed shell adoption acknowledgement".into());
            }
            self.abort_source(&short, &transferred, &error).await?;
            return Err(error);
        }
        if adopted.source.is_some() || adopted.task_ids != accepted {
            return Err("destination shell adoption acknowledged a different roster".into());
        }
        Ok(())
    }

    async fn recover_source(&self) -> Result<(), String> {
        let (short, exported, finish_started) = self.snapshot();
        let Some(exported) = exported else {
            return Ok(());
        };
        if exported.is_empty() {
            return Ok(());
        }
        {
            let progress = self
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if exported
                .iter()
                .all(|task| progress.restored.contains(&task.task_id))
            {
                return Ok(());
            }
        }
        if !finish_started {
            // The source has not attempted any commit, so a confirmed abort
            // fences even a partly published launch before source recovery.
            if let Some(short) = short {
                self.fence(
                    &short,
                    "source recovered interrupted background publication",
                )?;
            }
            return self.rollback(&exported).await;
        }
        let short = short.ok_or_else(|| {
            "background launch identity is unavailable after handoff started".to_string()
        })?;
        // Only this session's launch can be fenced. Once publication started,
        // even a missing spec is unknown: a completed job may have been removed.
        let launch = match background_launch::read_launch_spec(&self.home, &short) {
            Ok(launch) => launch,
            Err(error) => {
                return Err(format!(
                    "could not inspect background launch {short}: {error}"
                ))
            }
        };
        let expected: Vec<_> = exported.iter().map(|task| task.task_id.clone()).collect();
        let launch_ids: Vec<_> = launch
            .shell_handoff
            .iter()
            .map(|task| task.task_id.clone())
            .collect();
        if launch.session_id != self.session_id || launch_ids != expected {
            return Err(format!(
                "background launch {short} does not match the recorded shell handoff"
            ));
        }
        let commit = background_launch::read_shell_handoff_ack(&self.home, &short, "commit")
            .map_err(|error| format!("could not inspect shell handoff commit: {error}"))?;
        let Some(commit) = commit else {
            self.fence(&short, "source recovered interrupted shell handoff")?;
            return self.rollback(&exported).await;
        };
        let mut unique = std::collections::HashSet::new();
        if commit.error.is_some()
            || commit.source.is_some()
            || commit
                .task_ids
                .iter()
                .any(|id| !expected.contains(id) || !unique.insert(id.clone()))
        {
            return Err("invalid shell handoff commit roster".into());
        }
        let retained: Vec<_> = exported
            .iter()
            .filter(|task| !unique.contains(&task.task_id))
            .cloned()
            .collect();
        self.rollback(&retained).await?;
        if commit.task_ids.is_empty() {
            return Ok(());
        }
        let adopted = background_launch::read_shell_handoff_ack(&self.home, &short, "adopted")
            .map_err(|error| format!("could not inspect shell handoff adoption: {error}"))?
            .ok_or_else(|| {
                format!("background session {short} has not acknowledged shell adoption")
            })?;
        if let Some(error) = adopted.error {
            if !adopted.task_ids.is_empty() || adopted.source.is_some() {
                return Err("invalid failed shell adoption acknowledgement".into());
            }
            self.fence(&short, &error)?;
            let transferred: Vec<_> = exported
                .iter()
                .filter(|task| unique.contains(&task.task_id))
                .cloned()
                .collect();
            return self.rollback(&transferred).await;
        }
        if adopted.source.is_some() || adopted.task_ids != commit.task_ids {
            return Err("destination shell adoption acknowledged a different roster".into());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl lingxi_core::host::bg_session_forker::lifecycle::BackgroundLaunchRecovery
    for SourceHandoffRecovery
{
    fn description(&self) -> String {
        format!("shell handoff for background session {}", self.session_id)
    }

    async fn recover(&self) -> Result<(), String> {
        self.recover_source().await
    }
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
        rollback_failures: std::sync::atomic::AtomicUsize,
        rollback_calls: std::sync::atomic::AtomicUsize,
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
            self.rollback_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self
                .rollback_failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                return Err(TaskRegistryError::Internal(
                    "injected rollback failure".into(),
                ));
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
            let source = std::sync::Arc::new(Registry {
                source: true,
                fail_adopt: false,
                ownership: ownership.clone(),
                rollback_failures: std::sync::atomic::AtomicUsize::new(0),
                rollback_calls: std::sync::atomic::AtomicUsize::new(0),
            });
            let destination = Registry {
                source: false,
                fail_adopt,
                ownership: ownership.clone(),
                rollback_failures: std::sync::atomic::AtomicUsize::new(0),
                rollback_calls: std::sync::atomic::AtomicUsize::new(0),
            };
            let exported = vec![shell()];
            let recovery =
                SourceHandoffRecovery::new(home.path().to_path_buf(), "session".into(), source);
            recovery.record_short("feed1234");
            recovery.record_export(exported.clone());
            write_source_intent(home.path(), "feed1234", &exported).unwrap();
            let (sent, received) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                tokio::join!(
                    recovery.finish_source(),
                    restore_destination(home.path(), "feed1234", &destination, &exported)
                )
            })
            .await
            .expect("three-phase transfer must complete");
            assert_eq!(sent.is_ok(), !fail_adopt);
            assert_eq!(received.is_ok(), !fail_adopt);
            assert_eq!(*ownership.lock().unwrap(), (fail_adopt, !fail_adopt));
            if fail_adopt {
                let failure = sent.unwrap_err();
                // The failed adoption was fenced and restored to the source,
                // so a retry confirms that this error has settled ownership.
                recovery.recover_source().await.unwrap();
                let lifecycle = std::sync::Arc::new(
                    lingxi_core::host::bg_session_forker::lifecycle::BackgroundLaunchLifecycle::default(),
                );
                let error = lingxi_core::host::bg_session_forker::BgForkError::Dispatch(failure);
                assert!(lifecycle
                    .claim()
                    .unwrap()
                    .run_owned(async move { Err::<(), _>(error) })
                    .await
                    .is_err());
                assert!(lifecycle
                    .close_and_drain()
                    .await
                    .unconfirmed_recoveries
                    .is_empty());
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

    fn retry_registry(failures: usize) -> std::sync::Arc<Registry> {
        std::sync::Arc::new(Registry {
            source: true,
            fail_adopt: false,
            ownership: std::sync::Arc::new(std::sync::Mutex::new((true, false))),
            rollback_failures: std::sync::atomic::AtomicUsize::new(failures),
            rollback_calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn published_recovery(
        home: &Path,
        registry: std::sync::Arc<Registry>,
    ) -> std::sync::Arc<SourceHandoffRecovery> {
        use background_launch::{
            BackgroundLaunchKind, BackgroundLaunchOptions, BackgroundLaunchSpec, TerminalSize,
            LAUNCH_SPEC_VERSION,
        };
        let exported = vec![shell()];
        let recovery = std::sync::Arc::new(SourceHandoffRecovery::new(
            home.to_path_buf(),
            "session".into(),
            registry,
        ));
        recovery.record_export(exported.clone());
        recovery.record_short("feed1234");
        let spec = BackgroundLaunchSpec {
            schema_version: LAUNCH_SPEC_VERSION,
            short: "feed1234".into(),
            created_at: 0,
            preflight_approved: true,
            launch: BackgroundLaunchKind::Fork,
            session_id: "session".into(),
            transcript_path: "/tmp/session.jsonl".into(),
            cwd: "/tmp".into(),
            origin_cwd: "/tmp".into(),
            worktree_path: None,
            worktree_ownership_token: None,
            initial_prompt: None,
            handoff: None,
            shell_handoff: exported,
            options: BackgroundLaunchOptions::default(),
            env: Default::default(),
            terminal: TerminalSize::default(),
        };
        background_launch::write_launch_spec(home, "feed1234", &spec).unwrap();
        recovery
    }

    #[tokio::test]
    async fn cancelled_caller_preserves_error_and_retries_real_rollback() {
        use lingxi_core::host::bg_session_forker::{
            lifecycle::BackgroundLaunchLifecycle, BgForkError,
        };
        let home = tempfile::tempdir().unwrap();
        let registry = retry_registry(2);
        let recovery = published_recovery(home.path(), registry.clone());
        let lifecycle = std::sync::Arc::new(BackgroundLaunchLifecycle::default());
        let claim = lifecycle.claim().unwrap();
        let ticket = claim.register_recovery(recovery.clone());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let proceed = std::sync::Arc::new(tokio::sync::Notify::new());
        let owner_gate = proceed.clone();
        let owner_recovery = recovery.clone();
        let caller = tokio::spawn(claim.run_owned(async move {
            entered_tx.send(()).unwrap();
            owner_gate.notified().await;
            assert!(owner_recovery.recover_source().await.is_err());
            drop(ticket);
            Err::<(), _>(BgForkError::OwnershipUncertain(
                "publication failed and rollback remains pending".into(),
            ))
        }));
        entered_rx.await.unwrap();
        caller.abort();
        proceed.notify_one();
        let first = lifecycle.close_and_drain().await;
        assert!(!first.unconfirmed_recoveries.is_empty());
        assert!(first
            .diagnostics
            .iter()
            .any(|error| error.contains("publication failed")));
        assert!(
            background_launch::read_shell_handoff_ack(home.path(), "feed1234", "abort")
                .unwrap()
                .is_some()
        );
        assert_eq!(*registry.ownership.lock().unwrap(), (true, false));
        let retry = lifecycle.close_and_drain().await;
        assert!(
            retry.unconfirmed_recoveries.is_empty(),
            "{:?}",
            retry.unconfirmed_recoveries
        );
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            3
        );
    }

    #[tokio::test]
    async fn panic_after_commit_uses_ack_recovery_and_does_not_steal_destination() {
        use lingxi_core::host::bg_session_forker::lifecycle::BackgroundLaunchLifecycle;
        let home = tempfile::tempdir().unwrap();
        let registry = retry_registry(0);
        *registry.ownership.lock().unwrap() = (false, true);
        let recovery = published_recovery(home.path(), registry.clone());
        recovery.progress.lock().unwrap().finish_started = true;
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "commit",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "adopted",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        let lifecycle = std::sync::Arc::new(BackgroundLaunchLifecycle::default());
        let claim = lifecycle.claim().unwrap();
        let ticket = claim.register_recovery(recovery);
        let result = claim
            .run_owned(async move {
                let _ticket = ticket;
                panic!("injected source panic after destination adoption");
                #[allow(unreachable_code)]
                Ok::<(), lingxi_core::host::bg_session_forker::BgForkError>(())
            })
            .await;
        assert!(result.is_err());
        let report = lifecycle.close_and_drain().await;
        assert!(report.unconfirmed_recoveries.is_empty());
        assert!(report
            .diagnostics
            .iter()
            .any(|error| error.contains("panicked")));
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(*registry.ownership.lock().unwrap(), (false, true));
        assert!(lifecycle
            .close_and_drain()
            .await
            .unconfirmed_recoveries
            .is_empty());
    }

    #[tokio::test]
    async fn committed_without_adoption_ack_stays_unconfirmed() {
        let home = tempfile::tempdir().unwrap();
        let registry = retry_registry(0);
        *registry.ownership.lock().unwrap() = (false, false);
        let recovery = published_recovery(home.path(), registry.clone());
        recovery.progress.lock().unwrap().finish_started = true;
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "commit",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        assert!(recovery.recover_source().await.is_err());
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "adopted",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        recovery.recover_source().await.unwrap();
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn malformed_adoption_ack_remains_unconfirmed_until_corrected() {
        let home = tempfile::tempdir().unwrap();
        let registry = retry_registry(0);
        let recovery = published_recovery(home.path(), registry.clone());
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "ready",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        let mut malformed = ack(vec!["bshell001".into()], None);
        malformed.source = Some(background_launch::ShellHandoffSource {
            pid: 42,
            process_start: "unexpected".into(),
        });
        background_launch::write_shell_handoff_ack(home.path(), "feed1234", "adopted", &malformed)
            .unwrap();
        assert!(recovery.finish_source().await.is_err());
        assert!(recovery.recover_source().await.is_err());
        assert_eq!(*registry.ownership.lock().unwrap(), (false, false));
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "adopted",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        recovery.recover_source().await.unwrap();
    }

    #[tokio::test]
    async fn missing_published_launch_never_guesses_that_ownership_is_local() {
        let home = tempfile::tempdir().unwrap();
        let registry = retry_registry(0);
        let recovery = published_recovery(home.path(), registry.clone());
        recovery.progress.lock().unwrap().finish_started = true;
        std::fs::remove_file(background_launch::launch_spec_path(home.path(), "feed1234")).unwrap();
        assert!(recovery.recover_source().await.is_err());
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn panic_before_first_launch_write_fences_and_rolls_back() {
        let home = tempfile::tempdir().unwrap();
        let registry = retry_registry(0);
        let recovery = SourceHandoffRecovery::new(
            home.path().to_path_buf(),
            "session".into(),
            registry.clone(),
        );
        recovery.record_export(vec![shell()]);
        std::fs::create_dir_all(crate::agents_registry::jobs_dir(home.path()).join("feed1234"))
            .unwrap();
        recovery.record_short("feed1234");
        recovery.recover_source().await.unwrap();
        assert!(
            background_launch::read_shell_handoff_ack(home.path(), "feed1234", "abort")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn recovery_does_not_readopt_a_retained_row_already_rolled_back() {
        let home = tempfile::tempdir().unwrap();
        let registry = retry_registry(0);
        let recovery = published_recovery(home.path(), registry.clone());
        let mut retained = shell();
        retained.task_id = "bshell002".into();
        retained.process.task_id = retained.task_id.clone();
        let mut spec = background_launch::read_launch_spec(home.path(), "feed1234").unwrap();
        spec.shell_handoff.push(retained.clone());
        background_launch::write_launch_spec(home.path(), "feed1234", &spec).unwrap();
        recovery.record_export(spec.shell_handoff.clone());
        recovery.progress.lock().unwrap().finish_started = true;
        recovery.rollback(&[retained]).await.unwrap();
        *registry.ownership.lock().unwrap() = (false, true);
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "commit",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        background_launch::write_shell_handoff_ack(
            home.path(),
            "feed1234",
            "adopted",
            &ack(vec!["bshell001".into()], None),
        )
        .unwrap();
        recovery.recover_source().await.unwrap();
        assert_eq!(
            registry
                .rollback_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }
}
