import { resolve } from 'path';
import { defineConfig, externalizeDepsPlugin } from 'electron-vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  main: {
    // Keep node-side dependencies (`@lingxi/bridge-client`, its `ws` transitive
    // dep) external so they are resolved from node_modules at runtime rather than
    // bundled — `ws` reaches for optional native addons and node built-ins.
    plugins: [externalizeDepsPlugin()],
    build: {
      rollupOptions: {
        input: { index: resolve(__dirname, 'src/main/index.ts') },
      },
    },
  },
  preload: {
    // Sandboxed Electron preload scripts can only require Electron's small
    // built-in allowlist. Bundle the protocol constants reached through the
    // native-audio validators instead of leaving a package require behind.
    plugins: [externalizeDepsPlugin({ exclude: ['@lingxi/bridge-client'] })],
    build: {
      rollupOptions: {
        input: {
          index: resolve(__dirname, 'src/preload/index.ts'),
          // Inline visualization guests: a minimal bridge for the shell frame.
          visualization: resolve(__dirname, 'src/preload/visualization.ts'),
          localApp: resolve(__dirname, 'src/preload/localApp.ts'),
          // The hidden WebRTC window of Remote Control: hands its page one MessagePort.
          remoteRtc: resolve(__dirname, 'src/preload/remoteRtc.ts'),
        },
        output: {
          format: 'cjs',
          entryFileNames: '[name].cjs',
        },
      },
    },
  },
  renderer: {
    root: resolve(__dirname, 'src/renderer'),
    build: {
      rollupOptions: {
        input: {
          index: resolve(__dirname, 'src/renderer/index.html'),
          // Hidden page of Remote Control (WebRTC); never shown.
          remoteRtc: resolve(__dirname, 'src/renderer/remote-rtc.html'),
        },
      },
    },
    resolve: {
      dedupe: ['react', 'react-dom'],
      alias: {
        '@renderer': resolve(__dirname, 'src/renderer'),
      },
    },
    plugins: [react()],
  },
});
