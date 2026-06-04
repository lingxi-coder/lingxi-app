# LingXi Code — Desktop

A faithful, production-structured recreation of the LingXi Code desktop design
prototype, built with **Electron + Vite + React 18 + TypeScript**.

The renderer is wired to the real engine over the bridge: the main process
spawns the Rust `bridge-server`, connects through its discovery lockfile with
the shared `@lingxi/bridge-client` SDK, streams `ClientEvent`s to the renderer,
and surfaces engine permission requests as an allow/deny prompt. When the
`bridge-server` binary is not present (no engine), the renderer falls back to the
prototype's mock data so the design preview still works in a plain browser. See
[`../README-bridge.md`](../README-bridge.md) for the end-to-end run path.

## Stack

- **electron-vite** — bundles the Electron `main`, `preload`, and `renderer`
  with Vite (HMR in dev).
- **React 18 + TypeScript** for the renderer.
- A React context + the `tokens(dark)` factory drives the dark/light theme.
  Colors are CSS `oklch(...)` values (Electron/Chromium supports oklch).
- Google Fonts: Inter, Noto Sans SC, JetBrains Mono.

## Project structure

```
src/
  main/
    index.ts           Electron main process (frameless macOS-style window)
    bridge.ts          BridgeManager: spawns bridge-server, connects the client,
                       wires the renderer IPC seam (resolves the binary path)
  preload/index.ts     contextBridge surface (window.lingxi: prompts, permissions,
                       event/state subscriptions)
  renderer/
    main.tsx           React entry
    App.tsx            Root: window chrome, layout, top-level state
    global.css         Reset, keyframes, scrollbar, .mono
    bridge/
      useBridge.ts     Live-conversation store: folds ClientEvents, queues
                       permission requests, exposes approve/deny
      conversation.ts  Pure ClientEvent → view-model reducer
      lingxi.d.ts      Ambient typing for window.lingxi
    theme/             tokens(dark/light) + Theme context
    data/              All mock data (PROJECTS, RUN, FILES_CHANGED, MODELS, …)
    components/
      Icon.tsx         SVG icon set
      primitives.tsx   Kbd, ModeTabs, account/menu icons, iconBtn
      Sidebar.tsx      Chat/Cowork/Code tabs, project tree, account menu
      TopBar.tsx       Repo breadcrumb, branch chip, diff pill, theme toggle
      Stage.tsx        Agent-run scrollback (narration, agent cards, audio)
      Composer.tsx     Prompt input, slash menu, mic recording + waveform
      PermissionPrompt.tsx  Allow-once / allow-always / deny modal for engine
                       permission requests
      pickers.tsx      Permission / Model+Effort+Fast / Context donut popovers
      RightPanel.tsx   Diff / Plan / Tasks / Shell tabs
      BackgroundTasks.tsx  Running/finished task list + transcript view
      settings/        Full multi-page Settings (nav + pages)
```

## Connecting to the engine

On launch the main process resolves the `bridge-server` binary in this order:

1. `BridgeManagerOptions.serverBin` (programmatic override), else
2. the `LINGXI_BRIDGE_SERVER_BIN` environment variable, else
3. a path derived **relative to the repo** — it walks up from the bundled main
   process to the first existing `lingxi-code/target/{debug,release}/bridge-server`.

If none resolve, the bridge surfaces a clear `error` connection state telling you
to build the binary (`cd lingxi-code && cargo build -p bridge-server --bin
bridge-server`) or set `LINGXI_BRIDGE_SERVER_BIN`. No absolute author paths are
baked in, so a fresh clone works as long as the binary is built or the env var is
set. The engine's `ANTHROPIC_API_KEY` / `LINGXI_API_BASE_URL` pass through from
the environment untouched.

## Scripts

```bash
npm install        # install dependencies

npm run dev        # launch Electron with Vite HMR
npm run build      # type-check-free production build (main + preload + renderer)
npm run typecheck  # tsc --noEmit for both node + web tsconfigs
```

`npm run build` and `npm run typecheck` are the verification gates and must both
pass. (Launching the GUI requires a display; building + typechecking is the
headless verification.)

## Notes

- The window is frameless; the prototype draws its own macOS traffic-light
  chrome inside the React tree.
- Dark/light theme toggles live in the top bar (sun/moon) and in
  Settings → General (system/light/dark tri-state).
- The microphone composer uses `getUserMedia` + Web Audio for the live
  waveform; if permission is denied the mic icon turns red.
