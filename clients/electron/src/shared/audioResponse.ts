/**
 * Payload bounds for `ClientCommand::AudioResponse`, declared ONCE.
 *
 * Two processes need the same numbers for opposite reasons, which is exactly
 * why they cannot each keep their own copy:
 *
 * - `main/validation.ts` ENFORCES them. A response that fails its gate never
 *   reaches the engine at all.
 * - `renderer/audio/requests.ts` must STAY INSIDE them. The engine parks the
 *   originating call on a deadline (5s / 30s / 180s per op, see
 *   `audio_bridge.rs`), so a response rejected at the gate is not a validation
 *   error the user sees — it is a silent stall of exactly that length,
 *   followed by a failure with no explanation. The renderer therefore checks
 *   these bounds itself and answers with a real, honest failure instead.
 *
 * Two copies of these numbers would drift the same way `AllowedClientCommand`
 * already did before it was consolidated into `clientCommands.ts`, and the
 * symptom of the drift would be that stall — the hardest possible thing to
 * attribute back to a mismatched constant. Hence `shared/`, which both the
 * main process and the renderer can reach.
 */

/**
 * Bound on a base64 audio payload. Generous by design: a `stop_recording`
 * answer carries a whole clip, whose length the USER (not this process)
 * chooses by how long they hold the microphone. 24 MiB of base64 is ~18 MiB
 * of Opus — hours of speech — while still bounding an IPC frame.
 *
 * Raising it is not the fix for a clip that exceeds it: the renderer reports
 * an oversize clip as a failure, which the engine surfaces immediately.
 * Genuinely unbounded audio needs a chunked wire format, which is a protocol
 * change rather than a bigger number here.
 */
export const MAX_AUDIO_BASE64_LENGTH = 24 * 1024 * 1024;

/** Bound on `AudioResultDto::Failed`'s message. The renderer trims to fit. */
export const MAX_AUDIO_FAILURE_MESSAGE_LENGTH = 4096;

/** Bound on a recording's reported mime type (`audio/webm;codecs=opus` and friends). */
export const MAX_AUDIO_MIME_TYPE_LENGTH = 256;

/** Highest plausible PCM sample rate. `0` is legal — it is half of the "played in place" pair. */
export const MAX_AUDIO_SAMPLE_RATE_HZ = 768_000;
