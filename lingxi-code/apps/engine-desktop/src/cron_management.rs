//! Desktop management surface over the scheduler's authoritative task file.
use client_protocol::{commands::CronRequestDto, events::CronJobDto};
use cron::tasks_file::{CronTask, ScheduledTasks};
use platform_api::task_registry::TaskRegistryHandle;
use platform_api::{FileSystem, FsError};
use std::{path::Path, sync::Arc};

/// Apply a management operation under the same locks as scheduler firing.
/// Updates retain IDs and firing timestamps; all filesystem operations reject symlinks.
pub async fn manage(
    fs: &dyn FileSystem,
    cwd: &Path,
    request: CronRequestDto,
    registry: &Arc<dyn TaskRegistryHandle>,
    session_id: &str,
) -> Result<Vec<CronJobDto>, String> {
    if platform_api::env::is_env_truthy(std::env::var("LINGXI_DISABLE_CRON").ok().as_deref()) {
        return Err("Scheduled tasks are disabled by LINGXI_DISABLE_CRON".into());
    }
    if !matches!(
        request.action.as_str(),
        "list" | "create" | "update" | "delete"
    ) {
        return Err("Unknown scheduled task operation".into());
    }
    if request.durable == Some(false) {
        return Err("Scheduled task management supports durable workspace tasks only".into());
    }
    if request.action != "list" {
        cron::session_jobs(registry)
            .await
            .map_err(|e| format!("Scheduler unavailable: {e}"))?;
    }
    let _process_guard = cron::lock_cron_file().await;
    let _file_guard = if request.action == "list" {
        None
    } else {
        Some(
            cron::lock_scheduled_tasks(fs, cwd)
                .await
                .map_err(|e| e.to_string())?,
        )
    };
    let mut doc: ScheduledTasks = match cron::read_tasks_body(fs, cwd).await {
        Ok(body) => {
            serde_json::from_str(&body).map_err(|e| format!("Cannot read scheduled tasks: {e}"))?
        }
        Err(FsError::NotFound(_)) => ScheduledTasks::default(),
        Err(e) => return Err(e.to_string()),
    };
    if request.action != "list" {
        let before = doc.clone();
        let deleted = if request.action == "delete" {
            request.id.clone()
        } else {
            None
        };
        let updated_id = if request.action == "update" {
            request.id.clone()
        } else {
            None
        };
        let creating = request.action == "create";
        apply(&mut doc, request)?;
        if creating {
            doc.tasks.last_mut().unwrap().session_id = Some(session_id.to_string());
        }
        cron::write_tasks_body(fs, cwd, &cron::serialize_tasks(&doc))
            .await
            .map_err(|e| e.to_string())?;
        let live_result = if let Some(id) = deleted {
            cron::unregister_live_job(registry, &id, None)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        } else {
            let task = updated_id
                .as_ref()
                .and_then(|id| doc.tasks.iter().find(|t| &t.id == id))
                .or_else(|| doc.tasks.last())
                .ok_or("Missing saved task")?;
            cron::register_live_job(
                registry,
                cron::SessionCronTask {
                    id: task.id.clone(),
                    cron: task.cron.clone(),
                    prompt: task.prompt.clone(),
                    created_at: std::time::UNIX_EPOCH
                        + std::time::Duration::from_millis(task.created_at),
                    last_fired_at: task
                        .last_fired_at
                        .map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)),
                    recurring: task.recurring.unwrap_or(false),
                    owner: None,
                },
                true,
            )
            .await
            .map_err(|e| e.to_string())
        };
        if let Err(error) = live_result {
            cron::write_tasks_body(fs, cwd, &cron::serialize_tasks(&before))
                .await
                .map_err(|rollback| {
                    format!("Scheduler update failed: {error}; rollback failed: {rollback}")
                })?;
            return Err(format!("Scheduler update failed: {error}"));
        }
    }
    Ok(doc
        .tasks
        .into_iter()
        .map(|task| CronJobDto {
            id: task.id,
            cron: task.cron,
            prompt: task.prompt,
            recurring: task.recurring.unwrap_or(false),
            durable: true,
            permanent: task.permanent.unwrap_or(false),
            created_at: task.created_at,
            last_fired_at: task.last_fired_at,
            expires_at: task.expires_at,
            session_id: task.session_id.map(|id| {
                protocol::SessionId::parse_prefixed(&id)
                    .map_or(id, |session| session.as_uuid().to_string())
            }),
        })
        .collect())
}

fn apply(doc: &mut ScheduledTasks, request: CronRequestDto) -> Result<(), String> {
    let action = request.action.as_str();
    if request.no_expiry == Some(true) && request.expires_at.is_some() {
        return Err("Choose either an expiration date or no expiry".into());
    }
    if request.expires_at.is_some_and(|ms| {
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms) <= std::time::SystemTime::now()
    }) {
        return Err("Expiration must be in the future".into());
    }
    if action == "create" || action == "update" {
        let expression = request
            .cron
            .as_deref()
            .ok_or("A cron schedule is required")?;
        let schedule = cron::parse_cron(expression).map_err(|e| e.to_string())?;
        if schedule
            .next_match_after(std::time::SystemTime::now())
            .is_none()
        {
            return Err("Cron schedule does not match a calendar date in the next year".into());
        }
        if request
            .prompt
            .as_deref()
            .is_none_or(|prompt| prompt.trim().is_empty())
        {
            return Err("Task instructions are required".into());
        }
    }
    if action == "create" {
        if doc.tasks.len() >= 50 {
            return Err("Too many scheduled jobs (max 50). Cancel one first.".into());
        }
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis()
            .try_into()
            .map_err(|_| "Clock timestamp overflow")?;
        doc.tasks.push(CronTask {
            id: tool_cron::schedule_cron::generate_cron_task_id(),
            cron: request.cron.unwrap(),
            prompt: request.prompt.unwrap(),
            created_at,
            last_fired_at: None,
            recurring: Some(request.recurring.unwrap_or(true)),
            permanent: None,
            expires_at: request.expires_at,
            session_id: None,
        });
    } else {
        let id = request.id.as_deref().ok_or("A task ID is required")?;
        let index = doc
            .tasks
            .iter()
            .position(|task| task.id == id)
            .ok_or("Scheduled task no longer exists; refresh the list")?;
        if doc.tasks[index].permanent == Some(true) {
            return Err("System scheduled tasks cannot be edited or deleted".into());
        }
        if action == "delete" {
            doc.tasks.remove(index);
        } else {
            let task = &mut doc.tasks[index];
            task.cron = request.cron.unwrap();
            task.prompt = request.prompt.unwrap();
            if request.no_expiry == Some(true) {
                task.expires_at = None;
            } else if request.expires_at.is_some() {
                task.expires_at = request.expires_at;
            }
            if let Some(recurring) = request.recurring {
                task.recurring = Some(recurring);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(action: &str) -> CronRequestDto {
        CronRequestDto {
            action: action.into(),
            id: None,
            cron: Some("0 9 * * 1".into()),
            prompt: Some("Weekly report".into()),
            recurring: Some(true),
            durable: Some(true),
            expires_at: None,
            no_expiry: None,
        }
    }
    #[test]
    fn edits_preserve_identity_and_firing_history() {
        let mut doc = ScheduledTasks::default();
        apply(&mut doc, request("create")).unwrap();
        doc.tasks[0].last_fired_at = Some(123);
        let before = doc.tasks[0].clone();
        let mut edit = request("update");
        edit.id = Some(before.id.clone());
        edit.prompt = Some("Updated instructions".into());
        apply(&mut doc, edit).unwrap();
        assert_eq!(doc.tasks[0].id, before.id);
        assert_eq!(doc.tasks[0].created_at, before.created_at);
        assert_eq!(doc.tasks[0].last_fired_at, Some(123));
        assert_eq!(doc.tasks[0].prompt, "Updated instructions");
        let mut delete = request("delete");
        delete.id = Some(before.id);
        apply(&mut doc, delete).unwrap();
        assert!(doc.tasks.is_empty());
    }
    #[test]
    fn invalid_edits_do_not_modify_tasks() {
        let mut doc = ScheduledTasks::default();
        apply(&mut doc, request("create")).unwrap();
        let before = doc.clone();
        let mut edit = request("update");
        edit.id = Some(doc.tasks[0].id.clone());
        edit.cron = Some("invalid".into());
        assert!(apply(&mut doc, edit).is_err());
        assert_eq!(doc, before);
        let mut delete = request("delete");
        delete.id = Some(doc.tasks[0].id.clone());
        doc.tasks[0].permanent = Some(true);
        assert!(apply(&mut doc, delete).is_err());
        assert_eq!(doc.tasks.len(), 1);
    }
    async fn live_scheduler(
        root: &Path,
    ) -> (Arc<dyn TaskRegistryHandle>, Arc<cron::CronScheduler>) {
        let fs = Arc::new(platform_posix::PosixFileSystem::new(root.into()));
        let runtime = Arc::new(platform_posix::PosixRuntime::new());
        let registry = Arc::new(tasks::registry::TaskRegistry::new(
            runtime.clone(),
            fs.clone(),
            Arc::new(tasks::output_manager::TaskOutputManager::new(
                root.join("output"),
                fs.clone(),
            )),
        ));
        let scheduler = Arc::new(cron::CronScheduler::new(
            registry.clone(),
            fs,
            Arc::new(platform_posix::PosixClock),
            runtime,
            cron::scheduled_tasks_path(root),
        ));
        scheduler.clone().start().await.unwrap();
        (registry, scheduler)
    }

    #[tokio::test]
    async fn persisted_crud_and_corruption_preservation() {
        let temp = tempfile::tempdir().unwrap();
        let fs = platform_posix::PosixFileSystem::new(temp.path().into());
        let (registry, scheduler) = live_scheduler(temp.path()).await;
        let jobs = manage(
            &fs,
            temp.path(),
            request("create"),
            &registry,
            "session-test",
        )
        .await
        .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].session_id.as_deref(), Some("session-test"));
        let id = jobs[0].id.clone();
        let legacy_owner = "11111111-2222-3333-4444-555555555555";
        let body = cron::read_tasks_body(&fs, temp.path()).await.unwrap();
        let mut legacy_doc: ScheduledTasks = serde_json::from_str(&body).unwrap();
        legacy_doc.tasks[0].session_id = Some(format!("sess:{legacy_owner}"));
        cron::write_tasks_body(&fs, temp.path(), &cron::serialize_tasks(&legacy_doc))
            .await
            .unwrap();
        let listed = manage(&fs, temp.path(), request("list"), &registry, "session-test")
            .await
            .unwrap();
        assert_eq!(listed[0].session_id.as_deref(), Some(legacy_owner));
        let mut edit = request("update");
        edit.id = Some(id.clone());
        edit.prompt = Some("Changed".into());
        let jobs = manage(&fs, temp.path(), edit, &registry, "session-test")
            .await
            .unwrap();
        assert_eq!(jobs[0].id, id);
        assert_eq!(jobs[0].prompt, "Changed");
        let mut delete = request("delete");
        delete.id = Some(id);
        assert!(manage(&fs, temp.path(), delete, &registry, "session-test")
            .await
            .unwrap()
            .is_empty());
        let jobs = manage(
            &fs,
            temp.path(),
            request("create"),
            &registry,
            "session-test",
        )
        .await
        .unwrap();
        assert!(
            cron::unregister_live_job(&registry, &jobs[0].id, None)
                .await
                .unwrap(),
            "management must register jobs in the running scheduler"
        );
        let path = cron::scheduled_tasks_path(temp.path());
        std::fs::write(&path, "broken JSON").unwrap();
        assert!(manage(
            &fs,
            temp.path(),
            request("create"),
            &registry,
            "session-test"
        )
        .await
        .is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken JSON");
        scheduler.stop().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_symlinked_task_storage() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join(".lingxi")).unwrap();
        let fs = platform_posix::PosixFileSystem::new(temp.path().into());
        let (registry, scheduler) = live_scheduler(temp.path()).await;
        assert!(manage(
            &fs,
            temp.path(),
            request("create"),
            &registry,
            "session-test"
        )
        .await
        .is_err());
        assert!(!outside.path().join("scheduled_tasks.json").exists());
        scheduler.stop().await.unwrap();
    }
    #[tokio::test]
    async fn unavailable_scheduler_never_persists_a_task() {
        let temp = tempfile::tempdir().unwrap();
        let fs = platform_posix::PosixFileSystem::new(temp.path().into());
        let (registry, scheduler) = live_scheduler(temp.path()).await;
        scheduler.stop().await.unwrap();
        assert!(manage(
            &fs,
            temp.path(),
            request("create"),
            &registry,
            "session-test"
        )
        .await
        .is_err());
        assert!(!cron::scheduled_tasks_path(temp.path()).exists());
    }
    #[test]
    fn expiration_can_be_set_preserved_and_cleared() {
        let mut doc = ScheduledTasks::default();
        let mut create = request("create");
        let expiry = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64)
            + 86_400_000;
        create.expires_at = Some(expiry);
        apply(&mut doc, create).unwrap();
        let mut edit = request("update");
        edit.id = Some(doc.tasks[0].id.clone());
        apply(&mut doc, edit.clone()).unwrap();
        assert_eq!(doc.tasks[0].expires_at, Some(expiry));
        edit.no_expiry = Some(true);
        apply(&mut doc, edit).unwrap();
        assert_eq!(doc.tasks[0].expires_at, None);
    }
}
