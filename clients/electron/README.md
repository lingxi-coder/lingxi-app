# LingXi Code — Desktop

A faithful, production-structured recreation of the LingXi Code desktop design
prototype, built with **Electron + Vite + React 18 + TypeScript**.

This is the UI shell only: it renders the prototype's mock data. There is no
backend/engine wiring (that is a later milestone).

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
  main/index.ts        Electron main process (frameless macOS-style window)
  preload/index.ts     contextBridge surface (minimal; no backend yet)
  renderer/
    main.tsx           React entry
    App.tsx            Root: window chrome, layout, top-level state
    global.css         Reset, keyframes, scrollbar, .mono
    theme/             tokens(dark/light) + Theme context
    data/              All mock data (PROJECTS, RUN, FILES_CHANGED, MODELS, …)
    components/
      Icon.tsx         SVG icon set
      primitives.tsx   Kbd, ModeTabs, account/menu icons, iconBtn
      Sidebar.tsx      Chat/Cowork/Code tabs, project tree, account menu
      TopBar.tsx       Repo breadcrumb, branch chip, diff pill, theme toggle
      Stage.tsx        Agent-run scrollback (narration, agent cards, audio)
      Composer.tsx     Prompt input, slash menu, mic recording + waveform
      pickers.tsx      Permission / Model+Effort+Fast / Context donut popovers
      RightPanel.tsx   Diff / Plan / Tasks / Shell tabs
      BackgroundTasks.tsx  Running/finished task list + transcript view
      settings/        Full multi-page Settings (nav + pages)
```

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
