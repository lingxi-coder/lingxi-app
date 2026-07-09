//! `/branch` — create a fork of the current conversation at this point.
//!
//! 1:1 port of `claude-code/src/commands/branch/branch.ts`. Copies the live
//! session's transcript into a NEW `<uuid>.jsonl` under the same project dir,
//! rewriting `sessionId`, rechaining `parentUuid`, clearing `isSidechain`, and
//! stamping each entry with a `forkedFrom { sessionId, messageUuid }`
//! back-reference. Saves a `"<base> (Branch)"` custom title (collision-numbered),
//! then hands the new session id back so the CLI can switch INTO the branch via
//! the proven in-process re-mount seam (`mount_resumed_tui`).
//!
//! DISTINCT from `/fork`, which spawns a detached background agent; `/branch`
//! creates a sibling session file and moves the user into it. Mirrors claude's
//! `createFork` + `getUniqueForkName` + `deriveFirstPrompt`. LingXi has no
//! `content-replacement` transcript concept, so that copy step is a no-op here.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use traits::FileSystem;
use uuid::Uuid;

use crate::jsonl::{
    derive_fork_name, list_recent_sessions, route_lines, session_path, JsonlMessage,
};

/// Outcome of a successful [`create_branch`].
#[derive(Debug, Clone)]
pub struct BranchResult {
    /// The freshly-minted branch session id (the switch target).
    pub new_session_id: Uuid,
    /// The session that was branched (for the "resume the original" hint).
    pub source_session_id: Uuid,
    /// The effective `"… (Branch[ N])"` title saved for the branch.
    pub title: String,
    /// Count of conversation messages copied into the branch.
    pub message_count: usize,
}

/// Why a branch could not be created.
#[derive(Debug)]
pub enum BranchError {
    /// The source transcript is missing or empty (`"No conversation to branch"`).
    NoConversation,
    /// The source has no main-conversation messages (`"No messages to branch"`).
    NoMessages,
    /// A filesystem read/write failed.
    Io(std::io::Error),
}

impl std::fmt::Display for BranchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConversation => write!(f, "No conversation to branch"),
            Self::NoMessages => write!(f, "No messages to branch"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for BranchError {}

/// Create a branch of `source_session_id` under the same project dir.
///
/// `custom_title` is the trimmed `/branch [name]` argument (empty / `None` ⇒
/// derive the base name from the first user message). Returns the new session
/// id + saved title so the caller can switch into the branch.
///
/// # Errors
/// [`BranchError::NoConversation`] when the source file is missing/empty,
/// [`BranchError::NoMessages`] when it holds no chain messages, and
/// [`BranchError::Io`] on any read/write failure.
pub async fn create_branch(
    lingxi_home: &Path,
    cwd: &str,
    source_session_id: Uuid,
    custom_title: Option<&str>,
    fs: Arc<dyn FileSystem>,
) -> Result<BranchResult, BranchError> {
    // 1. Read the current transcript. Missing / empty ⇒ nothing to branch.
    let src_path = session_path(lingxi_home, cwd, &source_session_id.to_string());
    let content = match tokio::fs::read_to_string(&src_path).await {
        Ok(c) if !c.trim().is_empty() => c,
        Ok(_) => return Err(BranchError::NoConversation),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(BranchError::NoConversation)
        }
        Err(e) => return Err(BranchError::Io(e)),
    };

    // 2. Route to the linear main-conversation chain. `route_lines` yields only
    //    transcript chain participants (sidechains + metadata side-maps are
    //    filtered out), matching claude's `isTranscriptMessage && !isSidechain`.
    let messages = route_lines(&content).messages_in_order;
    if messages.is_empty() {
        return Err(BranchError::NoMessages);
    }

    // 3. Base name = provided title, else the first-prompt derivation
    //    (`deriveFirstPrompt`). Then append the collision-numbered "(Branch)".
    let base = match custom_title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => derive_fork_name(&messages),
    };
    let title = unique_fork_name(lingxi_home, cwd, &base, &fs).await;

    // 4. Rewrite each entry: new sessionId, rechained parentUuid, cleared
    //    sidechain flag, plus a `forkedFrom` back-reference to the origin.
    let new_session_id = Uuid::new_v4();
    let new_sid = new_session_id.to_string();
    let src_sid = source_session_id.to_string();
    let mut parent: Option<String> = None;
    let mut lines: Vec<String> = Vec::with_capacity(messages.len() + 1);
    for mut entry in messages {
        let original_uuid = entry.uuid.clone();
        entry.session_id = new_sid.clone();
        entry.parent_uuid = parent.clone();
        entry.is_sidechain = false;
        entry.extra.insert(
            "forkedFrom".to_string(),
            json!({ "sessionId": src_sid, "messageUuid": original_uuid }),
        );
        // JsonlMessage's hand-written Serialize emits claude's per-kind key order
        // and never fails for a well-formed message.
        lines.push(serde_json::to_string(&entry).expect("JsonlMessage serializes"));
        parent = Some(original_uuid);
    }

    // 5. Append the custom-title side-map entry so /resume + /status show
    //    "<base> (Branch)" (claude `saveCustomTitle`). Shape matches the
    //    `custom-title` branch of `route_lines`: {type,sessionId,customTitle}.
    lines.push(
        serde_json::to_string(&json!({
            "type": "custom-title",
            "sessionId": new_sid,
            "customTitle": title,
        }))
        .expect("custom-title serializes"),
    );
    let message_count = lines.len() - 1;

    // 6. Write the branch file (0o600 like claude; the project dir already
    //    exists — it holds the source we just read).
    let dst_path = session_path(lingxi_home, cwd, &new_sid);
    let body = format!("{}\n", lines.join("\n"));
    tokio::fs::write(&dst_path, body.as_bytes())
        .await
        .map_err(BranchError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(0o600)).await;
    }

    Ok(BranchResult {
        new_session_id,
        source_session_id,
        title,
        message_count,
    })
}

/// `getUniqueForkName` (`branch.ts:179`): `"<base> (Branch)"`, or
/// `"<base> (Branch N)"` when the plain form (or a lower N) already exists.
/// Best-effort — a session-scan failure degrades to the un-numbered name rather
/// than blocking the branch.
async fn unique_fork_name(
    lingxi_home: &Path,
    cwd: &str,
    base: &str,
    fs: &Arc<dyn FileSystem>,
) -> String {
    let plain = format!("{base} (Branch)");
    let titles: HashSet<String> = list_recent_sessions(lingxi_home, cwd, usize::MAX, fs.clone())
        .await
        .map(|rows| rows.into_iter().map(|s| s.title).collect())
        .unwrap_or_default();
    if !titles.contains(&plain) {
        return plain;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base} (Branch {n})");
        if !titles.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_posix::PosixFileSystem;

    /// A unique scratch dir under the OS temp root (session has no `tempfile`
    /// dev-dep; the writer tests use the same pattern).
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lingxi-branch-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn user_line(uuid: &str, sid: &str, parent: Option<&str>, text: &str) -> String {
        json!({
            "type": "user",
            "uuid": uuid,
            "parentUuid": parent,
            "sessionId": sid,
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": "/tmp/proj",
            "version": "0.0.0",
            "message": { "role": "user", "content": text },
        })
        .to_string()
    }

    #[tokio::test]
    async fn branch_rewrites_session_rechains_and_stamps_forked_from() {
        let home = scratch("rewrite");
        let cwd = "/tmp/proj";
        let src = Uuid::new_v4();
        let src_sid = src.to_string();
        let path = session_path(&home, cwd, &src_sid);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        let body = format!(
            "{}\n{}\n",
            user_line(
                "11111111-1111-1111-1111-111111111111",
                &src_sid,
                None,
                "first prompt"
            ),
            user_line(
                "22222222-2222-2222-2222-222222222222",
                &src_sid,
                Some("11111111-1111-1111-1111-111111111111"),
                "second",
            ),
        );
        tokio::fs::write(&path, body).await.unwrap();

        let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(std::path::PathBuf::from(cwd)));
        let result = create_branch(&home, cwd, src, None, fs).await.expect("branch");
        assert_eq!(result.message_count, 2);
        assert_ne!(result.new_session_id, src);
        assert!(result.title.contains("(Branch)"), "got {}", result.title);

        let new_sid = result.new_session_id.to_string();
        let written = tokio::fs::read_to_string(session_path(&home, cwd, &new_sid))
            .await
            .unwrap();
        let lines: Vec<&str> = written.lines().collect();
        // 2 conversation lines + 1 custom-title line.
        assert_eq!(lines.len(), 3);
        let first: JsonlMessage = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.session_id, new_sid);
        assert_eq!(first.parent_uuid, None, "first entry re-roots the chain");
        assert_eq!(
            first.extra.get("forkedFrom").unwrap()["sessionId"]
                .as_str()
                .unwrap(),
            src_sid
        );
        let second: JsonlMessage = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(
            second.parent_uuid.as_deref(),
            Some("11111111-1111-1111-1111-111111111111"),
            "parentUuid rechained to the prior ORIGINAL uuid"
        );
        let title_line: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(title_line["type"], "custom-title");
        assert_eq!(title_line["sessionId"], new_sid);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn missing_source_is_no_conversation() {
        let home = scratch("missing");
        let fs: Arc<dyn FileSystem> =
            Arc::new(PosixFileSystem::new(std::path::PathBuf::from("/tmp/proj")));
        let err = create_branch(&home, "/tmp/proj", Uuid::new_v4(), None, fs)
            .await
            .unwrap_err();
        assert!(matches!(err, BranchError::NoConversation));
        let _ = std::fs::remove_dir_all(&home);
    }
}
