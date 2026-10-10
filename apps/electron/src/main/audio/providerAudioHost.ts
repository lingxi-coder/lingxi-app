import { parseAudioUsageContext } from '../../shared/audioUsage.js';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { resolveServerBin } from '../bridgeDiscovery.js';
import { buildBridgeEnvironment } from '../host-utils.js';
import type { AudioCloudBinding, AudioConfigurationV4, AudioProviderCapability } from '../../shared/generatedAudioConfiguration.js';
import type { NativeAudioSnapshot } from '../../shared/nativeAudio.js';
import type { AudioOperationResultDto } from '@lingxi/bridge-client';
import { MAX_AUDIO_PAYLOAD_BYTES, isSendableAudioBase64, isSendableAudioText } from '../../shared/audioResponse.js';

export interface ProviderAudioSession { sessionId: string; profileId: string; accountScope?: string }
interface ProviderAudioCapabilityResponse {
  supported: boolean;
  readiness: 'ready' | 'needs_configuration' | 'unavailable' | 'unsupported';
  reason?: string;
  profileId?: string;
  providerId?: string;
  credentialId?: string;
  modelId?: string | null;
  models: Array<{ id: string | null; label?: string; voices: Array<{ id: string; label?: string }> }>;
  streaming: boolean;
  realtime: boolean;
}
interface ProviderAudioFailure { error: { kind: string; message: string } }
type Kind = 'recognition' | 'speech' | 'realtime';

export interface ProviderAudioHostOptions {
  isPackaged: boolean;
  resourcesPath: string;
  cwd(): string;
  session(): ProviderAudioSession | undefined;
  realtimeSupported?(): boolean;
  resolveCredential(credentialId: string): Promise<string | undefined>;
  spawnProcess?: typeof spawn;
}

function failed(message: string, kind: 'unsupported' | 'unavailable' | 'invalid_request' | 'cancelled' | 'native_failure' = 'unavailable'): AudioOperationResultDto {
  return { type: 'failed', error: { kind, message } };
}
function failure(value: unknown): value is ProviderAudioFailure {
  return !!value && typeof value === 'object' && 'error' in value;
}
function parseCapability(value: unknown): ProviderAudioCapabilityResponse {
  if (!value || typeof value !== 'object' || failure(value)) throw new Error(failure(value) ? value.error.message : 'Invalid provider audio capabilities');
  const item = value as ProviderAudioCapabilityResponse;
  if (typeof item.supported !== 'boolean' || !['ready', 'needs_configuration', 'unavailable', 'unsupported'].includes(item.readiness)
    || !Array.isArray(item.models) || item.models.length > 512 || item.models.some((model) => (model.id !== null && (typeof model.id !== 'string' || model.id.length > 256)) || !Array.isArray(model.voices) || model.voices.length > 512 || model.voices.some((voice) => !voice || typeof voice.id !== 'string' || voice.id.length > 512))
    || (item.profileId !== undefined && typeof item.profileId !== 'string') || (item.providerId !== undefined && typeof item.providerId !== 'string')) throw new Error('Invalid provider audio capabilities');
  return item;
}

/** Credentials, configuration loading, transport, and model admission stay in the Rust host. */
export class ProviderAudioHost {
  constructor(private readonly options: ProviderAudioHostOptions) {}

  private run(kind: string, request: Record<string, unknown>, payload: Record<string, unknown> = {}, providerKeys: Record<string, string> = {}, signal?: AbortSignal): Promise<unknown> {
    if (signal?.aborted) return Promise.reject(new Error('Audio operation cancelled'));
    const media = { ...(payload['text'] === undefined ? {} : { text: payload['text'] }), ...(payload['audioBase64'] === undefined ? {} : { audioBase64: payload['audioBase64'] }), ...(payload['mimeType'] === undefined ? {} : { mimeType: payload['mimeType'] }) };
    const input = JSON.stringify({ profilesJson: null, providerKeys, kind, request, ...media });
    if (Buffer.byteLength(input) > 2 * MAX_AUDIO_PAYLOAD_BYTES) return Promise.reject(new Error('Audio request exceeds the host payload limit'));
    const binary = resolveServerBin({ isPackaged: this.options.isPackaged, resourcesPath: this.options.resourcesPath });
    return new Promise((resolve, reject) => {
      const child = (this.options.spawnProcess ?? spawn)(binary, ['--audio-service-json'], {
        cwd: this.options.cwd(), env: buildBridgeEnvironment(process.env), stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
      });
      let settled = false;
      const chunks: Buffer[] = [];
      let bytes = 0;
      const finish = (error?: Error, value?: unknown) => {
        if (settled) return;
        settled = true; clearTimeout(timer); signal?.removeEventListener('abort', abort);
        if (error) reject(error); else resolve(value);
      };
      const abort = () => { child.kill(); finish(new Error('Audio operation cancelled')); };
      const timer = setTimeout(() => { child.kill(); finish(new Error('Provider audio request timed out')); }, 120_000);
      signal?.addEventListener('abort', abort, { once: true });
      child.stdout.on('data', (chunk: Buffer) => {
        bytes += chunk.length;
        if (bytes > 2 * MAX_AUDIO_PAYLOAD_BYTES) { child.kill(); finish(new Error('Provider audio response exceeds the host payload limit')); return; }
        chunks.push(chunk);
      });
      // Drain stderr without exposing credential-bearing provider diagnostics.
      child.stderr.on('data', () => undefined);
      child.once('error', (error) => finish(error));
      child.stdin.once('error', (error) => finish(error));
      child.once('close', (code) => {
        if (settled) return;
        if (code !== 0) { finish(new Error('Provider audio host could not complete the operation')); return; }
        try { finish(undefined, JSON.parse(Buffer.concat(chunks).toString('utf8'))); }
        catch { finish(new Error('Provider audio host returned invalid JSON')); }
      });
      child.stdin.end(input);
    });
  }

  private request(kind: Kind, cloud: AudioCloudBinding, session: ProviderAudioSession | undefined) {
    return { operationId: randomUUID(), kind, cloud, ...(session ? { session } : {}), timeoutMs: 120_000, maxPayloadBytes: MAX_AUDIO_PAYLOAD_BYTES };
  }

  async capabilities(configuration: AudioConfigurationV4, session = this.options.session()): Promise<Pick<NativeAudioSnapshot, 'providerCapabilities' | 'providerCatalog' | 'sessionContext' | 'realtimeReadiness'>> {
    const preferences = [configuration.recognition, configuration.speech];
    const providerCapabilities: AudioProviderCapability[] = [];
    const providerCatalog: NonNullable<NativeAudioSnapshot['providerCatalog']> = [];
    for (const [index, preference] of preferences.entries()) {
      const kind = index === 0 ? 'recognition' : 'speech';
      try {
        let result = parseCapability(await this.run('capabilities', { ...this.request(kind, preference.cloud, session), language: configuration.language, ...(kind === 'speech' ? { rate: configuration.rate } : {}) }));
        const profileId = result.profileId ?? (preference.cloud.binding === 'explicit_profile' ? preference.cloud.profileId : session?.profileId);
        if (!profileId) continue;
        const credentialId = result.credentialId ?? profileId;
        const credential = await this.options.resolveCredential(credentialId);
        if (credential) result = parseCapability(await this.run('capabilities', { ...this.request(kind, preference.cloud, session), language: configuration.language, ...(kind === 'speech' ? { rate: configuration.rate } : {}) }, {}, { [credentialId]: credential }));
        providerCatalog.push({ profileId, kind, models: result.models });
        providerCapabilities.push({ profileId, providerId: result.providerId ?? profileId, kind,
          supported: result.supported, readiness: result.readiness === 'ready' ? 'ready' : result.readiness === 'unavailable' ? 'unreachable' : result.readiness === 'unsupported' ? 'unsupported' : 'configurationRequired',
          defaultModelId: result.modelId ?? null, modelIds: result.models.map((entry) => entry.id) });
      } catch { /* A failed host preflight yields an unavailable route, never a local fallback. */ }
    }
    let realtimeReadiness: NonNullable<NativeAudioSnapshot['realtimeReadiness']> = { supported: false, ready: false, reason: '实时会话需要当前 Agent 的已验证历史、工具与权限契约。' };
    if (session && this.options.realtimeSupported?.()) {
      try {
        const request = { ...this.request('realtime', configuration.conversation.cloud, session), interaction: configuration.conversation.interaction, language: configuration.language, ...(configuration.conversation.voice ? { voice: configuration.conversation.voice.id } : {}) };
        let result = parseCapability(await this.run('capabilities', request));
        const credentialId = result.credentialId ?? result.profileId;
        const credential = credentialId ? await this.options.resolveCredential(credentialId) : undefined;
        if (credential && credentialId) result = parseCapability(await this.run('capabilities', request, {}, { [credentialId]: credential }));
        realtimeReadiness = { supported: result.supported, ready: result.readiness === 'ready' && configuration.conversation.interaction === 'turn_based', reason: configuration.conversation.interaction !== 'turn_based' ? '桌面未验证回声消除，请使用轮流说话。' : result.reason,
          ...(result.profileId ? { profileId: result.profileId } : {}), ...(result.providerId ? { providerId: result.providerId } : {}), ...(result.modelId ? { defaultModelId: result.modelId } : {}), modelIds: result.models.map((entry) => entry.id), models: result.models };
      } catch { realtimeReadiness = { supported: true, ready: false, reason: '实时音频服务无法连接，请检查 profile 与凭据。' }; }
    }
    return { providerCapabilities, providerCatalog, ...(session ? { sessionContext: session } : {}), realtimeReadiness };
  }

  async preflight(kind: 'recognition' | 'speech', configuration: AudioConfigurationV4, session: ProviderAudioSession | undefined, signal: AbortSignal): Promise<AudioOperationResultDto | null> {
    try {
      const preference = kind === 'recognition' ? configuration.recognition : configuration.speech;
      const request = this.request(kind, preference.cloud, session);
      let capability = parseCapability(await this.run('capabilities', request, {}, {}, signal));
      if (!capability.supported) return failed(capability.reason ?? 'The selected profile does not support this operation', 'unsupported');
      if (!capability.profileId) return failed(capability.reason ?? 'Choose a session or explicit audio profile');
      const credentialId = capability.credentialId ?? capability.profileId;
      const credential = await this.options.resolveCredential(credentialId);
      if (credential) capability = parseCapability(await this.run('capabilities', request, {}, { [credentialId]: credential }, signal));
      return capability.readiness === 'ready' ? null : failed(capability.reason ?? 'Configure a credential and supported audio model in settings');
    } catch (error) { return failed(signal.aborted ? 'Audio operation cancelled' : error instanceof Error ? error.message : 'Provider audio unavailable', signal.aborted ? 'cancelled' : 'unavailable'); }
  }

  async execute(kind: 'recognition' | 'speech', configuration: AudioConfigurationV4, payload: { text?: string; audioBase64?: string; mimeType?: string; language?: string; rate?: number; voice?: string; maxPayloadBytes?: number; timeoutMs?: number; onUsage?: (usage: unknown, route: { profileId: string; providerId: string; modelId: string | null; accountScope?: string }) => void; onResolvedRoute?: (route: { profileId: string; providerId: string; modelId: string | null; voiceId?: string }) => void }, session: ProviderAudioSession | undefined, signal: AbortSignal): Promise<AudioOperationResultDto> {
    try {
      const preference = kind === 'recognition' ? configuration.recognition : configuration.speech;
      const request = { ...this.request(kind, preference.cloud, session), language: payload.language ?? configuration.language, rate: payload.rate ?? configuration.rate, ...(payload.maxPayloadBytes === undefined ? {} : { maxPayloadBytes: payload.maxPayloadBytes }), ...(payload.timeoutMs === undefined ? {} : { timeoutMs: payload.timeoutMs }) };
      const admission = parseCapability(await this.run('capabilities', request, {}, {}, signal));
      if (!admission.supported) return failed(admission.reason ?? 'The selected profile does not support this audio operation', 'unsupported');
      const profileId = admission.profileId;
      const modelId = preference.cloud.modelId ?? admission.modelId;
      if (!profileId) return failed(admission.reason ?? 'Choose a configured audio profile and model');
      const voice = kind === 'speech' ? configuration.speech.voice : null;
      if (voice && (voice.source !== 'provider' || voice.profileId !== profileId || (voice.modelId ?? null) !== (modelId ?? null))) return failed('The selected voice belongs to a different audio profile or model', 'invalid_request');
      const credentialId = admission.credentialId ?? profileId;
      const credential = await this.options.resolveCredential(credentialId);
      const providerKeys = credential ? { [credentialId]: credential } : {};
      const result = await this.run(kind === 'recognition' ? 'transcribe' : 'synthesize', { ...request, ...(voice ? { voice: voice.id } : {}) }, payload, providerKeys, signal);
      if (signal.aborted) return failed('Audio operation cancelled', 'cancelled');
      if (failure(result)) return failed(result.error.message, result.error.kind === 'unsupported' ? 'unsupported' : 'unavailable');
      if (!result || typeof result !== 'object') return failed('Invalid provider audio result', 'native_failure');
      const usageContext = parseAudioUsageContext((result as { usageContext?: unknown }).usageContext);
      const actualRoute = usageContext ?? { profileId, providerId: admission.providerId ?? profileId, modelId: modelId ?? null };
      if (kind === 'recognition') {
        const transcript = result as { text: unknown; language?: string };
        if (!isSendableAudioText(transcript.text, MAX_AUDIO_PAYLOAD_BYTES)) return failed('Provider audio returned an invalid transcript', 'native_failure');
        payload.onResolvedRoute?.(actualRoute);
        if (usageContext) payload.onUsage?.((result as { usage?: unknown }).usage, usageContext);
        return { type: 'transcript', text: transcript.text as string, ...(transcript.language ? { language: transcript.language } : {}) };
      }
      const synthesized = result as { pcmBase64: unknown; sampleRateHz: unknown };
      if (!isSendableAudioBase64(synthesized.pcmBase64) || typeof synthesized.sampleRateHz !== 'number' || !Number.isInteger(synthesized.sampleRateHz) || synthesized.sampleRateHz < 8_000 || synthesized.sampleRateHz > 768_000) return failed('Provider audio returned invalid PCM', 'native_failure');
      payload.onResolvedRoute?.({ ...actualRoute, ...(voice ? { voiceId: voice.id } : {}) });
      if (usageContext) payload.onUsage?.((result as { usage?: unknown }).usage, usageContext);
      return { type: 'synthesized', pcm_base64: synthesized.pcmBase64 as string, sample_rate_hz: synthesized.sampleRateHz };
    } catch (error) {
      return failed(signal.aborted ? 'Audio operation cancelled' : error instanceof Error ? error.message : 'Provider audio host unavailable', signal.aborted ? 'cancelled' : 'unavailable');
    }
  }
}
