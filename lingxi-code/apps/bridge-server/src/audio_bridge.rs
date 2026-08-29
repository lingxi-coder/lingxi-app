//! `AudioBridge` — the engine-side proxies for the three device-audio traits.
//!
//! On mobile, Swift/Kotlin inject real `Arc<dyn SpeechToText>` /
//! `Arc<dyn TextToSpeech>` / `Arc<dyn VoiceRecorder>` implementations through
//! `UniFFI`. The desktop has no native implementation: the microphone and
//! speaker belong to the Electron client. This module turns each trait call
//! into one [`ClientEvent::AudioRequest`] pushed at the connected client and
//! parks the caller until the matching
//! [`ClientCommand::AudioResponse`](client_protocol::commands::ClientCommand::AudioResponse)
//! comes back.
//!
//! ## Shape (mirrors [`client_adapter::BridgeComputerAccessBroker`])
//!
//! Same inverted handshake as the `computer` tool's `request_access` prompt:
//!
//! 1. A trait method reserves a fresh `request_id` (`AtomicU64`), parks a
//!    `oneshot::Sender<AudioResultDto>` in the id-keyed map, and pushes the
//!    request out through the connection's [`AudioRequestSink`].
//! 2. [`AudioResponder::resolve`] is called by the transport from a DIFFERENT
//!    task on an inbound `AudioResponse`: it looks up the id, removes the parked
//!    sender, and sends the result — which resolves the ORIGINAL trait call
//!    awaiting on the engine's tool-dispatch task.
//!
//! The one structural difference from the computer-access broker is that there
//! is no `mpsc` receive loop to drive: the trait methods ARE the request
//! source, so [`AudioBridge`] is called directly rather than draining a channel.
//!
//! ## Every path terminates
//!
//! The engine may call these traits at any time and the desktop client may be
//! disconnected mid-turn, so a parked request must never be able to hang a turn
//! forever. Three exits, in order of how early they fire:
//!
//! - **Nobody is listening.** [`AudioRequestSink::emit_request`] returns
//!   `false` when there is no connected client to push to. The request is
//!   un-parked immediately and fails with
//!   [`AudioFailure::NoClient`] — the caller does not wait out a deadline for an
//!   answer that cannot come.
//! - **The client went away after being asked.** [`AudioResponder::drain`],
//!   called on transport teardown (the same place the permission gate and the
//!   computer-access broker are drained), drops every parked sender; each
//!   dropped sender resolves its receiver to `Err`, which becomes
//!   [`AudioFailure::NoAnswer`].
//! - **The client is connected but never answers.** Each request carries a
//!   deadline (see [`STATE_QUERY_DEADLINE`], [`DEVICE_CONTROL_DEADLINE`],
//!   [`CAPTURE_DEADLINE`], and [`synthesis_deadline`] for the one op whose
//!   duration the caller chooses); when it expires the request is un-parked and
//!   fails with [`AudioFailure::NoAnswer`]. This is the only exit for a client
//!   that holds the socket open and drops the request on the floor.
//!
//! Which error each of those becomes, per trait, is documented on
//! [`stt_error`], [`tts_error`] and [`voice_error`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use client_protocol::commands::{AudioErrorKindDto, AudioResultDto};
use client_protocol::events::{AudioOpDto, ClientEvent};
use tokio::sync::{oneshot, Mutex};
use traits::stt::{SpeechToText, SttError, SttOpts, SttTranscript};
use traits::tts::{TextToSpeech, TtsAudio, TtsError, TtsOpts};
use traits::voice::{VoiceError, VoiceRecorder, VoiceRecording, VoiceRecordingOpts};

/// Deadline for a pure state read (`IsRecording`).
///
/// A healthy client answers this from memory, well inside one frame. The
/// ceiling is deliberately the shortest of the three because `is_recording`
/// has no error channel and may be polled to paint a button: a hung client
/// must not stall that caller for longer than a person would tolerate before
/// the answer degrades to `false`.
const STATE_QUERY_DEADLINE: Duration = Duration::from_secs(5);

/// Deadline for a device-control op with no human in the loop
/// (`StartRecording` / `StopRecording`).
///
/// These do real I/O — acquiring the audio session, then finalizing and
/// encoding the captured file — but nothing in them waits on a person, so tens
/// of seconds is already far outside normal. Sized to be generous for a slow
/// machine while still failing inside one user's patience.
const DEVICE_CONTROL_DEADLINE: Duration = Duration::from_secs(30);

/// Deadline for an op that legitimately waits on a person or a network
/// (`Transcribe`).
///
/// `Transcribe` holds the microphone open for a whole utterance and may then
/// round-trip to a network recognizer. Minutes, not seconds, is the honest
/// ceiling here — a shorter one would abort work that was going to succeed,
/// which is worse than the hang it is meant to prevent. It exists only to bound
/// a client that never answers at all.
///
/// `Synthesize` used to share this constant and must not: its duration is
/// chosen by the caller (the length of the text), so a flat bound is reachable
/// by an ordinary long `speak` — see [`synthesis_deadline`]. `Transcribe`'s is
/// not: the person speaking decides when to stop, and no caller can hand it a
/// longer job.
const CAPTURE_DEADLINE: Duration = Duration::from_secs(180);

/// Fixed part of a [`AudioOpDto::Synthesize`] deadline: acquiring the audio
/// session, picking the voice, and — because `window.speechSynthesis` is one
/// global queue shared by every session — waiting behind an utterance another
/// session started. Generous on purpose: firing early aborts work that was
/// going to succeed, which is the failure this whole derivation exists to
/// remove.
const SYNTHESIS_START_ALLOWANCE_SECS: u64 = 90;

/// How long one character of text may take to speak, at the SLOWEST rate the
/// device offers (`normalizeRate` clamps to `[0.5, 2.0]`, and the Web Speech
/// API's default is ~175 wpm, so half of that is ~7 characters a second). 200ms
/// leaves margin on top of that. The client's own watchdog uses the same
/// number — see `spokenTextTimeoutMs` in
/// `clients/electron/src/renderer/audio/synthesis.ts`.
const SYNTHESIS_MILLIS_PER_CHAR: u64 = 200;

/// Ceiling on the derived deadline, so no single call can park a turn for
/// hours. The desktop client refuses a text longer than `MAX_SPOKEN_CHARACTERS`
/// outright — instantly, with a named failure — so this clamp is not the
/// operative bound for it; it exists for any other caller of the trait.
const MAX_SYNTHESIS_DEADLINE_SECS: u64 = 1200;

/// The deadline for speaking `text`.
///
/// A flat deadline is the wrong instrument here. `Synthesize` is the one op
/// whose duration is chosen by the CALLER: the client answers when the whole
/// utterance has finished playing, so "read this document aloud" legitimately
/// takes minutes, and a fixed 180s reported a failure to the model while the
/// machine was audibly still talking — after which a retry queued a second
/// utterance behind the first. Deriving the bound from the text means the
/// deadline can only fire for a client that is not speaking at a plausible
/// rate, which is what a deadline is for.
fn synthesis_deadline(text: &str) -> Duration {
    let chars = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
    let secs = SYNTHESIS_START_ALLOWANCE_SECS
        .saturating_add(chars.saturating_mul(SYNTHESIS_MILLIS_PER_CHAR) / 1000);
    Duration::from_secs(secs.min(MAX_SYNTHESIS_DEADLINE_SECS))
}

/// Transport-supplied destination for an outbound [`ClientEvent::AudioRequest`].
///
/// Object-safe, mirroring [`client_adapter::ComputerAccessRequestSink`]: the
/// transport wraps each push into a `Frame::Event`. It differs in returning
/// whether the request actually reached a client, which the computer-access
/// sink does not need (that one fails closed through the broker's own `deny`).
/// Here the caller is awaiting a VALUE, so "nobody is listening" has to be
/// distinguishable from "asked, but no answer came" — see the module doc.
#[async_trait]
pub trait AudioRequestSink: Send + Sync {
    /// Forward one [`ClientEvent::AudioRequest`] to the underlying transport.
    /// Returns `false` when there is no connected client to forward it to.
    /// Implementations should be cheap / non-blocking.
    async fn emit_request(&self, request: ClientEvent) -> bool;
}

/// Parked audio requests' reply channels, keyed by `request_id`.
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<AudioResultDto>>>>;

/// Why a round trip produced no usable [`AudioResultDto`].
///
/// Kept as one internal enum rather than three so the "which error does each
/// trait report" decision lives in exactly one place per trait
/// ([`stt_error`] / [`tts_error`] / [`voice_error`]) instead of being spread
/// across five call sites.
#[derive(Debug, Clone)]
enum AudioFailure {
    /// No client is connected to perform the operation. The request was never
    /// delivered, so retrying now cannot help — something has to reconnect.
    NoClient,
    /// The request was delivered but no answer came back: the client hit its
    /// deadline, or it disconnected while the request was parked. Transient by
    /// nature — the same request against a healthy client would work.
    NoAnswer(String),
    /// The client answered with a typed failure of its own.
    Reported {
        /// Coarse, branchable failure class.
        kind: AudioErrorKindDto,
        /// The client's human-readable detail.
        message: String,
    },
    /// The client answered, but with a result variant that does not answer the
    /// operation that was asked (e.g. a recording-state flag for a transcribe).
    Mismatched(String),
}

/// The message a [`AudioFailure::NoClient`] carries into every trait's error.
const NO_CLIENT_MESSAGE: &str = "no desktop client is connected to perform the audio operation";

/// Raise an [`AudioFailure`] to the speech-recognition trait's own error enum.
///
/// Every currently-defined [`AudioErrorKindDto`] is listed explicitly. `SttError`
/// has a home for the six kinds speech recognition can produce; the two that
/// belong to the other traits (`NotRecording`, `SynthesisFailed`) fall to
/// `Other` DELIBERATELY and are named below. The wildcard arm exists only
/// because [`AudioErrorKindDto`] is `#[non_exhaustive]` — it catches a kind
/// added to the contract AFTER this code was written, not a kind that should
/// have mapped.
//
// `match_same_arms` is allowed on purpose: several kinds share `Other` as their
// destination, and collapsing them into one arm is exactly what ruling 1
// forbids — a merged arm cannot say WHY each kind has no home, and would
// silently swallow a kind that later grows one.
#[allow(clippy::match_same_arms)]
fn stt_error(failure: AudioFailure) -> SttError {
    match failure {
        // Nobody is listening: from the engine's point of view the device has
        // no speech-recognition service at all right now.
        AudioFailure::NoClient => SttError::Unavailable,
        // Asked, but no answer came. `Retriable` is the honest class: the
        // recognizer may well answer the next time it is asked.
        AudioFailure::NoAnswer(message) => SttError::Retriable(message),
        AudioFailure::Mismatched(message) => SttError::Other(message),
        AudioFailure::Reported { kind, message } => match kind {
            AudioErrorKindDto::PermissionDenied => SttError::PermissionDenied,
            AudioErrorKindDto::NoSpeech => SttError::NoSpeech,
            AudioErrorKindDto::Unavailable => SttError::Unavailable,
            AudioErrorKindDto::Busy => SttError::Busy,
            AudioErrorKindDto::Retriable => SttError::Retriable(message),
            // A recording-session kind: `SttError` has no `NotRecording`, and
            // `transcribe` never opens a session for one to be missing from.
            AudioErrorKindDto::NotRecording => SttError::Other(message),
            // A synthesis kind; recognition has no equivalent.
            AudioErrorKindDto::SynthesisFailed => SttError::Other(message),
            AudioErrorKindDto::Other => SttError::Other(message),
            // Only reachable for a kind added to the contract after this code
            // was written (`AudioErrorKindDto` is `#[non_exhaustive]`).
            _ => SttError::Other(message),
        },
    }
}

/// Raise an [`AudioFailure`] to the speech-synthesis trait's own error enum.
///
/// `TtsError` has only `Unavailable` / `SynthesisFailed` / `Other`, so the five
/// kinds belonging to the other two traits have no home and fall to `Other`
/// DELIBERATELY — each is listed by name below with the reason, so a kind that
/// later grows a home cannot be silently swallowed by a catch-all.
/// `AudioErrorKindDto::SynthesisFailed` round-trips to
/// `TtsError::SynthesisFailed`; this proxy also raises that variant itself for
/// audio it cannot base64-decode (in [`TextToSpeech::synthesize`]).
//
// `match_same_arms` allowed for the same reason as in [`stt_error`].
#[allow(clippy::match_same_arms)]
fn tts_error(failure: AudioFailure) -> TtsError {
    match failure {
        // Nobody is listening: no usable TTS engine is reachable.
        AudioFailure::NoClient => TtsError::Unavailable,
        // Asked, but no answer came. `TtsError` has no retriable class, so this
        // is `Other` and the message carries the distinction.
        AudioFailure::NoAnswer(message) => TtsError::Other(message),
        AudioFailure::Mismatched(message) => TtsError::Other(message),
        AudioFailure::Reported { kind, message } => match kind {
            AudioErrorKindDto::Unavailable => TtsError::Unavailable,
            AudioErrorKindDto::SynthesisFailed => TtsError::SynthesisFailed(message),
            // Synthesis needs no microphone permission; a client reporting it
            // here is describing an audio-session/output failure `TtsError`
            // cannot name.
            AudioErrorKindDto::PermissionDenied => TtsError::Other(message),
            // "No speech was detected" is a recognition outcome; it is
            // meaningless for synthesis and has no `TtsError` home.
            AudioErrorKindDto::NoSpeech => TtsError::Other(message),
            // A recording-session kind; synthesis opens no session.
            AudioErrorKindDto::NotRecording => TtsError::Other(message),
            // `TtsError` has no `Busy` variant.
            AudioErrorKindDto::Busy => TtsError::Other(message),
            // `TtsError` has no retriable variant; `SynthesisFailed` would be a
            // LIE here — it names a permanent failure of this text/voice.
            AudioErrorKindDto::Retriable => TtsError::Other(message),
            AudioErrorKindDto::Other => TtsError::Other(message),
            // Only reachable for a kind added to the contract after this code
            // was written.
            _ => TtsError::Other(message),
        },
    }
}

/// Raise an [`AudioFailure`] to the microphone-capture trait's own error enum.
///
/// `VoiceError` has `PermissionDenied` / `NotRecording` / `Busy` / `Other`; the
/// four kinds belonging to the other two traits have no home and fall to
/// `Other` DELIBERATELY, listed by name below.
//
// `match_same_arms` allowed for the same reason as in [`stt_error`].
#[allow(clippy::match_same_arms)]
fn voice_error(failure: AudioFailure) -> VoiceError {
    match failure {
        // Nobody is listening. `VoiceError` has no `Unavailable` variant, so
        // this is `Other` and the message names the missing client.
        AudioFailure::NoClient => VoiceError::Other(NO_CLIENT_MESSAGE.to_string()),
        AudioFailure::NoAnswer(message) | AudioFailure::Mismatched(message) => {
            VoiceError::Other(message)
        }
        AudioFailure::Reported { kind, message } => match kind {
            AudioErrorKindDto::PermissionDenied => VoiceError::PermissionDenied,
            AudioErrorKindDto::NotRecording => VoiceError::NotRecording,
            AudioErrorKindDto::Busy => VoiceError::Busy,
            // Recording is not recognition: `VoiceError` has no `NoSpeech`.
            AudioErrorKindDto::NoSpeech => VoiceError::Other(message),
            // `VoiceError` has no `Unavailable` variant.
            AudioErrorKindDto::Unavailable => VoiceError::Other(message),
            // `VoiceError` has no retriable variant.
            AudioErrorKindDto::Retriable => VoiceError::Other(message),
            // A synthesis kind; recording produces no synthesized audio.
            AudioErrorKindDto::SynthesisFailed => VoiceError::Other(message),
            AudioErrorKindDto::Other => VoiceError::Other(message),
            // Only reachable for a kind added to the contract after this code
            // was written.
            _ => VoiceError::Other(message),
        },
    }
}

/// The message a result that does not answer the requested op produces.
fn mismatched(op: &str, result: &AudioResultDto) -> AudioFailure {
    AudioFailure::Mismatched(format!(
        "the client did not answer the requested operation ({op}): {result:?}"
    ))
}

/// The engine-side proxy implementing [`SpeechToText`], [`TextToSpeech`] and
/// [`VoiceRecorder`] over one connected client. See the module doc for the full
/// shape.
pub struct AudioBridge {
    /// Where outbound requests go (the transport).
    sink: Arc<dyn AudioRequestSink>,
    /// Monotonic `request_id` source. Connection-scoped, mirroring
    /// [`client_adapter::BridgeComputerAccessBroker`]'s id counter.
    next_id: AtomicU64,
    /// Parked requests' reply channels, keyed by `request_id`.
    pending: Pending,
}

/// The response side of the bridge, handed to the transport so an inbound
/// `AudioResponse` can resolve the parked request.
///
/// Cloneable and independent of [`AudioBridge`] so the connection can hold it
/// without owning the trait objects the engine was given.
#[derive(Clone)]
pub struct AudioResponder {
    /// The SAME map [`AudioBridge`] parks into.
    pending: Pending,
}

impl AudioResponder {
    /// Resolve a parked request with the client's outcome (an inbound
    /// `AudioResponse`). Returns `true` if a matching request was found and
    /// resolved, `false` if the id was unknown / already resolved / already
    /// timed out (a safe no-op, mirroring
    /// [`client_adapter::BridgeComputerAccessBroker::resolve`]).
    pub async fn resolve(&self, request_id: u64, result: AudioResultDto) -> bool {
        let Some(sender) = self.pending.lock().await.remove(&request_id) else {
            return false;
        };
        // If the receiver vanished (a drain or a deadline raced this resolve),
        // the send simply fails; that path already produced a failure for the
        // caller, so it is safe to ignore.
        sender.send(result).is_ok()
    }

    /// Drop every parked sender so all in-flight trait calls fail instead of
    /// waiting out their deadline. Call on transport teardown, alongside
    /// `AdapterPermissionGate::drain` and
    /// `BridgeComputerAccessBroker::drain`. Returns the number of requests
    /// drained.
    ///
    /// A dropped `oneshot::Sender` resolves its receiver to `Err`, which
    /// [`AudioBridge::request`] reports as [`AudioFailure::NoAnswer`].
    pub async fn drain(&self) -> usize {
        let mut pending = self.pending.lock().await;
        let n = pending.len();
        pending.clear();
        n
    }

    /// Number of requests currently parked (test/inspection helper).
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.len()
    }
}

/// Build the bridge and its responder over one connection's request sink.
///
/// The two halves share the pending-request table: the [`AudioBridge`] is what
/// the engine is injected with (as `Arc<dyn SpeechToText>` etc.), the
/// [`AudioResponder`] is what the transport calls when the client answers.
#[must_use]
pub fn new_audio_bridge(sink: Arc<dyn AudioRequestSink>) -> (Arc<AudioBridge>, AudioResponder) {
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let bridge = Arc::new(AudioBridge {
        sink,
        next_id: AtomicU64::new(1),
        pending: pending.clone(),
    });
    (bridge, AudioResponder { pending })
}

impl AudioBridge {
    /// One request/response round trip: park, push, await, un-park.
    ///
    /// `deadline` bounds the "connected but silent client" case; see the
    /// module doc for why all three exits exist and the deadline constants for
    /// why each is the length it is.
    async fn request(
        &self,
        op: AudioOpDto,
        deadline: Duration,
    ) -> Result<AudioResultDto, AudioFailure> {
        let request_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, tx);

        if !self
            .sink
            .emit_request(ClientEvent::AudioRequest { request_id, op })
            .await
        {
            // Nobody is listening: un-park now rather than making the caller
            // wait out `deadline` for an answer that can never arrive.
            self.pending.lock().await.remove(&request_id);
            return Err(AudioFailure::NoClient);
        }

        match tokio::time::timeout(deadline, rx).await {
            Ok(Ok(result)) => Ok(result),
            // The parked sender was dropped — `AudioResponder::drain` on
            // transport teardown, or the responder itself going away.
            Ok(Err(_)) => Err(AudioFailure::NoAnswer(
                "the desktop client disconnected and did not answer the audio request".to_string(),
            )),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(AudioFailure::NoAnswer(format!(
                    "the desktop client did not answer the audio request within {}s",
                    deadline.as_secs()
                )))
            }
        }
    }
}

#[async_trait]
impl SpeechToText for AudioBridge {
    async fn transcribe(&self, opts: SttOpts) -> Result<SttTranscript, SttError> {
        let result = self
            .request(
                AudioOpDto::Transcribe {
                    language: opts.language,
                },
                CAPTURE_DEADLINE,
            )
            .await
            .map_err(stt_error)?;
        match result {
            AudioResultDto::Transcript {
                text,
                language,
                confidence,
            } => Ok(SttTranscript {
                text,
                language,
                confidence,
            }),
            AudioResultDto::Failed { kind, message } => {
                Err(stt_error(AudioFailure::Reported { kind, message }))
            }
            other => Err(stt_error(mismatched("transcribe", &other))),
        }
    }
}

#[async_trait]
impl TextToSpeech for AudioBridge {
    /// ## The empty-PCM "played in place" convention
    ///
    /// The desktop client's synthesizer is `window.speechSynthesis`
    /// (`clients/electron/src/renderer/audio/synthesis.ts`), which PLAYS
    /// audio through the OS directly and hands the page no samples at all.
    /// That client therefore answers a `Synthesize` request with
    /// `AudioResultDto::Audio { pcm_base64: "", sample_rate_hz: 0 }` on a
    /// SUCCESSFUL synthesis — `""`/`0` here means "already played in place,"
    /// not a failure and not an omission. `base64::decode("")` is `Ok(vec![])`,
    /// so this falls straight through the `Ok` arm below to `TtsAudio { pcm:
    /// vec![], sample_rate_hz: 0 }` without hitting the decode-error branch.
    ///
    /// Do NOT "fix" this into treating an empty `pcm` as a failure — that
    /// would silently break desktop TTS the moment someone tightens this up
    /// on the strength of `TtsAudio { pcm: [] }` looking like a bug. See
    /// `synthesize_treats_empty_pcm_as_played_in_place_not_a_failure` below,
    /// which pins this by mutation: rejecting on empty `pcm` turns that test
    /// red while every other test in this module stays green.
    ///
    /// This convention is honest end to end: `tools/mobile/src/speech.rs`
    /// never forwards `TtsAudio.pcm` to the model — it reports `{"spoken":
    /// true, "sample_rate_hz": …, "pcm_bytes_len": …}`, which on desktop
    /// reads "spoke, produced 0 bytes." Accurate, not false.
    async fn synthesize(&self, opts: TtsOpts) -> Result<TtsAudio, TtsError> {
        // Derived from the text, not flat: see [`synthesis_deadline`].
        let deadline = synthesis_deadline(&opts.text);
        let result = self
            .request(
                AudioOpDto::Synthesize {
                    text: opts.text,
                    voice: opts.voice,
                },
                deadline,
            )
            .await
            .map_err(tts_error)?;
        match result {
            AudioResultDto::Audio {
                pcm_base64,
                sample_rate_hz,
            } => {
                let pcm = base64::engine::general_purpose::STANDARD
                    .decode(pcm_base64)
                    .map_err(|error| {
                        TtsError::SynthesisFailed(format!(
                            "the client returned undecodable synthesized audio: {error}"
                        ))
                    })?;
                Ok(TtsAudio {
                    pcm,
                    sample_rate_hz,
                })
            }
            AudioResultDto::Failed { kind, message } => {
                Err(tts_error(AudioFailure::Reported { kind, message }))
            }
            other => Err(tts_error(mismatched("synthesize", &other))),
        }
    }
}

#[async_trait]
impl VoiceRecorder for AudioBridge {
    async fn start_recording(&self, opts: VoiceRecordingOpts) -> Result<(), VoiceError> {
        let result = self
            .request(
                AudioOpDto::StartRecording {
                    sample_rate_hz: opts.sample_rate_hz,
                    format: opts.format,
                },
                DEVICE_CONTROL_DEADLINE,
            )
            .await
            .map_err(voice_error)?;
        match result {
            AudioResultDto::Ok => Ok(()),
            AudioResultDto::Failed { kind, message } => {
                Err(voice_error(AudioFailure::Reported { kind, message }))
            }
            other => Err(voice_error(mismatched("start_recording", &other))),
        }
    }

    async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError> {
        let result = self
            .request(AudioOpDto::StopRecording, DEVICE_CONTROL_DEADLINE)
            .await
            .map_err(voice_error)?;
        match result {
            AudioResultDto::Recording {
                audio_base64,
                mime_type,
            } => {
                let audio_bytes = base64::engine::general_purpose::STANDARD
                    .decode(audio_base64)
                    .map_err(|error| {
                        VoiceError::Other(format!(
                            "the client returned an undecodable recording: {error}"
                        ))
                    })?;
                Ok(VoiceRecording {
                    audio_bytes,
                    mime_type,
                })
            }
            AudioResultDto::Failed { kind, message } => {
                Err(voice_error(AudioFailure::Reported { kind, message }))
            }
            other => Err(voice_error(mismatched("stop_recording", &other))),
        }
    }

    async fn is_recording(&self) -> bool {
        // `VoiceRecorder::is_recording` returns a BARE `bool` — the trait gives
        // this proxy no error channel, so a failed round trip (no client, no
        // answer, or an answer that does not fit the op) has to pick a value.
        //
        // `false` is the safe direction. Claiming "not recording" when the
        // client is unreachable degrades to a stuck-off button: the user can
        // still press record, and the next call re-asks. Claiming "recording"
        // would strand the UI in a state the user cannot exit — a stop that
        // cannot be delivered, on a session that may not exist. Neither answer
        // is knowable, so we take the one whose failure mode the user can
        // recover from.
        match self
            .request(AudioOpDto::IsRecording, STATE_QUERY_DEADLINE)
            .await
        {
            Ok(AudioResultDto::RecordingState { recording }) => recording,
            Ok(_) | Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use client_protocol::commands::{AudioErrorKindDto, AudioResultDto};
    use client_protocol::events::{AudioOpDto, ClientEvent};
    use tokio::sync::mpsc;
    use traits::stt::{SpeechToText, SttError, SttOpts};
    use traits::tts::{TextToSpeech, TtsError, TtsOpts};
    use traits::voice::{VoiceError, VoiceRecorder, VoiceRecordingOpts};

    use super::{
        new_audio_bridge, synthesis_deadline, AudioBridge, AudioRequestSink, AudioResponder,
        Duration, MAX_SYNTHESIS_DEADLINE_SECS, SYNTHESIS_MILLIS_PER_CHAR,
        SYNTHESIS_START_ALLOWANCE_SECS,
    };

    /// An [`AudioRequestSink`] that captures every emitted request so a test can
    /// read back the assigned `request_id`, and that can be switched to
    /// "no client connected" to exercise the undeliverable path.
    struct MockSink {
        emitted: mpsc::UnboundedSender<ClientEvent>,
        connected: AtomicBool,
    }

    #[async_trait]
    impl AudioRequestSink for MockSink {
        async fn emit_request(&self, request: ClientEvent) -> bool {
            if !self.connected.load(Ordering::SeqCst) {
                return false;
            }
            let _ = self.emitted.send(request);
            true
        }
    }

    /// A bridge whose sink is connected: requests are delivered and park.
    fn test_bridge() -> (
        Arc<AudioBridge>,
        AudioResponder,
        mpsc::UnboundedReceiver<ClientEvent>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let sink = Arc::new(MockSink {
            emitted: tx,
            connected: AtomicBool::new(true),
        });
        let (bridge, responder) = new_audio_bridge(sink);
        (bridge, responder, rx)
    }

    /// A bridge whose sink reports that nobody is listening. The receiver is
    /// returned (not dropped) so a failure here can only come from the "no
    /// client" path, never from a closed channel.
    fn disconnected_bridge() -> (Arc<AudioBridge>, mpsc::UnboundedReceiver<ClientEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let sink = Arc::new(MockSink {
            emitted: tx,
            connected: AtomicBool::new(false),
        });
        let (bridge, _responder) = new_audio_bridge(sink);
        (bridge, rx)
    }

    /// Pull the next emitted request, asserting it is an `AudioRequest`.
    async fn next_request(emitted: &mut mpsc::UnboundedReceiver<ClientEvent>) -> (u64, AudioOpDto) {
        match emitted.recv().await.expect("an AudioRequest was emitted") {
            ClientEvent::AudioRequest { request_id, op } => (request_id, op),
            other => panic!("expected AudioRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transcribe_emits_a_request_and_resolves_on_the_client_response() {
        let (bridge, responder, mut emitted) = test_bridge();

        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .transcribe(SttOpts {
                        language: Some("zh-CN".to_string()),
                    })
                    .await
            }
        });

        let (request_id, op) = next_request(&mut emitted).await;
        assert_eq!(
            op,
            AudioOpDto::Transcribe {
                language: Some("zh-CN".to_string())
            }
        );

        assert!(
            responder
                .resolve(
                    request_id,
                    AudioResultDto::Transcript {
                        text: "你好".to_string(),
                        language: Some("zh-CN".to_string()),
                        confidence: None,
                    },
                )
                .await
        );

        let transcript = task.await.unwrap().unwrap();
        assert_eq!(transcript.text, "你好");
        assert_eq!(transcript.language, Some("zh-CN".to_string()));
    }

    #[tokio::test]
    async fn a_failed_response_becomes_an_stt_error_not_an_empty_transcript() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.transcribe(SttOpts::default()).await }
        });
        let (request_id, _) = next_request(&mut emitted).await;
        responder
            .resolve(
                request_id,
                AudioResultDto::Failed {
                    kind: AudioErrorKindDto::Other,
                    message: "the recognizer exploded".to_string(),
                },
            )
            .await;
        let error = task.await.unwrap().unwrap_err();
        assert!(
            format!("{error}").contains("the recognizer exploded"),
            "a client-side failure must surface as an error carrying its reason, got: {error}"
        );
    }

    /// Ruling 1: every wire kind lands on the RIGHT variant of each trait's own
    /// error enum, and the kinds with no home in a given enum fall to `Other`
    /// deliberately. Task 1 pinned the forward direction; this pins the reverse.
    #[tokio::test]
    async fn every_error_kind_maps_to_its_home_variant_in_the_stt_impl() {
        use AudioErrorKindDto as K;
        for (kind, expected) in [
            (K::PermissionDenied, "PermissionDenied"),
            (K::NoSpeech, "NoSpeech"),
            (K::Unavailable, "Unavailable"),
            (K::Busy, "Busy"),
            (K::Retriable, "Retriable"),
            // The two kinds owned by the other traits have no SttError home.
            (K::NotRecording, "Other"),
            (K::SynthesisFailed, "Other"),
            (K::Other, "Other"),
        ] {
            let error = stt_failure(kind).await;
            assert_eq!(stt_name(&error), expected, "SttError mapping for {kind:?}");
        }
    }

    #[tokio::test]
    async fn every_error_kind_maps_to_its_home_variant_in_the_tts_impl() {
        use AudioErrorKindDto as K;
        for (kind, expected) in [
            // TtsError has no permission/no-speech/not-recording/busy/retriable
            // variant, so those fall to `Other` deliberately (see `tts_error`).
            (K::PermissionDenied, "Other"),
            (K::NoSpeech, "Other"),
            (K::NotRecording, "Other"),
            (K::Unavailable, "Unavailable"),
            (K::Busy, "Other"),
            (K::Retriable, "Other"),
            (K::SynthesisFailed, "SynthesisFailed"),
            (K::Other, "Other"),
        ] {
            let error = tts_failure(kind).await;
            assert_eq!(tts_name(&error), expected, "TtsError mapping for {kind:?}");
        }
    }

    #[tokio::test]
    async fn every_error_kind_maps_to_its_home_variant_in_the_voice_impl() {
        use AudioErrorKindDto as K;
        for (kind, expected) in [
            (K::PermissionDenied, "PermissionDenied"),
            (K::NotRecording, "NotRecording"),
            // VoiceError has no no-speech/unavailable/retriable variant, and
            // SynthesisFailed belongs to TTS.
            (K::NoSpeech, "Other"),
            (K::Unavailable, "Other"),
            (K::Busy, "Busy"),
            (K::Retriable, "Other"),
            (K::SynthesisFailed, "Other"),
            (K::Other, "Other"),
        ] {
            let error = voice_failure(kind).await;
            assert_eq!(
                voice_name(&error),
                expected,
                "VoiceError mapping for {kind:?}"
            );
        }
    }

    /// Variant name of an `SttError`, so a mapping failure NAMES the variant
    /// rather than printing a payload comparison.
    fn stt_name(error: &SttError) -> &'static str {
        match error {
            SttError::PermissionDenied => "PermissionDenied",
            SttError::NoSpeech => "NoSpeech",
            SttError::Unavailable => "Unavailable",
            SttError::Busy => "Busy",
            SttError::Retriable(_) => "Retriable",
            SttError::Other(_) => "Other",
        }
    }

    /// Variant name of a `VoiceError`. See [`stt_name`].
    fn voice_name(error: &VoiceError) -> &'static str {
        match error {
            VoiceError::PermissionDenied => "PermissionDenied",
            VoiceError::NotRecording => "NotRecording",
            VoiceError::Busy => "Busy",
            VoiceError::Other(_) => "Other",
        }
    }

    /// Variant name of a `TtsError`. See [`stt_name`].
    fn tts_name(error: &TtsError) -> &'static str {
        match error {
            TtsError::Unavailable => "Unavailable",
            TtsError::SynthesisFailed(_) => "SynthesisFailed",
            TtsError::Other(_) => "Other",
        }
    }

    /// Forward map: the kind a `SttError` variant is lowered to on the wire.
    /// A COPY of `client-protocol`'s `stt_error_kind` (that one lives in a test
    /// of another crate and cannot be imported), kept exhaustive with no
    /// wildcard so a new upstream variant fails THIS compile too.
    fn stt_kind(error: &SttError) -> AudioErrorKindDto {
        match error {
            SttError::PermissionDenied => AudioErrorKindDto::PermissionDenied,
            SttError::NoSpeech => AudioErrorKindDto::NoSpeech,
            SttError::Unavailable => AudioErrorKindDto::Unavailable,
            SttError::Busy => AudioErrorKindDto::Busy,
            SttError::Retriable(_) => AudioErrorKindDto::Retriable,
            SttError::Other(_) => AudioErrorKindDto::Other,
        }
    }

    /// Forward map for `VoiceError`. See [`stt_kind`].
    fn voice_kind(error: &VoiceError) -> AudioErrorKindDto {
        match error {
            VoiceError::PermissionDenied => AudioErrorKindDto::PermissionDenied,
            VoiceError::NotRecording => AudioErrorKindDto::NotRecording,
            VoiceError::Busy => AudioErrorKindDto::Busy,
            VoiceError::Other(_) => AudioErrorKindDto::Other,
        }
    }

    /// Forward map for `TtsError`. See [`stt_kind`].
    fn tts_kind(error: &TtsError) -> AudioErrorKindDto {
        match error {
            TtsError::Unavailable => AudioErrorKindDto::Unavailable,
            TtsError::SynthesisFailed(_) => AudioErrorKindDto::SynthesisFailed,
            TtsError::Other(_) => AudioErrorKindDto::Other,
        }
    }

    /// Every source variant must survive a full round trip through the wire and
    /// back through the PRODUCTION reverse maps: lower it to a kind, put that
    /// kind on a real `AudioResultDto::Failed`, answer a real parked request
    /// with it, and get the SAME variant out of the trait call.
    ///
    /// `client-protocol`'s own round-trip test pins that the CONTRACT carries
    /// enough distinct kinds; it does so against a reverse map written in that
    /// test file. This one closes the loop on the map that actually ships —
    /// a reverse map could satisfy the contract test's twin and still be wrong
    /// here.
    #[tokio::test]
    async fn audio_error_kind_round_trips_every_source_variant_through_the_real_maps() {
        // Collected rather than asserted per-variant, so ONE run names EVERY
        // collapsed variant instead of stopping at the first.
        let mut collapsed: Vec<String> = Vec::new();

        for error in [
            SttError::PermissionDenied,
            SttError::NoSpeech,
            SttError::Unavailable,
            SttError::Busy,
            SttError::Retriable("network blip".to_string()),
            SttError::Other("native crash".to_string()),
        ] {
            let kind = stt_kind(&error);
            let back = stt_failure(kind).await;
            if stt_name(&back) != stt_name(&error) {
                collapsed.push(format!(
                    "SttError::{} -> {kind:?} -> SttError::{}",
                    stt_name(&error),
                    stt_name(&back)
                ));
            }
        }

        for error in [
            VoiceError::PermissionDenied,
            VoiceError::NotRecording,
            VoiceError::Busy,
            VoiceError::Other("native crash".to_string()),
        ] {
            let kind = voice_kind(&error);
            let back = voice_failure(kind).await;
            if voice_name(&back) != voice_name(&error) {
                collapsed.push(format!(
                    "VoiceError::{} -> {kind:?} -> VoiceError::{}",
                    voice_name(&error),
                    voice_name(&back)
                ));
            }
        }

        for error in [
            TtsError::Unavailable,
            TtsError::SynthesisFailed("bad voice id".to_string()),
            TtsError::Other("native crash".to_string()),
        ] {
            let kind = tts_kind(&error);
            let back = tts_failure(kind).await;
            if tts_name(&back) != tts_name(&error) {
                collapsed.push(format!(
                    "TtsError::{} -> {kind:?} -> TtsError::{}",
                    tts_name(&error),
                    tts_name(&back)
                ));
            }
        }

        assert!(
            collapsed.is_empty(),
            "these source variants do not survive the round trip through the \
             production reverse maps: {collapsed:#?}"
        );
    }

    /// Drive one `transcribe` to a `Failed { kind }` answer and return the error.
    async fn stt_failure(kind: AudioErrorKindDto) -> SttError {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.transcribe(SttOpts::default()).await }
        });
        let (request_id, _) = next_request(&mut emitted).await;
        responder
            .resolve(
                request_id,
                AudioResultDto::Failed {
                    kind,
                    message: "client said no".to_string(),
                },
            )
            .await;
        task.await.unwrap().unwrap_err()
    }

    /// Drive one `synthesize` to a `Failed { kind }` answer and return the error.
    async fn tts_failure(kind: AudioErrorKindDto) -> TtsError {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .synthesize(TtsOpts {
                        text: "hi".to_string(),
                        voice: None,
                    })
                    .await
            }
        });
        let (request_id, _) = next_request(&mut emitted).await;
        responder
            .resolve(
                request_id,
                AudioResultDto::Failed {
                    kind,
                    message: "client said no".to_string(),
                },
            )
            .await;
        task.await.unwrap().unwrap_err()
    }

    /// Drive one `stop_recording` to a `Failed { kind }` answer.
    async fn voice_failure(kind: AudioErrorKindDto) -> VoiceError {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.stop_recording().await }
        });
        let (request_id, _) = next_request(&mut emitted).await;
        responder
            .resolve(
                request_id,
                AudioResultDto::Failed {
                    kind,
                    message: "client said no".to_string(),
                },
            )
            .await;
        task.await.unwrap().unwrap_err()
    }

    #[tokio::test]
    async fn synthesize_decodes_the_base64_payload() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .synthesize(TtsOpts {
                        text: "hi".to_string(),
                        voice: Some("alloy".to_string()),
                    })
                    .await
            }
        });
        let (request_id, op) = next_request(&mut emitted).await;
        assert_eq!(
            op,
            AudioOpDto::Synthesize {
                text: "hi".to_string(),
                voice: Some("alloy".to_string()),
            }
        );
        responder
            .resolve(
                request_id,
                AudioResultDto::Audio {
                    // base64 of the four bytes 0x01 0x02 0x03 0x04.
                    pcm_base64: "AQIDBA==".to_string(),
                    sample_rate_hz: 24_000,
                },
            )
            .await;
        let audio = task.await.unwrap().unwrap();
        assert_eq!(audio.pcm, vec![1, 2, 3, 4]);
        assert_eq!(audio.sample_rate_hz, 24_000);
    }

    #[tokio::test]
    async fn undecodable_synthesized_audio_is_a_synthesis_failure() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .synthesize(TtsOpts {
                        text: "hi".to_string(),
                        voice: None,
                    })
                    .await
            }
        });
        let (request_id, _) = next_request(&mut emitted).await;
        responder
            .resolve(
                request_id,
                AudioResultDto::Audio {
                    pcm_base64: "not base64!!".to_string(),
                    sample_rate_hz: 24_000,
                },
            )
            .await;
        let error = task.await.unwrap().unwrap_err();
        assert!(
            matches!(error, TtsError::SynthesisFailed(_)),
            "undecodable audio is a synthesis failure, got {error:?}"
        );
    }

    /// Pins Ruling B-12's "played in place" convention (see the doc comment
    /// on `TextToSpeech::synthesize` above): the desktop client answers a
    /// successful synthesis with EMPTY pcm/rate because `speechSynthesis`
    /// already played the audio and has no samples to hand back. That must
    /// decode to a successful, empty `TtsAudio` — never a `TtsError`.
    #[tokio::test]
    async fn synthesize_treats_empty_pcm_as_played_in_place_not_a_failure() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .synthesize(TtsOpts {
                        text: "hi".to_string(),
                        voice: None,
                    })
                    .await
            }
        });
        let (request_id, _) = next_request(&mut emitted).await;
        responder
            .resolve(
                request_id,
                AudioResultDto::Audio {
                    pcm_base64: String::new(),
                    sample_rate_hz: 0,
                },
            )
            .await;
        let audio = task.await.unwrap().unwrap_or_else(|error| {
            panic!("empty pcm must be a success meaning \"played in place\", got an error instead: {error:?}")
        });
        assert_eq!(audio.pcm, Vec::<u8>::new());
        assert_eq!(audio.sample_rate_hz, 0);
    }

    #[tokio::test]
    async fn stop_recording_decodes_the_base64_payload() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.stop_recording().await }
        });
        let (request_id, op) = next_request(&mut emitted).await;
        assert_eq!(op, AudioOpDto::StopRecording);
        responder
            .resolve(
                request_id,
                AudioResultDto::Recording {
                    audio_base64: "AQIDBA==".to_string(),
                    mime_type: "audio/m4a".to_string(),
                },
            )
            .await;
        let recording = task.await.unwrap().unwrap();
        assert_eq!(recording.audio_bytes, vec![1, 2, 3, 4]);
        assert_eq!(recording.mime_type, "audio/m4a");
    }

    #[tokio::test]
    async fn start_recording_lowers_its_options_and_succeeds_on_ok() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move {
                bridge
                    .start_recording(VoiceRecordingOpts {
                        sample_rate_hz: 16_000,
                        format: "m4a".to_string(),
                    })
                    .await
            }
        });
        let (request_id, op) = next_request(&mut emitted).await;
        assert_eq!(
            op,
            AudioOpDto::StartRecording {
                sample_rate_hz: 16_000,
                format: "m4a".to_string(),
            }
        );
        responder.resolve(request_id, AudioResultDto::Ok).await;
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn is_recording_reports_the_client_state() {
        for reported in [true, false] {
            let (bridge, responder, mut emitted) = test_bridge();
            let task = tokio::spawn({
                let bridge = bridge.clone();
                async move { bridge.is_recording().await }
            });
            let (request_id, op) = next_request(&mut emitted).await;
            assert_eq!(op, AudioOpDto::IsRecording);
            responder
                .resolve(
                    request_id,
                    AudioResultDto::RecordingState {
                        recording: reported,
                    },
                )
                .await;
            assert_eq!(task.await.unwrap(), reported);
        }
    }

    /// Ruling 2: `is_recording` has no error channel, so a failed round trip
    /// must pick a value — `false`, in both failure shapes.
    #[tokio::test]
    async fn is_recording_is_false_when_no_client_is_connected() {
        let (bridge, _emitted) = disconnected_bridge();
        assert!(!bridge.is_recording().await);
    }

    #[tokio::test(start_paused = true)]
    async fn is_recording_is_false_when_the_client_never_answers() {
        let (bridge, _responder, _emitted) = test_bridge();
        assert!(!bridge.is_recording().await);
    }

    /// Ruling 3, half one: nobody is listening.
    #[tokio::test]
    async fn every_trait_reports_its_documented_error_when_no_client_is_connected() {
        let (bridge, _emitted) = disconnected_bridge();

        let stt = bridge.transcribe(SttOpts::default()).await.unwrap_err();
        assert!(
            matches!(stt, SttError::Unavailable),
            "STT reports Unavailable when nobody is listening, got {stt:?}"
        );

        let tts = bridge
            .synthesize(TtsOpts {
                text: "hi".to_string(),
                voice: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(tts, TtsError::Unavailable),
            "TTS reports Unavailable when nobody is listening, got {tts:?}"
        );

        let voice = bridge.stop_recording().await.unwrap_err();
        match &voice {
            VoiceError::Other(message) => assert!(
                message.contains("no desktop client"),
                "the voice error must name the missing client, got {message}"
            ),
            other => panic!("voice reports Other when nobody is listening, got {other:?}"),
        }
    }

    /// Ruling 3, half two: asked, but no answer came.
    #[tokio::test(start_paused = true)]
    async fn every_trait_reports_its_documented_error_when_the_client_never_answers() {
        let (bridge, _responder, _emitted) = test_bridge();

        let stt = bridge.transcribe(SttOpts::default()).await.unwrap_err();
        match &stt {
            SttError::Retriable(message) => assert!(
                message.contains("did not answer"),
                "the STT error must say no answer came, got {message}"
            ),
            other => panic!("STT reports Retriable on a silent client, got {other:?}"),
        }

        let tts = bridge
            .synthesize(TtsOpts {
                text: "hi".to_string(),
                voice: None,
            })
            .await
            .unwrap_err();
        match &tts {
            TtsError::Other(message) => assert!(
                message.contains("did not answer"),
                "the TTS error must say no answer came, got {message}"
            ),
            other => panic!("TTS reports Other on a silent client, got {other:?}"),
        }

        let voice = bridge.stop_recording().await.unwrap_err();
        match &voice {
            VoiceError::Other(message) => assert!(
                message.contains("did not answer"),
                "the voice error must say no answer came, got {message}"
            ),
            other => panic!("voice reports Other on a silent client, got {other:?}"),
        }
    }

    /// A `speak` whose duration the CALLER chose must not be abandoned while
    /// the client is still speaking it.
    ///
    /// `speech {action:"speak", text: <~600 words>}` — "read this document
    /// aloud" — is an ordinary request. The desktop client answers only when
    /// the whole utterance has finished playing (`utterance.onend`), and at the
    /// slowest supported rate that text takes minutes. A flat deadline reports
    /// a failure to the model while the machine is audibly still talking, and a
    /// retry then stacks a second utterance on top of the first.
    #[tokio::test(start_paused = true)]
    async fn a_long_speak_is_not_abandoned_while_the_client_is_still_speaking() {
        let (bridge, responder, mut emitted) = test_bridge();
        // 3000 characters: ~600 words, roughly one page read aloud.
        let text = "word ".repeat(600);
        let spoken_for = Duration::from_millis(
            u64::try_from(text.chars().count()).unwrap() * SYNTHESIS_MILLIS_PER_CHAR,
        );

        let task = tokio::spawn({
            let bridge = bridge.clone();
            let text = text.clone();
            async move { bridge.synthesize(TtsOpts { text, voice: None }).await }
        });
        let (request_id, _) = next_request(&mut emitted).await;

        // The client is speaking the whole time, then answers correctly.
        tokio::time::sleep(spoken_for).await;
        assert!(
            responder
                .resolve(
                    request_id,
                    AudioResultDto::Audio {
                        pcm_base64: String::new(),
                        sample_rate_hz: 0,
                    },
                )
                .await,
            "the engine gave up on a client that was answering correctly, just slowly: \
             the deadline for a {}-character utterance must cover the time it takes to speak it",
            text.chars().count()
        );
        assert!(
            task.await.unwrap().is_ok(),
            "a completed utterance must be a success, not a reported failure"
        );
    }

    /// The deadline is a function of the text, and bounded whatever the text.
    #[test]
    fn the_synthesis_deadline_grows_with_the_text_and_stops_growing() {
        let short = synthesis_deadline("hi");
        let long = synthesis_deadline(&"word ".repeat(600));
        assert!(
            short < long,
            "a deadline that does not depend on the text is the wrong instrument for an \
             operation whose duration the caller chose"
        );
        assert!(
            short >= Duration::from_secs(SYNTHESIS_START_ALLOWANCE_SECS),
            "even an empty text needs room to acquire the audio session"
        );
        assert_eq!(
            synthesis_deadline(&"x".repeat(10_000_000)),
            Duration::from_secs(MAX_SYNTHESIS_DEADLINE_SECS),
            "an unbounded derivation would let one call park a turn for hours"
        );
    }

    /// Counted in Unicode scalar values, matching the renderer's own
    /// `[...text].length` — see `spokenTextTimeoutMs` in `synthesis.ts`.
    #[test]
    fn the_synthesis_deadline_counts_characters_not_bytes() {
        assert_eq!(synthesis_deadline("aaaa"), synthesis_deadline("你好世界"));
    }

    /// A disconnect drains every parked request instead of leaving the caller
    /// to wait out the deadline.
    #[tokio::test]
    async fn drain_fails_every_parked_request_immediately() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.transcribe(SttOpts::default()).await }
        });
        let (_request_id, _) = next_request(&mut emitted).await;
        assert_eq!(responder.drain().await, 1);

        let error = task.await.unwrap().unwrap_err();
        match &error {
            SttError::Retriable(message) => assert!(
                message.contains("disconnected"),
                "a drained request must name the disconnect, got {message}"
            ),
            other => panic!("a drained request is Retriable, got {other:?}"),
        }
    }

    /// Resolving an unknown / already-resolved id is a safe no-op, mirroring
    /// `BridgeComputerAccessBroker::resolve`.
    #[tokio::test]
    async fn resolving_an_unknown_id_is_a_noop() {
        let (_bridge, responder, _emitted) = test_bridge();
        assert!(
            !responder
                .resolve(9_999, AudioResultDto::RecordingState { recording: true })
                .await
        );
    }

    #[tokio::test]
    async fn concurrent_requests_get_distinct_ids_and_resolve_independently() {
        let (bridge, responder, mut emitted) = test_bridge();
        let first = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.stop_recording().await }
        });
        let (first_id, _) = next_request(&mut emitted).await;
        let second = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.transcribe(SttOpts::default()).await }
        });
        let (second_id, _) = next_request(&mut emitted).await;
        assert_ne!(first_id, second_id);

        responder
            .resolve(
                second_id,
                AudioResultDto::Transcript {
                    text: "second".to_string(),
                    language: None,
                    confidence: None,
                },
            )
            .await;
        assert_eq!(second.await.unwrap().unwrap().text, "second");

        responder
            .resolve(
                first_id,
                AudioResultDto::Recording {
                    audio_base64: String::new(),
                    mime_type: "audio/wav".to_string(),
                },
            )
            .await;
        assert!(first.await.unwrap().unwrap().audio_bytes.is_empty());
    }

    /// A client answering with a result that does not fit the op is an error,
    /// never a silently-defaulted success.
    #[tokio::test]
    async fn a_mismatched_result_variant_is_an_error() {
        let (bridge, responder, mut emitted) = test_bridge();
        let task = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.transcribe(SttOpts::default()).await }
        });
        let (request_id, _) = next_request(&mut emitted).await;
        responder
            .resolve(
                request_id,
                AudioResultDto::RecordingState { recording: true },
            )
            .await;
        let error = task.await.unwrap().unwrap_err();
        match &error {
            SttError::Other(message) => assert!(
                message.contains("did not answer the requested operation"),
                "a mismatched answer must say so, got {message}"
            ),
            other => panic!("a mismatched answer is Other, got {other:?}"),
        }
    }
}
