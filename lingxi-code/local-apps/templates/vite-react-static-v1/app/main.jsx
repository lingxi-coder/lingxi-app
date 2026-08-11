import React from "react";
import { createRoot } from "react-dom/client";
import { useEffect, useState } from "react";
import { getLingXiBridge } from "../lib/lingxi-bridge";
import {
  getPlatformAdapter,
  platformNavigationItems,
  platformStyle,
} from "../lib/platform-adapter";
import "./globals.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("LingXi local app root element is missing");
}

function AppShell() {
  const [bridgeReady, setBridgeReady] = useState(false);
  const adapter = getPlatformAdapter();

  useEffect(() => {
    setBridgeReady(getLingXiBridge() !== null);
  }, []);

  return (
    <main
      className="app-shell"
      data-platform={adapter.key}
      data-navigation={adapter.navigation}
      style={platformStyle(adapter)}
    >
      <div className="app-frame">
        <nav
          className="platform-navigation"
          aria-label="主要导航"
          data-placement={adapter.navigationPlacement}
          data-state-layer={adapter.stateLayer}
        >
          {platformNavigationItems.map((item, index) => (
            <button
              key={item.id}
              type="button"
              className="platform-navigation-item"
              aria-current={index === 0 ? "page" : undefined}
            >
              <span aria-hidden="true" className="platform-navigation-glyph">
                {index === 0 ? "⌂" : index === 1 ? "◌" : index === 2 ? "◇" : "⚙"}
              </span>
              <span>{item.label}</span>
            </button>
          ))}
        </nav>
        <section className="app-main" aria-labelledby="app-title">
          <section className="app-card">
            <p className="eyebrow">LingXi Local App</p>
            <h1 id="app-title">准备开始设计</h1>
            <p className="status" data-ready={bridgeReady} role="status">
              {bridgeReady ? "本地数据桥已连接。" : "正在等待宿主连接本地数据桥…"}
            </p>
            <p className="subtle-copy" data-form-factor={adapter.context.formFactor}>
              {adapter.context.os}:{adapter.context.formFactor} · {adapter.navigation}
            </p>
          </section>
        </section>
      </div>
    </main>
  );
}

createRoot(root).render(<React.StrictMode><AppShell /></React.StrictMode>);
