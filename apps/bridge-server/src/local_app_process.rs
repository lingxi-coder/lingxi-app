//! Private Windows Local App process owner. The control stream is separate from
//! payload output; every payload byte is encoded inside an output frame.
//!
//! This module is only entered by the dedicated helper process. Its outer Job
//! protects even the interval before the suspended child joins its inner Job.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use platform_windows::process::owned_job::{
    initialize_helper_job, spawn_owned, startup_group_empty_verified, OwnedProcess,
    OwnedSpawnError, StartupGate,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const VERSION: u8 = 1;
const MAX_INPUT: usize = 256 * 1024;
const MAX_OUTPUT: usize = 16 * 1024;
const MAX_ID: usize = 128;
// Node represents the sequence as a JavaScript safe integer.
const MAX_SEQUENCE: u64 = 9_007_199_254_740_991;
const CHUNK: usize = 4096;
const QUEUE: usize = 64;
const CONTROL_QUEUE: usize = 16;
const SPAWN_WAIT: Duration = Duration::from_secs(5);
// The underlying operation can wait for the leader and then the Job, each
// for up to five seconds. Preserve a single attempt across both waits.
const CLEANUP_WAIT: Duration = Duration::from_secs(12);
const DRAIN_WAIT: Duration = Duration::from_secs(5);
const SEND_WAIT: Duration = Duration::from_secs(1);
const RETRY_WAIT: Duration = Duration::from_millis(250);

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum Request {
    Start {
        version: u8,
        id: String,
        command: String,
        args: Vec<String>,
        cwd: String,
        env: BTreeMap<String, String>,
    },
    Stop {
        version: u8,
        id: String,
        #[serde(rename = "attemptId")]
        attempt_id: String,
    },
}

#[derive(Debug)]
struct Start {
    id: String,
    command: String,
    args: Vec<String>,
    cwd: String,
    env: BTreeMap<String, String>,
}

enum Control {
    Start(Start),
    Stop(String),
}

#[derive(Default)]
struct Startup {
    gate: StartupGate,
    stop_accepted: Mutex<bool>,
}

impl Startup {
    fn cancel(&self) {
        let mut stopped = self.stop_accepted.lock().unwrap_or_else(|e| e.into_inner());
        *stopped = true;
        self.gate.cancel();
    }
}

struct ReaderGuard {
    startup: Arc<Startup>,
    lost: CancellationToken,
}

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        self.startup.cancel();
        self.lost.cancel();
    }
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.contains('\0')
}

/// Attempt IDs are owner ID plus a strictly increasing decimal sequence.
/// One high-water mark prevents replay without retaining historical IDs.
fn attempt_sequence(id: &str, attempt: &str) -> Option<u64> {
    let (owner, digits) = attempt.rsplit_once(':')?;
    if owner != id
        || digits.is_empty()
        || digits.starts_with('0')
        || digits.len() > 16
        || !digits.bytes().all(|digit| digit.is_ascii_digit())
    {
        return None;
    }
    let number = digits.parse::<u64>().ok()?;
    (number <= MAX_SEQUENCE).then_some(number)
}

/// Read a complete line without allocating unbounded memory. An incomplete line
/// is a protocol failure, not a valid final request.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "partial control frame",
                ))
            };
        }
        let end = bytes.iter().position(|byte| *byte == b'\n');
        let count = end.map_or(bytes.len(), |index| index + 1);
        if line.len() + count > MAX_INPUT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control frame too large",
            ));
        }
        line.extend_from_slice(&bytes[..count]);
        reader.consume(count);
        if end.is_some() {
            line.pop();
            return Ok(Some(line));
        }
    }
}

/// Accept cancellation synchronously in this reader before forwarding it to
/// the async coordinator. The gate and ResumeThread use the same H-owned lock.
fn control_reader(
    mut reader: impl BufRead,
    sender: mpsc::Sender<Control>,
    startup: Arc<Startup>,
    lost: CancellationToken,
) {
    // An unexpected reader panic is also loss of the control channel.
    let _guard = ReaderGuard {
        startup: startup.clone(),
        lost,
    };
    let mut id: Option<String> = None;
    let mut last_sequence = 0;
    loop {
        let request = match read_line(&mut reader).and_then(|line| match line {
            Some(line) => serde_json::from_slice::<Request>(&line)
                .map(Some)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid control frame")),
            None => Ok(None),
        }) {
            Ok(Some(request)) => request,
            Ok(None) | Err(_) => break,
        };
        let control = match request {
            Request::Start {
                version,
                id: request_id,
                command,
                args,
                cwd,
                env,
            } if version == VERSION && id.is_none() && valid_id(&request_id) => {
                id = Some(request_id.clone());
                Control::Start(Start {
                    id: request_id,
                    command,
                    args,
                    cwd,
                    env,
                })
            }
            Request::Stop {
                version,
                id: request_id,
                attempt_id,
            } => {
                let sequence = if version == VERSION && id.as_deref() == Some(request_id.as_str()) {
                    attempt_sequence(&request_id, &attempt_id)
                } else {
                    None
                };
                let Some(sequence) = sequence.filter(|value| *value > last_sequence) else {
                    break;
                };
                last_sequence = sequence;
                startup.cancel();
                Control::Stop(attempt_id)
            }
            _ => break,
        };
        if sender.try_send(control).is_err() {
            break;
        }
    }
}

struct Frame {
    bytes: Vec<u8>,
    ack: Option<oneshot::Sender<io::Result<()>>>,
}

#[derive(Clone)]
struct Output {
    sender: mpsc::Sender<Frame>,
}

fn serialize_frame(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_OUTPUT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "output frame too large",
        ));
    }
    Ok(bytes)
}

/// A blocked OS write cannot occupy the runtime's blocking pool or prevent the
/// coordinator from terminating the Job. The queue bounds retained output.
fn output_writer(mut sink: impl Write + Send + 'static) -> (Output, CancellationToken) {
    let (sender, mut receiver) = mpsc::channel::<Frame>(QUEUE);
    let failed = CancellationToken::new();
    let failure = failed.clone();
    std::thread::spawn(move || {
        // Detect both I/O errors and unexpected termination of this thread.
        let _guard = CancelOnDrop(failure);
        while let Some(frame) = receiver.blocking_recv() {
            let result = sink.write_all(&frame.bytes).and_then(|()| sink.flush());
            let is_error = result.is_err();
            if let Some(ack) = frame.ack {
                let _ = ack.send(result);
            }
            if is_error {
                return;
            }
        }
    });
    (Output { sender }, failed)
}

impl Output {
    /// Enqueue started atomically with respect to acceptance of stop/EOF.
    /// A frame already queued may be observed later on the independent pipe.
    async fn started(&self, startup: &Startup, id: &str) -> io::Result<bool> {
        let bytes = serialize_frame(&frame(id, "started"))?;
        let permit = tokio::time::timeout(SEND_WAIT, self.sender.reserve())
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "output queue blocked"))?
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "output channel closed"))?;
        let stopped = startup
            .stop_accepted
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *stopped {
            return Ok(false);
        }
        permit.send(Frame { bytes, ack: None });
        Ok(true)
    }

    async fn send(&self, value: Value) -> io::Result<oneshot::Receiver<io::Result<()>>> {
        let (ack, result) = oneshot::channel();
        let frame = Frame {
            bytes: serialize_frame(&value)?,
            ack: Some(ack),
        };
        tokio::time::timeout(SEND_WAIT, self.sender.send(frame))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "output queue blocked"))?
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "output channel closed"))?;
        Ok(result)
    }

    async fn send_confirmed(&self, value: Value) -> io::Result<()> {
        let ack = self.send(value).await?;
        tokio::time::timeout(SEND_WAIT, ack)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "output delivery timed out"))?
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "output channel closed"))?
    }

    async fn output(&self, id: &str, stream: &'static str, bytes: &[u8]) -> io::Result<()> {
        let frame = Frame {
            bytes: serialize_frame(&json!({
                "version": VERSION, "type": "output", "id": id,
                "stream": stream, "data": STANDARD.encode(bytes),
            }))?,
            ack: None,
        };
        self.sender
            .send(frame)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "output channel closed"))
    }
}

fn frame(id: &str, kind: &str) -> Value {
    json!({ "version": VERSION, "type": kind, "id": id })
}

async fn pump(
    mut input: impl AsyncRead + Unpin,
    output: Output,
    id: String,
    stream: &'static str,
    relay: bool,
) -> io::Result<()> {
    let mut bytes = [0u8; CHUNK];
    loop {
        let n = input.read(&mut bytes).await?;
        if n == 0 {
            return Ok(());
        }
        if relay {
            output.output(&id, stream, &bytes[..n]).await?;
        }
    }
}

struct Pump {
    task: JoinHandle<io::Result<()>>,
    result: Option<bool>,
}

impl Pump {
    async fn wait(&mut self) -> bool {
        if let Some(result) = self.result {
            return result;
        }
        let result = matches!((&mut self.task).await, Ok(Ok(())));
        self.result = Some(result);
        result
    }
}

fn command(start: &Start) -> Result<tokio::process::Command, &'static str> {
    if start.command.contains('\0')
        || !Path::new(&start.command).is_absolute()
        || start.cwd.contains('\0')
        || !Path::new(&start.cwd).is_absolute()
        || start.args.iter().any(|arg| arg.contains('\0'))
    {
        return Err("invalid command, arguments or working directory");
    }
    let mut names = HashSet::new();
    for (key, value) in &start.env {
        if key.is_empty()
            || (key.contains('=') || key.contains('\0'))
            || value.contains('\0')
            || !names.insert(key.to_ascii_uppercase())
        {
            return Err("invalid environment");
        }
    }
    let mut command = tokio::process::Command::new(&start.command);
    command
        .args(&start.args)
        .current_dir(&start.cwd)
        .env_clear()
        .envs(&start.env);
    Ok(command)
}

async fn start_error(
    output: &Output,
    id: &str,
    message: &'static str,
    no_child: bool,
) -> io::Result<()> {
    output
        .send_confirmed(json!({ "version": VERSION, "type": "start-error", "id": id,
        "message": message, "noChildStarted": no_child }))
        .await
}

async fn no_child(
    output: &Output,
    id: &str,
    mut control: mpsc::Receiver<Control>,
    lost: CancellationToken,
    writer_failed: CancellationToken,
    mut pending: VecDeque<String>,
) -> io::Result<()> {
    loop {
        if lost.is_cancelled() {
            while let Ok(message) = control.try_recv() {
                if let Control::Stop(attempt) = message {
                    pending.push_back(attempt);
                }
            }
        }
        if !pending.is_empty() {
            // A half-closed parent may still read replies. Bound delivery of
            // the whole accepted batch, not each item independently.
            let deadline = tokio::time::Instant::now() + SEND_WAIT;
            let mut last_ack = None;
            while let Some(attempt) = pending.pop_front() {
                let event = json!({ "version": VERSION, "type": "cleanup-result", "id": id,
                    "attemptId": attempt, "verified": true });
                last_ack = Some(
                    tokio::time::timeout_at(deadline, output.send(event))
                        .await
                        .map_err(|_| {
                            io::Error::new(io::ErrorKind::TimedOut, "cleanup receipt timed out")
                        })??,
                );
            }
            if let Some(ack) = last_ack {
                tokio::time::timeout_at(deadline, ack)
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "cleanup receipt timed out")
                    })?
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "output channel closed")
                    })??;
            }
        }
        if writer_failed.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "output channel closed",
            ));
        }
        if lost.is_cancelled() {
            return Ok(());
        }
        tokio::select! {
            biased;
            _ = lost.cancelled() => {},
            _ = writer_failed.cancelled() => {},
            request = control.recv() => match request {
                Some(Control::Stop(attempt)) => pending.push_back(attempt),
                Some(Control::Start(_)) | None => return Ok(()),
            },
        }
    }
}

trait CleanupOwner {
    async fn cleanup(&mut self) -> Option<ExitStatus>;
}

impl CleanupOwner for OwnedProcess {
    async fn cleanup(&mut self) -> Option<ExitStatus> {
        self.terminate_and_reap().await.ok()
    }
}

async fn cleanup_attempt(owner: &mut impl CleanupOwner) -> Option<ExitStatus> {
    tokio::time::timeout(CLEANUP_WAIT, owner.cleanup())
        .await
        .ok()
        .flatten()
}

async fn retry_closed_owner(owner: &mut impl CleanupOwner) -> ExitStatus {
    loop {
        tokio::time::sleep(RETRY_WAIT).await;
        if let Some(status) = cleanup_attempt(owner).await {
            return status;
        }
    }
}

/// Preserve the outer Job when Tokio failed without returning a child handle.
/// A suspended orphan can remain there indefinitely; without a held process
/// handle we cannot safely manufacture termination or a leader exit status.
async fn supervise_unknown_start(
    output: Output,
    id: String,
    mut control: mpsc::Receiver<Control>,
    lost: CancellationToken,
    writer_failed: CancellationToken,
    mut pending: VecDeque<Option<String>>,
    mut group_empty: impl FnMut() -> bool,
    initially_verified: bool,
    mut never_resumed: impl FnMut() -> bool,
) -> anyhow::Result<()> {
    let announced = start_error(&output, &id, "process startup is unverified", false)
        .await
        .is_ok();
    let mut closing = !announced || lost.is_cancelled() || writer_failed.is_cancelled();
    loop {
        if initially_verified || group_empty() {
            if announced && never_resumed() {
                output
                    .send_confirmed(json!({ "version": VERSION, "type": "startup-cleanup",
                    "id": id, "verified": true }))
                    .await?;
                return Ok(());
            }
            return Err(anyhow::anyhow!(
                "Local App startup delivery remains unverified"
            ));
        }
        if lost.is_cancelled() || writer_failed.is_cancelled() {
            closing = true;
        }
        if !closing && !pending.is_empty() {
            let deadline = tokio::time::Instant::now() + SEND_WAIT;
            while let Some(attempt) = pending.pop_front() {
                let event = json!({ "version": VERSION, "type": "cleanup-result", "id": id,
                    "attemptId": attempt, "verified": false, "error": "process startup is unverified" });
                if !matches!(
                    tokio::time::timeout_at(deadline, output.send(event)).await,
                    Ok(Ok(_))
                ) {
                    closing = true;
                    break;
                }
            }
        }
        tokio::select! {
            biased;
            _ = lost.cancelled(), if !closing => { closing = true; }
            _ = writer_failed.cancelled(), if !closing => { closing = true; }
            request = control.recv(), if !closing => match request {
                Some(Control::Stop(attempt)) => pending.push_back(Some(attempt)),
                Some(Control::Start(_)) | None => { closing = true; }
            },
            _ = tokio::time::sleep(RETRY_WAIT) => {}
        }
    }
}

async fn supervise_lost_spawn_sender(
    output: Output,
    id: String,
    control: mpsc::Receiver<Control>,
    lost: CancellationToken,
    writer_failed: CancellationToken,
    pending: VecDeque<Option<String>>,
    group_empty: impl FnMut() -> bool,
    never_resumed: impl FnMut() -> bool,
) -> anyhow::Result<()> {
    supervise_unknown_start(
        output,
        id,
        control,
        lost,
        writer_failed,
        pending,
        group_empty,
        false,
        never_resumed,
    )
    .await
}

/// A closed oneshot is observed before all spawn-thread destructors must have
/// run. Wait until that thread is actually finished before checking the Job.
fn after_spawn_thread(
    worker: std::thread::JoinHandle<()>,
    mut group_empty: impl FnMut() -> bool,
) -> impl FnMut() -> bool {
    let mut worker = Some(worker);
    move || {
        if worker.as_ref().is_some_and(|thread| !thread.is_finished()) {
            return false;
        }
        if let Some(thread) = worker.take() {
            let _ = thread.join();
        }
        group_empty()
    }
}

/// Own one child tree until the parent abandons its control stream. Any
/// unverified cleanup retains the same process and Job for a later stop.
async fn supervise(
    mut owner: OwnedProcess,
    id: String,
    output: Output,
    writer_failed: CancellationToken,
    mut control: mpsc::Receiver<Control>,
    lost: CancellationToken,
    mut pending: VecDeque<Option<String>>,
    started: bool,
) -> anyhow::Result<()> {
    let stdout = owner.take_stdout();
    let stderr = owner.take_stderr();
    let (mut out_pump, mut err_pump) = match (stdout, stderr) {
        (Some(stdout), Some(stderr)) => (
            Some(Pump {
                task: tokio::spawn(pump(stdout, output.clone(), id.clone(), "stdout", started)),
                result: None,
            }),
            Some(Pump {
                task: tokio::spawn(pump(stderr, output.clone(), id.clone(), "stderr", started)),
                result: None,
            }),
        ),
        _ => (None, None),
    };
    if !started {
        pending.push_back(None);
    }
    let mut leader: Option<ExitStatus> = None;
    let mut leader_announced = false;
    let mut job_stopped = false;
    let mut automatic_attempted = !started;
    let mut closing = lost.is_cancelled() || writer_failed.is_cancelled();
    if closing && pending.is_empty() {
        pending.push_back(None);
    }
    loop {
        if lost.is_cancelled() || writer_failed.is_cancelled() {
            closing = true;
            // The reader accepts stop before reporting EOF; account for all
            // requests already admitted to the bounded control channel.
            while let Ok(message) = control.try_recv() {
                if let Control::Stop(attempt) = message {
                    pending.push_back(Some(attempt));
                }
            }
            if pending.is_empty() {
                pending.push_back(None);
            }
        }
        if !pending.is_empty() {
            if !job_stopped {
                if let Some(status) = cleanup_attempt(&mut owner).await {
                    leader = Some(status);
                    job_stopped = true;
                }
            }
            if let Some(status) = leader.as_ref().filter(|_| !leader_announced) {
                let event = json!({ "version": VERSION, "type": "leader-exit", "id": id,
                    "code": status.code(), "signal": Value::Null });
                leader_announced = output.send(event).await.is_ok();
            }
            let drained = if job_stopped {
                match (&mut out_pump, &mut err_pump) {
                    (Some(stdout), Some(stderr)) => tokio::time::timeout(DRAIN_WAIT, async {
                        let (out, err) = tokio::join!(stdout.wait(), stderr.wait());
                        out && err
                    })
                    .await
                    .unwrap_or(false),
                    _ => false,
                }
            } else {
                false
            };
            let verified =
                job_stopped && drained && leader_announced && !writer_failed.is_cancelled();
            // All attempts share one send deadline; a slow or closed reader
            // cannot multiply shutdown time by the number of queued stops.
            let deadline = tokio::time::Instant::now() + SEND_WAIT;
            let mut last_ack = None;
            let mut all_queued = true;
            while let Some(attempt) = pending.pop_front() {
                let mut result = json!({ "version": VERSION, "type": "cleanup-result", "id": id,
                    "attemptId": attempt, "verified": verified });
                if !verified {
                    result["error"] = json!("process cleanup or output delivery is unverified");
                }
                match tokio::time::timeout_at(deadline, output.send(result)).await {
                    Ok(Ok(ack)) => last_ack = Some(ack),
                    _ => {
                        all_queued = false;
                        break;
                    }
                }
            }
            closing |= lost.is_cancelled() || writer_failed.is_cancelled();
            if closing {
                if !job_stopped {
                    // Keep the very same Job and child while their state is
                    // unknown. EOF cannot be allowed to discard ownership on
                    // the first failed verification attempt.
                    leader = Some(retry_closed_owner(&mut owner).await);
                    job_stopped = true;
                    pending.push_back(None);
                    continue;
                }
                // A parent that half-closes stdin may still read the final
                // receipt; do not let a blocked writer hold the helper open.
                let delivered = if let Some(ack) = last_ack {
                    matches!(tokio::time::timeout_at(deadline, ack).await, Ok(Ok(Ok(()))))
                } else {
                    false
                };
                return if verified && all_queued && delivered {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("Local App output delivery is unverified"))
                };
            }
        }
        tokio::select! {
            biased;
            _ = lost.cancelled() => { closing = true; pending.push_back(None); }
            _ = writer_failed.cancelled() => { closing = true; pending.push_back(None); }
            request = control.recv() => match request {
                Some(Control::Stop(attempt)) => pending.push_back(Some(attempt)),
                Some(Control::Start(_)) | None => { closing = true; pending.push_back(None); }
            },
            result = owner.wait_leader(), if leader.is_none() && !automatic_attempted => {
                automatic_attempted = true;
                if let Ok(status) = result { leader = Some(status); }
                pending.push_back(None);
            }
        }
    }
}

pub async fn run() -> anyhow::Result<()> {
    // The platform initializer clears inherited control-handle flags before
    // any thread can use those handles, and establishes the outer Job.
    let initialized = initialize_helper_job().is_ok();
    let startup = Arc::new(Startup::default());
    let (control_tx, mut control_rx) = mpsc::channel(CONTROL_QUEUE);
    let lost = CancellationToken::new();
    let reader_startup = startup.clone();
    let reader_lost = lost.clone();
    std::thread::spawn(move || {
        control_reader(
            io::BufReader::new(io::stdin()),
            control_tx,
            reader_startup,
            reader_lost,
        )
    });
    let (output, writer_failed) = output_writer(io::stdout());

    let start = tokio::select! {
        biased;
        request = control_rx.recv() => match request { Some(Control::Start(start)) => start, _ => return Err(anyhow::anyhow!("invalid Local App control stream")) },
        _ = lost.cancelled() => return Err(anyhow::anyhow!("Local App control stream closed")),
        _ = writer_failed.cancelled() => return Err(anyhow::anyhow!("Local App control output closed")),
    };
    let cmd = match command(&start) {
        Ok(cmd) => cmd,
        Err(error) => {
            start_error(&output, &start.id, error, true).await?;
            no_child(
                &output,
                &start.id,
                control_rx,
                lost,
                writer_failed,
                VecDeque::new(),
            )
            .await?;
            return Ok(());
        }
    };
    if !initialized {
        start_error(
            &output,
            &start.id,
            "unable to initialize process ownership",
            true,
        )
        .await?;
        no_child(
            &output,
            &start.id,
            control_rx,
            lost,
            writer_failed,
            VecDeque::new(),
        )
        .await?;
        return Ok(());
    }

    // A plain thread avoids leaving a blocked Tokio blocking-pool job behind
    // if process creation itself stalls. Enter the runtime for Tokio's child.
    let (spawn_tx, mut spawn_rx) = oneshot::channel::<Result<OwnedProcess, OwnedSpawnError>>();
    let spawn_gate = startup.gate.clone();
    let runtime = tokio::runtime::Handle::current();
    let spawn_thread = std::thread::spawn(move || {
        let _entered = runtime.enter();
        let _ = spawn_tx.send(spawn_owned(cmd, &spawn_gate));
    });
    let mut pending = VecDeque::new();
    let mut spawn_deadline: Option<tokio::time::Instant> = None;
    let mut closing = false;
    let outcome = loop {
        let wait_until = spawn_deadline;
        tokio::select! {
            biased;
            result = &mut spawn_rx => break result.map_err(|_| ()),
            _ = lost.cancelled(), if !closing => {
                startup.cancel();
                closing = true;
                spawn_deadline = Some(tokio::time::Instant::now() + SPAWN_WAIT);
            }
            _ = writer_failed.cancelled(), if !closing => {
                startup.cancel(); lost.cancel();
                closing = true;
                spawn_deadline = Some(tokio::time::Instant::now() + SPAWN_WAIT);
            }
            request = control_rx.recv(), if !closing => match request {
                Some(Control::Stop(attempt)) => {
                    pending.push_back(Some(attempt));
                    spawn_deadline.get_or_insert_with(|| tokio::time::Instant::now() + SPAWN_WAIT);
                }
                _ => {
                    startup.cancel(); lost.cancel(); closing = true;
                    spawn_deadline = Some(tokio::time::Instant::now() + SPAWN_WAIT);
                }
            },
            _ = async move {
                match wait_until {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            }, if wait_until.is_some() => {
                // The spawn thread may already hold a child. Report the
                // attempt as unknown while retaining the outer Job and
                // waiting for the actual owner to be returned.
                let deadline = tokio::time::Instant::now() + SEND_WAIT;
                while let Some(attempt) = pending.pop_front() {
                    let result = json!({ "version": VERSION, "type": "cleanup-result", "id": start.id,
                        "attemptId": attempt, "verified": false, "error": "process startup is unverified" });
                    if !matches!(tokio::time::timeout_at(deadline, output.send(result)).await, Ok(Ok(_))) {
                        break;
                    }
                }
                spawn_deadline = closing.then(|| tokio::time::Instant::now() + SPAWN_WAIT);
            }
        }
    };
    let outcome = match outcome {
        Ok(value) => value,
        // Sender loss means the only spawn thread has unwound and cannot
        // create another child. Its Job is still needed until group proof.
        Err(_) => {
            return supervise_lost_spawn_sender(
                output,
                start.id,
                control_rx,
                lost,
                writer_failed,
                pending,
                after_spawn_thread(spawn_thread, || {
                    startup_group_empty_verified().unwrap_or(false)
                }),
                || !startup.gate.may_have_resumed(),
            )
            .await;
        }
    };
    match outcome {
        Ok(owner) => {
            // No payload output task starts before the started frame is queued.
            let started = match output.started(&startup, &start.id).await {
                Ok(true) => true,
                Ok(false) => {
                    if start_error(&output, &start.id, "process startup was cancelled", false)
                        .await
                        .is_err()
                    {
                        lost.cancel();
                    }
                    false
                }
                Err(_) => {
                    startup.cancel();
                    lost.cancel();
                    false
                }
            };
            supervise(
                owner,
                start.id,
                output,
                writer_failed,
                control_rx,
                lost,
                pending,
                started,
            )
            .await
        }
        Err(error) => match (error.owner, error.may_have_spawned_child) {
            (Some(owner), _) => {
                if start_error(&output, &start.id, "owned process did not start", false)
                    .await
                    .is_err()
                {
                    lost.cancel();
                }
                supervise(
                    owner,
                    start.id,
                    output,
                    writer_failed,
                    control_rx,
                    lost,
                    pending,
                    false,
                )
                .await
            }
            (None, true) => {
                supervise_unknown_start(
                    output,
                    start.id,
                    control_rx,
                    lost,
                    writer_failed,
                    pending,
                    || startup_group_empty_verified().unwrap_or(false),
                    error.spawn_group_empty_verified,
                    || true,
                )
                .await
            }
            (None, false) => {
                start_error(&output, &start.id, "process was not started", true).await?;
                no_child(
                    &output,
                    &start.id,
                    control_rx,
                    lost,
                    writer_failed,
                    pending.into_iter().flatten().collect(),
                )
                .await?;
                Ok(())
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn accepted_stop_prevents_new_started_frame() {
        let startup = Startup::default();
        let (sender, mut receiver) = mpsc::channel(1);
        let output = Output { sender };
        startup.cancel();
        assert!(!output.started(&startup, "owner").await.unwrap());
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let second = Startup::default();
        assert!(output.started(&second, "owner").await.unwrap());
        second.cancel();
        let sent = receiver.recv().await.unwrap();
        let value: Value = serde_json::from_slice(&sent.bytes).unwrap();
        assert_eq!(value["type"], "started");
    }

    #[tokio::test]
    async fn output_thread_panic_reports_transport_loss() {
        struct PanickingWriter;
        impl Write for PanickingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                panic!("controlled writer failure");
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (output, failed) = output_writer(PanickingWriter);
        let ack = output.send(frame("owner", "started")).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), failed.cancelled())
            .await
            .expect("writer loss was not reported");
        assert!(ack.await.is_err());
    }

    #[tokio::test]
    async fn panicked_spawn_sender_survives_eof_until_group_proof() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        struct HoldUnwind(std::sync::mpsc::Receiver<()>);
        impl Drop for HoldUnwind {
            fn drop(&mut self) {
                let _ = self.0.recv();
            }
        }
        let (spawn_sender, spawn_receiver) = oneshot::channel::<()>();
        let (release, barrier) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _hold_unwind = HoldUnwind(barrier);
            let _sender = spawn_sender;
            panic!("controlled spawn worker failure");
        });
        assert!(spawn_receiver.await.is_err());
        let (sender, mut receiver) = mpsc::channel::<Frame>(8);
        let output = Output { sender };
        let collector = tokio::spawn(async move {
            let mut events = Vec::new();
            while let Some(frame) = receiver.recv().await {
                events.push(serde_json::from_slice::<Value>(&frame.bytes).unwrap());
                if let Some(ack) = frame.ack {
                    let _ = ack.send(Ok(()));
                }
            }
            events
        });
        let (control_sender, control) = mpsc::channel(4);
        let lost = CancellationToken::new();
        lost.cancel();
        let group_empty = Arc::new(AtomicBool::new(false));
        let probes = Arc::new(AtomicUsize::new(0));
        let proof = group_empty.clone();
        let count = probes.clone();
        let task = tokio::spawn(supervise_lost_spawn_sender(
            output,
            "owner".into(),
            control,
            lost,
            CancellationToken::new(),
            VecDeque::new(),
            after_spawn_thread(worker, move || {
                count.fetch_add(1, Ordering::SeqCst);
                proof.load(Ordering::SeqCst)
            }),
            || true,
        ));
        tokio::time::sleep(RETRY_WAIT * 3).await;
        assert!(
            !task.is_finished(),
            "an unwinding spawn worker is not a proof"
        );
        assert_eq!(probes.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while probes.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("group was not rechecked");
        assert!(
            !task.is_finished(),
            "EOF must not discard an unverified Job"
        );
        group_empty.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("group proof not rechecked")
            .expect("supervisor task panicked")
            .expect("group proof not delivered");
        drop(control_sender);
        let events = collector.await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "start-error");
        assert_eq!(events[0]["noChildStarted"], false);
        assert_eq!(events[1]["type"], "startup-cleanup");
        assert_eq!(events[1]["verified"], true);
    }

    #[tokio::test]
    async fn lost_spawn_sender_cannot_prove_never_resumed() {
        let (sender, mut receiver) = mpsc::channel::<Frame>(8);
        let collector = tokio::spawn(async move {
            let mut events = Vec::new();
            while let Some(frame) = receiver.recv().await {
                events.push(serde_json::from_slice::<Value>(&frame.bytes).unwrap());
                if let Some(ack) = frame.ack {
                    let _ = ack.send(Ok(()));
                }
            }
            events
        });
        let (_control_sender, control) = mpsc::channel(1);
        let result = supervise_lost_spawn_sender(
            Output { sender },
            "owner".into(),
            control,
            CancellationToken::new(),
            CancellationToken::new(),
            VecDeque::new(),
            || true,
            || false,
        )
        .await;
        assert!(result.is_err());
        let events = collector.await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "start-error");
        assert_eq!(events[0]["noChildStarted"], false);
    }

    #[tokio::test]
    async fn unknown_startup_survives_writer_failure_until_group_proof() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let (sender, receiver) = mpsc::channel::<Frame>(1);
        drop(receiver);
        let (_control_sender, control) = mpsc::channel(1);
        let group_empty = Arc::new(AtomicBool::new(false));
        let probes = Arc::new(AtomicUsize::new(0));
        let proof = group_empty.clone();
        let count = probes.clone();
        let task = tokio::spawn(supervise_unknown_start(
            Output { sender },
            "owner".into(),
            control,
            CancellationToken::new(),
            CancellationToken::new(),
            VecDeque::new(),
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                proof.load(Ordering::SeqCst)
            },
            false,
            || true,
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            while probes.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("group was not rechecked");
        assert!(!task.is_finished(), "writer loss must retain the outer Job");
        group_empty.store(true, Ordering::SeqCst);
        let result = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("group proof not rechecked")
            .expect("supervisor task panicked");
        assert!(
            result.is_err(),
            "undeliverable proof cannot be reported as success"
        );
    }

    #[tokio::test]
    async fn closed_control_keeps_the_same_owner_until_cleanup_is_verified() {
        use std::os::windows::process::ExitStatusExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct ControlledOwner {
            attempts: Arc<AtomicUsize>,
            drops: Arc<AtomicUsize>,
        }
        impl CleanupOwner for ControlledOwner {
            async fn cleanup(&mut self) -> Option<ExitStatus> {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
                (attempt >= 2).then(|| ExitStatus::from_raw(0))
            }
        }
        impl Drop for ControlledOwner {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }
        let attempts = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let mut owner = ControlledOwner {
            attempts: attempts.clone(),
            drops: drops.clone(),
        };
        assert!(cleanup_attempt(&mut owner).await.is_none());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(retry_closed_owner(&mut owner).await.success());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(owner);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn repeated_unverified_cleanup_keeps_owner_past_128_attempts() {
        use std::os::windows::process::ExitStatusExt;

        struct ControlledOwner {
            attempts: usize,
        }
        impl CleanupOwner for ControlledOwner {
            async fn cleanup(&mut self) -> Option<ExitStatus> {
                self.attempts += 1;
                (self.attempts == 130).then(|| ExitStatus::from_raw(0))
            }
        }
        let mut owner = ControlledOwner { attempts: 0 };
        for _ in 0..129 {
            assert!(cleanup_attempt(&mut owner).await.is_none());
        }
        assert!(cleanup_attempt(&mut owner).await.unwrap().success());
        assert_eq!(owner.attempts, 130);
    }

    #[test]
    fn attempt_sequence_uses_a_bounded_high_water_mark() {
        let owner = "owner:with-colon";
        assert_eq!(attempt_sequence(owner, "owner:with-colon:1"), Some(1));
        assert_eq!(
            attempt_sequence(owner, "owner:with-colon:9007199254740991"),
            Some(MAX_SEQUENCE)
        );
        for invalid in [
            "owner:with-colon:0",
            "owner:with-colon:01",
            "owner:with-colon:9007199254740992",
            "other:1",
        ] {
            assert_eq!(attempt_sequence(owner, invalid), None);
        }

        let mut input = format!(
            "{}\n",
            json!({ "version": 1, "type": "start", "id": owner,
            "command": "C:\\\\test.exe", "args": [], "cwd": "C:\\\\", "env": {} })
        );
        for sequence in 1..=130 {
            input.push_str(&format!(
                "{}\n",
                json!({ "version": 1, "type": "stop", "id": owner,
                "attemptId": format!("{owner}:{sequence}") })
            ));
        }
        // A duplicate is a replay: it closes control instead of admitting the
        // later, higher sequence. Historical IDs are never stored.
        input.push_str(&format!(
            "{}\n",
            json!({ "version": 1, "type": "stop", "id": owner,
            "attemptId": format!("{owner}:130") })
        ));
        input.push_str(&format!(
            "{}\n",
            json!({ "version": 1, "type": "stop", "id": owner,
            "attemptId": format!("{owner}:131") })
        ));
        let (sender, mut receiver) = mpsc::channel(256);
        let startup = Arc::new(Startup::default());
        let lost = CancellationToken::new();
        control_reader(Cursor::new(input), sender, startup.clone(), lost.clone());
        assert!(matches!(receiver.try_recv(), Ok(Control::Start(_))));
        for sequence in 1..=130 {
            match receiver.try_recv() {
                Ok(Control::Stop(id)) => assert_eq!(id, format!("{owner}:{sequence}")),
                _ => panic!("missing accepted stop {sequence}"),
            }
        }
        assert!(receiver.try_recv().is_err());
        assert!(lost.is_cancelled());
        assert!(startup.gate.is_cancelled());
    }

    #[test]
    fn bounded_control_rejects_partial_and_oversized_frames() {
        assert!(read_line(&mut Cursor::new(b"partial")).is_err());
        assert!(read_line(&mut Cursor::new(vec![b'x'; MAX_INPUT + 1])).is_err());
        assert_eq!(
            read_line(&mut Cursor::new(b"{}\n")).unwrap(),
            Some(b"{}".to_vec())
        );
    }

    #[test]
    fn output_encodes_business_bytes_instead_of_control_frames() {
        let id = "test-owner";
        let bytes = b"{\"type\":\"cleanup-result\",\"verified\":true}\n";
        let value = json!({ "version": VERSION, "type": "output", "id": id,
            "stream": "stdout", "data": STANDARD.encode(bytes) });
        let encoded = serialize_frame(&value).unwrap();
        assert_eq!(encoded.iter().filter(|byte| **byte == b'\n').count(), 1);
        let parsed: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(parsed["type"], "output");
        assert_eq!(
            STANDARD.decode(parsed["data"].as_str().unwrap()).unwrap(),
            bytes
        );
    }

    #[test]
    fn command_rejects_duplicate_environment_names_and_nuls() {
        let mut start = Start {
            id: "x".into(),
            command: r"C:\Windows\System32\cmd.exe".into(),
            args: Vec::new(),
            cwd: r"C:\Windows".into(),
            env: BTreeMap::new(),
        };
        start.env.insert("Path".into(), "first".into());
        start.env.insert("PATH".into(), "second".into());
        assert!(command(&start).is_err());
        start.env.remove("Path");
        start.env.insert("BAD".into(), "bad\0value".into());
        assert!(command(&start).is_err());
    }
}
