/**
 * What a viewer may do to a session, one method per RPC. Every parameter is
 * checked here (the viewer is another computer's code), every refusal is a
 * `RemoteRpcError`, and nothing reaches the runtime that the desktop's own UI
 * could not also send, with these restrictions on top:
 *
 * - a permission or question is answered once: whoever comes second finds it
 *   no longer pending (`not_pending`), whether the first was the desktop or
 *   another viewer;
 * - no viewer switches a session to bypassPermissions, or approves a request
 *   that would (`forbidden`), and "always allow" needs an explicit host
 *   setting;
 * - the only slash commands are those in `REMOTE_SLASH_COMMANDS`;
 * - computer access is not here at all: it is granted on the desktop.
 */

import { randomBytes } from 'node:crypto';

import type { ImageRefDto, PermissionModeId, PermissionResponseDto, ReasoningSelectionDto } from '@lingxi/bridge-client';
import {
  RemoteRpcError,
  type Clock,
  type ImageGetResult,
  type ModelsListResult,
  type PromptSendResult,
  type SessionHistoryResult,
} from '@lingxi/bridge-client/remote';

import { REMOTE_SLASH_COMMANDS } from '../../shared/clientCommands.js';
import { slashRequestEchoes } from '../bridgeEvents.js';
import { isPermissionModeId, sanitizeDiagnostic } from '../host-utils.js';
import { validateImageRefs, validatePrompt } from '../validation.js';
import type { RemoteViewer } from './gateway.js';
import type { RemoteRuntime } from './remoteRuntime.js';
import { historyPage, resolveImage } from './snapshot.js';

/** The renderer shows this as a user row, marked as sent from another device. */
export interface RemotePromptEcho {
  sessionId: string;
  turnId: number;
  text: string;
  images: ImageRefDto[];
  origin: 'device';
  deviceName: string;
}

/**
 * Decoding and scaling images, which Electron's `nativeImage` does in
 * production (`nativeImageCodec.ts`); tests supply a fake.
 */
export interface ImageCodec {
  /** The pixel size, or `undefined` if the bytes cannot be decoded. */
  size(bytes: Uint8Array): { width: number; height: number } | undefined;
  /** A copy scaled to the given size and encoded as asked; `undefined` if that is not possible. */
  scale(bytes: Uint8Array, width: number, height: number, encode: { type: 'png' } | { type: 'jpeg'; quality: number }): Uint8Array | undefined;
}

export interface RemoteCommandsDeps {
  /** Whether "always allow" may be given remotely. */
  allowAlwaysAllow(): boolean;
  /** Told after a viewer's prompt has been sent, so the renderer can show it. */
  echo(echo: RemotePromptEcho): void;
  codec?: ImageCodec;
  clock: Clock;
  /** A random integer in 1…2^53−1. */
  randomTurnId?: () => number;
}

export const MAX_PROMPT_IMAGES = 4;
export const IMAGE_GET_MAX_SIDE = 1568;
export const IMAGE_GET_MAX_BYTES = 2 * 1024 * 1024;
/** A retry of a prompt with the same `client_request_id` within this long gets the first answer. */
export const IDEMPOTENCY_WINDOW_MS = 5 * 60_000;
const IDEMPOTENCY_MAX_ENTRIES = 512;
const MAX_HISTORY_LIMIT = 200;
const MAX_ID_CHARS = 128;
const SLASH_COMMAND = /^\/[^\s/]+(?:\s|$)/;
const JPEG_QUALITIES = [85, 70, 55, 40] as const;

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json => typeof value === 'object' && value !== null && !Array.isArray(value);

function invalid(message: string): RemoteRpcError {
  return new RemoteRpcError('invalid_params', message);
}

function params(value: unknown): Json {
  if (!isObject(value)) throw invalid('params must be an object');
  return value;
}

function requestId(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw invalid('invalid request_id');
  return value;
}

function boundedString(value: unknown, name: string, max: number): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > max || value.includes('\0')) throw invalid(`invalid ${name}`);
  return value;
}

/** What a runtime refused, in words a viewer can show. */
function refused(error: unknown): RemoteRpcError {
  if (error instanceof RemoteRpcError) return error;
  return invalid(sanitizeDiagnostic(error).slice(0, 300));
}

/** A random 53-bit turn id, never 0. */
function randomTurnIdFromCrypto(): number {
  for (;;) {
    const bytes = randomBytes(8);
    const id = (bytes.readUInt32BE(0) & 0x1fffff) * 2 ** 32 + bytes.readUInt32BE(4);
    if (id > 0) return id;
  }
}

/** Fit an image to the size `image.get` allows, or say it cannot be done. */
export function fitImage(
  codec: ImageCodec | undefined,
  image: { media_type: string; base64: string },
): { media_type: string; base64: string } {
  const bytes = Buffer.from(image.base64, 'base64');
  const size = codec?.size(bytes);
  const longest = size === undefined ? 0 : Math.max(size.width, size.height);
  if (bytes.length <= IMAGE_GET_MAX_BYTES && (size === undefined || longest <= IMAGE_GET_MAX_SIDE)) return image;
  if (codec === undefined || size === undefined) throw new RemoteRpcError('internal', 'the image is too large to send');
  const factor = longest > IMAGE_GET_MAX_SIDE ? IMAGE_GET_MAX_SIDE / longest : 1;
  const width = Math.max(1, Math.round(size.width * factor));
  const height = Math.max(1, Math.round(size.height * factor));
  const attempts: Array<{ type: 'png' } | { type: 'jpeg'; quality: number }> = [
    ...(image.media_type === 'image/png' ? [{ type: 'png' } as const] : []),
    ...JPEG_QUALITIES.map((quality) => ({ type: 'jpeg', quality }) as const),
  ];
  for (const encode of attempts) {
    const scaled = codec.scale(bytes, width, height, encode);
    if (scaled !== undefined && scaled.length <= IMAGE_GET_MAX_BYTES) {
      return { media_type: encode.type === 'png' ? 'image/png' : 'image/jpeg', base64: Buffer.from(scaled).toString('base64') };
    }
  }
  throw new RemoteRpcError('internal', 'the image is too large to send');
}

interface SentPrompt {
  at: number;
  result: Promise<PromptSendResult>;
}

export class RemoteCommands {
  private readonly sent = new Map<string, SentPrompt>();
  private readonly randomTurnId: () => number;

  constructor(private readonly deps: RemoteCommandsDeps) {
    this.randomTurnId = deps.randomTurnId ?? randomTurnIdFromCrypto;
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Prompts
  // ───────────────────────────────────────────────────────────────────────────

  /** `prompt.send`. A retry with the same `client_request_id` from the same device gets the first answer instead of a second turn. */
  promptSend(runtime: RemoteRuntime, viewer: RemoteViewer, value: unknown): Promise<PromptSendResult> {
    const input = params(value);
    const clientRequestId = boundedString(input['client_request_id'], 'client_request_id', MAX_ID_CHARS);
    const key = `${viewer.device_id}\0${runtime.sessionId}\0${clientRequestId}`;
    const now = this.deps.clock.now();
    const earlier = this.sent.get(key);
    if (earlier !== undefined && now - earlier.at < IDEMPOTENCY_WINDOW_MS) return earlier.result;
    this.prune(now);
    const result = this.sendPrompt(runtime, viewer, input);
    this.sent.set(key, { at: now, result });
    // A prompt that was refused or failed may be sent again.
    result.catch(() => { if (this.sent.get(key)?.result === result) this.sent.delete(key); });
    return result;
  }

  private prune(now: number): void {
    for (const [key, entry] of this.sent) {
      if (now - entry.at >= IDEMPOTENCY_WINDOW_MS || this.sent.size >= IDEMPOTENCY_MAX_ENTRIES) this.sent.delete(key);
    }
  }

  private async sendPrompt(runtime: RemoteRuntime, viewer: RemoteViewer, input: Json): Promise<PromptSendResult> {
    const text = input['text'];
    const rawImages = input['images'];
    if (typeof text !== 'string') throw invalid('invalid text');
    if (rawImages !== undefined && (!Array.isArray(rawImages) || rawImages.length > MAX_PROMPT_IMAGES)) {
      throw invalid(`at most ${MAX_PROMPT_IMAGES} images`);
    }
    this.requireConnected(runtime);
    const source = { origin: 'device', deviceName: viewer.device_name } as const;
    const turnId = this.randomTurnId();
    const trimmed = text.trimStart();

    if (SLASH_COMMAND.test(trimmed)) {
      const name = trimmed.split(/\s/, 1)[0]!.toLowerCase();
      if (!(REMOTE_SLASH_COMMANDS as readonly string[]).includes(name)) {
        throw new RemoteRpcError('forbidden', `${name} can only be run on the computer`);
      }
      if (rawImages !== undefined && rawImages.length > 0) throw invalid('a slash command takes no images');
      const raw = text.trim();
      try {
        await runtime.dispatchCommand({ type: 'run_slash_command', raw, images: [], turn_id: turnId }, source);
      } catch (error) {
        throw refused(error);
      }
      if (slashRequestEchoes(raw)) this.echo(runtime, viewer, turnId, raw, []);
      return { turn_id: turnId };
    }

    let prompt: string;
    let images: ImageRefDto[];
    try {
      prompt = validatePrompt(text);
      images = validateImageRefs(rawImages);
    } catch (error) {
      throw refused(error);
    }
    // While a turn runs the desktop takes plain text only; so does a viewer.
    if (runtime.turnActive && images.length > 0) throw invalid('images cannot be sent while a turn is running');
    try {
      await runtime.sendPrompt(prompt, images, turnId, undefined, source);
    } catch (error) {
      throw refused(error);
    }
    this.echo(runtime, viewer, turnId, prompt, images);
    return { turn_id: turnId };
  }

  private echo(runtime: RemoteRuntime, viewer: RemoteViewer, turnId: number, text: string, images: ImageRefDto[]): void {
    try {
      this.deps.echo({ sessionId: runtime.sessionId, turnId, text, images, origin: 'device', deviceName: viewer.device_name });
    } catch { /* The prompt was sent; a renderer that cannot show it changes nothing. */ }
  }

  /** `turn.cancel`. */
  turnCancel(runtime: RemoteRuntime, value: unknown): Record<string, never> {
    const input = params(value);
    let turnId: number | undefined;
    if (input['turn_id'] !== undefined) {
      if (typeof input['turn_id'] !== 'number' || !Number.isSafeInteger(input['turn_id']) || input['turn_id'] < 0) throw invalid('invalid turn_id');
      turnId = input['turn_id'];
    }
    this.requireConnected(runtime);
    try {
      runtime.cancelTurn(turnId);
    } catch (error) {
      throw refused(error);
    }
    return {};
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Answers
  // ───────────────────────────────────────────────────────────────────────────

  /** `permission.respond`. */
  permissionRespond(runtime: RemoteRuntime, value: unknown): Record<string, never> {
    const input = params(value);
    const id = requestId(input['request_id']);
    const decision = this.decision(input['decision']);
    const pending = runtime.pendingPermission(id);
    if (pending === undefined) throw new RemoteRpcError('not_pending', 'that request has already been answered');
    if (decision.type !== 'deny') {
      // Approving this request is what switches the session to bypassPermissions.
      if (pending.kind.type === 'bypass_permissions_mode') {
        throw new RemoteRpcError('forbidden', 'bypass permissions can only be turned on at the computer');
      }
      if ((decision.type === 'allow_always' || decision.type === 'allow_auto') && !this.deps.allowAlwaysAllow()) {
        throw new RemoteRpcError('forbidden', 'always allow is not available from a remote device');
      }
    }
    this.requireConnected(runtime);
    try {
      if (decision.type === 'deny') runtime.denyPermission(id);
      else runtime.approvePermission(id, decision);
    } catch (error) {
      throw this.lost(error);
    }
    return {};
  }

  private decision(value: unknown): PermissionResponseDto {
    if (!isObject(value)) throw invalid('invalid decision');
    const type = value['type'];
    if (type !== 'allow_once' && type !== 'allow_always' && type !== 'allow_auto' && type !== 'deny') throw invalid('invalid decision');
    if (Object.keys(value).some((key) => key !== 'type')) throw invalid('invalid decision');
    return { type };
  }

  /** `question.answer`. */
  questionAnswer(runtime: RemoteRuntime, value: unknown): Record<string, never> {
    const input = params(value);
    const id = requestId(input['request_id']);
    const answers = input['answers'];
    if (!isObject(answers) || Object.keys(answers).length > 64 || !Object.values(answers).every((answer) => typeof answer === 'string')) {
      throw invalid('invalid answers');
    }
    if (runtime.pendingQuestion(id) === undefined) throw new RemoteRpcError('not_pending', 'that question has already been answered');
    this.requireConnected(runtime);
    try {
      runtime.answerAskUserQuestion(id, answers);
    } catch (error) {
      throw this.lost(error);
    }
    return {};
  }

  /** `question.cancel`. */
  questionCancel(runtime: RemoteRuntime, value: unknown): Record<string, never> {
    const input = params(value);
    const id = requestId(input['request_id']);
    if (runtime.pendingQuestion(id) === undefined) throw new RemoteRpcError('not_pending', 'that question has already been answered');
    this.requireConnected(runtime);
    try {
      runtime.cancelAskUserQuestion(id);
    } catch (error) {
      throw this.lost(error);
    }
    return {};
  }

  /** An approve/answer call that found the request gone after all: the other side won. */
  private lost(error: unknown): RemoteRpcError {
    return error instanceof Error && /not pending/.test(error.message)
      ? new RemoteRpcError('not_pending', 'that request has already been answered')
      : refused(error);
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Controls
  // ───────────────────────────────────────────────────────────────────────────

  /** `controls.set_model`. */
  async setModel(runtime: RemoteRuntime, viewer: RemoteViewer, value: unknown): Promise<Record<string, never>> {
    const input = params(value);
    const model = boundedString(input['model'], 'model', 256);
    await this.dispatch(runtime, viewer, { type: 'set_model', model });
    return {};
  }

  /** `controls.set_reasoning`. The selection is validated by the same code as the desktop's. */
  async setReasoning(runtime: RemoteRuntime, viewer: RemoteViewer, value: unknown): Promise<Record<string, never>> {
    const input = params(value);
    if (!isObject(input['selection'])) throw invalid('invalid selection');
    await this.dispatch(runtime, viewer, { type: 'set_reasoning_selection', selection: input['selection'] as unknown as ReasoningSelectionDto });
    return {};
  }

  /** `controls.set_permission_mode`. Never to bypassPermissions. */
  async setPermissionMode(runtime: RemoteRuntime, viewer: RemoteViewer, value: unknown): Promise<Record<string, never>> {
    const input = params(value);
    const mode = input['mode'];
    if (!isPermissionModeId(mode)) throw invalid('invalid mode');
    if (mode === 'bypassPermissions') throw new RemoteRpcError('forbidden', 'bypass permissions can only be turned on at the computer');
    await this.dispatch(runtime, viewer, { type: 'set_permission_mode', mode: mode as PermissionModeId });
    return {};
  }

  private async dispatch(runtime: RemoteRuntime, viewer: RemoteViewer, command: unknown): Promise<void> {
    this.requireConnected(runtime);
    try {
      await runtime.dispatchCommand(command, { origin: 'device', deviceName: viewer.device_name });
    } catch (error) {
      throw refused(error);
    }
  }

  /** `models.list`. */
  async modelsList(runtime: RemoteRuntime): Promise<ModelsListResult> {
    this.requireConnected(runtime);
    let catalog;
    try {
      catalog = await runtime.scheduledModelCatalog();
    } catch (error) {
      throw refused(error);
    }
    return { models: catalog.models, current: catalog.current, ...(catalog.details !== undefined ? { details: catalog.details } : {}) };
  }

  // ───────────────────────────────────────────────────────────────────────────
  // Reading
  // ───────────────────────────────────────────────────────────────────────────

  /** `session.history`. */
  history(runtime: RemoteRuntime, value: unknown): SessionHistoryResult {
    const input = params(value);
    const before = input['before_index'];
    const limit = input['limit'];
    if (typeof before !== 'number' || !Number.isSafeInteger(before) || before < 0) throw invalid('invalid before_index');
    if (typeof limit !== 'number' || !Number.isSafeInteger(limit) || limit < 1 || limit > MAX_HISTORY_LIMIT) throw invalid('invalid limit');
    return historyPage(runtime.remoteView(), before, limit);
  }

  /** `image.get`, for a reference the session's records carried. */
  imageGet(runtime: RemoteRuntime | undefined, value: unknown): ImageGetResult {
    const input = params(value);
    const ref = boundedString(input['ref'], 'ref', 128);
    if (runtime === undefined) throw invalid('that image is no longer available');
    const image = resolveImage(runtime.remoteView(), ref, (sequence, index) => runtime.promptImage(sequence, index));
    if (image === undefined) throw invalid('that image is no longer available');
    const fitted = fitImage(this.deps.codec, image);
    return { media_type: fitted.media_type, base64: fitted.base64 };
  }

  private requireConnected(runtime: RemoteRuntime): void {
    if (runtime.connectionState.status !== 'connected') throw new RemoteRpcError('internal', 'the session is not connected');
  }
}
