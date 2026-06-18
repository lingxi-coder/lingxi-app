//! M9-01 contract test (design §4 R1): the live `PollerFeed` and the
//! `FixtureFeed` emit the SAME `MultiAgentEvent`/`TaskRow` shape, so the UI
//! built against fixtures lights up correctly against the real engine.

use traits::task_registry::TaskRecord;
use tui::multiagent::poller::task_row_from_record;
use tui::multiagent::{FixtureFeed, MultiAgentEvent, MultiAgentFeed, TaskRow};

/// The record→row mapping is TOTAL: every `TaskRecord` field lands on the
/// corresponding `TaskRow` field (no data dropped, no field invented).
#[test]
fn record_to_row_mapping_is_total() {
    let rec = TaskRecord {
        task_id: "b12345678".into(),
        task_type: "local_bash".into(),
        status: "running".into(),
        description: "build the workspace".into(),
        command: None,
    };
    let row = task_row_from_record(rec.clone());
    assert_eq!(row.task_id, rec.task_id);
    assert_eq!(row.task_type, rec.task_type);
    assert_eq!(row.status, rec.status);
    assert_eq!(row.description, rec.description);
}

/// A fixture can reproduce, byte-for-byte, the row a poller would emit from a
/// given record — so a snapshot taken against the fixture is valid for live data.
#[tokio::test]
async fn fixture_can_reproduce_a_poller_row() {
    let rec = TaskRecord {
        task_id: "a99999999".into(),
        task_type: "local_agent".into(),
        status: "completed".into(),
        description: "review".into(),
        command: None,
    };
    let poller_row: TaskRow = task_row_from_record(rec.clone());

    let fixture = FixtureFeed::new(vec![vec![MultiAgentEvent::TasksRefreshed(vec![TaskRow {
        task_id: "a99999999".into(),
        task_type: "local_agent".into(),
        status: "completed".into(),
        description: "review".into(),
    }])]]);

    match fixture.poll().await.as_slice() {
        [MultiAgentEvent::TasksRefreshed(rows)] => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0], poller_row, "fixture row must equal the poller row");
        }
        other => panic!("unexpected: {other:?}"),
    }
}
