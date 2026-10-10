/**
 * Inline visualizations: constants shared by the main process, the guest
 * preload and the renderer.
 *
 * Each widget renders in a dedicated `<webview>` on an in-memory partition.
 * Its main frame is the trusted shell served from {@link VISUALIZATION_ORIGIN};
 * the author fragment runs one level deeper in a sandboxed, opaque-origin
 * frame. Every byte comes from the engine (`visualization` bridge requests).
 */

/** Privileged scheme registered before `app.ready`. */
export const VISUALIZATION_SCHEME = 'lingxi-viz';

/** The only origin a visualization guest may load. */
export const VISUALIZATION_ORIGIN = `${VISUALIZATION_SCHEME}://visualization`;

/** The trusted shell page every guest starts on. */
export const VISUALIZATION_SHELL_URL = `${VISUALIZATION_ORIGIN}/shell.html`;

/** In-memory partition (no `persist:` prefix): nothing survives a restart. */
export const VISUALIZATION_PARTITION = 'lingxi-visualization';

/** Channel between the guest preload and the embedding renderer. */
export const VISUALIZATION_GUEST_CHANNEL = 'lingxi-visualization';

/** Renderer → main: authorize a mount for a session's widget. */
export const CH_VISUALIZATION_MOUNT = 'lingxi:visualization:mount';
/** Renderer → main: compare-and-swap a widget state write. */
export const CH_VISUALIZATION_WRITE_STATE = 'lingxi:visualization:writeState';
/** Renderer → main: retire a mount. */
export const CH_VISUALIZATION_UNMOUNT = 'lingxi:visualization:unmount';
/** Main → renderer: a guest crashed or asked to close (`{ webContentsId, reason }`). */
export const CH_VISUALIZATION_GUEST_EVENT = 'lingxi:visualization:guestEvent';

/** Largest single shell message, in UTF-16 code units. */
export const MAX_GUEST_MESSAGE_CHARS = 64 * 1024;

/** Concurrently mounted widgets before the least recently visible is suspended. */
export const MAX_ACTIVE_VISUALIZATIONS = 4;

/** Inline height bounds of a widget, in CSS pixels. */
export const VISUALIZATION_MIN_HEIGHT = 32;
export const VISUALIZATION_MAX_INLINE_HEIGHT = 640;

export interface VisualizationReference {
  readonly id: string;
  readonly revision: number;
}

export interface VisualizationTheme {
  readonly dark: boolean;
  readonly tokens: Readonly<Record<string, string>>;
}

export interface VisualizationMount {
  readonly token: string;
  readonly generation: number;
  readonly docUrl: string;
  readonly title: string;
}

export interface VisualizationStateWrite {
  readonly saved: boolean;
  readonly version: number;
  readonly reason?: string;
  readonly currentState?: unknown;
}

/** Guest-side events main forwards to the renderer that embeds the guest. */
export interface VisualizationGuestEvent {
  readonly webContentsId: number;
  readonly reason: 'crashed' | 'escape';
}

const ID = /^[A-Za-z0-9_-]{1,64}$/;

/** Validate a reference received over IPC. */
export function parseVisualizationReference(value: unknown): VisualizationReference {
  if (typeof value !== 'object' || value === null) throw new Error('invalid visualization reference');
  const { id, revision } = value as Record<string, unknown>;
  if (typeof id !== 'string' || !ID.test(id)) throw new Error('invalid visualization id');
  if (!Number.isSafeInteger(revision) || (revision as number) < 1) throw new Error('invalid visualization revision');
  return { id, revision: revision as number };
}

const TOKEN_NAME = /^--[a-z0-9-]{1,64}$/;
const TOKEN_VALUE = /^[A-Za-z0-9 #%(),./-]{1,64}$/;

/** Validate a theme received over IPC; unknown-shaped tokens are dropped. */
export function parseVisualizationTheme(value: unknown): VisualizationTheme {
  if (typeof value !== 'object' || value === null) throw new Error('invalid visualization theme');
  const { dark, tokens } = value as Record<string, unknown>;
  if (typeof dark !== 'boolean') throw new Error('invalid visualization theme');
  const clean: Record<string, string> = {};
  if (typeof tokens === 'object' && tokens !== null) {
    for (const [name, token] of Object.entries(tokens as Record<string, unknown>)) {
      if (TOKEN_NAME.test(name) && typeof token === 'string' && TOKEN_VALUE.test(token)) clean[name] = token;
    }
  }
  return { dark, tokens: clean };
}
