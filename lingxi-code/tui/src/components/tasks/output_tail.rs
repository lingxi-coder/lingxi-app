//! Live output tail (M9-04): accumulate a task's spool by incrementally
//! reading `TaskRegistryHandle::output(id, Some(offset))` and appending only
//! the new bytes. `render_output_tail` shows the last N lines.

use traits::task_registry::{TaskRegistryError, TaskRegistryHandle};

/// Accumulated tail state for one task's spool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputTailState {
    /// All content seen so far.
    pub content: String,
    /// Byte offset of the next unread spool byte.
    pub offset: u64,
    /// Total line count reported by the spool.
    pub total_lines: u64,
    /// Whether the last chunk was truncated by a limit.
    pub truncated: bool,
}

/// Fetch the delta since `state.offset`, append it, and advance the offset by
/// the new content's byte length. Returns `Ok(true)` when new content arrived.
pub async fn tail_once(
    handle: &dyn TaskRegistryHandle,
    id: &str,
    state: &mut OutputTailState,
) -> Result<bool, TaskRegistryError> {
    let chunk = handle.output(id, Some(state.offset)).await?;
    let added = !chunk.content.is_empty();
    if added {
        state.offset += chunk.content.len() as u64;
        state.content.push_str(&chunk.content);
    }
    state.total_lines = chunk.total_lines;
    state.truncated = chunk.truncated;
    Ok(added)
}

/// The last `max_lines` lines of the accumulated content.
#[must_use]
pub fn render_output_tail(state: &OutputTailState, max_lines: usize) -> String {
    let lines: Vec<&str> = state.content.lines().collect();
    if lines.len() <= max_lines {
        return state.content.clone();
    }
    lines[lines.len() - max_lines..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use traits::task_registry::{
        TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskUpdatePatch,
    };

    /// Minimal stub: `output` returns the spool sliced at the byte offset.
    struct StubTasks {
        spool: Mutex<String>,
    }

    #[async_trait]
    impl TaskRegistryHandle for StubTasks {
        async fn create(&self, _i: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }
        async fn list(&self, _f: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(vec![])
        }
        async fn update(
            &self,
            _id: &str,
            _p: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn set_status(&self, _id: &str, _s: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
            Err(TaskRegistryError::Internal("unused".into()))
        }
        async fn output(
            &self,
            id: &str,
            offset: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            let spool = self.spool.lock().unwrap();
            let off = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
            let content = spool
                .as_bytes()
                .get(off..)
                .map_or(String::new(), |b| String::from_utf8_lossy(b).into_owned());
            Ok(TaskOutputChunk {
                task_id: id.to_string(),
                content,
                total_lines: spool.lines().count() as u64,
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn tail_advances_on_new_spool_bytes() {
        let stub = StubTasks {
            spool: Mutex::new("hello\n".to_string()),
        };
        let mut state = OutputTailState::default();

        // First tail: reads "hello\n".
        let added = tail_once(&stub, "b12345678", &mut state).await.unwrap();
        assert!(added);
        assert_eq!(state.content, "hello\n");
        assert_eq!(state.offset, 6);
        assert_eq!(state.total_lines, 1);

        // No new bytes → no advance.
        let added = tail_once(&stub, "b12345678", &mut state).await.unwrap();
        assert!(!added);
        assert_eq!(state.content, "hello\n");
        assert_eq!(state.offset, 6);

        // Append to the spool, tail again → only the new bytes are added.
        *stub.spool.lock().unwrap() = "hello\nworld\n".to_string();
        let added = tail_once(&stub, "b12345678", &mut state).await.unwrap();
        assert!(added);
        assert_eq!(state.content, "hello\nworld\n");
        assert_eq!(state.offset, 12);
        assert_eq!(state.total_lines, 2);
    }

    #[test]
    fn render_tail_limits_lines() {
        let state = OutputTailState {
            content: "a\nb\nc\nd\ne".into(),
            offset: 9,
            total_lines: 5,
            truncated: false,
        };
        assert_eq!(render_output_tail(&state, 2), "d\ne");
        assert_eq!(render_output_tail(&state, 10), "a\nb\nc\nd\ne");
    }
}
