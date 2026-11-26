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
// Legacy rows may carry four inline 20 MiB attachments before projection.
const MAX_RAW_ROW_BYTES: usize = 112 * 1024 * 1024;
const MAX_PREVIEW_SCAN_BYTES: u64 = 128 * 1024 * 1024;
const PAGE_ROWS: usize = 60;
pub(super) const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

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
    /// File offset of the current chunk, or the exclusive read bound initially.
    start: u64,
    floor: u64,
    reached_scan_limit: bool,
    chunk: Vec<u8>,
    end: usize,
    skip_final_newline: bool,
    #[cfg(test)]
    scanned_bytes: usize,
    #[cfg(test)]
    copied_bytes: usize,
}

impl ReverseLines {
    fn new(file: File, start: u64) -> Self {
        Self {
            file,
            start,
            floor: 0,
            reached_scan_limit: false,
            chunk: Vec::new(),
            end: 0,
            skip_final_newline: true,
            #[cfg(test)]
            scanned_bytes: 0,
            #[cfg(test)]
            copied_bytes: 0,
        }
    }

    fn with_scan_limit(mut self, bytes: u64) -> Self {
        self.floor = self.start.saturating_sub(bytes);
        self
    }

    fn next(&mut self) -> Result<Option<(u64, String)>, String> {
        // Chunks are moved here in reverse file order. Scan only unread bytes
        // and assemble a completed row once, even when it spans many chunks.
        let mut fragments = Vec::<Vec<u8>>::new();
        let mut bytes = 0;
        loop {
            if self.end == 0 && self.start > self.floor {
                let start = self.start.saturating_sub(CHUNK).max(self.floor);
                self.chunk = vec![0; (self.start - start) as usize];
                self.file
                    .seek(SeekFrom::Start(start))
                    .map_err(|e| e.to_string())?;
                self.file
                    .read_exact(&mut self.chunk)
                    .map_err(|e| e.to_string())?;
                self.start = start;
                self.end = self.chunk.len();
            }
            if self.skip_final_newline {
                if self.end > 0 && self.chunk[self.end - 1] == b'\n' {
                    self.end -= 1;
                }
                self.skip_final_newline = false;
            }
            let newline = self.chunk[..self.end]
                .iter()
                .rposition(|byte| *byte == b'\n');
            #[cfg(test)]
            {
                self.scanned_bytes += self.end - newline.map_or(0, |at| at + 1);
            }
            if let Some(newline) = newline {
                let offset = self.start + newline as u64 + 1;
                let line = self.assemble(newline + 1, bytes, fragments)?;
                self.end = newline;
                return Ok(Some((offset, line)));
            }
            if self.start == 0 {
                if self.end == 0 && fragments.is_empty() {
                    return Ok(None);
                }
                let line = self.assemble(0, bytes, fragments)?;
                self.end = 0;
                return Ok(Some((0, line)));
            }
            if self.start == self.floor {
                // The start of this row lies outside the preview window.
                self.reached_scan_limit = true;
                self.end = 0;
                return Ok(None);
            }
            bytes += self.end;
            if bytes > MAX_RAW_ROW_BYTES {
                return Err("One history message exceeds the page size limit".into());
            }
            self.chunk.truncate(self.end);
            fragments.push(std::mem::take(&mut self.chunk));
            self.end = 0;
        }
    }

    fn assemble(
        &mut self,
        from: usize,
        bytes: usize,
        fragments: Vec<Vec<u8>>,
    ) -> Result<String, String> {
        let length = bytes + self.end - from;
        if length > MAX_RAW_ROW_BYTES {
            return Err("One history message exceeds the page size limit".into());
        }
        let mut line = Vec::with_capacity(length);
        line.extend_from_slice(&self.chunk[from..self.end]);
        for fragment in fragments.into_iter().rev() {
            line.extend_from_slice(&fragment);
        }
        #[cfg(test)]
        {
            self.copied_bytes += line.len();
        }
        String::from_utf8(line).map_err(|_| "History contains invalid UTF-8".into())
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
    let file = File::open(path).map_err(|e| e.to_string())?;
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
    read_page(file, opened, session_id, Some(cursor), None)
}

/// Hold the very same no-symlink file across scope validation and preview by
/// reopening from the pinned root. A concurrent append or replacement drops
/// the speculative view; it never changes the ordinary selected-open path.
pub(super) struct LatestSource {
    file: File,
    home: std::path::PathBuf,
    relative: std::path::PathBuf,
    revision: PreviewRevision,
}

#[derive(PartialEq, Eq)]
struct PreviewRevision {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl LatestSource {
    fn open_file(home: &Path, relative: &Path) -> Result<File, String> {
        use lingxi_core::host::rooted_fs;
        let root = rooted_fs::root_identity(home).map_err(|error| error.to_string())?;
        let file = rooted_fs::open_read_file_pinned(home, relative, Some(&root))
            .map_err(|error| error.to_string())?;
        rooted_fs::validate_open_file_single_link(&file).map_err(|error| error.to_string())?;
        Ok(file)
    }

    pub(super) fn open(home: &Path, source: &Path, id: uuid::Uuid) -> Result<Self, String> {
        let (home, relative) = super::selected_transcript_location(home, source, id)?;
        let file = Self::open_file(&home, &relative)?;
        let revision = Self::revision(&file)?;
        Ok(Self {
            file,
            home,
            relative,
            revision,
        })
    }

    fn revision(file: &File) -> Result<PreviewRevision, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(PreviewRevision {
            len: metadata.len(),
            modified: metadata.modified().map_err(|error| error.to_string())?,
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }

    pub(super) fn read(self, session_id: uuid::Uuid) -> Result<Page, String> {
        use lingxi_core::host::rooted_fs::opened_file_identity;
        if Self::revision(&self.file)? != self.revision {
            return Err("History source changed before preview".into());
        }
        let page = read_page(
            self.file.try_clone().map_err(|error| error.to_string())?,
            self.file.metadata().map_err(|error| error.to_string())?,
            session_id,
            None,
            Some(MAX_PREVIEW_SCAN_BYTES),
        )?;
        let current = Self::open_file(&self.home, &self.relative)?;
        if opened_file_identity(&current).map_err(|error| error.to_string())?
            != opened_file_identity(&self.file).map_err(|error| error.to_string())?
            || Self::revision(&current)? != self.revision
        {
            return Err("History source changed during preview".into());
        }
        Ok(page)
    }

    pub(super) fn read_message(
        self,
        id: uuid::Uuid,
        location: Option<(u64, String)>,
    ) -> Result<session::jsonl::JsonlMessage, String> {
        use lingxi_core::host::rooted_fs::opened_file_identity;
        let id = id.to_string();
        let find = |line: &str| {
            line.contains(&id)
                .then(|| {
                    session::jsonl::route_lines(line)
                        .messages_in_order
                        .into_iter()
                        .find(|row| {
                            row.uuid == id && row.message_type == "user" && !row.is_sidechain
                        })
                })
                .flatten()
        };
        let mut file = self.file.try_clone().map_err(|error| error.to_string())?;
        let row = if let Some((offset, expected)) = location {
            let metadata = self.file.metadata().map_err(|error| error.to_string())?;
            if offset >= self.revision.len || identity(&metadata) != expected {
                return Err("History image source changed".into());
            }
            if offset > 0 {
                file.seek(SeekFrom::Start(offset - 1))
                    .map_err(|error| error.to_string())?;
                let mut delimiter = [0];
                file.read_exact(&mut delimiter)
                    .map_err(|error| error.to_string())?;
                if delimiter != [b'\n'] {
                    return Err("History image record changed".into());
                }
            } else {
                file.seek(SeekFrom::Start(0))
                    .map_err(|error| error.to_string())?;
            }
            let mut line = Vec::new();
            BufReader::new(&mut file)
                .take((MAX_RAW_ROW_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|error| error.to_string())?;
            if line.len() > MAX_RAW_ROW_BYTES {
                return Err("History image record exceeds the row size limit".into());
            }
            find(std::str::from_utf8(&line).map_err(|_| "Invalid history image record")?)
        } else {
            let mut reader = ReverseLines::new(file, self.revision.len);
            let mut row = None;
            while let Some((_, line)) = reader.next()? {
                row = find(&line);
                if row.is_some() {
                    break;
                }
            }
            row
        };
        let row = row.ok_or("History image is unavailable")?;
        let current = Self::open_file(&self.home, &self.relative)?;
        if opened_file_identity(&current).map_err(|error| error.to_string())?
            != opened_file_identity(&self.file).map_err(|error| error.to_string())?
        {
            return Err("History image source changed".into());
        }
        Ok(row)
    }
}

fn read_page(
    mut file: File,
    opened: std::fs::Metadata,
    session_id: uuid::Uuid,
    cursor: Option<Cursor>,
    scan_limit: Option<u64>,
) -> Result<Page, String> {
    let source_identity = identity(&opened);
    let mut found = cursor.as_ref().is_none_or(|cursor| cursor.offset.is_some());
    let mut ancestors = HashSet::new();
    let mut groups = HashSet::new();
    if let Some((cursor, offset)) = cursor
        .as_ref()
        .and_then(|cursor| cursor.offset.map(|offset| (cursor, offset)))
    {
        if offset >= opened.len() {
            return Err("History cursor is no longer valid".into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut line = Vec::new();
        BufReader::new(&mut file)
            .take((MAX_RAW_ROW_BYTES + 1) as u64)
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
    let mut reader = ReverseLines::new(
        file,
        cursor
            .as_ref()
            .and_then(|cursor| cursor.offset)
            .unwrap_or(opened.len()),
    );
    if let Some(bytes) = scan_limit {
        reader = reader.with_scan_limit(bytes);
    }
    let mut rows = Vec::new();
    let mut pending_siblings = HashMap::<_, Vec<(u64, session::jsonl::JsonlMessage)>>::new();
    let mut pending_calls = HashSet::new();
    let mut pending_summary = false;
    let mut safe_offsets = Vec::new();
    let mut bytes = 0;
    let page_budget = if scan_limit.is_some() { MAX_PREVIEW_BYTES } else { MAX_PAGE_BYTES };
    while let Some((offset, line)) = reader.next()? {
        let loaded = session::jsonl::route_lines(&line);
        let Some(mut row) = loaded
            .messages_in_order
            .into_iter()
            .find(|row| !row.is_sidechain)
        else {
            continue;
        };
        if !found {
            if cursor
                .as_ref()
                .is_some_and(|cursor| row.uuid == cursor.uuid)
            {
                found = true;
                ancestors.extend(row.parent_uuid.iter().cloned());
                groups.extend(assistant_group(&row));
            }
            continue;
        }
        let projected_image = super::history_image::project_images(&mut row, offset, &source_identity)?;
        if line.len() > page_budget {
            // Full pre-edit files and raw image tool results are omitted by the
            // canonical display lowerer. Avoid spending the page budget on them.
            if let Some(result) = row.tool_use_result_projection.as_mut() {
                let _ = result.remove_field("originalFile");
                if result.string_at("/type") == Some("image") {
                    let _ = result.remove_field("file");
                }
            }
        }
        let weight = if projected_image || line.len() > page_budget {
            serde_json::to_vec(&row).map_err(|error| error.to_string())?.len()
        } else {
            line.len()
        };
        if bytes + weight > page_budget {
            if !truncate_complete_page(&mut rows, &safe_offsets, &pending_siblings) {
                return Err("One history activity exceeds the page size limit".into());
            }
            break;
        }
        bytes += weight;
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
    if reader.reached_scan_limit
        && !truncate_complete_page(&mut rows, &safe_offsets, &pending_siblings)
    {
        return Err("Recent history exceeds the preview scan limit".into());
    }
    let (offsets, rows): (Vec<_>, Vec<_>) = rows.into_iter().rev().unzip();
    let mut first = 0;
    loop {
        let before = offsets.get(first).filter(|offset| **offset > 0).map(|offset| Cursor {
            uuid: rows[first].uuid.clone(),
            offset: Some(*offset),
            identity: Some(source_identity.clone()),
        });
        let page = Page { messages: lower_history_rows(session_id, &rows[first..]), before };
        if scan_limit.is_none() || serde_json::to_vec(&page).map_err(|e| e.to_string())?.len() <= page_budget {
            return Ok(page);
        }
        // The display DTO can grow while lowering structured results. Retry
        // with a smaller complete tail and keep its exclusive physical cursor.
        let target = first + (rows.len() - first).div_ceil(2);
        let pending_offset = latest_pending_offset(&pending_siblings);
        first = ((first + 1)..rows.len())
            .filter(|index| safe_offsets.contains(&offsets[*index])
                && pending_offset.is_none_or(|pending| offsets[*index] > pending))
            .min_by_key(|index| index.abs_diff(target))
            .ok_or("One history activity exceeds the preview output limit")?;
    }
}

fn lower_history_rows(session_id: uuid::Uuid, rows: &[session::jsonl::JsonlMessage]) -> Vec<MessageDto> {
    let state = orchestrator::resume::state_from_messages(session_id, rows);
    let results = orchestrator::resume::client_state_tool_results_from_messages(rows);
    let timestamps = rows
        .iter()
        .map(|row| (row.uuid.as_str(), &row.timestamp))
        .collect::<std::collections::HashMap<_, _>>();
    client::adapter::lowering::lower_transcript_with_identities(&state.history, &results)
            .into_iter()
            .map(|(id, mut message)| {
                let uuid = id.as_uuid().to_string();
                message.sent_at_ms = timestamps
                    .get(uuid.as_str())
                    .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                    .map(|at| at.as_millisecond());
                message.transcript_uuid = Some(uuid);
                super::history_image::reference_images(&mut message);
                message
            })
            .collect()
}

type AssistantGroup = (String, String, String);
type HistoryRow = (u64, session::jsonl::JsonlMessage);

fn truncate_complete_page(
    rows: &mut Vec<HistoryRow>,
    safe_offsets: &[u64],
    pending_siblings: &HashMap<AssistantGroup, Vec<HistoryRow>>,
) -> bool {
    let pending_offset = latest_pending_offset(pending_siblings);
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

fn latest_pending_offset(pending: &HashMap<AssistantGroup, Vec<HistoryRow>>) -> Option<u64> {
    pending.values().flatten().map(|(offset, _)| *offset).max()
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
    fn preview_scan_limit_keeps_the_complete_tail_and_an_exclusive_cursor() {
        let (_root, path, id, ids) = transcript(5);
        let source = std::fs::read(&path).unwrap();
        let newlines: Vec<_> = source
            .iter()
            .enumerate()
            .filter_map(|(offset, byte)| (*byte == b'\n').then_some(offset))
            .collect();
        let read_preview = |scan_limit| {
            let file = File::open(&path).unwrap();
            let metadata = file.metadata().unwrap();
            read_page(file, metadata, id, None, Some(scan_limit))
        };
        // Stop in the middle of row 2, after consuming complete rows 3 and 4.
        let scan_limit = source.len() - newlines[2] + 12;
        let preview = read_preview(scan_limit as u64).unwrap();
        let message_ids = |page: &Page| {
            page.messages
                .iter()
                .filter_map(|message| message.transcript_uuid.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(message_ids(&preview), ids[3..]);
        let cursor = preview.before.unwrap();
        assert_eq!(cursor.offset, Some(newlines[2] as u64 + 1));
        let older = read(&path, id, cursor).unwrap();
        assert_eq!(message_ids(&older), ids[..3]);
        assert!(older.before.is_none());
        assert_eq!(
            read_preview(12).err().as_deref(),
            Some("Recent history exceeds the preview scan limit")
        );
    }

    #[test]
    fn preview_with_large_text_keeps_a_byte_bounded_tail() {
        let id = uuid::Uuid::new_v4();
        let value = "x".repeat(40 * 1024);
        let (_root, path, tip) = saved_rows((0..60).map(|_| text_row(&value)).collect(), id);
        let file = File::open(&path).unwrap();
        let metadata = file.metadata().unwrap();
        let page = read_page(file, metadata, id, None, Some(MAX_PREVIEW_SCAN_BYTES)).unwrap();
        assert!(!page.messages.is_empty());
        assert!(page.messages.len() < 60);
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_PREVIEW_BYTES);
        assert_eq!(
            page.messages.last().unwrap().transcript_uuid.as_deref(),
            Some(tip.uuid.as_str())
        );
        let previous = read(&path, id, page.before.unwrap()).unwrap();
        assert_eq!(previous.messages.len() + page.messages.len(), 60);
    }

    #[test]
    fn legacy_image_locator_checks_offset_and_keeps_the_uuid_only_fallback() {
        use base64::Engine as _;

        let id = uuid::Uuid::new_v4();
        let image = uuid::Uuid::new_v4();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(640, 480)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let original = base64::engine::general_purpose::STANDARD.encode(encoded.into_inner());
        let row = serde_json::json!({"uuid":image,"type":"user","message":{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":original}}
        ]}});
        let (root, path, tip) = saved_rows(vec![row, text_row("newer")], id);
        let folder = root.path().join("projects").join("workspace");
        std::fs::create_dir_all(&folder).unwrap();
        let selected = folder.join(path.file_name().unwrap());
        std::fs::rename(path, &selected).unwrap();
        let open = || LatestSource::open(root.path(), &selected, id).unwrap();
        let page = read(&selected, id, tip).unwrap();
        let image_ref = &page.messages[0].images[0].url;
        assert!(image_ref.starts_with(&format!("lingxi-history-image:{image}:0:png:0:")));
        let identity = identity(&std::fs::metadata(&selected).unwrap());
        let located = open()
            .read_message(image, Some((0, identity.clone())))
            .unwrap();
        let legacy = open().read_message(image, None).unwrap();
        assert_eq!(located.uuid, image.to_string());
        assert_eq!(legacy.uuid, located.uuid);
        assert!(open().read_message(image, Some((1, identity))).is_err());
        assert!(
            open()
                .read_message(image, Some((0, "different".into())))
                .is_err()
        );
        let read_image = |reference: &str, size: &str| {
            let request = serde_json::from_value(serde_json::json!({
                "reference": reference, "size": size,
            }))
            .unwrap();
            serde_json::to_value(super::super::history_image::read(open(), request).unwrap())
                .unwrap()
        };
        let legacy_ref = format!("lingxi-history-image:{image}:0:png");
        assert_eq!(read_image(&legacy_ref, "original")["base64"], original);
        let thumbnail = read_image(image_ref, "thumbnail");
        let data = base64::engine::general_purpose::STANDARD
            .decode(thumbnail["base64"].as_str().unwrap())
            .unwrap();
        let image = image::load_from_memory(&data).unwrap();
        assert_eq!((image.width(), image.height()), (320, 240));
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
        let length = file.metadata().unwrap().len();
        let mut reader = ReverseLines::new(file, length);
        let mut result = Vec::new();
        while let Some((_, line)) = reader.next().unwrap() {
            result.push(line);
        }
        result.reverse();
        assert_eq!(result, expected);
    }

    #[test]
    fn reverse_reader_preserves_offsets_and_scans_and_copies_long_rows_once() {
        let (_root, path, _id, _) = transcript(0);
        for length in [
            CHUNK as usize - 1,
            CHUNK as usize,
            CHUNK as usize + 1,
            MAX_PAGE_BYTES - 1,
            MAX_PAGE_BYTES,
            MAX_PAGE_BYTES + CHUNK as usize,
        ] {
            let prefix = "中文🧪\\ud800";
            let middle = format!("{prefix}{}", "x".repeat(length - prefix.len()));
            let expected = [
                (0, "first\r"),
                (7, middle.as_str()),
                (8 + length as u64, "last"),
            ];
            for final_newline in [false, true] {
                let mut source = format!("first\r\n{middle}\nlast");
                if final_newline {
                    source.push('\n');
                }
                std::fs::write(&path, &source).unwrap();
                let file = File::open(&path).unwrap();
                let mut reader = ReverseLines::new(file, source.len() as u64);
                for (offset, line) in expected.iter().rev() {
                    assert_eq!(reader.next().unwrap(), Some((*offset, (*line).to_owned())));
                }
                assert!(reader.next().unwrap().is_none());
                assert!(reader.scanned_bytes <= source.len());
                assert_eq!(reader.copied_bytes, 6 + length + 4);
            }
        }
    }

    #[test]
    fn reverse_reader_rejects_an_oversized_row_and_invalid_utf8() {
        let (_root, path, _id, _) = transcript(0);
        let length = MAX_RAW_ROW_BYTES + 1;
        // A sparse oversized row exercises the raw limit without storing 112 MiB.
        File::create(&path).unwrap().set_len(length as u64).unwrap();
        let mut reader = ReverseLines::new(File::open(&path).unwrap(), length as u64);
        assert!(reader.next().unwrap_err().contains("page size limit"));
        assert_eq!(
            reader.copied_bytes, 0,
            "reject before assembling the oversized row"
        );
        std::fs::write(&path, [0xc0, 0x80]).unwrap();
        let mut reader = ReverseLines::new(File::open(&path).unwrap(), 2);
        assert!(reader.next().unwrap_err().contains("invalid UTF-8"));
    }

    #[test]
    fn near_limit_rows_page_without_gaps_with_exact_byte_cursors_and_lone_surrogates() {
        let id = uuid::Uuid::new_v4();
        let ids = (0..3)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect::<Vec<_>>();
        let mut rows = ids
            .iter()
            .enumerate()
            .map(|(index, uuid)| {
                let mut row = text_row(&format!(
                    "row-{index} 中文🧪 {} surrogate-marker",
                    "x".repeat(MAX_PAGE_BYTES - 4096)
                ));
                row["uuid"] = uuid.clone().into();
                row
            })
            .collect::<Vec<_>>();
        rows.push(text_row("anchor"));
        let (_root, path, mut cursor) = saved_rows(rows, id);
        let source = std::fs::read_to_string(&path)
            .unwrap()
            .replace("surrogate-marker", "\\ud800");
        std::fs::write(&path, &source).unwrap();
        let mut offset = 0;
        let offsets = source
            .split_inclusive('\n')
            .map(|line| {
                let start = offset;
                offset += line.len() as u64;
                start
            })
            .collect::<Vec<_>>();
        let mut seen = Vec::new();
        for index in (0..3).rev() {
            let page = read(&path, id, cursor).unwrap();
            assert_eq!(page.messages.len(), 1);
            assert_eq!(
                page.messages[0].transcript_uuid.as_deref(),
                Some(ids[index].as_str())
            );
            assert!(serde_json::to_string(&page.messages)
                .unwrap()
                .contains("\\ud800"));
            seen.insert(0, page.messages[0].transcript_uuid.clone().unwrap());
            if index == 0 {
                assert!(page.before.is_none());
                break;
            }
            cursor = page.before.unwrap();
            assert_eq!(cursor.offset, Some(offsets[index]));
        }
        assert_eq!(seen, ids);
    }
}
