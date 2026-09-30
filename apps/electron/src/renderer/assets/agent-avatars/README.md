# Agent avatars

28 light/dark SVG pairs from the locally installed Codex webview bundled in
ChatGPT.app (extracted 2026-09-11). Source: `app.asar/webview/assets/`, referenced
by `app-primary-defe25a79fce.js` (avatar component `_Yt`, palette `xYt`).
Some source SVGs were inline data URLs; others were standalone assets.
SVG content is preserved, with filenames normalized to the original palette order.

`AgentAvatar.tsx` uses the same ID hash (base 31, modulo 2147483647, then 28).
Keep the palette order stable so existing agents retain their visual identity.
Both theme variants belong to the same identity; status is displayed separately.
