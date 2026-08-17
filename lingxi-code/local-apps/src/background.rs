//! Host-owned persistence for declarative local-app background tasks.

use crate::error::AppError;
use crate::manifest::AppLayout;
use crate::runtime_v2::{
    BackgroundJournalEntry, BackgroundTaskRecord, RUNTIME_CONTRACT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use traits::rooted_fs::{self, AtomicWriteOptions};
use traits::FsError;

const TASKS_FILE: &str = "background-tasks.json";
const JOURNAL_FILE: &str = "background-journal.json";
const MAX_BACKGROUND_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundTaskCatalog {
    schema_version: u32,
    tasks: Vec<BackgroundTaskRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundJournalCatalog {
    schema_version: u32,
    entries: Vec<BackgroundJournalEntry>,
}

fn path(layout: &AppLayout, file: &str) -> PathBuf {
    layout.app_dir_rel().join(file)
}

/// Load the persisted per-app background task catalog.
pub fn load_tasks(layout: &AppLayout) -> Result<Vec<BackgroundTaskRecord>, AppError> {
    let body = match rooted_fs::read_to_string_limited(
        layout.root(),
        &path(layout, TASKS_FILE),
        MAX_BACKGROUND_BYTES,
    ) {
        Ok(body) => body,
        Err(FsError::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(AppError::from_fs("read background task catalog", &error)),
    };
    let catalog: BackgroundTaskCatalog = serde_json::from_str(&body)
        .map_err(|error| AppError::StorageCorrupt(format!("background task catalog: {error}")))?;
    if catalog.schema_version != RUNTIME_CONTRACT_SCHEMA_VERSION {
        return Err(AppError::StorageCorrupt(format!(
            "background task catalog schemaVersion {} is unsupported",
            catalog.schema_version
        )));
    }
    for task in &catalog.tasks {
        if task.app_id != layout.app_id() || task.flow_id != task.flow.flow_id {
            return Err(AppError::StorageCorrupt(
                "background task ownership or flow id mismatch".into(),
            ));
        }
    }
    Ok(catalog.tasks)
}

/// Persist the complete per-app background task catalog atomically.
pub fn save_tasks(layout: &AppLayout, tasks: &[BackgroundTaskRecord]) -> Result<(), AppError> {
    if tasks
        .iter()
        .any(|task| task.app_id != layout.app_id() || task.flow_id != task.flow.flow_id)
    {
        return Err(AppError::InvalidRequest(
            "background task ownership or flow id mismatch".into(),
        ));
    }
    let mut body = serde_json::to_vec_pretty(&BackgroundTaskCatalog {
        schema_version: RUNTIME_CONTRACT_SCHEMA_VERSION,
        tasks: tasks.to_vec(),
    })
    .map_err(|error| AppError::Io(format!("serialize background task catalog: {error}")))?;
    body.push(b'\n');
    if body.len() as u64 > MAX_BACKGROUND_BYTES {
        return Err(AppError::InvalidRequest(
            "background task catalog exceeds its size limit".into(),
        ));
    }
    layout.initialize()?;
    rooted_fs::atomic_write(
        layout.root(),
        &path(layout, TASKS_FILE),
        &body,
        AtomicWriteOptions {
            file_mode: 0o600,
            ..AtomicWriteOptions::default()
        },
    )
    .map_err(|error| AppError::from_fs("write background task catalog", &error))
}

/// Load the resumable background execution journal for one app.
pub fn load_journal(layout: &AppLayout) -> Result<Vec<BackgroundJournalEntry>, AppError> {
    let body = match rooted_fs::read_to_string_limited(
        layout.root(),
        &path(layout, JOURNAL_FILE),
        MAX_BACKGROUND_BYTES,
    ) {
        Ok(body) => body,
        Err(FsError::NotFound(_)) => return Ok(Vec::new()),
        Err(error) => return Err(AppError::from_fs("read background journal", &error)),
    };
    let catalog: BackgroundJournalCatalog = serde_json::from_str(&body)
        .map_err(|error| AppError::StorageCorrupt(format!("background journal: {error}")))?;
    if catalog.schema_version != RUNTIME_CONTRACT_SCHEMA_VERSION {
        return Err(AppError::StorageCorrupt(
            "background journal schemaVersion is unsupported".into(),
        ));
    }
    Ok(catalog.entries)
}

/// Persist the complete per-app background execution journal atomically.
pub fn save_journal(
    layout: &AppLayout,
    entries: &[BackgroundJournalEntry],
) -> Result<(), AppError> {
    let mut body = serde_json::to_vec_pretty(&BackgroundJournalCatalog {
        schema_version: RUNTIME_CONTRACT_SCHEMA_VERSION,
        entries: entries.to_vec(),
    })
    .map_err(|error| AppError::Io(format!("serialize background journal: {error}")))?;
    body.push(b'\n');
    if body.len() as u64 > MAX_BACKGROUND_BYTES {
        return Err(AppError::InvalidRequest(
            "background journal exceeds its size limit".into(),
        ));
    }
    layout.initialize()?;
    rooted_fs::atomic_write(
        layout.root(),
        &path(layout, JOURNAL_FILE),
        &body,
        AtomicWriteOptions {
            file_mode: 0o600,
            ..AtomicWriteOptions::default()
        },
    )
    .map_err(|error| AppError::from_fs("write background journal", &error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_v2::{
        BackgroundTaskStatus, BackgroundTrigger, CapabilityId, FlowDefinition, FlowStep,
    };

    #[test]
    fn task_and_journal_round_trip_under_app_layout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let layout = AppLayout::new(temp.path(), "abc12345").expect("layout");
        let flow = FlowDefinition {
            flow_id: "flow-1".into(),
            version: 1,
            steps: vec![FlowStep {
                step_id: "step-1".into(),
                capability: CapabilityId::RuntimeStatus,
                depends_on: Vec::new(),
                input_json: "{}".into(),
            }],
        };
        let task = BackgroundTaskRecord {
            schema_version: RUNTIME_CONTRACT_SCHEMA_VERSION,
            task_id: "task-1".into(),
            app_id: "abc12345".into(),
            flow_id: flow.flow_id.clone(),
            flow,
            trigger: BackgroundTrigger::Schedule {
                interval_ms: 900_000,
            },
            status: BackgroundTaskStatus::Scheduled,
            updated_at_ms: 1,
        };
        save_tasks(&layout, std::slice::from_ref(&task)).expect("save tasks");
        assert_eq!(load_tasks(&layout).expect("load tasks"), vec![task]);
        let journal = BackgroundJournalEntry {
            task_id: "task-1".into(),
            flow_id: "flow-1".into(),
            next_step_id: Some("step-1".into()),
            next_run_at_ms: Some(900_000),
            attempt: 1,
            last_error: None,
            updated_at_ms: 2,
        };
        save_journal(&layout, std::slice::from_ref(&journal)).expect("save journal");
        assert_eq!(load_journal(&layout).expect("load journal"), vec![journal]);
    }
}
