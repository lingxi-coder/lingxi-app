//! Streaming binary file lane shared by iOS and Android. The wire matches
//! bridge-client/remote/files.ts; file contents never enter JSON or RTDB.
use crate::connection::ViewerConnection;
use async_trait::async_trait;
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

/// SCTP frame bound and binary-header size, shared with the web client.
pub const FRAME_BYTES: usize = 16 * 1024;
const HEADER: usize = 25;
const CHUNK: usize = FRAME_BYTES - HEADER;
const SAFE_INTEGER: u64 = (1_u64 << 53) - 1;
/// Original-file metadata; no MIME conversion is performed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileOffer {
    /// 128-bit lowercase hexadecimal transfer identity.
    pub transfer_id: String,
    /// Attached desktop session owning this file.
    pub session_id: String,
    /// Display basename, never used as a remote filesystem path.
    pub name: String,
    /// Original byte count, bounded to the JavaScript safe-integer range.
    pub size: u64,
    /// Lowercase hexadecimal SHA-256 of the original bytes.
    pub sha256: String,
}
/// Published attachment, available only after the receiver verifies SHA-256.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileReceipt {
    #[serde(flatten)]
    /// Verified original metadata.
    pub offer: FileOffer,
    /// Opaque published reference accepted by prompt.send.
    pub r#ref: String,
}
/// Binary channel events. A bounded queue overflow closes the file lane.
pub enum FileEvent {
    /// The independent file DataChannel opened.
    Open,
    /// One bounded binary DataChannel message.
    Message(Vec<u8>),
}
/// Native adapter's file-channel sending half.
#[async_trait]
pub trait FileSender: Send + Sync {
    /// Negotiated message size, additionally bounded by the protocol maximum.
    fn max_frame_bytes(&self) -> usize {
        FRAME_BYTES
    }
    /// Send a frame under SCTP backpressure.
    async fn send(&self, bytes: Vec<u8>) -> Result<(), String>;
    /// Close only this binary file channel.
    fn close(&self);
}
/// One independent ordered reliable DataChannel.
pub struct FilePeer {
    /// Sending surface, independent of control traffic.
    pub sender: Arc<dyn FileSender>,
    /// Bounded event receiver, consumed exactly once.
    pub events: mpsc::Receiver<FileEvent>,
}
struct Ack {
    committed: watch::Sender<u64>,
    drained: watch::Sender<u64>,
    sent: AtomicU64,
}
/// One live connection's binary lane. Ingress is drained independently of sends.
pub struct FileLane {
    sender: Arc<dyn FileSender>,
    open: AtomicBool,
    closed: CancellationToken,
    acks: Mutex<HashMap<String, Arc<Ack>>>,
    downloads: Mutex<HashMap<String, Arc<Mutex<Download>>>>,
    registrations: Mutex<HashMap<String, bool>>,
}
impl FileLane {
    /// Start draining an independent file peer.
    pub fn start(mut peer: FilePeer) -> Arc<Self> {
        let lane = Arc::new(Self {
            sender: peer.sender,
            open: AtomicBool::new(false),
            closed: CancellationToken::new(),
            acks: Mutex::new(HashMap::new()),
            downloads: Mutex::new(HashMap::new()),
            registrations: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&lane);
        tokio::spawn(async move {
            while let Some(event) = peer.events.recv().await {
                let Some(lane) = weak.upgrade() else {
                    break;
                };
                if lane.closed.is_cancelled() {
                    break;
                }
                match event {
                    FileEvent::Open => {
                        lane.open.store(true, Ordering::Release);
                    }
                    FileEvent::Message(bytes) => {
                        if lane.receive(bytes).await.is_err() {
                            lane.close();
                            break;
                        }
                    }
                }
            }
            if let Some(lane) = weak.upgrade() {
                lane.close();
            }
        });
        lane
    }
    fn check(&self) -> Result<(), String> {
        if self.closed.is_cancelled() || !self.open.load(Ordering::Acquire) {
            Err("file channel is not open".into())
        } else {
            Ok(())
        }
    }
    fn reserve(self: &Arc<Self>, id: &str, download: bool) -> Result<Registration, String> {
        let mut entries = self.registrations.lock().expect("registrations");
        if entries.contains_key(id) || entries.values().filter(|&&v| v == download).count() >= 4 {
            return Err("file transfer busy".into());
        }
        entries.insert(id.to_owned(), download);
        Ok(Registration {
            lane: self.clone(),
            id: id.to_owned(),
            download,
        })
    }
    /// Close this lane and wake pending senders.
    pub fn close(&self) {
        self.closed.cancel();
        self.sender.close();
    }
    async fn receive(&self, bytes: Vec<u8>) -> Result<(), String> {
        let (kind, id, offset) = decode(&bytes)?;
        if kind == 4 {
            let ack = self
                .acks
                .lock()
                .expect("acks")
                .get(&id)
                .cloned()
                .ok_or("unknown file flush acknowledgement")?;
            if offset != ack.sent.load(Ordering::Acquire) {
                return Err("invalid file flush acknowledgement".into());
            }
            ack.drained.send_replace(offset);
        } else if kind == 1 {
            let ack = self
                .acks
                .lock()
                .expect("acks")
                .get(&id)
                .cloned()
                .ok_or("unknown file acknowledgement")?;
            if offset < *ack.committed.borrow() || offset > ack.sent.load(Ordering::Acquire) {
                return Err("invalid acknowledgement".into());
            }
            ack.committed.send_replace(offset);
        } else {
            let download = self
                .downloads
                .lock()
                .expect("downloads")
                .get(&id)
                .cloned()
                .ok_or("file not accepted")?;
            let next = tokio::task::spawn_blocking(move || {
                let mut download = download.lock().expect("download");
                if kind == 3 {
                    if offset != download.received {
                        return Err("invalid file flush barrier".into());
                    }
                    download.checkpoint()
                } else {
                    download.write(offset, &bytes[HEADER..], kind == 2)
                }
            })
            .await
            .map_err(err)??;
            self.sender
                .send(encode(if kind == 3 { 4 } else { 1 }, &id, next, &[])?)
                .await?;
        }
        Ok(())
    }
}
struct Registration {
    lane: Arc<FileLane>,
    id: String,
    download: bool,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if self.download {
            self.lane
                .downloads
                .lock()
                .expect("downloads")
                .remove(&self.id);
        } else {
            self.lane.acks.lock().expect("acks").remove(&self.id);
        }
        self.lane
            .registrations
            .lock()
            .expect("registrations")
            .remove(&self.id);
    }
}
impl Drop for FileLane {
    fn drop(&mut self) {
        self.close();
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn valid_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl FileOffer {
    /// Reject unsafe names, IDs, hashes and sizes.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_hex(&self.transfer_id, 32)
            || !valid_hex(&self.sha256, 64)
            || self.session_id.is_empty()
            || self.session_id.len() > 128
            || self.name.is_empty()
            || self.name.len() > 255
            || self.name == "."
            || self.name == ".."
            || self
                .name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            || self.size > SAFE_INTEGER
        {
            Err("invalid file metadata".into())
        } else {
            Ok(())
        }
    }
}
/// Encode a wire frame. Offset is u64 but restricted to JS-safe integer range.
pub fn encode(kind: u8, id: &str, offset: u64, payload: &[u8]) -> Result<Vec<u8>, String> {
    if kind > 4
        || !valid_hex(id, 32)
        || offset > SAFE_INTEGER
        || payload.len() > CHUNK
        || ((kind == 0 || kind == 2) && payload.is_empty())
        || ((kind == 1 || kind == 3 || kind == 4) && !payload.is_empty())
    {
        return Err("invalid file frame".into());
    }
    let mut bytes = Vec::with_capacity(HEADER + payload.len());
    bytes.push(kind);
    for i in 0..16 {
        bytes.push(u8::from_str_radix(&id[i * 2..i * 2 + 2], 16).map_err(err)?);
    }
    bytes.extend_from_slice(&offset.to_be_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}
/// Parse and validate the fixed header without interpreting file contents.
pub fn decode(bytes: &[u8]) -> Result<(u8, String, u64), String> {
    if bytes.len() < HEADER
        || bytes.len() > FRAME_BYTES
        || bytes[0] > 4
        || ((bytes[0] == 1 || bytes[0] == 3 || bytes[0] == 4) && bytes.len() != HEADER)
        || ((bytes[0] == 0 || bytes[0] == 2) && bytes.len() == HEADER)
    {
        return Err("invalid file frame".into());
    }
    let offset = u64::from_be_bytes(bytes[17..25].try_into().map_err(err)?);
    if offset > SAFE_INTEGER {
        return Err("invalid file offset".into());
    }
    Ok((bytes[0], hex(&bytes[1..17]), offset))
}
fn hash_path(path: &Path, cancel: &CancellationToken) -> Result<(u64, String), String> {
    let mut file = File::open(path).map_err(err)?;
    let size = file.metadata().map_err(err)?.len();
    let mut hash = Context::new(&SHA256);
    let mut buffer = vec![0; 256 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let n = file.read(&mut buffer).map_err(err)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok((size, hex(hash.finish().as_ref())))
}
/// Prepare an original file off the UI thread. A deterministic id permits
/// reselecting the same file after process death and resuming its checkpoint.
pub async fn prepare(
    session: String,
    path: PathBuf,
    name: String,
    cancel: CancellationToken,
) -> Result<FileOffer, String> {
    let (size, hash) = tokio::task::spawn_blocking(move || hash_path(&path, &cancel))
        .await
        .map_err(err)??;
    let id = hex(&crate::encoding::sha256(format!("{session}\0{name}\0{hash}").as_bytes())[..16]);
    let offer = FileOffer {
        transfer_id: id,
        session_id: session,
        name,
        size,
        sha256: hash,
    };
    offer.validate()?;
    Ok(offer)
}
/// Send original bytes with at most one 256-KiB window outstanding. Progress
/// records durable receiver ACKs. Network loss preserves the remote checkpoint.
pub async fn upload(
    connection: ViewerConnection,
    offer: FileOffer,
    path: PathBuf,
    cancel: CancellationToken,
    progress: Arc<AtomicU64>,
) -> Result<FileReceipt, String> {
    offer.validate()?;
    let (lane, rpc) = connection.file_transport()?;
    lane.check()?;
    let (tx, mut rx) = watch::channel(0);
    let ack = Arc::new(Ack {
        committed: tx,
        drained: watch::channel(0).0,
        sent: AtomicU64::new(0),
    });
    let _registration = lane.reserve(&offer.transfer_id, false)?;
    lane.acks
        .lock()
        .expect("acks")
        .insert(offer.transfer_id.clone(), ack.clone());
    let chunk_bytes = lane
        .sender
        .max_frame_bytes()
        .min(FRAME_BYTES)
        .checked_sub(HEADER)
        .filter(|n| *n > 0)
        .ok_or("negotiated message size too small")?;
    let result = async {
        let value = rpc
            .request(
                "file.offer",
                Some(serde_json::to_value(&offer).map_err(err)?),
                None,
            )
            .await
            .map_err(err)?;
        lane.check()?;
        let offset = value
            .get("offset")
            .and_then(|v| v.as_u64())
            .filter(|o| *o <= offer.size)
            .ok_or("invalid checkpoint")?;
        ack.sent.store(offset, Ordering::Release);
        ack.committed.send_replace(offset);
        progress.store(offset, Ordering::Release);
        let mut file = tokio::fs::File::open(&path).await.map_err(err)?;
        if file.metadata().await.map_err(err)?.len() != offer.size {
            return Err("file size changed".into());
        }
        file.seek(SeekFrom::Start(offset)).await.map_err(err)?;
        let mut sent = offset;
        while sent < offer.size {
            let end = (sent + chunk_bytes as u64 * 16).min(offer.size);
            while sent < end {
                if cancel.is_cancelled() {
                    return Err("cancelled".into());
                }
                lane.check()?;
                let mut bytes = vec![0; ((end - sent) as usize).min(chunk_bytes)];
                file.read_exact(&mut bytes).await.map_err(err)?;
                let kind = if sent + bytes.len() as u64 == end {
                    2
                } else {
                    0
                };
                let frame = encode(kind, &offer.transfer_id, sent, &bytes)?;
                sent += bytes.len() as u64;
                ack.sent.store(sent, Ordering::Release);
                // Do not drop a partially-completed send on user cancellation:
                // the barrier must count exactly the frames handed to SCTP.
                let sending = tokio::select! {
                    _ = lane.closed.cancelled() => Err("file channel closed".into()),
                    result = tokio::time::timeout(Duration::from_secs(60), lane.sender.send(frame)) => result.map_err(err).and_then(|v| v),
                };
                if let Err(error) = sending {
                    lane.close();
                    return Err(error);
                }
            }
            tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    if *rx.borrow_and_update() >= end {
                        return Ok::<(), String>(());
                    }
                    tokio::select! {
                        _ = cancel.cancelled() => return Err("cancelled".into()),
                        _ = lane.closed.cancelled() => return Err("file channel closed".into()),
                        result = rx.changed() => { result.map_err(err)?; },
                    }
                }
            })
            .await
            .map_err(err)??;
            progress.store(end, Ordering::Release);
        }
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let raw = rpc
            .request(
                "file.commit",
                Some(json!({ "session_id": offer.session_id, "transfer_id": offer.transfer_id })),
                Some(Duration::from_secs(300)),
            )
            .await
            .map_err(err)?;
        let receipt: FileReceipt = serde_json::from_value(raw).map_err(err)?;
        if receipt.offer != offer || receipt.r#ref != offer.transfer_id {
            return Err("invalid file receipt".into());
        }
        Ok(receipt)
    }
    .await;
    // Every failed offer must release the receiver, not just explicit cancellation.
    // Keep the registration until all ordered frames and ACKs have drained.
    if result.is_err() && !lane.closed.is_cancelled() {
        let sent = ack.sent.load(Ordering::Acquire);
        if sent > *rx.borrow() && !lane.closed.is_cancelled() {
            let mut drain = ack.drained.subscribe();
            let drained = tokio::time::timeout(Duration::from_secs(60), async {
                lane.sender
                    .send(encode(3, &offer.transfer_id, sent, &[])?)
                    .await?;
                loop {
                    if *drain.borrow_and_update() == sent {
                        return Ok::<(), String>(());
                    }
                    tokio::select! {
                        _ = lane.closed.cancelled() => return Err("file channel closed".into()),
                        value = drain.changed() => { value.map_err(err)?; },
                    }
                }
            })
            .await;
            if !matches!(drained, Ok(Ok(()))) {
                lane.close();
            }
        }
        if !lane.closed.is_cancelled()
            && rpc
                .request(
                    "file.cancel",
                    Some(json!({"session_id":offer.session_id,"transfer_id":offer.transfer_id})),
                    Some(Duration::from_secs(75)),
                )
                .await
                .is_err()
        {
            lane.close();
        }
    }
    result
}
struct Download {
    offer: FileOffer,
    path: PathBuf,
    part: PathBuf,
    checkpoint: PathBuf,
    file: File,
    offset: u64,
    received: u64,
    progress: Arc<AtomicU64>,
    cancel: CancellationToken,
}
impl Download {
    fn open(
        offer: FileOffer,
        path: PathBuf,
        progress: Arc<AtomicU64>,
        cancel: CancellationToken,
    ) -> Result<Self, String> {
        offer.validate()?;
        let part = PathBuf::from(format!("{}.{}.part", path.display(), offer.transfer_id));
        let checkpoint = part.with_extension("checkpoint");
        let saved = fs::read(&checkpoint)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<(FileOffer, u64)>(&bytes).ok());
        let mut offset = match saved {
            Some((ref metadata, offset)) if *metadata == offer && offset <= offer.size => offset,
            _ => 0,
        };
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&part)
            .map_err(err)?;
        if file.metadata().map_err(err)?.len() < offset {
            // The OS may evict or truncate private cache bytes independently
            // from the checkpoint. Retry from zero instead of trapping every
            // future download behind the same unrecoverable offset.
            offset = 0;
            match fs::remove_file(&checkpoint) {
                Ok(()) => sync_parent(&checkpoint)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(err(error)),
            }
        }
        file.set_len(offset).map_err(err)?;
        progress.store(offset, Ordering::Release);
        Ok(Self {
            offer,
            path,
            part,
            checkpoint,
            file,
            offset,
            received: offset,
            progress,
            cancel,
        })
    }
    fn write(&mut self, at: u64, bytes: &[u8], flush: bool) -> Result<u64, String> {
        if at != self.received || self.received + bytes.len() as u64 > self.offer.size {
            return Err("invalid file chunk".into());
        }
        self.file.seek(SeekFrom::Start(at)).map_err(err)?;
        self.file.write_all(bytes).map_err(err)?;
        self.received = at + bytes.len() as u64;
        if !flush {
            return Ok(self.offset);
        }
        self.checkpoint()
    }
    fn checkpoint(&mut self) -> Result<u64, String> {
        self.file.sync_all().map_err(err)?;
        let next = self.received;
        let temp = self.checkpoint.with_extension("tmp");
        let mut f = File::create(&temp).map_err(err)?;
        f.write_all(&serde_json::to_vec(&(&self.offer, next)).map_err(err)?)
            .map_err(err)?;
        f.sync_all().map_err(err)?;
        fs::rename(temp, &self.checkpoint).map_err(err)?;
        sync_parent(&self.checkpoint)?;
        self.offset = next;
        self.progress.store(next, Ordering::Release);
        Ok(next)
    }
    fn discard(&mut self) -> Result<(), String> {
        self.file.set_len(0).map_err(err)?;
        self.file.sync_all().map_err(err)?;
        for path in [&self.part, &self.checkpoint] {
            if let Err(error) = fs::remove_file(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(err(error));
                }
            }
        }
        sync_parent(&self.part)
    }
    fn finish(&mut self) -> Result<(), String> {
        if self.offset != self.offer.size {
            return Err("incomplete file".into());
        }
        self.file.sync_all().map_err(err)?;
        let (_, hash) = hash_path(&self.part, &self.cancel)?;
        if hash != self.offer.sha256 {
            self.file.set_len(0).map_err(err)?;
            self.file.sync_all().map_err(err)?;
            let _ = fs::remove_file(&self.checkpoint);
            self.offset = 0;
            return Err("file checksum mismatch".into());
        }
        fs::rename(&self.part, &self.path).map_err(err)?;
        sync_parent(&self.path)?;
        let _ = fs::remove_file(&self.checkpoint);
        Ok(())
    }
}
fn sync_parent(path: &Path) -> Result<(), String> {
    File::open(path.parent().ok_or("missing parent")?)
        .and_then(|f| f.sync_all())
        .map_err(err)
}
/// Download only an authorized receipt, into an application-selected local
/// destination. Failure retains the sidecar checkpoint for a later reconnect.
pub async fn download(
    connection: ViewerConnection,
    receipt: FileReceipt,
    path: PathBuf,
    cancel: CancellationToken,
    progress: Arc<AtomicU64>,
) -> Result<FileReceipt, String> {
    let (lane, rpc) = connection.file_transport()?;
    lane.check()?;
    receipt.offer.validate()?;
    if receipt.r#ref != receipt.offer.transfer_id {
        return Err("invalid file receipt".into());
    }
    let id = receipt.offer.transfer_id.clone();
    // Reserve before opening/truncating the partial file. A duplicate operation
    // must never touch the live receiver's checkpoint or unregister its slot.
    let _registration = lane.reserve(&id, true)?;
    let offer = receipt.offer.clone();
    let token = cancel.clone();
    let download = Arc::new(Mutex::new(
        tokio::task::spawn_blocking(move || Download::open(offer, path, progress, token))
            .await
            .map_err(err)??,
    ));
    lane.downloads
        .lock()
        .expect("downloads")
        .insert(id.clone(), download.clone());
    let offset = download.lock().expect("download").offset;
    let result = tokio::select! {
        _ = cancel.cancelled() => Err("cancelled".into()),
        _ = lane.closed.cancelled() => Err("file channel closed".into()),
        result = rpc.request("file.download", Some(json!({"session_id":receipt.offer.session_id,"transfer_id":id,"offset":offset})), Some(Duration::from_secs(24*60*60))) => result.map_err(err),
    };
    if result.is_err() && !lane.closed.is_cancelled() {
        // The host drains its file lane before acknowledging cancellation.
        // If control cleanup fails, stop ingress before dropping the receiver.
        if rpc
            .request(
                "file.cancel",
                Some(json!({"session_id":receipt.offer.session_id,"transfer_id":id})),
                Some(Duration::from_secs(75)),
            )
            .await
            .is_err()
        {
            lane.close();
        }
    }
    if cancel.is_cancelled() {
        if !lane.closed.is_cancelled() {
            tokio::task::spawn_blocking(move || download.lock().expect("download").discard())
                .await
                .map_err(err)??;
        }
        return Err("cancelled".into());
    }
    result?;
    // Keep the registration through verification/publication; otherwise a retry
    // can reopen this partial file while the previous finish still owns it.
    tokio::task::spawn_blocking(move || download.lock().expect("download").finish())
        .await
        .map_err(err)??;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dropping_ffi_future_cancels_worker_without_dropping_cleanup() {
        let task = Arc::new(FileTask::default());
        task.active.store(true, Ordering::Release);
        let running = task.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (drained, drain) = tokio::sync::oneshot::channel();
        let cleaned = Arc::new(AtomicBool::new(false));
        let cleanup = cleaned.clone();
        let caller = tokio::spawn(async move {
            let cancel = running.cancel.clone();
            running
                .run(&tokio::runtime::Handle::current(), async move {
                    started.send(()).unwrap();
                    cancel.cancelled().await;
                    drain.await.unwrap();
                    cleanup.store(true, Ordering::Release);
                    Ok(())
                })
                .await
        });
        ready.await.unwrap();
        caller.abort();
        let _ = caller.await;
        assert!(task.cancel.is_cancelled());
        assert!(
            task.active.load(Ordering::Acquire),
            "slot must remain reserved while draining"
        );
        assert!(!cleaned.load(Ordering::Acquire));
        drained.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while task.active.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(cleaned.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn reservation_rejects_duplicates_in_both_directions_until_owner_exits() {
        struct Sender;
        #[async_trait]
        impl FileSender for Sender {
            async fn send(&self, _: Vec<u8>) -> Result<(), String> {
                Ok(())
            }
            fn close(&self) {}
        }
        let (_events, rx) = mpsc::channel(8);
        let lane = FileLane::start(FilePeer {
            sender: Arc::new(Sender),
            events: rx,
        });
        let first = lane.reserve("id", true).unwrap();
        assert!(lane.reserve("id", true).is_err());
        assert!(lane.reserve("id", false).is_err());
        drop(first);
        let retry = lane.reserve("id", true).unwrap();
        assert!(lane.reserve("id", true).is_err());
        drop(retry);
        assert!(lane.reserve("id", false).is_ok());
    }
    #[test]
    fn frames_match_typescript_vector_and_large_offsets() {
        let bytes = encode(0, &"ab".repeat(16), 5 * 1024 * 1024 * 1024, &[0, 255]).unwrap();
        assert_eq!(
            hex(&bytes),
            "00abababababababababababababababab000000014000000000ff"
        );
        assert_eq!(
            decode(&bytes).unwrap(),
            (0, "ab".repeat(16), 5 * 1024 * 1024 * 1024)
        );
        assert_eq!(
            decode(&encode(2, &"ab".repeat(16), 0, &[7]).unwrap())
                .unwrap()
                .0,
            2
        );
        assert!(encode(2, &"ab".repeat(16), 0, &[]).is_err());
        assert!(decode(&[0; 4]).is_err());
        assert!(encode(1, &"aa".repeat(16), 0, &[1]).is_err());
    }
    #[test]
    fn download_recovers_when_partial_bytes_are_evicted_or_truncated() {
        for evicted in [true, false] {
            let directory = std::env::temp_dir().join(format!(
                "rc-download-recovery-{}",
                crate::encoding::random_id()
            ));
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("result.bin");
            let offer = FileOffer {
                transfer_id: "ab".repeat(16),
                session_id: "session".into(),
                name: "result.bin".into(),
                size: 2,
                sha256: hex(ring::digest::digest(&ring::digest::SHA256, &[7, 8]).as_ref()),
            };
            let open = || {
                Download::open(
                    offer.clone(),
                    path.clone(),
                    Arc::new(AtomicU64::new(0)),
                    CancellationToken::new(),
                )
            };
            let mut first = open().unwrap();
            first.write(0, &[7], true).unwrap();
            let partial = first.part.clone();
            drop(first);
            if evicted {
                fs::remove_file(&partial).unwrap();
            } else {
                File::create(&partial).unwrap();
            }
            let mut retry = open().unwrap();
            assert_eq!(retry.offset, 0);
            retry.write(0, &[7, 8], true).unwrap();
            retry.finish().unwrap();
            assert_eq!(fs::read(&path).unwrap(), [7, 8]);
            drop(retry);
            fs::remove_dir_all(&directory).unwrap();
        }
    }
    #[tokio::test]
    async fn cancellation_barrier_has_a_distinct_ack_after_durable_flush() {
        struct Sender(Mutex<Vec<Vec<u8>>>);
        #[async_trait]
        impl FileSender for Sender {
            async fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
                self.0.lock().unwrap().push(bytes);
                Ok(())
            }
            fn close(&self) {}
        }
        let path =
            std::env::temp_dir().join(format!("rc-barrier-{}", crate::encoding::random_id()));
        let offer = FileOffer {
            transfer_id: "ab".repeat(16),
            session_id: "session".into(),
            name: "test.bin".into(),
            size: 2,
            sha256: "00".repeat(32),
        };
        let sender = Arc::new(Sender(Mutex::new(Vec::new())));
        let (tx, events) = mpsc::channel(8);
        let lane = FileLane::start(FilePeer {
            sender: sender.clone(),
            events,
        });
        let download = Arc::new(Mutex::new(
            Download::open(
                offer.clone(),
                path,
                Arc::new(AtomicU64::new(0)),
                CancellationToken::new(),
            )
            .unwrap(),
        ));
        lane.downloads
            .lock()
            .unwrap()
            .insert(offer.transfer_id.clone(), download.clone());
        tx.send(FileEvent::Open).await.unwrap();
        tx.send(FileEvent::Message(
            encode(0, &offer.transfer_id, 0, &[7, 8]).unwrap(),
        ))
        .await
        .unwrap();
        tx.send(FileEvent::Message(
            encode(3, &offer.transfer_id, 2, &[]).unwrap(),
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if sender.0.lock().unwrap().len() == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let frames = sender.0.lock().unwrap();
        assert_eq!(
            decode(&frames[0]).unwrap(),
            (1, offer.transfer_id.clone(), 0)
        );
        assert_eq!(
            decode(&frames[1]).unwrap(),
            (4, offer.transfer_id.clone(), 2)
        );
        assert_eq!(download.lock().unwrap().offset, 2);
        download.lock().unwrap().discard().unwrap();
        lane.close();
    }
    #[tokio::test]
    async fn native_staging_resumes_and_checks_original_hash() {
        let path = std::env::temp_dir().join(format!("rc-file-{}", crate::encoding::random_id()));
        fs::write(&path, [0, 255, 17]).unwrap();
        let offer = prepare(
            "session".into(),
            path.clone(),
            "file.zip".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let target = path.with_extension("out");
        let progress = Arc::new(AtomicU64::new(0));
        let mut first = Download::open(
            offer.clone(),
            target.clone(),
            progress.clone(),
            CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(first.write(0, &[0], true).unwrap(), 1);
        assert_eq!(first.write(1, &[99], false).unwrap(), 1);
        assert_eq!(first.received, 2);
        drop(first);
        let mut second =
            Download::open(offer, target.clone(), progress, CancellationToken::new()).unwrap();
        assert_eq!(second.offset, 1);
        assert_eq!(second.received, 1);
        second.write(1, &[255, 17], true).unwrap();
        second.finish().unwrap();
        assert_eq!(fs::read(&target).unwrap(), [0, 255, 17]);
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(target);
    }
}

/// Cancellable progress state retained by the platform FFI during one operation.
#[derive(Default)]
pub struct FileTask {
    /// Last durably acknowledged byte offset.
    pub progress: Arc<AtomicU64>,
    /// Cooperative cancellation of hashing or transfer.
    pub cancel: CancellationToken,
    /// Whether the platform operation is still running.
    pub active: AtomicBool,
}

impl FileTask {
    /// Run independently of the caller's FFI future. Dropping that future requests
    /// cooperative cancellation, while the worker retains its transfer slot until
    /// the ordered lane and remote receiver have been drained.
    pub async fn run<T, F>(
        self: &Arc<Self>,
        runtime: &tokio::runtime::Handle,
        work: F,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        F: std::future::Future<Output = Result<T, String>> + Send + 'static,
    {
        struct Finish(Arc<FileTask>);
        impl Drop for Finish {
            fn drop(&mut self) {
                self.0.active.store(false, Ordering::Release);
            }
        }
        let _cancel_on_drop = self.cancel.clone().drop_guard();
        let finished = Finish(self.clone());
        runtime
            .spawn(async move {
                let _finished = finished;
                work.await
            })
            .await
            .map_err(err)?
    }
}
