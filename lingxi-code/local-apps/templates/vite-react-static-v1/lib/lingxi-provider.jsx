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
