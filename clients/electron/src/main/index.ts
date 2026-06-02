import { app, shell, BrowserWindow } from 'electron';
import { join } from 'path';
import { fileURLToPath } from 'url';
import { dirname } from 'path';

import { BridgeManager } from './bridge.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

/**
 * The single bridge manager for this app instance (M10 A1 — C2). It owns the
 * `bridge-server` child + the WS client + the renderer IPC seam. Created on
 * `whenReady`, disposed on quit.
 */
let bridge: BridgeManager | null = null;

function createWindow(): BrowserWindow {
  const mainWindow = new BrowserWindow({
    width: 1320,
    height: 860,
    minWidth: 900,
    minHeight: 600,
    show: false,
    backgroundColor: '#0c0b10',
    // The prototype renders its own macOS-style window chrome (traffic lights),
    // so we hide the native title bar but keep the window frameless.
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: -100, y: -100 },
    autoHideMenuBar: true,
    webPreferences: {
      preload: join(__dirname, '../preload/index.mjs'),
      sandbox: false,
      contextIsolation: true,
    },
  });

  // Route bridge events/permissions/state to this window's renderer.
  bridge?.registerWindow(mainWindow.webContents);

  mainWindow.on('ready-to-show', () => {
    mainWindow.show();
  });

  mainWindow.webContents.setWindowOpenHandler((details) => {
    void shell.openExternal(details.url);
    return { action: 'deny' };
  });

  // electron-vite injects ELECTRON_RENDERER_URL in dev for HMR.
  const rendererUrl = process.env['ELECTRON_RENDERER_URL'];
  if (rendererUrl) {
    void mainWindow.loadURL(rendererUrl);
  } else {
    void mainWindow.loadFile(join(__dirname, '../renderer/index.html'));
  }

  return mainWindow;
}

app.whenReady().then(() => {
  // Register the IPC handlers up front so the renderer can call them as soon as
  // it loads, even while the child is still spawning/connecting. start() runs in
  // the background; failures surface to the renderer as a `connectionState`
  // transition (the IPC handlers are registered synchronously inside start()).
  bridge = new BridgeManager();
  void bridge.start().catch((err: unknown) => {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`[bridge] start failed: ${message}`);
  });

  createWindow();

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on('window-all-closed', () => {
  bridge?.dispose();
  bridge = null;
  if (process.platform !== 'darwin') app.quit();
});

// Belt-and-suspenders: also reap the child on quit (covers the macOS path where
// the app stays alive after all windows close, then quits later).
app.on('will-quit', () => {
  bridge?.dispose();
  bridge = null;
});
