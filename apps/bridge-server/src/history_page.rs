//! Read older display rows backwards from an exact transcript. No engine or account IO.
use lingxi_core::types::utf16_json::JsonValueView;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use client::protocol::message::MessageDto;
use serde::{Deserialize, Serialize};

const CHUNK: u64 = 64 * 1024;
const MAX_PAGE_BYTES: usize = 6 * 1024 * 1024;
const PAGE_ROWS: usize = 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Cursor {
    pub uuid: String,
    pub offset: Option<u64>,
    pub identity: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Page {
    pub messages: Vec<MessageDto>,
    pub before: Option<Cursor>,
}

struct ReverseLines {
    file: File,
    start: u64,
    buffer: Vec<u8>,
}

impl ReverseLines {
    fn next(&mut self) -> Result<Option<(u64, String)>, String> {
        loop {
            if self.buffer.last() == Some(&b'\n') {
                self.buffer.pop();
            }
            if let Some(newline) = self.buffer.iter().rposition(|byte| *byte == b'\n') {
                let line = self.buffer.split_off(newline + 1);
                let offset = self.start + newline as u64 + 1;
                return String::from_utf8(line)
                    .map(|line| Some((offset, line)))
                    .map_err(|_| "History contains invalid UTF-8".into());
            }
            if self.start == 0 {
                if self.buffer.is_empty() {
                    return Ok(None);
                }
                let line = std::mem::take(&mut self.buffer);
                return String::from_utf8(line)
                    .map(|line| Some((0, line)))
                    .map_err(|_| "History contains invalid UTF-8".into());
            }
            let start = self.start.saturating_sub(CHUNK);
            let mut chunk = vec![0; (self.start - start) as usize];
            self.file
                .seek(SeekFrom::Start(start))
                .map_err(|e| e.to_string())?;
            self.file
                .read_exact(&mut chunk)
                .map_err(|e| e.to_string())?;
            chunk.append(&mut self.buffer);
            self.buffer = chunk;
            self.start = start;
            if self.buffer.len() > MAX_PAGE_BYTES + CHUNK as usize {
                return Err("One history message exceeds the page size limit".into());
            }
        }
    }
}

fn identity(metadata: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", metadata.created().ok())
    }
}

pub(super) fn read(path: &Path, session_id: uuid::Uuid, cursor: Cursor) -> Result<Page, String> {
    if path
        .file_stem()
        .and_then(|v| v.to_str())
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        != Some(session_id)
        || path.extension().and_then(|v| v.to_str()) != Some("jsonl")
        || uuid::Uuid::parse_str(&cursor.uuid).is_err()
        || cursor.offset.is_some() != cursor.identity.is_some()
    {
        return Err("Invalid history source or cursor".into());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("History source is not a regular file".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    let source_identity = identity(&opened);
    if source_identity != identity(&metadata)
        || cursor
            .identity
            .as_ref()
            .is_some_and(|v| v != &source_identity)
    {
        return Err("History source changed. Reopen this chat to reload history.".into());
    }
    let mut found = cursor.offset.is_some();
    let mut ancestors = HashSet::new();
    let mut groups = HashSet::new();
    if let Some(offset) = cursor.offset {
        if offset >= opened.len() {
            return Err("History cursor is no longer valid".into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut line = Vec::new();
        BufReader::new(&mut file)
            .take((MAX_PAGE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        let source = String::from_utf8(line).map_err(|_| "Invalid history cursor".to_string())?;
        let loaded = session::jsonl::route_lines(&source);
        let anchor = loaded
            .messages_in_order
            .iter()
            .find(|row| row.uuid == cursor.uuid)
            .ok_or("History moved since the previous page. Reopen this chat.")?;
        ancestors.extend(anchor.parent_uuid.iter().cloned());
        groups.extend(assistant_group(anchor));
    }
    let mut reader = ReverseLines {
        file,
        start: cursor.offset.unwrap_or(opened.len()),
        buffer: Vec::new(),
    };
    let mut rows = Vec::new();
    let mut pending_siblings = HashMap::<_, Vec<(u64, session::jsonl::JsonlMessage)>>::new();
    let mut pending_calls = HashSet::new();
    let mut pending_summary = false;
    let mut safe_offsets = Vec::new();
    let mut bytes = 0;
    let mut before = None;
    while let Some((offset, line)) = reader.next()? {
        let loaded = session::jsonl::route_lines(&line);
        let Some(row) = loaded
            .messages_in_order
            .into_iter()
            .find(|row| !row.is_sidechain)
        else {
            continue;
        };
        if !found {
            if row.uuid == cursor.uuid {
                found = true;
                ancestors.extend(row.parent_uuid.iter().cloned());
                groups.extend(assistant_group(&row));
            }
            continue;
        }
        if bytes + line.len() > MAX_PAGE_BYTES {
            if !truncate_complete_page(&mut rows, &safe_offsets, &pending_siblings) {
                return Err("One history activity exceeds the page size limit".into());
            }
            break;
        }
        bytes += line.len();
        let group = assistant_group(&row);
        let owned = uuid::Uuid::parse_str(
            row.session_id
                .strip_prefix("sess:")
                .unwrap_or(&row.session_id),
        )
        .ok()
            == Some(session_id);
        if !owned
            && !ancestors.contains(&row.uuid)
            && !group.as_ref().is_some_and(|group| groups.contains(group))
        {
            // A foreign assistant sibling may precede its proven ancestor in
            // reverse order. Keep it bounded by this page's byte budget until
            // that ancestor proves the whole assistant group belongs here.
            if let Some(group) = group {
                pending_siblings
                    .entry(group)
                    .or_default()
                    .push((offset, row));
            }
            continue;
        }
        ancestors.extend(row.parent_uuid.iter().cloned());
        if let Some(group) = group {
            groups.insert(group.clone());
            if let Some(siblings) = pending_siblings.remove(&group) {
                for sibling in siblings {
                    ancestors.extend(sibling.1.parent_uuid.iter().cloned());
                    update_activity(&sibling.1, &mut pending_calls, &mut pending_summary);
                    rows.push(sibling);
                }
                rows.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
            }
        }
        update_activity(&row, &mut pending_calls, &mut pending_summary);
        rows.push((offset, row));
        if pending_calls.is_empty() && !pending_summary {
            safe_offsets.push(offset);
        }
        if rows.len() >= PAGE_ROWS
            && truncate_complete_page(&mut rows, &safe_offsets, &pending_siblings)
        {
            // Keep a call/result activity together, allowing a larger single
            // batch within the byte budget rather than dropping raw results.
            break;
        }
    }
    if !found {
        return Err("History anchor is unavailable. Reopen this chat.".into());
    }
    if let Some((offset, row)) = rows.last() {
        before = Some(Cursor {
            uuid: row.uuid.clone(),
            offset: Some(*offset),
            identity: Some(source_identity.clone()),
        });
    }
    if before
        .as_ref()
        .is_some_and(|cursor| cursor.offset == Some(0))
        || rows.is_empty()
    {
        before = None;
    }
    rows.reverse();
    let rows = rows.into_iter().map(|(_, row)| row).collect::<Vec<_>>();
    let state = orchestrator::resume::state_from_messages(session_id, &rows);
    let results = orchestrator::resume::client_state_tool_results_from_messages(&rows);
    let timestamps = rows
        .iter()
        .map(|row| (row.uuid.as_str(), &row.timestamp))
        .collect::<std::collections::HashMap<_, _>>();
    let messages =
        client::adapter::lowering::lower_transcript_with_identities(&state.history, &results)
            .into_iter()
            .map(|(id, mut message)| {
                let uuid = id.as_uuid().to_string();
                message.sent_at_ms = timestamps
                    .get(uuid.as_str())
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                    .map(|at| at.timestamp_millis());
                message.transcript_uuid = Some(uuid);
                message
            })
            .collect();
    Ok(Page { messages, before })
}

type AssistantGroup = (String, String, String);
type HistoryRow = (u64, session::jsonl::JsonlMessage);

fn truncate_complete_page(
    rows: &mut Vec<HistoryRow>,
    safe_offsets: &[u64],
    pending_siblings: &HashMap<AssistantGroup, Vec<HistoryRow>>,
) -> bool {
    let pending_offset = pending_siblings
        .values()
        .flatten()
        .map(|(offset, _)| *offset)
        .max();
    let Some(cut) = safe_offsets
        .iter()
        .rev()
        .find(|offset| pending_offset.is_none_or(|pending| **offset > pending))
    else {
        return false;
    };
    // An unproven sibling must stay on the older side of the cursor so the
    // next page can still discover its ancestor and recover it.
    rows.retain(|(offset, _)| offset >= cut);
    true
}

fn assistant_group(row: &session::jsonl::JsonlMessage) -> Option<AssistantGroup> {
    (row.message_type == "assistant").then(|| {
        Some((
            uuid::Uuid::parse_str(
                row.session_id
                    .strip_prefix("sess:")
                    .unwrap_or(&row.session_id),
            )
            .map_or_else(|_| row.session_id.clone(), |owner| owner.to_string()),
            row.message.string_at("/id")?.to_string(),
            row.extra.string_at("/agentId").unwrap_or("").to_string(),
        ))
    })?
}

fn update_activity(
    row: &session::jsonl::JsonlMessage,
    pending_calls: &mut HashSet<String>,
    pending_summary: &mut bool,
) {
    if row
        .extra
        .at_pointer("/isCompactSummary")
        .and_then(JsonValueView::as_bool)
        == Some(true)
    {
        *pending_summary = true;
    }
    if row.message_type == "system" && row.extra.string_at("/subtype") == Some("compact_boundary") {
        *pending_summary = false;
    }
    if let Some(content) = row.message.at_pointer("/content") {
        for index in 0..content.array_len().unwrap_or(0) {
            let Some(block) = content.get_index(index) else {
                continue;
            };
            match block.get_property("type").and_then(JsonValueView::as_str) {
                Some("tool_result") => {
                    if let Some(id) = block
                        .get_property("tool_use_id")
                        .and_then(JsonValueView::as_str)
                    {
                        pending_calls.insert(id.to_string());
                    }
                }
                Some("tool_use") => {
                    if let Some(id) = block.get_property("id").and_then(JsonValueView::as_str) {
                        pending_calls.remove(id);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::protocol::message::MessageBlockDto;

    fn saved_rows(
        rows: Vec<serde_json::Value>,
        id: uuid::Uuid,
    ) -> (tempfile::TempDir, std::path::PathBuf, Cursor) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(format!("{id}.jsonl"));
        let mut parent = None;
        let mut source = String::new();
        for mut row in rows {
            let uuid = row
                .get("uuid")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            row["uuid"] = uuid.clone().into();
            if row.get("sessionId").is_none() {
                row["sessionId"] = id.to_string().into();
            }
            if row.get("parentUuid").is_none() {
                row["parentUuid"] = serde_json::json!(parent);
            }
            row["timestamp"] = "2026-11-12T00:00:00Z".into();
            parent = Some(uuid);
            source.push_str(&format!("{row}\n"));
        }
        std::fs::write(&path, source).unwrap();
        (
            root,
            path,
            Cursor {
                uuid: parent.unwrap(),
                offset: None,
                identity: None,
            },
        )
    }

    fn text_row(text: &str) -> serde_json::Value {
        serde_json::json!({"type":"user","message":{"role":"user","content":text}})
    }

    fn agent_call(id: &str) -> serde_json::Value {
        serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":id,"name":"Agent","input":{"description":"inspect"}}]}})
    }

    fn agent_result(id: &str) -> serde_json::Value {
        serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":"model text"}]},"toolUseResult":{"agentId":"saved-worker","status":"completed"}})
    }

    #[test]
    fn pages_keep_calls_with_structured_results_at_the_soft_row_limit() {
        let id = uuid::Uuid::new_v4();
        let mut rows = vec![agent_call("agent"), agent_result("agent")];
        rows.extend((0..60).map(|_| text_row("newer")));
        let (_root, path, cursor) = saved_rows(rows, id);
        let first = read(&path, id, cursor).unwrap();
        assert_eq!(first.messages.len(), 59);
        let second = read(&path, id, first.before.unwrap()).unwrap();
        assert_eq!(second.messages.len(), 2);
        let result = second
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .find_map(|block| match block {
                MessageBlockDto::ToolResult {
                    tool, result_json, ..
                } => Some((tool, result_json)),
                _ => None,
            })
            .unwrap();
        assert_eq!(result.0, "Agent");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result.1).unwrap()["agentId"],
            "saved-worker"
        );
        assert!(second.before.is_none());
    }

    #[test]
    fn a_single_parallel_activity_can_exceed_the_soft_row_limit() {
        let id = uuid::Uuid::new_v4();
        let calls = (0..65).map(|index|serde_json::json!({"type":"tool_use","id":format!("call-{index}"),"name":"Agent","input":{}})).collect::<Vec<_>>();
        let mut rows = vec![
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":calls}}),
        ];
        rows.extend((0..65).map(|index| agent_result(&format!("call-{index}"))));
        rows.push(text_row("anchor"));
        let (_root, path, cursor) = saved_rows(rows, id);
        let page = read(&path, id, cursor).unwrap();
        assert_eq!(page.messages.len(), 66);
        assert_eq!(
            page.messages
                .iter()
                .flat_map(|message| &message.blocks)
                .filter(
                    |block| matches!(block,MessageBlockDto::ToolResult { tool,.. } if tool=="Agent")
                )
                .count(),
            65
        );
        assert!(page.before.is_none());
    }

    #[test]
    fn compact_summary_and_boundary_are_never_split_between_pages() {
        let id = uuid::Uuid::new_v4();
        let mut summary = text_row("Summary body");
        summary["isCompactSummary"] = true.into();
        let mut rows = vec![
            serde_json::json!({"type":"system","subtype":"compact_boundary","content":"compact","compactMetadata":{}}),
            summary,
        ];
        rows.extend((0..60).map(|_| text_row("newer")));
        let (_root, path, cursor) = saved_rows(rows, id);
        let first = read(&path, id, cursor).unwrap();
        assert_eq!(first.messages.len(), 59);
        let older = read(&path, id, first.before.unwrap()).unwrap();
        assert_eq!(older.messages.len(), 1);
        assert!(older.messages[0].blocks.iter().any(|block|matches!(block,MessageBlockDto::CompactBoundary { summary,.. } if summary.contains("Summary body"))));
    }

    #[test]
    fn foreign_rows_cannot_supply_tool_results_but_proven_parallel_ancestors_survive() {
        let id = uuid::Uuid::new_v4();
        let ancestor = uuid::Uuid::new_v4();
        let first = uuid::Uuid::new_v4();
        let mut a = agent_call("first");
        a["uuid"] = first.to_string().into();
        a["sessionId"] = ancestor.to_string().into();
        a["message"]["id"] = "parallel".into();
        let mut b = agent_call("second");
        b["sessionId"] = ancestor.to_string().into();
        b["message"]["id"] = "parallel".into();
        let mut result = agent_result("first");
        result["parentUuid"] = first.to_string().into();
        result["message"]["content"].as_array_mut().unwrap().push(serde_json::json!({"type":"tool_result","tool_use_id":"second","content":"second result"}));
        result.as_object_mut().unwrap().remove("toolUseResult");
        let mut foreign = agent_result("first");
        foreign["sessionId"] = uuid::Uuid::new_v4().to_string().into();
        foreign["parentUuid"] = serde_json::Value::Null;
        let mut anchor = text_row("anchor");
        anchor["parentUuid"] = first.to_string().into();
        let (_root, path, cursor) = saved_rows(vec![a, b, result, foreign, anchor], id);
        let page = read(&path, id, cursor).unwrap();
        assert_eq!(
            page.messages.len(),
            2,
            "parallel assistant blocks share one DTO row"
        );
        assert_eq!(
            page.messages
                .iter()
                .flat_map(|message| &message.blocks)
                .filter(
                    |block| matches!(block,MessageBlockDto::ToolUse { tool,.. } if tool=="Agent")
                )
                .count(),
            2
        );
        assert_eq!(
            page.messages
                .iter()
                .flat_map(|message| &message.blocks)
                .filter(|block| matches!(block, MessageBlockDto::ToolResult { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn byte_budget_returns_a_complete_newer_page_and_rejects_one_oversized_activity() {
        let id = uuid::Uuid::new_v4();
        let mut huge = agent_result("agent");
        huge["message"]["content"][0]["content"] = "x".repeat(MAX_PAGE_BYTES).into();
        let (_root, path, cursor) = saved_rows(
            vec![
                agent_call("agent"),
                huge,
                text_row("kept newer row"),
                text_row("anchor"),
            ],
            id,
        );
        let newer = read(&path, id, cursor).unwrap();
        assert_eq!(newer.messages.len(), 1);
        assert!(read(&path, id, newer.before.unwrap())
            .err()
            .unwrap()
            .contains("page size limit"));
    }

    #[test]
    fn a_page_cursor_never_skips_a_parallel_sibling_awaiting_ancestor_proof() {
        let id = uuid::Uuid::new_v4();
        let ancestor = uuid::Uuid::new_v4();
        let first = uuid::Uuid::new_v4();
        let child = uuid::Uuid::new_v4();
        let mut a = agent_call("first");
        a["uuid"] = first.to_string().into();
        a["sessionId"] = ancestor.to_string().into();
        a["message"]["id"] = "parallel".into();
        let mut d = text_row("child of the ancestor");
        d["uuid"] = child.to_string().into();
        d["parentUuid"] = first.to_string().into();
        let mut c = serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"parallel sibling prose"}]}});
        c["sessionId"] = format!("sess:{ancestor}").into();
        c["message"]["id"] = "parallel".into();
        let mut rows = vec![a, d, c];
        for index in 0..59 {
            let mut row = text_row("newer");
            if index == 0 {
                row["parentUuid"] = child.to_string().into();
            }
            rows.push(row);
        }
        rows.push(text_row("anchor"));
        let (_root, path, cursor) = saved_rows(rows, id);
        let newer = read(&path, id, cursor).unwrap();
        assert_eq!(newer.messages.len(), 59);
        let older = read(&path, id, newer.before.unwrap()).unwrap();
        assert_eq!(older.messages.len(), 3);
        assert_eq!(
            older
                .messages
                .iter()
                .flat_map(|message| &message.blocks)
                .filter(
                    |block| matches!(block,MessageBlockDto::ToolUse { tool,.. } if tool=="Agent")
                )
                .count(),
            1
        );
        assert!(serde_json::to_string(&older.messages)
            .unwrap()
            .contains("parallel sibling prose"));
        assert!(older.before.is_none());
    }

    fn transcript(
        count: usize,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        uuid::Uuid,
        Vec<String>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4();
        let path = root.path().join(format!("{id}.jsonl"));
        let ids = (0..count)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect::<Vec<_>>();
        let mut source = String::new();
        for (index, row_id) in ids.iter().enumerate() {
            source.push_str(&serde_json::json!({"type":"user","uuid":row_id,"sessionId":id,"parentUuid":null,
                "timestamp":"2026-11-12T00:00:00Z","message":{"role":"user","content":format!("message {index} 中文")}}).to_string());
            source.push('\n');
        }
        std::fs::write(&path, source).unwrap();
        (root, path, id, ids)
    }

    #[test]
    fn pages_are_exclusive_ordered_complete_and_survive_appends() {
        let (_root, path, id, ids) = transcript(151);
        let mut cursor = Some(Cursor {
            uuid: ids[150].clone(),
            offset: None,
            identity: None,
        });
        let mut pages = Vec::new();
        while let Some(before) = cursor.take() {
            let page = read(&path, id, before).unwrap();
            pages.push(
                page.messages
                    .iter()
                    .map(|message| message.transcript_uuid.clone().unwrap())
                    .collect::<Vec<_>>(),
            );
            cursor = page.before;
            // Appending to the current session cannot invalidate an older cursor.
            use std::io::Write;
            writeln!(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap(),
                "{{\"type\":\"custom-title\",\"customTitle\":\"append\"}}"
            )
            .unwrap();
        }
        pages.reverse();
        assert_eq!(pages.concat(), ids[..150]);
        assert_eq!(pages.len(), 3);
    }

    #[test]
    fn cursor_rejects_replaced_or_truncated_sources_and_foreign_uuids() {
        let (_root, path, id, ids) = transcript(80);
        let first = read(
            &path,
            id,
            Cursor {
                uuid: ids[79].clone(),
                offset: None,
                identity: None,
            },
        )
        .unwrap();
        assert!(read(&path, uuid::Uuid::new_v4(), first.before.clone().unwrap()).is_err());
        let backup = path.with_extension("backup");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::copy(&backup, &path).unwrap();
        assert!(read(&path, id, first.before.clone().unwrap()).is_err());
        std::fs::write(&path, "").unwrap();
        assert!(read(&path, id, first.before.unwrap()).is_err());
    }

    #[test]
    fn reverse_reader_handles_chunk_boundaries_unicode_and_missing_final_newline() {
        let (_root, path, _id, _) = transcript(0);
        let expected = vec![
            "first".to_string(),
            "中文🧪".repeat(20000),
            "last".to_string(),
        ];
        std::fs::write(&path, expected.join("\n")).unwrap();
        let file = File::open(&path).unwrap();
        let mut reader = ReverseLines {
            start: file.metadata().unwrap().len(),
            file,
            buffer: Vec::new(),
        };
        let mut result = Vec::new();
        while let Some((_, line)) = reader.next().unwrap() {
            result.push(line);
        }
        result.reverse();
        assert_eq!(result, expected);
    }
}
