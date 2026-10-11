import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
export const voiceRoot = path.resolve(scriptDir, "..");
export const schemaPath = path.join(voiceRoot, "audio-config-schema.json");
export const fixturePath = path.join(voiceRoot, "audio-config-fixtures.json");
export const templateRoot = path.join(voiceRoot, "templates");
export const generatedAudioConfigurationPaths = {
  typescript: path.resolve(voiceRoot, "../../apps/electron/src/shared/generatedAudioConfiguration.ts"),
  iosSwift: path.resolve(voiceRoot, "../../apps/ios/native/Sources/Voice/GeneratedAudioConfiguration.swift"),
  macSwift: path.resolve(voiceRoot, "../../apps/electron/native/audio-helper/GeneratedAudioConfiguration.swift"),
  kotlin: path.resolve(voiceRoot, "../../apps/android/native/app/src/main/java/com/lingxi/code/voice/audio/GeneratedAudioConfiguration.kt"),
};

export function readAudioConfigurationSchema(filePath = schemaPath) {
  const schema = JSON.parse(fs.readFileSync(filePath, "utf8"));
  assert.equal(schema.schemaVersion, 4, "audio config schemaVersion must be 4");
  assert.deepEqual(schema.sources, ["automatic", "system", "offline", "provider"]);
  assert.equal(schema.defaults.schemaVersion, schema.schemaVersion);
  assert.equal(schema.defaults.language, schema.languageAuto);
  assert.equal(schema.defaults.rate, schema.defaultRate);
  assert.equal(schema.defaults.autoPlayReplies, false);
  assert.equal(schema.defaults.recognition.source, "automatic");
  assert.equal(schema.defaults.speech.source, "automatic");
  assert.equal(schema.defaults.speech.voice, null);
  assert.ok(schema.minimumRate > 0 && schema.minimumRate < schema.defaultRate);
  assert.ok(schema.maximumRate > schema.defaultRate);
  return schema;
}

function template(name, schema) {
  const input = fs.readFileSync(path.join(templateRoot, name), "utf8");
  const numeric = (value) => Number.isInteger(value) ? `${value}.0` : String(value);
  return input
    .replaceAll("{{SCHEMA_VERSION}}", String(schema.schemaVersion))
    .replaceAll("{{LANGUAGE_AUTO}}", JSON.stringify(schema.languageAuto))
    .replaceAll("{{MIN_RATE}}", numeric(schema.minimumRate))
    .replaceAll("{{MAX_RATE}}", numeric(schema.maximumRate))
    .replaceAll("{{DEFAULT_RATE}}", numeric(schema.defaultRate))
    .replaceAll("{{DEFAULT_JSON}}", JSON.stringify(schema.defaults, null, 2));
}

export function renderGeneratedAudioConfiguration(schema = readAudioConfigurationSchema()) {
  const typescript = template("GeneratedAudioConfiguration.ts.in", schema);
  const swift = template("GeneratedAudioConfiguration.swift.in", schema);
  const kotlin = template("GeneratedAudioConfiguration.kt.in", schema);
  return {
    [generatedAudioConfigurationPaths.typescript]: typescript,
    [generatedAudioConfigurationPaths.iosSwift]: swift,
    [generatedAudioConfigurationPaths.macSwift]: swift,
    [generatedAudioConfigurationPaths.kotlin]: kotlin,
  };
}

export function writeGeneratedAudioConfiguration(schema = readAudioConfigurationSchema()) {
  const outputs = renderGeneratedAudioConfiguration(schema);
  for (const [outputPath, contents] of Object.entries(outputs)) {
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });
    fs.writeFileSync(outputPath, contents, "utf8");
  }
}

export function checkGeneratedAudioConfiguration(schema = readAudioConfigurationSchema()) {
  const drift = [];
  for (const [outputPath, expected] of Object.entries(renderGeneratedAudioConfiguration(schema))) {
    const actual = fs.existsSync(outputPath) ? fs.readFileSync(outputPath, "utf8") : null;
    if (actual !== expected) drift.push(outputPath);
  }
  return drift;
}

const defaults = readAudioConfigurationSchema().defaults;
const validSystemStates = new Set(["available", "permissionRequired", "denied", "unavailable"]);

function object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function stringValue(value, fallback = "") {
  return typeof value === "string" ? value : fallback;
}

function sourceValue(value, fallback = "automatic") {
  const source = typeof value === "string" ? value.trim() : "";
  return source || fallback;
}

function modelIdValue(value) {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function languageValue(value) {
  const language = typeof value === "string" ? value.trim() : "";
  return !language || language.toLowerCase() === "auto" ? "auto" : language;
}

function rateValue(value) {
  const rate = typeof value === "number" && Number.isFinite(value) ? value : 1;
  return Math.min(2, Math.max(0.5, rate));
}

function normalizeVoice(value) {
  if (value === null || value === undefined) return null;
  const raw = object(value);
  const source = sourceValue(raw.source, "");
  const id = typeof raw.id === "string" ? raw.id : "";
  if (!source || !id) return null;
  if (source === "provider") return { source, id, ...(modelIdValue(raw.profileId) ? { profileId: raw.profileId } : {}), ...(modelIdValue(raw.modelId) ? { modelId: raw.modelId } : {}) };
  if (source === "offline") {
    const modelId = modelIdValue(raw.modelId);
    return modelId ? { source, modelId, id } : { source, id };
  }
  return { source, id };
}

export function normalizeCloudBinding(value) {
  const raw = object(value);
  return { binding: stringValue(raw.binding, "follow_session"), profileId: modelIdValue(raw.profileId), modelId: modelIdValue(raw.modelId) };
}

function normalizePreference(value, kind) {
  const raw = object(value);
  const source = sourceValue(raw.source);
  const offlineModelId = modelIdValue(raw.offlineModelId);
  const preference = { source, offlineModelId, cloud: normalizeCloudBinding(raw.cloud) };
  if (kind === "speech") preference.voice = normalizeVoice(raw.voice);
  return preference;
}

export function audioConfigurationDefaults() {
  return JSON.parse(JSON.stringify(defaults));
}

export function normalizeAudioConfiguration(value) {
  const raw = object(value);
  if (raw.schemaVersion !== 4) return audioConfigurationDefaults();
  const recognition = normalizePreference(raw.recognition, "recognition");
  const speech = normalizePreference(raw.speech, "speech");
  if (speech.source === "automatic") speech.voice = null;
  return {
    schemaVersion: 4,
    recognition,
    speech,
    conversation: { interaction: stringValue(object(raw.conversation).interaction, "turn_based"), mode: stringValue(object(raw.conversation).mode, "agent"), cloud: normalizeCloudBinding(object(raw.conversation).cloud), voice: normalizeVoice(object(raw.conversation).voice) },
    language: languageValue(raw.language),
    rate: rateValue(raw.rate),
    autoPlayReplies: raw.autoPlayReplies === true,
  };
}

export function resolveAudioLanguage(configured, deviceLocale) {
  const language = languageValue(configured);
  if (language !== "auto") return language;
  const locale = typeof deviceLocale === "string" ? deviceLocale.trim() : "";
  return locale || "en-US";
}

function languageMatches(modelLanguages, language) {
  const requested = language.toLowerCase();
  return modelLanguages.some((supported) => {
    const candidate = supported.toLowerCase();
    return requested === candidate || requested.startsWith(`${candidate}-`);
  });
}

function routeResult(requested, effective, status, reason, fallbackReason = null) {
  return { requested, effective, status, reason, fallbackReason };
}

function unavailable(requested, reason, status = "unavailable", fallbackReason = null) {
  return routeResult(requested, null, status, reason, fallbackReason);
}

function resolveSystemRoute(requested, request, voice) {
  const state = validSystemStates.has(request.systemStatus) ? request.systemStatus : "unavailable";
  if (state === "available" || state === "permissionRequired") {
    const voiceId = voice?.id && voice.id !== "default" ? voice.id : null;
    if (voiceId && Array.isArray(request.systemVoiceIds) && !request.systemVoiceIds.includes(voiceId)) {
      return unavailable(requested, "systemVoiceUnknown");
    }
    return routeResult(
      requested,
      { source: "system", modelId: null, voiceId },
      state === "available" ? "ready" : "permissionRequired",
      state === "available" ? "ready" : "systemPermissionRequired",
    );
  }
  return unavailable(requested, state === "denied" ? "systemDenied" : "systemUnavailable");
}

function resolveOfflineRoute(requested, request, modelId, voice) {
  const models = Array.isArray(request.offlineModels) ? request.offlineModels : [];
  let model;
  if (modelId !== null) {
    model = models.find((entry) => entry.id === modelId);
    if (!model) return unavailable(requested, "offlineModelUnknown");
    if (model.kind !== request.kind) return unavailable(requested, "offlineModelKindMismatch");
    if (!languageMatches(Array.isArray(model.languages) ? model.languages : [], request.language)) {
      return unavailable(requested, "offlineModelUnsupportedLanguage");
    }
    if (model.installed !== true) return unavailable(requested, "offlineModelNotInstalled");
  } else {
    const compatible = models.filter((entry) => entry.kind === request.kind && languageMatches(Array.isArray(entry.languages) ? entry.languages : [], request.language));
    model = compatible.find((entry) => entry.installed === true);
    if (!model) return unavailable(requested, compatible.length ? "offlineModelNotInstalled" : "noCompatibleOfflineModel");
  }
  if (voice?.source === "offline") {
    if (voice.modelId !== model.id) return unavailable(requested, "offlineModelConflict", "invalidRequest");
    if (Array.isArray(model.voiceIds) && !model.voiceIds.includes(voice.id)) return unavailable(requested, "offlineVoiceUnknown");
  }
  return routeResult(
    requested,
    { source: "offline", modelId: model.id, voiceId: voice?.source === "offline" ? voice.id : null },
    "ready",
    "ready",
  );
}

export function resolveAudioRoute(request) {
  const kind = request?.kind === "speech" ? "speech" : "recognition";
  const preference = object(request?.preference);
  const source = sourceValue(preference.source);
  const offlineModelId = modelIdValue(preference.offlineModelId);
  const baseVoice = kind === "speech" ? normalizeVoice(preference.voice) : null;
  const voice = kind === "speech" && request.voiceOverride !== undefined
    ? normalizeVoice(request.voiceOverride)
    : baseVoice;
  const requested = { source, offlineModelId, voice };
  const language = languageValue(request.language);
  const normalizedRequest = { ...request, kind, language };

  if (!language || language === "auto") return unavailable(requested, "languageUnresolved", "invalidRequest");
  if (kind !== request.kind) return unavailable(requested, "unsupportedKind", "invalidRequest");
  if (source === "provider") return resolveProviderAudioRoute(normalizedRequest, requested, voice);
  if (source !== "automatic" && source !== "system" && source !== "offline") return unavailable(requested, "unsupportedSource", "unavailable");
  if (voice && voice.source !== "system" && voice.source !== "offline") return unavailable(requested, "unsupportedVoiceSource", "invalidRequest");
  if (voice && source !== "automatic" && voice.source !== source) return unavailable(requested, "voiceSourceMismatch", "invalidRequest");

  if (voice?.source === "offline" && offlineModelId && offlineModelId !== voice.modelId) {
    return unavailable(requested, "offlineModelConflict", "invalidRequest");
  }
  if (source === "system" || (source === "automatic" && voice?.source === "system")) {
    return resolveSystemRoute(requested, normalizedRequest, voice?.source === "system" ? voice : null);
  }
  if (source === "offline" || (source === "automatic" && voice?.source === "offline")) {
    const selectedModel = voice?.source === "offline" ? voice.modelId ?? null : offlineModelId;
    return resolveOfflineRoute(requested, normalizedRequest, selectedModel, voice);
  }
  const systemResult = resolveSystemRoute(requested, normalizedRequest, null);
  if (systemResult.status === "ready" || systemResult.status === "permissionRequired") return systemResult;
  const offlineResult = resolveOfflineRoute(requested, normalizedRequest, offlineModelId, null);
  return { ...offlineResult, fallbackReason: systemResult.reason };
}

export function isAudioFallbackAllowed(failure, operationStarted) {
  return operationStarted !== true && (failure === "permission" || failure === "unavailable");
}

export function resolveProviderAudioRoute(request, requested, voice) {
  const cloud = normalizeCloudBinding(request.preference.cloud);
  if (cloud.binding !== "follow_session" && cloud.binding !== "explicit_profile") return unavailable(requested, "providerBindingInvalid", "invalidRequest");
  const profileId = cloud.binding === "follow_session" ? request.sessionContext?.profileId : cloud.profileId;
  if (!profileId) return unavailable(requested, cloud.binding === "follow_session" ? "sessionProfileRequired" : "providerProfileRequired");
  const capability = request.providerCapabilities?.find((entry) => entry.profileId === profileId && entry.kind === request.kind);
  if (!capability || !capability.supported) return unavailable(requested, "providerOperationUnsupported");
  if (capability.readiness !== "ready") return unavailable(requested, capability.readiness === "unreachable" ? "providerUnreachable" : "providerConfigurationRequired");
  const modelId = cloud.modelId ?? capability.defaultModelId;
  if (!capability.modelIds.includes(modelId)) return unavailable(requested, "providerModelUnsupported");
  if (voice && (voice.source !== "provider" || voice.profileId !== profileId || (voice.modelId ?? null) !== modelId)) return unavailable(requested, "providerVoiceScopeMismatch", "invalidRequest");
  return routeResult(requested, { source: "provider", profileId, providerId: capability.providerId, modelId, voiceId: voice?.id ?? null }, "ready", "ready");
}
