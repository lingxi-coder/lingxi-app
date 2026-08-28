import type { ClientCommand } from '@lingxi/bridge-client';

/**
 * The bounded, Desktop-facing command surface the main process accepts from
 * the renderer. Both the preload bridge (`src/preload/index.ts`) and the
 * renderer's ambient types (`src/renderer/bridge/lingxi.d.ts`) import this
 * single definition — it used to be declared twice and the two copies had
 * already drifted (preload was missing three command types the renderer
 * copy had).
 */
export type AllowedClientCommand =
  | Extract<ClientCommand, {
      type: 'set_model' | 'list_models' | 'new_session' | 'resume_session' | 'list_sessions' |
        'task_list' | 'task_output' | 'task_stop' | 'set_permission_mode' | 'run_slash_command' |
        'get_conversation_controls' | 'set_reasoning_selection' | 'set_fast_mode' |
        'update_settings' | 'update_permission_rules' | 'set_default_permission_mode' |
        'update_workspace_directories' | 'upsert_mcp_server' | 'remove_mcp_server';
    }>
  | {
      type: 'refresh_listings';
      which: Array<{ type: 'status' | 'doctor' | 'slash_commands' | 'settings' | 'mcp' | 'skills' }>;
    };
