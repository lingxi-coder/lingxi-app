const BRIDGE_VERSION = "v1";

export function getLingXiBridge() {
  if (typeof window === "undefined") {
    return null;
  }

  const bridge = window.lingxi?.[BRIDGE_VERSION];
  return bridge && typeof bridge === "object" ? bridge : null;
}

export async function queryCollection(request) {
  const bridge = getLingXiBridge();
  if (!bridge?.data?.query) {
    throw new Error("LingXi data bridge is unavailable");
  }
  return bridge.data.query(request);
}

export async function mutateCollection(request) {
  const bridge = getLingXiBridge();
  if (!bridge?.data?.mutate) {
    throw new Error("LingXi data bridge is unavailable");
  }
  return bridge.data.mutate(request);
}

export async function requestNetwork(request) {
  const bridge = getLingXiBridge();
  const send = bridge?.network?.fetch ?? bridge?.network?.request;
  if (!send) {
    throw new Error("LingXi network bridge is unavailable");
  }
  return send(request);
}

export async function requestRuntimeStatus(request = {}) {
  const bridge = getLingXiBridge();
  const inspect =
    bridge?.runtime?.info ??
    bridge?.runtime?.status ??
    bridge?.runtimeStatus;
  if (!inspect) {
    throw new Error("LingXi runtime bridge is unavailable");
  }
  return inspect(request);
}

// ---------------------------------------------------------------------------
// Device capabilities. Every one of these needs the matching capability in the
// app's confirmed plan; the first call raises a permission sheet the user
// answers. A rejection carries `error.code` (capability_not_declared,
// permission_denied, cancelled, audio_session_busy, …) so UI can branch
// without matching on message text.
// ---------------------------------------------------------------------------

function device(name) {
  const bridge = getLingXiBridge();
  const call = bridge?.device?.[name];
  if (!call) {
    throw new Error(`LingXi device bridge is unavailable (${name})`);
  }
  return call;
}

/** Take a photo. Needs the `camera` capability. */
export async function capturePhoto(options = {}) {
  return device("capturePhoto")(options);
}

/** Pick an image from the photo library. Needs `photo_library`. */
export async function pickImage(options = {}) {
  return device("pickImage")(options);
}

/** Start recording. Needs `microphone`. Always pair with stopRecording. */
export async function startRecording(options = {}) {
  return device("recordAudioStart")(options);
}

/** Stop recording and get the audio back. */
export async function stopRecording() {
  return device("recordAudioStop")();
}

/** Read the current location once. Needs `location`. */
export async function getCurrentLocation() {
  return device("getLocation")();
}

/**
 * Listen once and get back what was said. Needs `microphone`.
 *
 * This is how speech reaches the model: audio bytes cannot be sent to it,
 * so transcribe first and send the text.
 */
export async function transcribeSpeech(options = {}) {
  return device("transcribeSpeech")(options);
}

/** Post a local notification. Needs `notifications`. */
export async function postNotification(request) {
  return device("postNotification")(request);
}

/**
 * Turn a `{ base64, mimeType }` capture into an object URL you can put in
 * `<img src>` or `<audio src>`. Revoke it with `URL.revokeObjectURL` when the
 * element goes away.
 */
export function mediaObjectURL(media) {
  const binary = atob(media.base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return URL.createObjectURL(new Blob([bytes], { type: media.mimeType }));
}

// ---------------------------------------------------------------------------
// AI + assistant
// ---------------------------------------------------------------------------

/**
 * Ask the user's configured AI model. Needs the `llm` capability, and spends
 * the user's own model quota — call it when the user asked for something, not
 * on a timer.
 *
 * Attach a capture by its `mediaId` rather than its base64. Model input has a
 * bounded 8 MiB lane, but a handle avoids base64 expansion and repeated IPC
 * copies. Other bridge control operations remain capped at 64 KiB.
 *
 *   const shot = await capturePhoto();
 *   const { text } = await requestLlmChat({
 *     messages: [{ role: "user", content: [
 *       { type: "text", text: "这张图里是什么？" },
 *       { type: "image", mediaId: shot.mediaId },
 *     ] }],
 *   });
 *
 * The model is always the one the user currently has selected; an app cannot
 * choose it. Answers arrive whole (no streaming), so render a waiting state.
 */
export async function requestLlmChat(request) {
  const bridge = getLingXiBridge();
  if (!bridge?.llm?.chat) {
    throw new Error("LingXi AI bridge is unavailable");
  }
  return bridge.llm.chat(request);
}

/**
 * Tell the user's assistant something happened. Needs `agent_notify`.
 * Not real-time: the assistant collects these when it next looks. Send small
 * structured facts, never a data dump.
 */
export async function postAgentEvent(request) {
  const bridge = getLingXiBridge();
  if (!bridge?.agent?.post) {
    throw new Error("LingXi agent bridge is unavailable");
  }
  return bridge.agent.post(request);
}
