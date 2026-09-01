/**
 * Renderer-facing entry point for the desktop voice preferences model.
 *
 * The canonical declaration lives in `shared/voicePreferences.ts`: it also
 * has to be reachable from the main process, which persists it as part of
 * `PersistedSettings` (`main/host-utils.ts`), so it cannot live under
 * `renderer/`. This module re-exports it so renderer-side audio code (the
 * voice settings page, playback) has its own import path under
 * `renderer/audio` without a second declaration of the type.
 */
export * from '../../shared/voicePreferences.js';
