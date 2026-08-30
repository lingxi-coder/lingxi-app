import type { ClientCommand } from '@lingxi/bridge-client';

/**
 * The runtime-checkable source of truth for the bounded, Desktop-facing
 * command surface the main process accepts from the renderer. Both the
 * compile-time gate (`AllowedClientCommand` below) and the runtime gate
 * (`src/main/validation.ts`'s `ALLOWED_COMMANDS`) derive from this one array
 * — neither restates it — so the two gates cannot independently drift the
 * way they already had: this array used to list `new_session` and
 * `resume_session` as part of the allowed surface while the runtime gate had
 * never accepted either. That was latent only because the renderer creates
 * and resumes sessions through the dedicated `CH_SESSION_NEW` /
 * `CH_SESSION_OPEN` IPC channels, never through `command()` — verified by
 * grepping `src/renderer/` for both wire names before removing them here.
 * `validation.test.ts`'s exclusion test for both is the record of why they
 * stay out.
 */
export const ALLOWED_CLIENT_COMMAND_TYPES = [
  'set_model',
  'list_models',
  'list_sessions',
  'task_list',
  'task_output',
  'task_stop',
  'set_permission_mode',
  'run_slash_command',
  'get_conversation_controls',
  'set_reasoning_selection',
  'set_fast_mode',
  'update_settings',
  'update_permission_rules',
  'set_default_permission_mode',
  'update_workspace_directories',
  'upsert_mcp_server',
  'remove_mcp_server',
  // The renderer's answer to `ClientEvent::AudioRequest` — the client side of
  // `audio_bridge.rs`'s `AudioBridge`. Unlike every other entry here it is
  // never sent because a user clicked something: the engine PARKS a call on a
  // deadline waiting for it (5s / 30s / 180s per op), so leaving it off this
  // array is not "one fewer feature", it is every microphone and
  // text-to-speech call in the product stalling and then failing.
  'audio_response',
] as const;

/**
 * The listing kinds `refresh_listings` may request — its own bounded
 * sub-surface, shared the same way as `ALLOWED_CLIENT_COMMAND_TYPES` above.
 */
export const ALLOWED_REFRESH_LISTING_KINDS = [
  'status',
  'doctor',
  'slash_commands',
  'settings',
  'mcp',
  'skills',
] as const;

/**
 * The bounded, Desktop-facing command surface the main process accepts from
 * the renderer. Both the preload bridge (`src/preload/index.ts`) and the
 * renderer's ambient types (`src/renderer/bridge/lingxi.d.ts`) import this
 * single definition — it used to be declared twice and the two copies had
 * already drifted (preload was missing three command types the renderer
 * copy had).
 */
export type AllowedClientCommand =
  | Extract<ClientCommand, { type: (typeof ALLOWED_CLIENT_COMMAND_TYPES)[number] }>
  | {
      type: 'refresh_listings';
      which: Array<{ type: (typeof ALLOWED_REFRESH_LISTING_KINDS)[number] }>;
    };
