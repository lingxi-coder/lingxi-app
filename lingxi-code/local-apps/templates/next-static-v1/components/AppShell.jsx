"use client";

import { useEffect, useState } from "react";
import { getLingXiBridge } from "../lib/lingxi-bridge";

export function AppShell() {
  const [bridgeReady, setBridgeReady] = useState(false);

  useEffect(() => {
    setBridgeReady(getLingXiBridge() !== null);
  }, []);

  return (
    <main className="app-shell">
      <section className="app-card" aria-labelledby="app-title">
        <p className="eyebrow">LingXi Local App</p>
        <h1 id="app-title">准备开始设计</h1>
        <p className="status" data-ready={bridgeReady} role="status">
          {bridgeReady
            ? "本地数据桥已连接。"
            : "正在等待宿主连接本地数据桥…"}
        </p>
      </section>
    </main>
  );
}
