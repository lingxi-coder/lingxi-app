import { createContext, useContext, useEffect, useMemo, useState } from "react";
import { getDeviceContext, getLingXiBridge } from "@/lib/lingxi-bridge";
import { getPlatformAdapter, platformStyle } from "@/lib/platform-adapter";

const LingXiBridgeContext = createContext(null);

function readSnapshot() {
  const bridge = getLingXiBridge();
  const device = getDeviceContext();
  const adapter = getPlatformAdapter(device);
  return { bridge, bridgeReady: bridge !== null, device, adapter };
}

/// The native `deviceContext` is LIVE — the host defines `viewport`,
/// `safeArea`, `colorScheme`, `reducedMotion` and `inputMode` as getters, so
/// reading it again always returns current values. But `getDeviceContext`
/// normalizes into a plain object, which snapshots those getters at call time.
/// Without this comparison-and-resubscribe the provider would freeze the very
/// first read: an iPad rotation, a Stage Manager / Split View resize, or a
/// dark-mode toggle would never reach the app.
function sameDevice(a, b) {
  return (
    a.os === b.os &&
    a.formFactor === b.formFactor &&
    a.colorScheme === b.colorScheme &&
    a.reducedMotion === b.reducedMotion &&
    a.inputMode === b.inputMode &&
    a.viewport.width === b.viewport.width &&
    a.viewport.height === b.viewport.height &&
    a.safeArea.top === b.safeArea.top &&
    a.safeArea.right === b.safeArea.right &&
    a.safeArea.bottom === b.safeArea.bottom &&
    a.safeArea.left === b.safeArea.left
  );
}

const MEDIA_QUERIES = [
  "(prefers-color-scheme: dark)",
  "(prefers-reduced-motion: reduce)",
  "(pointer: fine)",
];

export function LingXiBridgeProvider({ children }) {
  const [snapshot, setSnapshot] = useState(readSnapshot);

  useEffect(() => {
    if (snapshot.bridgeReady) return undefined;

    let frame = 0;
    let attempts = 0;
    const refresh = () => {
      const next = readSnapshot();
      if (next.bridgeReady || attempts >= 120) {
        setSnapshot(next);
        return;
      }
      attempts += 1;
      frame = window.requestAnimationFrame(refresh);
    };
    frame = window.requestAnimationFrame(refresh);
    return () => window.cancelAnimationFrame(frame);
  }, [snapshot.bridgeReady]);

  // Resubscribe once the bridge exists. Every source that can change a
  // deviceContext getter is watched: window resize covers rotation and iPad
  // multitasking, visualViewport covers the software keyboard and pinch-zoom
  // insets, and the media queries cover appearance, motion and input-mode.
  useEffect(() => {
    if (!snapshot.bridgeReady) return undefined;

    const resync = () => {
      setSnapshot((current) => {
        const next = readSnapshot();
        return sameDevice(current.device, next.device) ? current : next;
      });
    };

    window.addEventListener("resize", resync);
    window.addEventListener("orientationchange", resync);
    const viewport = window.visualViewport ?? null;
    viewport?.addEventListener("resize", resync);

    const lists = MEDIA_QUERIES.map((query) => window.matchMedia?.(query)).filter(
      Boolean,
    );
    for (const list of lists) list.addEventListener("change", resync);

    // The first paint may land before the host has applied safe-area insets,
    // so take one catch-up read instead of trusting the mount-time snapshot.
    resync();

    return () => {
      window.removeEventListener("resize", resync);
      window.removeEventListener("orientationchange", resync);
      viewport?.removeEventListener("resize", resync);
      for (const list of lists) list.removeEventListener("change", resync);
    };
  }, [snapshot.bridgeReady]);

  const value = useMemo(
    () => ({ ...snapshot, platformStyle: platformStyle(snapshot.adapter) }),
    [snapshot],
  );

  return (
    <LingXiBridgeContext.Provider value={value}>
      {children}
    </LingXiBridgeContext.Provider>
  );
}

export function useLingXi() {
  const value = useContext(LingXiBridgeContext);
  if (!value) throw new Error("useLingXi must be used inside LingXiBridgeProvider");
  return value;
}
