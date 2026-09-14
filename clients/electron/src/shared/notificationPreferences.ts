/**
 * Desktop OS-notification preferences.
 *
 * The VALUE vocabulary is Claude Code 2.1.270's, not ours: the notification
 * taxonomy (`idle_prompt`, `permission_prompt`, `agent_needs_input`,
 * `agent_completed`), the two timing constants, and the two boolean gates all
 * come from the oracle, verified against the shipped binary rather than
 * inferred:
 *
 *   - `RJe = 6000` — a permission prompt must stay unanswered this long
 *     before its notification fires; answering inside the window fires
 *     nothing at all (upstream arms a `setTimeout` and `clearTimeout`s it in
 *     a `finally`).
 *   - `DEFAULT_GLOBAL_CONFIG.messageIdleNotifThresholdMs = 60000` — how long
 *     a session must sit idle AFTER a turn ends before "waiting for your
 *     input" fires.
 *   - `inputNeededNotifEnabled` / `taskCompleteNotifEnabled` — upstream's own
 *     setting keys, kept verbatim so the two mobile clients and this one
 *     cannot drift into three vocabularies for one concept.
 *
 * What is DELIBERATELY NOT mirrored:
 *
 *   - `preferredNotifChannel` (auto / iterm2 / kitty / ghostty /
 *     terminal_bell / iterm2_with_bell / notifications_disabled). Every value
 *     but the last names a TERMINAL escape sequence, which a GUI window has
 *     no analogue for. `enabled` here is the GUI equivalent of the one
 *     meaningful distinction, `notifications_disabled` vs everything else.
 *   - `agentPushNotifEnabled`. It gates the `PushNotification` tool, which is
 *     registered-but-disabled in this port (`tools/ui/src/push_notification.rs`
 *     — its flag has no live backend), so the toggle could never do anything.
 *   - A scheduled-run kind. The desktop has no scheduled-task subsystem at
 *     this commit, so a toggle for it would be exactly the decorative setting
 *     this module replaces on mobile. Add it with its producer, not before.
 *   - The default of `inputNeededNotifEnabled`. Upstream defaults it OFF
 *     because it gates a REMOTE push that costs a round trip to a phone;
 *     these are local notifications on the machine the user is already at, so
 *     the whole set defaults ON.
 *
 * Lives in `shared/` for the same reason `voicePreferences.ts` does: the main
 * process persists it, the preload bridge hands it across, and the renderer
 * renders it — three independent declarations is how
 * `bypassPermissionsModeAccepted` drifted.
 */

/** Schema version of a persisted `NotificationPreferences` value. */
export const NOTIFICATION_SCHEMA_VERSION = 1 as const;

/**
 * `messageIdleNotifThresholdMs` default, byte-faithful to upstream's
 * `DEFAULT_GLOBAL_CONFIG` (and to the port's own CLI copy of it,
 * `lingxi-code/apps/cli/src/idle_notify.rs` `MESSAGE_IDLE_NOTIF_THRESHOLD_MS`).
 */
export const DEFAULT_IDLE_NOTIF_THRESHOLD_MS = 60_000;

/**
 * How long a permission prompt stays unanswered before its notification
 * fires. Upstream `RJe = 6000`; the port's CLI copy is
 * `permission_prompt_notify.rs` `PERMISSION_PROMPT_NOTIFY_DELAY_MS`.
 * Not user-configurable upstream, so it is a constant here too.
 */
export const PERMISSION_PROMPT_NOTIFY_DELAY_MS = 6_000;

/** Clamp bounds for the idle threshold. 5s floor keeps a mistyped `1` from
 * turning every finished turn into an instant banner; 1h ceiling keeps a
 * mistyped value from silently disabling the notification instead of saying
 * so through `enabled`. */
const MIN_IDLE_THRESHOLD_MS = 5_000;
const MAX_IDLE_THRESHOLD_MS = 3_600_000;

export interface NotificationPreferences {
  schemaVersion: typeof NOTIFICATION_SCHEMA_VERSION;
  /** Master switch. GUI equivalent of `preferredNotifChannel !== 'notifications_disabled'`. */
  enabled: boolean;
  /** Gates `idle_prompt`. Upstream has no per-type toggle for this one (the
   * channel setting is its only gate); a GUI user who wants permission
   * prompts but not idle pings has no other way to say so. */
  idlePromptNotifEnabled: boolean;
  /** Upstream key. Gates `permission_prompt` and `agent_needs_input`
   * ("Push when actions required"). */
  inputNeededNotifEnabled: boolean;
  /** Upstream key. Gates `agent_completed`. */
  taskCompleteNotifEnabled: boolean;
  /** Upstream key `messageIdleNotifThresholdMs`. */
  messageIdleNotifThresholdMs: number;
}

export function defaultNotificationPreferences(): NotificationPreferences {
  return {
    schemaVersion: NOTIFICATION_SCHEMA_VERSION,
    enabled: true,
    idlePromptNotifEnabled: true,
    inputNeededNotifEnabled: true,
    taskCompleteNotifEnabled: true,
    messageIdleNotifThresholdMs: DEFAULT_IDLE_NOTIF_THRESHOLD_MS,
  };
}

export function normalizeIdleThresholdMs(raw: unknown): number {
  if (typeof raw !== 'number' || !Number.isFinite(raw)) return DEFAULT_IDLE_NOTIF_THRESHOLD_MS;
  return Math.min(MAX_IDLE_THRESHOLD_MS, Math.max(MIN_IDLE_THRESHOLD_MS, Math.round(raw)));
}

/**
 * Accepts anything and returns a complete value. A missing or malformed
 * field falls back to its default rather than disabling the notification it
 * gates — a corrupt settings file must not silently turn the feature off,
 * which is the failure mode a user cannot diagnose from the UI.
 */
export function parseNotificationPreferences(value: unknown): NotificationPreferences {
  const defaults = defaultNotificationPreferences();
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return defaults;
  const input = value as Record<string, unknown>;
  const bool = (key: keyof NotificationPreferences): boolean =>
    typeof input[key] === 'boolean' ? input[key] as boolean : defaults[key] as boolean;
  return {
    schemaVersion: NOTIFICATION_SCHEMA_VERSION,
    enabled: bool('enabled'),
    idlePromptNotifEnabled: bool('idlePromptNotifEnabled'),
    inputNeededNotifEnabled: bool('inputNeededNotifEnabled'),
    taskCompleteNotifEnabled: bool('taskCompleteNotifEnabled'),
    messageIdleNotifThresholdMs: normalizeIdleThresholdMs(input['messageIdleNotifThresholdMs']),
  };
}

/**
 * Upstream's `notificationType` discriminator, kept verbatim. The port's CLI
 * already uses these exact strings for the `Notification` hook
 * (`idle_notify.rs` `IDLE_PROMPT_NOTIFICATION_TYPE`,
 * `permission_prompt_notify.rs` `PERMISSION_PROMPT_NOTIFICATION_TYPE`), so a
 * third spelling here would be the drift this module exists to prevent.
 */
export type NotificationKind =
  | 'idle_prompt'
  | 'permission_prompt'
  | 'agent_needs_input'
  | 'agent_completed';

/** Which preference gates a given notification kind. */
export function isKindEnabled(prefs: NotificationPreferences, kind: NotificationKind): boolean {
  if (!prefs.enabled) return false;
  switch (kind) {
    case 'idle_prompt': return prefs.idlePromptNotifEnabled;
    case 'permission_prompt':
    case 'agent_needs_input': return prefs.inputNeededNotifEnabled;
    case 'agent_completed': return prefs.taskCompleteNotifEnabled;
  }
}
