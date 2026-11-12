//! Read older display rows backwards from an exact transcript. No engine or account IO.
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
        if !session::jsonl::route_lines(&source)
            .messages_in_order
            .iter()
            .any(|row| row.uuid == cursor.uuid)
        {
            return Err("History moved since the previous page. Reopen this chat.".into());
        }
    }
    let mut reader = ReverseLines {
        file,
        start: cursor.offset.unwrap_or(opened.len()),
        buffer: Vec::new(),
    };
    let mut rows = Vec::new();
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
            }
            continue;
        }
        if bytes + line.len() > MAX_PAGE_BYTES && !rows.is_empty() {
            break;
        }
        bytes += line.len();
        before = Some(Cursor {
            uuid: row.uuid.clone(),
            offset: Some(offset),
            identity: Some(source_identity.clone()),
        });
        rows.push(row);
        if rows.len() >= PAGE_ROWS {
            break;
        }
    }
    if !found {
        return Err("History anchor is unavailable. Reopen this chat.".into());
    }
    if before
        .as_ref()
        .is_some_and(|cursor| cursor.offset == Some(0))
        || rows.is_empty()
    {
        before = None;
    }
    rows.reverse();
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

#[cfg(test)]
mod tests {
    use super::*;

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
