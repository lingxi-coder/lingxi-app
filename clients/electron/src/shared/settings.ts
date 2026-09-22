/**
 * The device-level (Electron store) settings shape, in ONE place.
 *
 * `PublicSettings` crosses all three processes — the main process persists
 * it (`main/host-utils.ts`), the preload bridge hands it over the context
 * bridge (`preload/index.ts`), and the renderer reads it off
 * `window.lingxi` (`renderer/bridge/lingxi.d.ts`) — and it used to be
 * declared independently in each of them. That is exactly the shape
 * `shared/clientCommands.ts` exists to prevent for the command surface, and
 * it drifted the same way: when `bypassPermissionsModeAccepted` was added
 * for the Permissions settings page, two of the three copies grew the field
 * and the preload one did not, so the value the main process really sends
 * was invisible to the type the renderer's own bridge declaration derives
 * from. Nothing broke, because both readers happened to go through the other
 * two copies — a third copy that silently disagrees is a bug waiting for its
 * first reader, not a harmless duplication.
 *
 * This module is TYPES plus the one version constant, with no imports
 * beyond the standard library and sibling `shared/` modules (never `main/`
 * or `renderer/`), so all three targets can take it without dragging a
 * process-specific dependency across a boundary.
 */

import type { VoicePreferences } from './voicePreferences.js';
export type { VoicePreferences } from './voicePreferences.js';
import type { NotificationPreferences } from './notificationPreferences.js';
export type { NotificationPreferences } from './notificationPreferences.js';

/** Schema version of the persisted device settings file. */
export const SETTINGS_VERSION = 1 as const;

/** A session identified by the project it belongs to. */
export interface SessionRef {
  projectPath: string;
  sessionId: string;
}

/** A session the user pinned, as persisted (the pin time is assigned by the main process). */
export interface PinnedSessionRecord {
  projectPath: string;
  sessionId: string;
  title: string;
  pinnedAt: string;
}

/** What a caller supplies to pin a session — `pinnedAt` is the main process's to assign. */
export type SessionPinInput = Omit<PinnedSessionRecord, 'pinnedAt'>;

export interface ProviderModelPickerVisibility {
  showInModelPicker?: boolean;
  visibleModelIds?: string[];
}

export type ModelPickerVisibilitySettings = Record<string, ProviderModelPickerVisibility>;

/** Preferences that govern how project chats are organized in the desktop sidebar. */
export interface SidebarPreferences {
  organization: 'project' | 'list';
  chatSort: 'priority' | 'updated' | 'manual';
  /** Session IDs in user-defined order, keyed by their canonical project path. */
  manualSessionOrder: Record<string, string[]>;
}

/**
 * The subset of the persisted device settings that leaves the main process.
 * Everything in `PersistedSettings` that is NOT here (today:
 * `trustedWorkspaces`) is deliberately withheld.
 */
export interface ArchivedSessionRecord extends SessionRef { title?: string; archivedAt?: string }

export interface PublicSettings {
  archivedSessions?: ArchivedSessionRecord[];
  version: typeof SETTINGS_VERSION;
  theme?: 'dark' | 'light' | 'system';
  collapseThoughtsByDefault?: boolean;
  model?: string;
  apiBaseUrl?: string;
  activeProject?: string;
  activeSession?: SessionRef;
  projects: string[];
  pinnedSessions: PinnedSessionRecord[];
  /**
   * Read-only surface of `PersistedSettings.bypassPermissionsModeAccepted`
   * (Task 18: the Permissions settings page renders this as a fixed `设备`
   * row so a person can see whether the one-time Bypass Permissions
   * acceptance has already happened, without exposing a way to SET it from
   * here — the only writer stays the blocking acceptance dialog
   * (`src/main/index.ts`). Omitted (not `false`) when unset, matching every
   * other optional field here.
   */
  bypassPermissionsModeAccepted?: boolean;
  /**
   * Voice recognition/synthesis preferences (Task 4: mirrors the VALUE
   * vocabulary of iOS's `VoicePreferencesSnapshot` and Android's
   * `VoiceConfig`, normalized by `shared/voicePreferences.ts`). Omitted
   * (not defaulted) when never written, matching every other optional
   * field here.
   */
  voice?: VoicePreferences;
  /**
   * OS-notification preferences (`shared/notificationPreferences.ts`). The
   * vocabulary is upstream Claude Code's (`inputNeededNotifEnabled`,
   * `taskCompleteNotifEnabled`, `messageIdleNotifThresholdMs`). Omitted (not
   * defaulted) when never written, matching every other optional field here —
   * readers take `defaultNotificationPreferences()` for the undefined case.
   */
  notifications?: NotificationPreferences;
  modelPickerVisibility?: ModelPickerVisibilitySettings;
  sidebar?: SidebarPreferences;
}
