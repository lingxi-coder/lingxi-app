//! Desktop codec subprocess candidate. RTC, route authorization, signaling and
//! remote Close remain in the host. This module is still under validation.
#[path = "clock_origin.rs"]
mod clock_origin;
use lingxi_webrtc_core::ReceivedRtp;
use lingxi_webrtc_media::{
    ports::DecodedAudioPlayout,
    video::{VideoFrame, VideoLayout},
    video_ports::{NativeVideo, RemoteVideoFrame},
    AudioProcessor, EncodedAudio,
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::{self, Read, Write},
    net::{Shutdown, TcpStream as StdStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self as std_mpsc, SyncSender},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{watch, Mutex},
    time::timeout,
};

const MAX_HEADER: usize = 64 * 1024;
const MAX_PAYLOAD: usize = 20 * 1024 * 1024;
const MAX_PENDING_RELEASES: usize = 128;
const START_TIMEOUT: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const CLOCK_MARGIN: Duration = Duration::from_secs(60);
const CLOCK_SYNC_ROUNDS: usize = 5;
const CLOCK_MAX_UNCERTAINTY: Duration = Duration::from_millis(50);
const BOOTSTRAP_VERSION: &[u8; 8] = b"LXCODEC1";

#[derive(Clone, Debug)]
pub struct CodecLaunch {
    /// Absolute product-owned executable. No shell or PATH lookup is used.
    pub executable: PathBuf,
    /// Product-owned worker-mode arguments; tests may supply an explicit fixture.
    pub arguments: Vec<OsString>,
    /// Product-selected dynamic plugin directory, absent for static plugins.
    pub plugin_directory: Option<PathBuf>,
    /// Host-selected shared-core loader directory, independent of plugin mode.
    pub loader_directory: Option<PathBuf>,
    /// Host-owned temporary parent, independent of workspace TMPDIR.
    pub private_root: PathBuf,
}

/// Resolve the same configured distribution that the worker will initialize.
pub fn configured_plugin_directory() -> io::Result<Option<PathBuf>> {
    lingxi_webrtc_media::configured_codec_plugin_directory()
        .map_err(|_| failure("media plugin directory is unavailable"))
}

/// SDK library location selected by the host, if an explicit path is needed.
pub fn configured_loader_directory() -> Option<PathBuf> {
    lingxi_webrtc_media::configured_codec_loader_directory()
}

#[derive(Clone, Copy)]
struct Clock {
    origin: Instant,
}
impl Clock {
    fn parent() -> Self {
        let now = Instant::now();
        Self {
            // A machine may start this process before its monotonic clock has
            // accumulated the usual margin. It can still use times captured
            // after worker startup, so do not fail solely on that margin.
            origin: clock_origin::with_margin(now, CLOCK_MARGIN),
        }
    }
    fn child_at(received_at: Instant, parent_midpoint_elapsed: u64) -> io::Result<Self> {
        Ok(Self {
            origin: received_at
                .checked_sub(Duration::from_nanos(parent_midpoint_elapsed))
                .ok_or_else(|| failure("clock origin unavailable"))?,
        })
    }
    fn encode(&self, at: Instant) -> io::Result<u64> {
        let delta = at
            .checked_duration_since(self.origin)
            .ok_or_else(|| failure("timestamp precedes codec session"))?;
        u64::try_from(delta.as_nanos())
            .map_err(|_| failure("timestamp exceeds codec session bounds"))
    }
    fn decode(&self, ns: u64) -> io::Result<Instant> {
        self.origin
            .checked_add(Duration::from_nanos(ns))
            .ok_or_else(|| failure("timestamp exceeds codec session bounds"))
    }
}
fn failure(message: &'static str) -> io::Error {
    io::Error::other(message)
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    AudioPush { id: u64, at_ns: u64 },
    AudioPoll,
    AudioEncode { rate: u32, at_ns: u64 },
    AudioReset,
    VideoPush { id: u64, at_ns: u64 },
    VideoEnable { enabled: bool },
    VideoPoll,
}
#[derive(Serialize, Deserialize, Debug)]
struct AudioMeta {
    sequence: u64,
    at_ns: u64,
    sample_rate_hz: u32,
    channels: u16,
}
#[derive(Serialize, Deserialize, Debug)]
struct PacketMeta {
    at_ns: u64,
    size: usize,
}
#[derive(Serialize, Deserialize, Debug)]
struct VideoMeta {
    ssrc: u32,
    at_ns: u64,
    width: u32,
    height: u32,
    offsets: Vec<usize>,
    strides: Vec<i32>,
    size: usize,
    pts_ns: u64,
}
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Reply {
    Empty,
    Audio { meta: Option<AudioMeta> },
    Encoded { packets: Vec<PacketMeta> },
    Video { frames: Vec<VideoMeta> },
    Error,
}

fn read_frame<T: for<'a> Deserialize<'a>>(stream: &mut StdStream) -> io::Result<(T, Vec<u8>)> {
    let mut lens = [0u8; 8];
    stream.read_exact(&mut lens)?;
    let h = u32::from_be_bytes(lens[..4].try_into().unwrap()) as usize;
    let p = u32::from_be_bytes(lens[4..].try_into().unwrap()) as usize;
    if h == 0 || h > MAX_HEADER || p > MAX_PAYLOAD {
        return Err(failure("codec frame exceeds bounds"));
    }
    let mut header = vec![0; h];
    stream.read_exact(&mut header)?;
    let message =
        serde_json::from_slice(&header).map_err(|_| failure("codec frame header invalid"))?;
    let mut payload = vec![0; p];
    stream.read_exact(&mut payload)?;
    Ok((message, payload))
}
fn write_frame<T: Serialize>(
    stream: &mut StdStream,
    message: &T,
    payload: &[u8],
) -> io::Result<()> {
    let h = serde_json::to_vec(message).map_err(|_| failure("codec frame encoding failed"))?;
    if h.is_empty() || h.len() > MAX_HEADER || payload.len() > MAX_PAYLOAD {
        return Err(failure("codec frame exceeds bounds"));
    }
    stream.write_all(&(h.len() as u32).to_be_bytes())?;
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(&h)?;
    stream.write_all(payload)?;
    stream.flush()
}
async fn read_frame_async<T: for<'a> Deserialize<'a>>(
    stream: &mut TcpStream,
) -> io::Result<(T, Vec<u8>)> {
    let mut lens = [0u8; 8];
    stream.read_exact(&mut lens).await?;
    let h = u32::from_be_bytes(lens[..4].try_into().unwrap()) as usize;
    let p = u32::from_be_bytes(lens[4..].try_into().unwrap()) as usize;
    if h == 0 || h > MAX_HEADER || p > MAX_PAYLOAD {
        return Err(failure("codec frame exceeds bounds"));
    }
    let mut header = vec![0; h];
    stream.read_exact(&mut header).await?;
    let message =
        serde_json::from_slice(&header).map_err(|_| failure("codec frame header invalid"))?;
    let mut payload = vec![0; p];
    stream.read_exact(&mut payload).await?;
    Ok((message, payload))
}
async fn write_frame_async<T: Serialize>(
    stream: &mut TcpStream,
    message: &T,
    payload: &[u8],
) -> io::Result<()> {
    let h = serde_json::to_vec(message).map_err(|_| failure("codec frame encoding failed"))?;
    if h.is_empty() || h.len() > MAX_HEADER || payload.len() > MAX_PAYLOAD {
        return Err(failure("codec frame exceeds bounds"));
    }
    stream.write_all(&(h.len() as u32).to_be_bytes()).await?;
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    stream.write_all(&h).await?;
    stream.write_all(payload).await?;
    stream.flush().await
}

struct TicketedPacket {
    bytes: Vec<u8>,
    id: u64,
    release: SyncSender<u64>,
    fatal: Arc<AtomicBool>,
    control: StdStream,
}
impl AsRef<[u8]> for TicketedPacket {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for TicketedPacket {
    fn drop(&mut self) {
        if self.release.try_send(self.id).is_err() {
            self.fatal.store(true, Ordering::Release);
            let _ = self.control.shutdown(Shutdown::Both);
        }
    }
}
fn ticket(
    bytes: Vec<u8>,
    id: u64,
    release: &SyncSender<u64>,
    fatal: &Arc<AtomicBool>,
    control: &StdStream,
) -> io::Result<TicketedPacket> {
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return Err(failure("RTP packet exceeds bounds"));
    }
    Ok(TicketedPacket {
        bytes,
        id,
        release: release.clone(),
        fatal: fatal.clone(),
        control: control.try_clone()?,
    })
}

fn audio_lane(
    mut stream: StdStream,
    clock: Clock,
    release: SyncSender<u64>,
    fatal: Arc<AtomicBool>,
    control: StdStream,
    enabled: bool,
) {
    let mut playout = if enabled {
        match DecodedAudioPlayout::new(48_000) {
            Ok(p) => Some(p),
            Err(_) => return,
        }
    } else {
        None
    };
    let mut encoder: Option<(u32, AudioProcessor)> = None;
    while let Ok((request, payload)) = read_frame::<Request>(&mut stream) {
        if fatal.load(Ordering::Acquire) {
            break;
        }
        let response = (|| -> io::Result<(Reply, Vec<u8>)> {
            match request {
                Request::AudioPush { id, at_ns } => {
                    let at = clock.decode(at_ns)?;
                    let packet = ticket(payload, id, &release, &fatal, &control)?;
                    playout
                        .as_mut()
                        .ok_or_else(|| failure("audio unavailable"))?
                        .push_owned(packet, at)
                        .map_err(|_| failure("audio push failed"))?;
                    Ok((Reply::Empty, vec![]))
                }
                Request::AudioPoll if payload.is_empty() => {
                    let frame = playout
                        .as_mut()
                        .ok_or_else(|| failure("audio unavailable"))?
                        .try_read()
                        .map_err(|_| failure("audio poll failed"))?;
                    match frame {
                        Some(frame) => Ok((
                            Reply::Audio {
                                meta: Some(AudioMeta {
                                    sequence: frame.sequence,
                                    at_ns: clock.encode(frame.at)?,
                                    sample_rate_hz: frame.sample_rate_hz,
                                    channels: frame.channels,
                                }),
                            },
                            frame.pcm16le,
                        )),
                        None => Ok((Reply::Audio { meta: None }, vec![])),
                    }
                }
                Request::AudioEncode { rate, at_ns }
                    if payload.len().is_multiple_of(4) && !payload.is_empty() =>
                {
                    let requested_at = clock.decode(at_ns)?;
                    if encoder.as_ref().is_none_or(|(r, _)| *r != rate) {
                        encoder = Some((
                            rate,
                            AudioProcessor::new_with_capture_start(rate, rate, requested_at)
                                .map_err(|_| failure("audio encoder unavailable"))?,
                        ));
                    }
                    let samples = payload
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                        .collect::<Vec<_>>();
                    if samples.iter().any(|s| !s.is_finite()) {
                        return Err(failure("invalid audio samples"));
                    }
                    let mut encoded = Vec::new();
                    let mut packets = Vec::new();
                    let processor = &mut encoder.as_mut().unwrap().1;
                    for (index, chunk) in samples.chunks(256).enumerate() {
                        let offset_ns = (index as u128 * 256 * 1_000_000_000) / u128::from(rate);
                        let offset_ns = u64::try_from(offset_ns)
                            .map_err(|_| failure("audio sample timestamp overflow"))?;
                        let chunk_at = requested_at
                            .checked_add(Duration::from_nanos(offset_ns))
                            .ok_or_else(|| failure("audio sample timestamp overflow"))?;
                        for packet in processor
                            .capture(chunk, chunk_at)
                            .map_err(|_| failure("audio encoding failed"))?
                        {
                            if encoded
                                .len()
                                .checked_add(packet.data.len())
                                .is_none_or(|n| n > MAX_PAYLOAD)
                            {
                                return Err(failure("encoded audio exceeds bounds"));
                            }
                            packets.push(PacketMeta {
                                at_ns: clock.encode(packet.at)?,
                                size: packet.data.len(),
                            });
                            encoded.extend_from_slice(&packet.data);
                        }
                    }
                    Ok((Reply::Encoded { packets }, encoded))
                }
                Request::AudioReset if payload.is_empty() => {
                    if let Some((_, processor)) = encoder.as_mut() {
                        processor
                            .reset()
                            .map_err(|_| failure("audio reset failed"))?;
                    }
                    Ok((Reply::Empty, vec![]))
                }
                _ => Err(failure("invalid audio request")),
            }
        })();
        let (reply, bytes) = response.unwrap_or((Reply::Error, vec![]));
        if write_frame(&mut stream, &reply, &bytes).is_err() {
            break;
        }
    }
    if let Some(p) = playout.as_mut() {
        let _ = p.interrupt();
    }
}
fn video_lane(
    mut stream: StdStream,
    clock: Clock,
    release: SyncSender<u64>,
    fatal: Arc<AtomicBool>,
    control: StdStream,
    enabled: bool,
) {
    let (mut video, _, _) = NativeVideo::new();
    if video.controls(false, false, enabled).is_err() {
        return;
    }
    while let Ok((request, payload)) = read_frame::<Request>(&mut stream) {
        if fatal.load(Ordering::Acquire) {
            break;
        }
        let response = (|| -> io::Result<(Reply, Vec<u8>)> {
            match request {
                Request::VideoPush { id, at_ns } => {
                    let at = clock.decode(at_ns)?;
                    let packet = ticket(payload, id, &release, &fatal, &control)?;
                    video
                        .receive_owned(packet, at)
                        .map_err(|_| failure("video push failed"))?;
                    Ok((Reply::Empty, vec![]))
                }
                Request::VideoEnable { enabled } if payload.is_empty() => {
                    video
                        .controls(false, false, enabled)
                        .map_err(|_| failure("video control failed"))?;
                    Ok((Reply::Empty, vec![]))
                }
                Request::VideoPoll if payload.is_empty() => {
                    let mut bytes = Vec::new();
                    let mut frames = Vec::new();
                    for remote in video
                        .take_remote_frames()
                        .map_err(|_| failure("video poll failed"))?
                    {
                        let frame = remote.frame;
                        if bytes
                            .len()
                            .checked_add(frame.pixels.len())
                            .is_none_or(|n| n > MAX_PAYLOAD)
                        {
                            return Err(failure("decoded video exceeds bounds"));
                        }
                        frames.push(VideoMeta {
                            ssrc: remote.ssrc,
                            at_ns: clock.encode(remote.at)?,
                            width: frame.layout.width,
                            height: frame.layout.height,
                            offsets: frame.layout.offsets,
                            strides: frame.layout.strides,
                            size: frame.pixels.len(),
                            pts_ns: u64::try_from(frame.pts.as_nanos())
                                .map_err(|_| failure("video timestamp overflow"))?,
                        });
                        bytes.extend_from_slice(&frame.pixels);
                    }
                    Ok((Reply::Video { frames }, bytes))
                }
                _ => Err(failure("invalid video request")),
            }
        })();
        let (reply, bytes) = response.unwrap_or((Reply::Error, vec![]));
        if write_frame(&mut stream, &reply, &bytes).is_err() {
            break;
        }
    }
}

// Calibration brackets the child's sampling instant between the parent's
// request and reply. The midpoint has a measured maximum error of RTT/2;
// selecting the best of several probes avoids startup/scheduling delay bias.
fn calibrate_worker_clock(control: &mut StdStream) -> io::Result<Clock> {
    let mut best: Option<(u64, Instant, u64)> = None;
    loop {
        let mut command = [0u8; 1];
        control.read_exact(&mut command)?;
        match command[0] {
            b'P' => {
                let sampled = Instant::now();
                control.write_all(b"P")?;
                control.flush()?;
                let mut timing = [0u8; 16];
                control.read_exact(&mut timing)?;
                let midpoint = u64::from_be_bytes(timing[..8].try_into().unwrap());
                let uncertainty = u64::from_be_bytes(timing[8..].try_into().unwrap());
                if best
                    .as_ref()
                    .is_none_or(|(previous, _, _)| uncertainty < *previous)
                {
                    best = Some((uncertainty, sampled, midpoint));
                }
            }
            b'F' => {
                let (uncertainty, sampled, midpoint) =
                    best.ok_or_else(|| failure("clock calibration missing"))?;
                if Duration::from_nanos(uncertainty) > CLOCK_MAX_UNCERTAINTY {
                    return Err(failure("clock calibration uncertainty exceeded"));
                }
                return Clock::child_at(sampled, midpoint);
            }
            _ => return Err(failure("invalid clock calibration")),
        }
    }
}

async fn calibrate_parent_clock(control: &mut TcpStream, clock: Clock) -> io::Result<Duration> {
    let mut best = Duration::MAX;
    for _ in 0..CLOCK_SYNC_ROUNDS {
        let start = Instant::now();
        control.write_all(b"P").await?;
        let mut response = [0u8; 1];
        control.read_exact(&mut response).await?;
        if response[0] != b'P' {
            return Err(failure("invalid clock calibration reply"));
        }
        let elapsed = Instant::now().duration_since(start);
        let midpoint = start
            .checked_add(elapsed / 2)
            .ok_or_else(|| failure("clock midpoint overflow"))?;
        let uncertainty_ns = u64::try_from((elapsed.as_nanos() + 1) / 2)
            .map_err(|_| failure("clock uncertainty overflow"))?;
        let uncertainty = Duration::from_nanos(uncertainty_ns);
        best = best.min(uncertainty);
        control
            .write_all(&clock.encode(midpoint)?.to_be_bytes())
            .await?;
        control.write_all(&uncertainty_ns.to_be_bytes()).await?;
    }
    if best > CLOCK_MAX_UNCERTAINTY {
        return Err(failure("clock calibration uncertainty exceeded"));
    }
    control.write_all(b"F").await?;
    control.flush().await?;
    Ok(best)
}

/// Entry point used exclusively by the product's dedicated worker invocation.
/// Bootstrap credentials arrive through the inherited private stdin pipe.
pub fn run_worker() -> io::Result<()> {
    let mut bootstrap = [0u8; 48];
    io::stdin().read_exact(&mut bootstrap)?;
    let control_port = u16::from_be_bytes(bootstrap[0..2].try_into().unwrap());
    let audio_port = u16::from_be_bytes(bootstrap[2..4].try_into().unwrap());
    let video_port = u16::from_be_bytes(bootstrap[4..6].try_into().unwrap());
    let token = &bootstrap[6..38];
    if &bootstrap[38..46] != BOOTSTRAP_VERSION {
        return Err(failure("codec bootstrap version mismatch"));
    }
    let audio_enabled = bootstrap[46] == 1;
    let video_enabled = bootstrap[47] == 1;
    lingxi_webrtc_media::runtime::initialize_configured(video_enabled)
        .map_err(|_| failure("codec runtime initialization failed"))?;
    fn connect(port: u16, kind: u8, token: &[u8]) -> io::Result<StdStream> {
        let mut stream = StdStream::connect(("127.0.0.1", port))?;
        stream.set_nodelay(true)?;
        stream.write_all(token)?;
        stream.write_all(&[kind])?;
        stream.flush()?;
        Ok(stream)
    }
    let mut control = connect(control_port, b'C', token)?;
    let audio = connect(audio_port, b'A', token)?;
    let video = connect(video_port, b'V', token)?;
    let clock = calibrate_worker_clock(&mut control)?;
    let control_writer = control.try_clone()?;
    let control_audio = control.try_clone()?;
    let control_video = control.try_clone()?;
    let audio_shutdown = audio.try_clone()?;
    let video_shutdown = video.try_clone()?;
    let fatal = Arc::new(AtomicBool::new(false));
    let (release, releases) = std_mpsc::sync_channel::<u64>(MAX_PENDING_RELEASES);
    let writer = std::thread::spawn(move || {
        let mut stream = control_writer;
        while let Ok(id) = releases.recv() {
            let kind = if id == 0 { b'D' } else { b'R' };
            if stream
                .write_all(&[kind])
                .and_then(|_| stream.write_all(&id.to_be_bytes()))
                .and_then(|_| stream.flush())
                .is_err()
            {
                break;
            }
        }
    });
    let a_release = release.clone();
    let a_fatal = fatal.clone();
    let a = std::thread::spawn(move || {
        audio_lane(
            audio,
            clock,
            a_release,
            a_fatal,
            control_audio,
            audio_enabled,
        )
    });
    let v_release = release.clone();
    let v_fatal = fatal.clone();
    let v = std::thread::spawn(move || {
        video_lane(
            video,
            clock,
            v_release,
            v_fatal,
            control_video,
            video_enabled,
        )
    });
    let mut command = [0u8; 1];
    let requested_stop = control.read_exact(&mut command).is_ok() && command[0] == b'Q';
    let _ = audio_shutdown.shutdown(Shutdown::Both);
    let _ = video_shutdown.shutdown(Shutdown::Both);
    let audio_ok = a.join().is_ok();
    let video_ok = v.join().is_ok();
    if requested_stop && audio_ok && video_ok && !fatal.load(Ordering::Acquire) {
        let _ = release.send(0);
    }
    drop(release);
    let _ = writer.join();
    if requested_stop && audio_ok && video_ok && !fatal.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(failure("codec worker stopped without completion"))
    }
}

#[derive(Clone, Copy, Debug)]
struct ExitReceipt {
    success: bool,
    graceful: bool,
}
struct Shared {
    stopping: AtomicBool,
    graceful: AtomicBool,
    protocol_fault: AtomicBool,
    leases: StdMutex<HashMap<u64, ReceivedRtp>>,
    exit: watch::Sender<Option<ExitReceipt>>,
}
struct Inner {
    audio: Mutex<TcpStream>,
    video: Mutex<TcpStream>,
    control: tokio::sync::mpsc::Sender<()>,
    force: tokio::sync::mpsc::Sender<()>,
    state: Arc<Shared>,
    next: AtomicU64,
    clock: Clock,
    clock_uncertainty: Duration,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.state.stopping.store(true, Ordering::Release);
        let _ = self.control.try_send(());
        let _ = self.force.try_send(());
    }
}
#[derive(Clone)]
pub struct CodecClient {
    inner: Arc<Inner>,
}
struct StartupGuard {
    force: tokio::sync::mpsc::Sender<()>,
    armed: bool,
}
impl Drop for StartupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.force.try_send(());
        }
    }
}
struct RequestGuard<'a> {
    inner: &'a Inner,
    completed: bool,
}
impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.inner.state.stopping.store(true, Ordering::Release);
            let _ = self.inner.control.try_send(());
        }
    }
}

async fn accept_authenticated(
    listener: TcpListener,
    token: [u8; 32],
    kind: u8,
) -> io::Result<TcpStream> {
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        let (mut stream, _) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| failure("codec worker authentication timed out"))??;
        let mut proof = [0u8; 33];
        let read = timeout(Duration::from_millis(500), stream.read_exact(&mut proof)).await;
        if matches!(read, Ok(Ok(_)))
            && proof[32] == kind
            && bool::from(proof[..32].ct_eq(token.as_slice()))
        {
            stream.set_nodelay(true)?;
            return Ok(stream);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn codec_auth_rejects_wrong_token_and_lane_before_accepting_valid_peer() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let token = [0x6du8; 32];
        let task = tokio::spawn(accept_authenticated(listener, token, b'A'));

        for (candidate, kind) in [([0x6cu8; 32], b'A'), (token, b'V')] {
            let mut socket = TcpStream::connect(address).await.unwrap();
            socket.write_all(&candidate).await.unwrap();
            socket.write_all(&[kind]).await.unwrap();
            let mut byte = [0u8; 1];
            assert_eq!(
                timeout(Duration::from_secs(2), socket.read(&mut byte))
                    .await
                    .expect("invalid peer must be closed")
                    .unwrap(),
                0
            );
            assert!(!task.is_finished(), "invalid peer was authenticated");
        }
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket.write_all(&token).await.unwrap();
        socket.write_all(b"A").await.unwrap();
        let accepted = timeout(Duration::from_secs(2), task)
            .await
            .expect("valid peer must be accepted")
            .expect("accept task panicked")
            .expect("valid peer was rejected");
        drop(accepted);
    }
}
fn add_system_environment(
    command: &mut Command,
    plugin_directory: Option<&std::path::Path>,
    loader_directory: Option<&std::path::Path>,
) -> io::Result<()> {
    // Only host-selected OS support variables; no inherited GST, loader, PATH,
    // credentials, workspace settings or project-controlled cwd.
    #[cfg(not(any(target_os = "linux", windows)))]
    let _ = loader_directory;
    for key in [
        "HOME",
        "USERPROFILE",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SystemRoot",
        "WINDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if let Some(path) = plugin_directory {
        command.env("LINGXI_GSTREAMER_PLUGIN_DIR", path);
    }
    command.env("GST_PLUGIN_PATH_1_0", "");
    command.env("GST_PLUGIN_SYSTEM_PATH_1_0", "");
    command.env("GST_REGISTRY_DISABLE", "yes");
    command.env("GST_REGISTRY_UPDATE", "no");
    command.env("GST_REGISTRY_FORK", "no");
    #[cfg(target_os = "linux")]
    if let Some(path) = loader_directory {
        command.env("LD_LIBRARY_PATH", path);
    }
    #[cfg(windows)]
    {
        let mut paths = Vec::new();
        if let Some(path) = loader_directory {
            paths.push(path.to_path_buf());
        }
        if let Some(system) = std::env::var_os("SystemRoot") {
            paths.push(PathBuf::from(system).join("System32"));
        }
        command.env(
            "PATH",
            std::env::join_paths(paths).map_err(|_| failure("media loader path invalid"))?,
        );
    }
    Ok(())
}
impl CodecClient {
    pub async fn spawn(
        launch: CodecLaunch,
        audio_enabled: bool,
        video_enabled: bool,
    ) -> io::Result<Self> {
        if !launch.executable.is_absolute()
            || launch
                .plugin_directory
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            || launch
                .loader_directory
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            || !launch.private_root.is_absolute()
            || (!lingxi_webrtc_media::codec_uses_static_plugins()
                && launch.plugin_directory.is_none())
        {
            return Err(failure(
                "codec executable, plugin and private paths must be absolute",
            ));
        }
        let executable = launch.executable.canonicalize()?;
        let plugin_directory = launch
            .plugin_directory
            .map(|path| path.canonicalize())
            .transpose()?;
        let loader_directory = launch
            .loader_directory
            .map(|path| path.canonicalize())
            .transpose()?;
        let private_root = launch.private_root.canonicalize()?;
        let directory = tempfile::Builder::new()
            .prefix("lingxi-codec-")
            .tempdir_in(private_root)?;
        let registry = directory.path().join("registry.bin");
        // GIO can otherwise auto-relocate modules next to libgio or fall back
        // to a build-machine path. Keep both module loading and plugin
        // installation inside this private process-owned directory; packaged
        // codec functionality with this configuration must also be verified.
        let gio_modules = directory.path().join("gio-modules");
        let disabled_installer = directory.path().join("disabled-installer");
        std::fs::create_dir(&gio_modules)?;
        std::fs::create_dir(&disabled_installer)?;
        let control_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let audio_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let video_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let mut token = [0u8; 32];
        SystemRandom::new()
            .fill(&mut token)
            .map_err(|_| failure("codec authentication unavailable"))?;
        let clock = Clock::parent();
        let mut command = Command::new(executable);
        command
            .args(&launch.arguments)
            .env_clear()
            .current_dir(directory.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        add_system_environment(
            &mut command,
            plugin_directory.as_deref(),
            loader_directory.as_deref(),
        )?;
        command.env("GST_REGISTRY_1_0", &registry);
        command.env("GIO_MODULE_DIR", &gio_modules);
        // A directory cannot be executed as the plugin installer helper.
        command.env("GST_INSTALL_PLUGINS_HELPER", &disabled_installer);
        let mut child = command.spawn()?;
        let input = child.stdin.take();
        let (exit_tx, _) = watch::channel(None);
        let state = Arc::new(Shared {
            stopping: AtomicBool::new(false),
            graceful: AtomicBool::new(false),
            protocol_fault: AtomicBool::new(false),
            leases: StdMutex::new(HashMap::new()),
            exit: exit_tx,
        });
        let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<()>(1);
        let (force_tx, mut force_rx) = tokio::sync::mpsc::channel::<()>(1);
        let (reader_tx, reader_rx) = tokio::sync::oneshot::channel::<tokio::task::JoinHandle<()>>();
        let reaper_state = state.clone();
        tokio::spawn(async move {
            // Keep the directory and packet permits until wait() confirms the
            // exact child exited, including cancelled/failed startup.
            let mut force = false;
            let status = loop {
                tokio::select! {
                    result = child.wait() => match result {
                        Ok(status) => break status,
                        Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                    },
                    request = force_rx.recv(), if !force => {
                        force = true;
                        if request.is_some() { let _ = child.start_kill(); }
                    }
                }
            };
            if let Ok(reader) = reader_rx.await {
                let _ = reader.await;
            }
            reaper_state
                .leases
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            let directory_removed = directory.close().is_ok();
            let receipt = ExitReceipt {
                success: status.success()
                    && directory_removed
                    && !reaper_state.protocol_fault.load(Ordering::Acquire),
                graceful: reaper_state.graceful.load(Ordering::Acquire),
            };
            reaper_state.exit.send_replace(Some(receipt));
        });
        let mut startup = StartupGuard {
            force: force_tx.clone(),
            armed: true,
        };
        let mut bootstrap = Vec::with_capacity(48);
        bootstrap.extend_from_slice(&control_listener.local_addr()?.port().to_be_bytes());
        bootstrap.extend_from_slice(&audio_listener.local_addr()?.port().to_be_bytes());
        bootstrap.extend_from_slice(&video_listener.local_addr()?.port().to_be_bytes());
        bootstrap.extend_from_slice(&token);
        bootstrap.extend_from_slice(BOOTSTRAP_VERSION);
        bootstrap.push(u8::from(audio_enabled));
        bootstrap.push(u8::from(video_enabled));
        let mut input = input.ok_or_else(|| failure("codec bootstrap unavailable"))?;
        if input.write_all(&bootstrap).await.is_err() || input.shutdown().await.is_err() {
            return Err(failure("codec bootstrap failed"));
        }
        let (control, audio, video) = tokio::join!(
            accept_authenticated(control_listener, token, b'C'),
            accept_authenticated(audio_listener, token, b'A'),
            accept_authenticated(video_listener, token, b'V')
        );
        let (mut control, audio, video) = match (control, audio, video) {
            (Ok(c), Ok(a), Ok(v)) => (c, a, v),
            _ => return Err(failure("codec worker failed to start")),
        };
        let clock_uncertainty = timeout(START_TIMEOUT, calibrate_parent_clock(&mut control, clock))
            .await
            .map_err(|_| failure("clock calibration timed out"))??;
        let (mut control_read, mut control_write) = control.into_split();
        tokio::spawn(async move {
            if control_rx.recv().await.is_some() {
                let _ = control_write.write_all(b"Q").await;
                let _ = control_write.shutdown().await;
            }
        });
        let read_state = state.clone();
        let reader_force = force_tx.clone();
        let reader = tokio::spawn(async move {
            let mut event = [0u8; 9];
            let mut invalid = false;
            while control_read.read_exact(&mut event).await.is_ok() {
                let id = u64::from_be_bytes(event[1..].try_into().unwrap());
                match (event[0], id) {
                    (b'R', id) if id != 0 && !read_state.graceful.load(Ordering::Acquire) => {
                        let present = read_state
                            .leases
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id)
                            .is_some();
                        if !present {
                            invalid = true;
                            break;
                        }
                    }
                    (b'D', 0)
                        if read_state.stopping.load(Ordering::Acquire)
                            && !read_state.graceful.swap(true, Ordering::AcqRel) => {}
                    _ => {
                        invalid = true;
                        break;
                    }
                }
            }
            if invalid {
                read_state.protocol_fault.store(true, Ordering::Release);
            }
            // A broken or invalid control stream must fence new operations even
            // if no data request happens to notice the child failure first.
            if invalid || !read_state.graceful.load(Ordering::Acquire) {
                read_state.stopping.store(true, Ordering::Release);
                let _ = reader_force.try_send(());
            }
        });
        if reader_tx.send(reader).is_err() {
            return Err(failure("codec owner stopped during startup"));
        }
        startup.armed = false;
        Ok(Self {
            inner: Arc::new(Inner {
                audio: Mutex::new(audio),
                video: Mutex::new(video),
                control: control_tx,
                force: force_tx,
                state,
                next: AtomicU64::new(1),
                clock,
                clock_uncertainty,
            }),
        })
    }

    /// Worst-case cross-process clock mapping error from the best startup probe.
    pub fn clock_uncertainty(&self) -> Duration {
        self.inner.clock_uncertainty
    }

    async fn request(
        &self,
        video: bool,
        request: Request,
        payload: &[u8],
    ) -> io::Result<(Reply, Vec<u8>)> {
        let mut guard = RequestGuard {
            inner: &self.inner,
            completed: false,
        };
        if self.inner.state.stopping.load(Ordering::Acquire) {
            return Err(failure("codec worker is stopping"));
        }
        let lane = if video {
            &self.inner.video
        } else {
            &self.inner.audio
        };
        let mut stream = lane.lock().await;
        if self.inner.state.stopping.load(Ordering::Acquire) {
            return Err(failure("codec worker is stopping"));
        }
        let result = timeout(IO_TIMEOUT, async {
            write_frame_async(&mut stream, &request, payload).await?;
            read_frame_async::<Reply>(&mut stream).await
        })
        .await;
        match result {
            Ok(Ok((Reply::Error, _))) | Ok(Err(_)) | Err(_) => {
                self.inner.state.stopping.store(true, Ordering::Release);
                let _ = self.inner.control.try_send(());
                Err(failure("codec operation could not be confirmed"))
            }
            Ok(Ok(reply)) => {
                guard.completed = true;
                Ok(reply)
            }
        }
    }
    fn protocol_error(&self, message: &'static str) -> io::Error {
        self.inner
            .state
            .protocol_fault
            .store(true, Ordering::Release);
        self.inner.state.stopping.store(true, Ordering::Release);
        let _ = self.inner.force.try_send(());
        failure(message)
    }
    fn hold(&self, packet: ReceivedRtp) -> io::Result<(u64, u64, Vec<u8>)> {
        if self.inner.state.stopping.load(Ordering::Acquire) {
            return Err(failure("codec worker is stopping"));
        }
        let at = self.inner.clock.encode(packet.at)?;
        let bytes = packet.as_ref().to_vec();
        if bytes.is_empty() || bytes.len() > 64 * 1024 {
            return Err(failure("RTP packet exceeds bounds"));
        }
        let id = self
            .inner
            .next
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| failure("codec packet identity exhausted"))?;
        let mut leases = self
            .inner
            .state
            .leases
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if leases.len() >= MAX_PENDING_RELEASES {
            return Err(failure("codec packet capacity exhausted"));
        }
        leases.insert(id, packet);
        Ok((id, at, bytes))
    }
    pub async fn push_audio(&self, packet: ReceivedRtp) -> io::Result<()> {
        let (id, at_ns, bytes) = self.hold(packet)?;
        match self
            .request(false, Request::AudioPush { id, at_ns }, &bytes)
            .await?
        {
            (Reply::Empty, payload) if payload.is_empty() => Ok(()),
            _ => Err(self.protocol_error("invalid audio push reply")),
        }
    }
    pub async fn poll_audio(
        &self,
    ) -> io::Result<Option<lingxi_webrtc_media::ports::DecodedAudioFrame>> {
        let (reply, bytes) = self.request(false, Request::AudioPoll, &[]).await?;
        match reply {
            Reply::Audio { meta: None } if bytes.is_empty() => Ok(None),
            Reply::Audio { meta: Some(m) }
                if m.channels == 1
                    && m.sample_rate_hz == 48_000
                    && !bytes.is_empty()
                    && bytes.len().is_multiple_of(2)
                    && bytes.len() <= 23_040 =>
            {
                Ok(Some(lingxi_webrtc_media::ports::DecodedAudioFrame {
                    sequence: m.sequence,
                    at: self
                        .inner
                        .clock
                        .decode(m.at_ns)
                        .map_err(|_| self.protocol_error("invalid decoded audio timestamp"))?,
                    sample_rate_hz: m.sample_rate_hz,
                    channels: m.channels,
                    pcm16le: bytes,
                }))
            }
            _ => Err(self.protocol_error("invalid decoded audio reply")),
        }
    }
    pub async fn encode_audio(&self, rate: u32, samples: &[f32]) -> io::Result<Vec<EncodedAudio>> {
        self.encode_audio_at(rate, samples, Instant::now()).await
    }
    /// Encode samples whose first-sample monotonic timestamp was captured by
    /// the host before a request could be delayed in the IPC queue.
    pub async fn encode_audio_at(
        &self,
        rate: u32,
        samples: &[f32],
        first_sample_at: Instant,
    ) -> io::Result<Vec<EncodedAudio>> {
        if first_sample_at > Instant::now() {
            return Err(failure("audio capture timestamp is in the future"));
        }
        let mut bytes = Vec::with_capacity(samples.len().saturating_mul(4));
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let (reply, bytes) = self
            .request(
                false,
                Request::AudioEncode {
                    rate,
                    at_ns: self.inner.clock.encode(first_sample_at)?,
                },
                &bytes,
            )
            .await?;
        let Reply::Encoded { packets } = reply else {
            return Err(self.protocol_error("invalid encoded audio reply"));
        };
        let mut offset: usize = 0;
        let mut result = Vec::with_capacity(packets.len());
        for packet in packets {
            let end = offset
                .checked_add(packet.size)
                .ok_or_else(|| self.protocol_error("encoded audio size overflow"))?;
            if packet.size == 0 || packet.size > 1275 || end > bytes.len() {
                return Err(self.protocol_error("invalid encoded audio reply"));
            }
            result.push(EncodedAudio {
                at: self
                    .inner
                    .clock
                    .decode(packet.at_ns)
                    .map_err(|_| self.protocol_error("invalid encoded audio timestamp"))?,
                data: bytes[offset..end].to_vec(),
            });
            offset = end;
        }
        if offset != bytes.len() {
            return Err(self.protocol_error("invalid encoded audio reply"));
        }
        Ok(result)
    }
    pub async fn reset_audio(&self) -> io::Result<()> {
        match self.request(false, Request::AudioReset, &[]).await? {
            (Reply::Empty, payload) if payload.is_empty() => Ok(()),
            _ => Err(self.protocol_error("invalid audio reset reply")),
        }
    }
    pub async fn push_video(&self, packet: ReceivedRtp) -> io::Result<()> {
        let (id, at_ns, bytes) = self.hold(packet)?;
        match self
            .request(true, Request::VideoPush { id, at_ns }, &bytes)
            .await?
        {
            (Reply::Empty, payload) if payload.is_empty() => Ok(()),
            _ => Err(self.protocol_error("invalid video push reply")),
        }
    }
    pub async fn enable_video(&self, enabled: bool) -> io::Result<()> {
        match self
            .request(true, Request::VideoEnable { enabled }, &[])
            .await?
        {
            (Reply::Empty, payload) if payload.is_empty() => Ok(()),
            _ => Err(self.protocol_error("invalid video control reply")),
        }
    }
    pub async fn poll_video(&self) -> io::Result<Vec<RemoteVideoFrame>> {
        let (reply, bytes) = self.request(true, Request::VideoPoll, &[]).await?;
        let Reply::Video { frames } = reply else {
            return Err(self.protocol_error("invalid decoded video reply"));
        };
        if frames.len() > 4 {
            return Err(self.protocol_error("invalid decoded video reply"));
        }
        let mut offset: usize = 0;
        let mut result = Vec::with_capacity(frames.len());
        for frame in frames {
            let end = offset
                .checked_add(frame.size)
                .ok_or_else(|| self.protocol_error("decoded video size overflow"))?;
            if frame.width == 0
                || frame.height == 0
                || frame.width > 4096
                || frame.height > 4096
                || frame.size > 4 * 1024 * 1024
                || end > bytes.len()
                || frame.offsets.len() < 3
                || frame.strides.len() < 3
            {
                return Err(self.protocol_error("invalid decoded video reply"));
            }
            result.push(RemoteVideoFrame {
                ssrc: frame.ssrc,
                at: self
                    .inner
                    .clock
                    .decode(frame.at_ns)
                    .map_err(|_| self.protocol_error("invalid decoded video timestamp"))?,
                frame: VideoFrame {
                    layout: VideoLayout {
                        width: frame.width,
                        height: frame.height,
                        size: frame.size,
                        offsets: frame.offsets,
                        strides: frame.strides,
                    },
                    pixels: bytes[offset..end].to_vec(),
                    pts: Duration::from_nanos(frame.pts_ns),
                },
            });
            offset = end;
        }
        if offset != bytes.len() {
            return Err(self.protocol_error("invalid decoded video reply"));
        }
        Ok(result)
    }
    /// Close admission and wait for a clean worker receipt plus actual exit.
    /// If the worker does not finish, kill is requested and an unverified
    /// result is returned unless a graceful receipt was observed.
    pub async fn shutdown(&self) -> io::Result<()> {
        self.inner.state.stopping.store(true, Ordering::Release);
        let _ = self.inner.control.try_send(());
        let mut exit = self.inner.state.exit.subscribe();
        let wait = async {
            loop {
                if let Some(receipt) = *exit.borrow_and_update() {
                    return Ok::<_, io::Error>(receipt);
                }
                exit.changed()
                    .await
                    .map_err(|_| failure("codec exit witness unavailable"))?;
            }
        };
        let receipt = match timeout(STOP_TIMEOUT, wait).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = self.inner.force.try_send(());
                return Err(failure("codec worker stop unverified"));
            }
        };
        if receipt.success && receipt.graceful {
            Ok(())
        } else {
            Err(failure("codec worker stop unverified"))
        }
    }
}
