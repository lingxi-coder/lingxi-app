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
