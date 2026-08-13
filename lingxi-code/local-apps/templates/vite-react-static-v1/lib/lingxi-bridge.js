const BRIDGE_VERSION = "v1";

export function getLingXiBridge() {
  if (typeof window === "undefined") {
    return null;
  }

  const bridge = window.lingxi?.[BRIDGE_VERSION];
  return bridge && typeof bridge === "object" ? bridge : null;
}

export const FALLBACK_DEVICE_CONTEXT = {
  os: "unknown",
  formFactor: "unknown",
  viewport: { width: 0, height: 0 },
  safeArea: { top: 0, right: 0, bottom: 0, left: 0 },
  colorScheme: "light",
  reducedMotion: false,
  inputMode: "touch",
};

function nonNegativeNumber(value) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : 0;
}

export function normalizeDeviceContext(value) {
  if (!value || typeof value !== "object") return FALLBACK_DEVICE_CONTEXT;
  return {
    os: typeof value.os === "string" ? value.os : "unknown",
    formFactor:
      typeof value.formFactor === "string" ? value.formFactor : "unknown",
    viewport: {
      width: nonNegativeNumber(value.viewport?.width),
      height: nonNegativeNumber(value.viewport?.height),
    },
    safeArea: {
      top: nonNegativeNumber(value.safeArea?.top),
      right: nonNegativeNumber(value.safeArea?.right),
      bottom: nonNegativeNumber(value.safeArea?.bottom),
      left: nonNegativeNumber(value.safeArea?.left),
    },
    colorScheme: value.colorScheme === "dark" ? "dark" : "light",
    reducedMotion: value.reducedMotion === true,
    inputMode: value.inputMode === "pointer" ? "pointer" : "touch",
  };
}

export function getDeviceContext() {
  const bridge = getLingXiBridge();
  return normalizeDeviceContext(
    bridge?.deviceContext ?? bridge?.runtime?.deviceContext,
  );
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

function device(name) {
  const bridge = getLingXiBridge();
  const call = bridge?.device?.[name];
  if (!call) {
    throw new Error(`LingXi device bridge is unavailable (${name})`);
  }
  return call;
}

export async function capturePhoto(options = {}) {
  return device("capturePhoto")(options);
}

export async function pickImage(options = {}) {
  return device("pickImage")(options);
}

export async function startRecording(options = {}) {
  return device("recordAudioStart")(options);
}

export async function stopRecording() {
  return device("recordAudioStop")();
}

export async function getCurrentLocation() {
  return device("getLocation")();
}

export async function transcribeSpeech(options = {}) {
  return device("transcribeSpeech")(options);
}

export async function postNotification(request) {
  return device("postNotification")(request);
}

export function mediaObjectURL(media) {
  const binary = atob(media.base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return URL.createObjectURL(new Blob([bytes], { type: media.mimeType }));
}

export async function requestLlmChat(request) {
  const bridge = getLingXiBridge();
  if (!bridge?.llm?.chat) {
    throw new Error("LingXi AI bridge is unavailable");
  }
  return bridge.llm.chat(request);
}

export async function postAgentEvent(request) {
  const bridge = getLingXiBridge();
  if (!bridge?.agent?.post) {
    throw new Error("LingXi agent bridge is unavailable");
  }
  return bridge.agent.post(request);
}
