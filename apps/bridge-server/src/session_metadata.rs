//! Private Host metadata operations on one exact, already selected transcript.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use lingxi_core::host::rooted_fs::RootIdentity;
use lingxi_core::host::session_context::ConversationMode;
use lingxi_core::types::SessionId;
use serde::{Deserialize, Serialize};
use session::jsonl::writer::{SessionMetadataOperation, TranscriptFileAuthority};

const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Private request sent to the live connection that owns this writer.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MetadataMutation {
    Capture {
        session_id: String,
        transcript_path: PathBuf,
        session_mode: ConversationMode,
    },
    Rename {
        receipt: String,
        title: String,
    },
    Touch {
        receipt: String,
    },
    Verify {
        receipt: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActiveReceipt {
    version: u8,
    session_id: String,
    transcript_path: PathBuf,
    session_mode: ConversationMode,
    incarnation: uuid::Uuid,
}

fn active_receipt(raw: &str) -> Result<ActiveReceipt, String> {
    if raw.len() > MAX_REQUEST_BYTES as usize {
        return Err("session metadata receipt is too large".into());
    }
    let receipt: ActiveReceipt = serde_json::from_str(raw).map_err(|_| "invalid active receipt")?;
    if receipt.version != 1
        || !receipt.transcript_path.is_absolute()
        || SessionId::parse_prefixed(&receipt.session_id).is_none()
    {
        return Err("invalid active receipt".into());
    }
    Ok(receipt)
}

impl MetadataMutation {
    pub(crate) fn session_id(&self) -> Result<SessionId, String> {
        let id = match self {
            Self::Capture { session_id, .. } => session_id.clone(),
            Self::Rename { receipt, .. } | Self::Touch { receipt } | Self::Verify { receipt } => {
                active_receipt(receipt)?.session_id
            }
        };
        SessionId::parse_prefixed(&id).ok_or_else(|| "invalid session identity".into())
    }
}

pub(crate) async fn apply_active(
    writer: &Arc<session::JsonlWriter>,
    request: MetadataMutation,
) -> Result<serde_json::Value, String> {
    let session_id = request.session_id()?;
    let (path, mode, expected, operation) = match request {
        MetadataMutation::Capture {
            transcript_path,
            session_mode,
            ..
        } => (
            transcript_path,
            session_mode,
            None,
            SessionMetadataOperation::Capture,
        ),
        MetadataMutation::Rename { receipt, title } => {
            let receipt = active_receipt(&receipt)?;
            (
                receipt.transcript_path,
                receipt.session_mode,
                Some(receipt.incarnation),
                SessionMetadataOperation::Rename(title),
            )
        }
        MetadataMutation::Touch { receipt } => {
            let receipt = active_receipt(&receipt)?;
            (
                receipt.transcript_path,
                receipt.session_mode,
                Some(receipt.incarnation),
                SessionMetadataOperation::Touch,
            )
        }
        MetadataMutation::Verify { receipt } => {
            let receipt = active_receipt(&receipt)?;
            (
                receipt.transcript_path,
                receipt.session_mode,
                Some(receipt.incarnation),
                SessionMetadataOperation::Verify,
            )
        }
    };
    let is_capture = matches!(&operation, SessionMetadataOperation::Capture);
    let incarnation = writer
        .update_active_session_metadata(session_id, &path, mode, expected, operation)
        .await
        .map_err(|error| error.to_string())?;
    if is_capture {
        let receipt = serde_json::to_string(&ActiveReceipt {
            version: 1,
            session_id: session_id.as_uuid().to_string(),
            transcript_path: path,
            session_mode: mode,
            incarnation,
        })
        .map_err(|error| error.to_string())?;
        Ok(serde_json::json!({"receipt":receipt}))
    } else {
        Ok(serde_json::json!({"ok":true}))
    }
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum MetadataCommand {
    Capture {
        session_id: String,
        transcript_path: PathBuf,
        session_mode: ConversationMode,
    },
    Rename {
        receipt: String,
        title: String,
    },
    Touch {
        receipt: String,
    },
    Verify {
        receipt: String,
    },
}

// This value remains an opaque string in JavaScript so filesystem identities
// are never rounded through a JS number. Its root is still revalidated against
// the trusted home and the same UUID lease before any mutation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceReceipt {
    version: u8,
    session_id: String,
    session_mode: ConversationMode,
    relative: PathBuf,
    root_identity: RootIdentity,
    file_identity: RootIdentity,
}

fn checked_source(
    authority: &TranscriptFileAuthority,
    session_id: SessionId,
    mode: ConversationMode,
) -> Result<String, String> {
    checked_source_with_hook(authority, session_id, mode, || {})
}

fn checked_source_with_hook(
    authority: &TranscriptFileAuthority,
    session_id: SessionId,
    mode: ConversationMode,
    after_read: impl FnOnce(),
) -> Result<String, String> {
    let content = String::from_utf8(
        authority
            .read_checked()
            .map_err(|error| error.to_string())?,
    )
    .map_err(|_| "transcript is not UTF-8")?;
    if !content.ends_with('\n') {
        return Err("transcript has an incomplete final record".into());
    }
    let id = session_id.as_uuid();
    let actual = session::JsonlWriter::fixed_session_mode_from_source(&content, &id.to_string())?;
    if actual != mode {
        return Err("session mode changed".into());
    }
    let loaded = match session::jsonl::loader::route_captured_session_source(
        &authority.path(),
        id,
        &content,
    ) {
        Ok(loaded) => loaded,
        Err(_) if session::jsonl::is_mode_only_draft(&content, &id.to_string()) => {
            session::jsonl::route_lines(&content)
        }
        Err(error) => return Err(format!("selected transcript is not resumable: {error}")),
    };
    crate::boot::validate_captured_transcript(&authority.path(), id, &content, &loaded, true)?;
    if authority
        .read_checked()
        .map_err(|error| error.to_string())?
        != content.as_bytes()
    {
        return Err("transcript changed during validation".into());
    }
    after_read();
    authority
        .verify_current()
        .map_err(|error| error.to_string())?;
    Ok(content)
}

fn capture(
    home: &Path,
    session_id: &str,
    path: &Path,
    mode: ConversationMode,
) -> Result<String, String> {
    let session_id = SessionId::parse_prefixed(session_id).ok_or("invalid session identity")?;
    let (authority, captured) =
        crate::boot::capture_selected_transcript_source(home, path, session_id.as_uuid())?;
    let content = checked_source(&authority, session_id, mode)?;
    if content != captured {
        return Err("transcript changed during metadata capture".into());
    }
    let pinned = authority.pinned_resume_source(String::new());
    serde_json::to_string(&SourceReceipt {
        version: 1,
        session_id: session_id.as_uuid().to_string(),
        session_mode: mode,
        relative: pinned.relative,
        root_identity: pinned.root_identity,
        file_identity: pinned.file_identity,
    })
    .map_err(|error| error.to_string())
}

fn authority_from_receipt(
    home: &Path,
    raw: &str,
) -> Result<(SourceReceipt, SessionId, TranscriptFileAuthority), String> {
    if raw.len() > MAX_REQUEST_BYTES as usize {
        return Err("session metadata receipt is too large".into());
    }
    let receipt: SourceReceipt =
        serde_json::from_str(raw).map_err(|_| "invalid session receipt")?;
    let id = SessionId::parse_prefixed(&receipt.session_id).ok_or("invalid session identity")?;
    let parts: Vec<_> = receipt.relative.components().collect();
    if receipt.version != 1
        || parts.len() != 3
        || !matches!(parts[0], Component::Normal(value) if value == "projects")
        || !matches!(parts[1], Component::Normal(_))
        || !matches!(parts[2], Component::Normal(_))
        || receipt
            .relative
            .extension()
            .and_then(|value| value.to_str())
            != Some("jsonl")
        || receipt
            .relative
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            != Some(id.as_uuid())
    {
        return Err("invalid session receipt source".into());
    }
    let root = std::fs::canonicalize(home).map_err(|_| "session home is unavailable")?;
    let authority = TranscriptFileAuthority::from_capture(
        root,
        receipt.relative.clone(),
        receipt.root_identity,
        receipt.file_identity,
    )
    .map_err(|error| error.to_string())?;
    Ok((receipt, id, authority))
}

async fn mutate_cold(
    home: &Path,
    raw: &str,
    operation: SessionMetadataOperation,
) -> Result<(), String> {
    let (receipt, id, authority) = authority_from_receipt(home, raw)?;
    let owner_home = authority.pinned_resume_source(String::new()).root;
    if lingxi_core::host::rooted_fs::root_identity(&owner_home)
        .map_err(|error| error.to_string())?
        != receipt.root_identity
    {
        return Err("session home identity changed".into());
    }
    let lease =
        lingxi_core::host::live_sessions::LiveSessionDir::at_live(owner_home.join("sessions"))
            .claim_session_id(&id.as_uuid().to_string(), std::process::id())
            .map_err(|error| format!("session is not available for metadata: {error}"))?
            .into_shared();
    if lingxi_core::host::rooted_fs::root_identity(&owner_home)
        .map_err(|error| error.to_string())?
        != receipt.root_identity
    {
        return Err("session home identity changed during claim".into());
    }
    checked_source(&authority, id, receipt.session_mode)?;
    if matches!(&operation, SessionMetadataOperation::Verify) {
        // The UUID claim serializes this read against cooperating writers.
        // No session-state directory or transcript modification is needed.
        return Ok(());
    }
    // This opens only session state, never a model, hook, or MCP client. The
    // coordinator keeps the UUID lease alive until the mutation has settled.
    let coordinator = harness_runtime::desktop::session_state::SessionStateCoordinator::open(
        &owner_home,
        id,
        lease,
    )
    .map_err(|error| error.to_string())?;
    let journal = coordinator.journal();
    let durable = Arc::new(session::jsonl::DurableTranscriptWriter::from_pinned(
        journal.root().to_path_buf(),
        journal.root_identity(),
    ));
    let path = authority.path();
    let fs: Arc<dyn lingxi_core::host::FileSystem> =
        Arc::new(crate::HostFileSystem::new(owner_home.clone()));
    let writer = Arc::new(session::JsonlWriter::new(path.clone(), fs).with_durable_lock(durable));
    writer
        .activate_session_target_with_authority(id, path.clone(), owner_home, Some(authority))
        .map_err(|error| error.to_string())?;
    // A cancelled CLI observer must not release the UUID claim while the
    // writer's detached, lock-owning operation is still finishing.
    tokio::spawn(async move {
        let _coordinator = coordinator;
        let incarnation = writer
            .update_active_session_metadata(
                id,
                &path,
                receipt.session_mode,
                None,
                SessionMetadataOperation::Capture,
            )
            .await
            .map_err(|error| error.to_string())?;
        writer
            .update_active_session_metadata(
                id,
                &path,
                receipt.session_mode,
                Some(incarnation),
                operation,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn execute(home: &Path, command: MetadataCommand) -> Result<serde_json::Value, String> {
    match command {
        MetadataCommand::Capture {
            session_id,
            transcript_path,
            session_mode,
        } => {
            let receipt = capture(home, &session_id, &transcript_path, session_mode)?;
            Ok(serde_json::json!({"receipt": receipt}))
        }
        MetadataCommand::Rename { receipt, title } => {
            mutate_cold(home, &receipt, SessionMetadataOperation::Rename(title)).await?;
            Ok(serde_json::json!({"ok": true}))
        }
        MetadataCommand::Touch { receipt } => {
            mutate_cold(home, &receipt, SessionMetadataOperation::Touch).await?;
            Ok(serde_json::json!({"ok": true}))
        }
        MetadataCommand::Verify { receipt } => {
            mutate_cold(home, &receipt, SessionMetadataOperation::Verify).await?;
            Ok(serde_json::json!({"ok": true}))
        }
    }
}

/// Execute a bounded, single-request CLI operation without assembling an engine.
pub async fn run(home: &Path, input: impl Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    input
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_REQUEST_BYTES as usize {
        return Err("session metadata request is too large".into());
    }
    let command = serde_json::from_slice(&bytes).map_err(|_| "invalid session metadata request")?;
    serde_json::to_string(&execute(home, command).await?).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, SessionId, PathBuf, String) {
        let directory = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(directory.path()).unwrap();
        let id = SessionId::new();
        let path = home
            .join("projects")
            .join("owner")
            .join(format!("{}.jsonl", id.as_uuid()));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"fixed-session-mode","sessionId":id.as_uuid(),"sessionMode":"code","instructionEpoch":0}),
            serde_json::json!({"type":"user","uuid":uuid::Uuid::new_v4(),"parentUuid":null,"sessionId":id.as_uuid(),"cwd":home,"message":{"role":"user","content":"original"}})
        );
        std::fs::write(&path, &content).unwrap();
        (directory, id, path, content)
    }

    #[tokio::test]
    async fn cold_rename_preserves_valid_empty_draft_discovery() {
        let directory = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(directory.path()).unwrap();
        let cwd = home.join("workspace");
        std::fs::create_dir(&cwd).unwrap();
        let id = SessionId::new();
        let sid = id.as_uuid().to_string();
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &sid);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let content = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"fixed-session-mode","sessionId":sid,"sessionMode":"code","instructionEpoch":0}),
            serde_json::json!({"type":"custom-title","sessionId":sid,"customTitle":"Draft","mobileEmptySession":1,"cwd":cwd})
        );
        std::fs::write(&path, content).unwrap();
        let receipt = capture(&home, &sid, &path, ConversationMode::Code).unwrap();
        mutate_cold(
            &home,
            &receipt,
            SessionMetadataOperation::Rename("Renamed".into()),
        )
        .await
        .unwrap();
        let renamed = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            session::jsonl::validated_empty_draft_cwd(&renamed, &sid),
            Some(cwd.clone())
        );
        session::jsonl::loader::describe_session_source(
            &home,
            &cwd.to_string_lossy(),
            session::jsonl::SessionDiscoveryScope::Current,
            &path,
            id.as_uuid(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn cold_metadata_uses_selected_source_and_serializes_with_uuid_owner() {
        let (_directory, id, path, content) = fixture();
        let home = path.parent().unwrap().parent().unwrap().parent().unwrap();
        let sibling = home
            .join("projects/sibling")
            .join(path.file_name().unwrap());
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, &content).unwrap();
        let receipt = capture(
            home,
            &id.as_uuid().to_string(),
            &path,
            ConversationMode::Code,
        )
        .unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        mutate_cold(home, &receipt, SessionMetadataOperation::Verify)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        let lease =
            lingxi_core::host::live_sessions::LiveSessionDir::at_live(home.join("sessions"))
                .claim_session_id(&id.as_uuid().to_string(), std::process::id())
                .unwrap();
        assert!(mutate_cold(
            home,
            &receipt,
            SessionMetadataOperation::Rename("Busy".into())
        )
        .await
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        drop(lease);
        mutate_cold(
            home,
            &receipt,
            SessionMetadataOperation::Rename("Selected".into()),
        )
        .await
        .unwrap();
        let renamed = std::fs::read_to_string(&path).unwrap();
        assert!(renamed.contains("Selected"));
        assert_eq!(std::fs::read_to_string(&sibling).unwrap(), content);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
            .unwrap();
        drop(file);
        mutate_cold(home, &receipt, SessionMetadataOperation::Touch)
            .await
            .unwrap();
        assert!(
            std::fs::metadata(&path).unwrap().modified().unwrap()
                > std::time::SystemTime::UNIX_EPOCH
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), renamed);
    }

    #[tokio::test]
    async fn cold_metadata_rejects_replaced_inode_and_wrong_profile() {
        let (_directory, id, path, content) = fixture();
        let home = path.parent().unwrap().parent().unwrap().parent().unwrap();
        assert!(capture(
            home,
            &id.as_uuid().to_string(),
            &path,
            ConversationMode::Chat
        )
        .is_err());
        let receipt = capture(
            home,
            &id.as_uuid().to_string(),
            &path,
            ConversationMode::Code,
        )
        .unwrap();
        let old = path.with_extension("old");
        std::fs::rename(&path, &old).unwrap();
        std::fs::write(&path, &content).unwrap();
        assert!(mutate_cold(
            home,
            &receipt,
            SessionMetadataOperation::Rename("Wrong".into())
        )
        .await
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert_eq!(std::fs::read_to_string(&old).unwrap(), content);
    }

    #[test]
    fn final_cold_validation_rejects_replacement_after_body_read() {
        let (_directory, id, path, content) = fixture();
        let home = path.parent().unwrap().parent().unwrap().parent().unwrap();
        let receipt = capture(
            home,
            &id.as_uuid().to_string(),
            &path,
            ConversationMode::Code,
        )
        .unwrap();
        let (_, _, authority) = authority_from_receipt(home, &receipt).unwrap();
        let old = path.with_extension("old");
        assert!(
            checked_source_with_hook(&authority, id, ConversationMode::Code, || {
                std::fs::rename(&path, &old).unwrap();
                std::fs::write(&path, &content).unwrap();
            })
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert_eq!(std::fs::read_to_string(&old).unwrap(), content);
    }

    #[tokio::test]
    async fn private_cli_json_round_trips_opaque_receipt() {
        let (_directory, id, path, content) = fixture();
        let home = path.parent().unwrap().parent().unwrap().parent().unwrap();
        let request = serde_json::json!({
            "action":"capture", "sessionId":id.as_uuid(),
            "transcriptPath":path, "sessionMode":"code"
        })
        .to_string();
        let captured: serde_json::Value =
            serde_json::from_str(&run(home, request.as_bytes()).await.unwrap()).unwrap();
        let receipt = captured["receipt"].as_str().unwrap();
        let request = serde_json::json!({"action":"verify", "receipt":receipt}).to_string();
        let verified: serde_json::Value =
            serde_json::from_str(&run(home, request.as_bytes()).await.unwrap()).unwrap();
        assert_eq!(verified, serde_json::json!({"ok":true}));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }
}
