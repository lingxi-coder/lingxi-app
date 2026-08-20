import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
export const voiceRoot = path.resolve(scriptDir, "..");
export const manifestPath = path.join(voiceRoot, "models.json");
export const kotlinOutputPath = path.resolve(
  voiceRoot,
  "../android/app/src/main/java/com/lingxi/code/voice/offline/GeneratedVoiceModels.kt",
);
export const swiftOutputPath = path.resolve(
  voiceRoot,
  "../ios/Sources/Voice/GeneratedVoiceModels.swift",
);

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function isPlainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertStringArray(value, label) {
  assert.ok(Array.isArray(value), `${label} must be an array`);
  for (const [index, item] of value.entries()) {
    assert.equal(typeof item, "string", `${label}[${index}] must be a string`);
  }
}

function assertDisplayNameMap(value, label) {
  assert.ok(isPlainObject(value), `${label} must be an object`);
  assert.ok(Object.keys(value).length > 0, `${label} must not be empty`);
  for (const [key, entryValue] of Object.entries(value)) {
    assert.equal(typeof key, "string", `${label} key must be a string`);
    assert.equal(typeof entryValue, "string", `${label}.${key} must be a string`);
  }
}

function validateRuntimeArtifactMetadata(artifact, label) {
  assert.ok(isPlainObject(artifact), `${label} must be an object`);
  assert.equal(typeof artifact.name, "string", `${label}.name must be a string`);
  assert.equal(typeof artifact.sizeBytes, "number", `${label}.sizeBytes must be a number`);
  assert.ok(artifact.sizeBytes > 0, `${label}.sizeBytes must be positive`);
  assert.equal(typeof artifact.url, "string", `${label}.url must be a string`);
  assert.equal(typeof artifact.sha256, "string", `${label}.sha256 must be a string`);
  assert.match(artifact.sha256, /^[0-9a-f]{64}$/u, `${label}.sha256 must be 64 lowercase hex chars`);
}

function validateRuntimeParams(runtimeParams, label) {
  assert.ok(isPlainObject(runtimeParams), `${label} must be an object`);
  assert.equal(typeof runtimeParams.type, "string", `${label}.type must be a string`);
  assert.equal(typeof runtimeParams.numThreads, "number", `${label}.numThreads must be a number`);
  switch (runtimeParams.type) {
    case "asr-online-transducer":
      assert.equal(typeof runtimeParams.decoding, "string", `${label}.decoding must be a string`);
      break;
    case "asr-offline-moonshine":
    case "tts-vits":
    case "tts-kitten":
      assert.equal(runtimeParams.decoding, undefined, `${label}.decoding is not allowed`);
      break;
    default:
      throw new Error(`Unsupported runtime param type: ${runtimeParams.type}`);
  }
}

function validateVoice(voice, label) {
  assert.ok(isPlainObject(voice), `${label} must be an object`);
  assert.equal(typeof voice.id, "string", `${label}.id must be a string`);
  assert.equal(typeof voice.displayName, "string", `${label}.displayName must be a string`);
  assert.equal(typeof voice.language, "string", `${label}.language must be a string`);
}

export function readManifest(filePath = manifestPath) {
  const manifest = readJson(filePath);
  validateManifest(manifest);
  return manifest;
}

export function validateManifest(manifest) {
  assert.ok(isPlainObject(manifest), "manifest must be an object");
  assert.equal(typeof manifest.schemaVersion, "number", "schemaVersion must be a number");
  assert.ok(isPlainObject(manifest.runtime), "runtime must be an object");
  assert.equal(typeof manifest.runtime.version, "string", "runtime.version must be a string");
  validateRuntimeArtifactMetadata(manifest.runtime.android, "runtime.android");
  validateRuntimeArtifactMetadata(manifest.runtime.ios, "runtime.ios");
  assert.ok(Array.isArray(manifest.models), "models must be an array");
  assert.ok(Array.isArray(manifest.packs), "packs must be an array");

  const modelIds = new Set();
  for (const [index, model] of manifest.models.entries()) {
    const label = `models[${index}]`;
    assert.ok(isPlainObject(model), `${label} must be an object`);
    assert.equal(typeof model.id, "string", `${label}.id must be a string`);
    assert.ok(!modelIds.has(model.id), `Duplicate model id: ${model.id}`);
    modelIds.add(model.id);
    assert.ok(model.kind === "stt" || model.kind === "tts", `${label}.kind must be stt or tts`);
    assertDisplayNameMap(model.displayName, `${label}.displayName`);
    assertStringArray(model.languages, `${label}.languages`);
    assert.equal(typeof model.streaming, "boolean", `${label}.streaming must be a boolean`);
    assert.equal(typeof model.sampleRateHz, "number", `${label}.sampleRateHz must be a number`);
    assert.equal(typeof model.approxSizeBytes, "number", `${label}.approxSizeBytes must be a number`);
    assert.ok(model.approxSizeBytes > 0, `${label}.approxSizeBytes must be positive`);
    assert.equal(typeof model.sha256, "string", `${label}.sha256 must be a string`);
    assert.match(model.sha256, /^[0-9a-f]{64}$/u, `${label}.sha256 must be 64 lowercase hex chars`);
    assertStringArray(model.files, `${label}.files`);
    assertStringArray(model.requiredDirectories, `${label}.requiredDirectories`);
    assert.equal(typeof model.sourceUrl, "string", `${label}.sourceUrl must be a string`);
    validateRuntimeParams(model.runtimeParams, `${label}.runtimeParams`);
    assert.ok(Array.isArray(model.voices), `${label}.voices must be an array`);
    for (const [voiceIndex, voice] of model.voices.entries()) {
      validateVoice(voice, `${label}.voices[${voiceIndex}]`);
    }
    assert.equal(typeof model.license, "string", `${label}.license must be a string`);
  }

  for (const [index, pack] of manifest.packs.entries()) {
    const label = `packs[${index}]`;
    assert.ok(isPlainObject(pack), `${label} must be an object`);
    assert.equal(typeof pack.language, "string", `${label}.language must be a string`);
    assert.equal(typeof pack.title, "string", `${label}.title must be a string`);
    assert.equal(typeof pack.subtitle, "string", `${label}.subtitle must be a string`);
    assertStringArray(pack.modelIds, `${label}.modelIds`);
    assert.ok(pack.modelIds.length > 0, `${label}.modelIds must not be empty`);
    for (const modelId of pack.modelIds) {
      assert.ok(modelIds.has(modelId), `${label}.modelIds references unknown model: ${modelId}`);
    }
  }
}

export function resolvePackModels(manifest, pack) {
  const byId = new Map(manifest.models.map((model) => [model.id, model]));
  return pack.modelIds.map((modelId) => {
    const model = byId.get(modelId);
    assert.ok(model, `Missing model for pack reference: ${modelId}`);
    return model;
  });
}

export function packTotalBytes(manifest, pack) {
  return resolvePackModels(manifest, pack).reduce((total, model) => total + model.approxSizeBytes, 0);
}

function jsString(value) {
  return JSON.stringify(value);
}

function indent(text, spaces) {
  const prefix = " ".repeat(spaces);
  return text
    .split("\n")
    .map((line) => (line.length > 0 ? `${prefix}${line}` : line))
    .join("\n");
}

function renderKotlinString(value) {
  return jsString(value);
}

function renderSwiftString(value) {
  return jsString(value);
}

function renderKotlinRuntimeParams(runtimeParams) {
  switch (runtimeParams.type) {
    case "asr-online-transducer":
      return `GeneratedSherpaRuntimeParams.Asr.OnlineTransducer(numThreads = ${runtimeParams.numThreads}, decoding = ${renderKotlinString(runtimeParams.decoding)})`;
    case "asr-offline-moonshine":
      return `GeneratedSherpaRuntimeParams.Asr.OfflineMoonshine(numThreads = ${runtimeParams.numThreads})`;
    case "tts-vits":
      return `GeneratedSherpaRuntimeParams.Tts.Vits(numThreads = ${runtimeParams.numThreads})`;
    case "tts-kitten":
      return `GeneratedSherpaRuntimeParams.Tts.Kitten(numThreads = ${runtimeParams.numThreads})`;
    default:
      throw new Error(`Unsupported runtime param type: ${runtimeParams.type}`);
  }
}

function renderSwiftRuntimeParams(runtimeParams) {
  switch (runtimeParams.type) {
    case "asr-online-transducer":
      return `.asrOnlineTransducer(numThreads: ${runtimeParams.numThreads}, decoding: ${renderSwiftString(runtimeParams.decoding)})`;
    case "asr-offline-moonshine":
      return `.asrOfflineMoonshine(numThreads: ${runtimeParams.numThreads})`;
    case "tts-vits":
      return `.ttsVits(numThreads: ${runtimeParams.numThreads})`;
    case "tts-kitten":
      return `.ttsKitten(numThreads: ${runtimeParams.numThreads})`;
    default:
      throw new Error(`Unsupported runtime param type: ${runtimeParams.type}`);
  }
}

function renderKotlinStringMap(map) {
  const entries = Object.entries(map).map(
    ([key, value]) => `${renderKotlinString(key)} to ${renderKotlinString(value)}`,
  );
  return `mapOf(${entries.join(", ")})`;
}

function renderKotlinStringList(values) {
  return `listOf(${values.map(renderKotlinString).join(", ")})`;
}

function renderKotlinRuntimeArtifactMetadata(artifact) {
  return `GeneratedRuntimeArtifactMetadata(
    name = ${renderKotlinString(artifact.name)},
    sizeBytes = ${artifact.sizeBytes}L,
    url = ${renderKotlinString(artifact.url)},
    sha256 = ${renderKotlinString(artifact.sha256)},
)`;
}

function renderKotlinStringSet(values) {
  return `setOf(${values.map(renderKotlinString).join(", ")})`;
}

function renderKotlinVoices(voices) {
  if (voices.length === 0) return "emptyList()";
  const body = voices
    .map(
      (voice) =>
        `GeneratedTtsVoiceEntry(id = ${renderKotlinString(voice.id)}, displayName = ${renderKotlinString(voice.displayName)}, language = ${renderKotlinString(voice.language)})`,
    )
    .join(",\n");
  return `listOf(\n${indent(body, 8)}\n    )`;
}

function renderKotlinModel(model) {
  const requiredDirectories =
    model.requiredDirectories.length === 0 ? "emptyList()" : renderKotlinStringList(model.requiredDirectories);
  const languages = renderKotlinStringSet(model.languages);
  return `GeneratedOfflineModelEntry(
    id = ${renderKotlinString(model.id)},
    kind = GeneratedModelKind.${model.kind === "stt" ? "Stt" : "Tts"},
    displayName = ${renderKotlinStringMap(model.displayName)},
    languages = ${languages},
    streaming = ${model.streaming},
    sampleRateHz = ${model.sampleRateHz},
    approxSizeBytes = ${model.approxSizeBytes}L,
    sha256 = ${renderKotlinString(model.sha256)},
    files = ${renderKotlinStringList(model.files)},
    requiredDirectories = ${requiredDirectories},
    sourceUrl = ${renderKotlinString(model.sourceUrl)},
    runtimeParams = ${renderKotlinRuntimeParams(model.runtimeParams)},
    voices = ${renderKotlinVoices(model.voices)},
    license = ${renderKotlinString(model.license)},
)`;
}

function renderKotlinPack(pack) {
  return `GeneratedVoicePack(
    language = ${renderKotlinString(pack.language)},
    title = ${renderKotlinString(pack.title)},
    subtitle = ${renderKotlinString(pack.subtitle)},
    modelIds = ${renderKotlinStringList(pack.modelIds)},
)`;
}

export function renderKotlin(manifest) {
  const models = manifest.models.map(renderKotlinModel).join(",\n");
  const packs = manifest.packs.map(renderKotlinPack).join(",\n");
  return `package com.lingxi.code.voice.offline

// Generated from clients/voice/models.json. Do not edit by hand.

enum class GeneratedModelKind { Stt, Tts }

data class GeneratedTtsVoiceEntry(
    val id: String,
    val displayName: String,
    val language: String,
)

sealed interface GeneratedSherpaRuntimeParams {
    sealed interface Asr : GeneratedSherpaRuntimeParams {
        data class OnlineTransducer(val numThreads: Int, val decoding: String) : Asr
        data class OfflineMoonshine(val numThreads: Int) : Asr
    }

    sealed interface Tts : GeneratedSherpaRuntimeParams {
        data class Vits(val numThreads: Int) : Tts
        data class Kitten(val numThreads: Int) : Tts
    }
}

data class GeneratedOfflineModelEntry(
    val id: String,
    val kind: GeneratedModelKind,
    val displayName: Map<String, String>,
    val languages: Set<String>,
    val streaming: Boolean,
    val sampleRateHz: Int,
    val approxSizeBytes: Long,
    val sha256: String,
    val files: List<String>,
    val requiredDirectories: List<String> = emptyList(),
    val sourceUrl: String,
    val runtimeParams: GeneratedSherpaRuntimeParams,
    val voices: List<GeneratedTtsVoiceEntry> = emptyList(),
    val license: String,
)

data class GeneratedVoicePack(
    val language: String,
    val title: String,
    val subtitle: String,
    val modelIds: List<String>,
) {
    val models: List<GeneratedOfflineModelEntry> get() = modelIds.mapNotNull(GeneratedVoiceModelCatalog::byId)
    val totalBytes: Long get() = models.sumOf(GeneratedOfflineModelEntry::approxSizeBytes)
}

data class GeneratedRuntimeArtifactMetadata(
    val name: String,
    val sizeBytes: Long,
    val url: String,
    val sha256: String,
)

object GeneratedVoiceModelCatalog {
    const val schemaVersion: Int = ${manifest.schemaVersion}
    const val runtimeVersion: String = ${renderKotlinString(manifest.runtime.version)}
    val androidRuntimeArtifact: GeneratedRuntimeArtifactMetadata = ${renderKotlinRuntimeArtifactMetadata(manifest.runtime.android)}
    val iosRuntimeArtifact: GeneratedRuntimeArtifactMetadata = ${renderKotlinRuntimeArtifactMetadata(manifest.runtime.ios)}

    val all: List<GeneratedOfflineModelEntry> = listOf(
${indent(models, 8)}
    )

    val packs: List<GeneratedVoicePack> = listOf(
${indent(packs, 8)}
    )

    fun byId(id: String): GeneratedOfflineModelEntry? = all.firstOrNull { it.id == id }

    fun packFor(language: String): List<GeneratedOfflineModelEntry> =
        packs.firstOrNull { it.language == language }?.models ?: emptyList()
}
`;
}

function renderSwiftDictionary(map) {
  return `[${Object.entries(map)
    .map(([key, value]) => `${renderSwiftString(key)}: ${renderSwiftString(value)}`)
    .join(", ")}]`;
}

function renderSwiftStringArray(values) {
  return `[${values.map(renderSwiftString).join(", ")}]`;
}

function renderSwiftRuntimeArtifactMetadata(artifact) {
  return `GeneratedRuntimeArtifactMetadata(
        name: ${renderSwiftString(artifact.name)},
        sizeBytes: ${artifact.sizeBytes},
        url: ${renderSwiftString(artifact.url)},
        sha256: ${renderSwiftString(artifact.sha256)}
    )`;
}

function renderSwiftVoices(voices) {
  if (voices.length === 0) return "[]";
  const body = voices
    .map(
      (voice) =>
        `GeneratedTtsVoiceEntry(id: ${renderSwiftString(voice.id)}, displayName: ${renderSwiftString(voice.displayName)}, language: ${renderSwiftString(voice.language)})`,
    )
    .join(",\n");
  return `[\n${indent(body, 12)}\n        ]`;
}

function renderSwiftModel(model) {
  return `GeneratedOfflineModelEntry(
        id: ${renderSwiftString(model.id)},
        kind: .${model.kind},
        displayName: ${renderSwiftDictionary(model.displayName)},
        languages: ${renderSwiftStringArray(model.languages)},
        streaming: ${model.streaming},
        sampleRateHz: ${model.sampleRateHz},
        approxSizeBytes: ${model.approxSizeBytes},
        sha256: ${renderSwiftString(model.sha256)},
        files: ${renderSwiftStringArray(model.files)},
        requiredDirectories: ${renderSwiftStringArray(model.requiredDirectories)},
        sourceURL: ${renderSwiftString(model.sourceUrl)},
        runtimeParams: ${renderSwiftRuntimeParams(model.runtimeParams)},
        voices: ${renderSwiftVoices(model.voices)},
        license: ${renderSwiftString(model.license)}
    )`;
}

function renderSwiftPack(pack) {
  return `GeneratedVoicePack(
        language: ${renderSwiftString(pack.language)},
        title: ${renderSwiftString(pack.title)},
        subtitle: ${renderSwiftString(pack.subtitle)},
        modelIDs: ${renderSwiftStringArray(pack.modelIds)}
    )`;
}

export function renderSwift(manifest) {
  const models = manifest.models.map(renderSwiftModel).join(",\n");
  const packs = manifest.packs.map(renderSwiftPack).join(",\n");
  return `import Foundation

// Generated from clients/voice/models.json. Do not edit by hand.

enum GeneratedVoiceModelKind: String, Sendable {
    case stt
    case tts
}

struct GeneratedTtsVoiceEntry: Equatable, Sendable {
    let id: String
    let displayName: String
    let language: String
}

enum GeneratedSherpaRuntimeParams: Equatable, Sendable {
    case asrOnlineTransducer(numThreads: Int, decoding: String)
    case asrOfflineMoonshine(numThreads: Int)
    case ttsVits(numThreads: Int)
    case ttsKitten(numThreads: Int)
}

struct GeneratedOfflineModelEntry: Equatable, Sendable {
    let id: String
    let kind: GeneratedVoiceModelKind
    let displayName: [String: String]
    let languages: [String]
    let streaming: Bool
    let sampleRateHz: Int
    let approxSizeBytes: Int64
    let sha256: String
    let files: [String]
    let requiredDirectories: [String]
    let sourceURL: String
    let runtimeParams: GeneratedSherpaRuntimeParams
    let voices: [GeneratedTtsVoiceEntry]
    let license: String
}

struct GeneratedVoicePack: Equatable, Sendable {
    let language: String
    let title: String
    let subtitle: String
    let modelIDs: [String]

    var models: [GeneratedOfflineModelEntry] {
        modelIDs.compactMap { GeneratedVoiceModelCatalog.byID($0) }
    }

    var totalBytes: Int64 {
        models.reduce(0) { $0 + $1.approxSizeBytes }
    }
}

struct GeneratedRuntimeArtifactMetadata: Equatable, Sendable {
    let name: String
    let sizeBytes: Int64
    let url: String
    let sha256: String
}

enum GeneratedVoiceModelCatalog {
    static let schemaVersion = ${manifest.schemaVersion}
    static let runtimeVersion = ${renderSwiftString(manifest.runtime.version)}
    static let androidRuntimeArtifact = ${renderSwiftRuntimeArtifactMetadata(manifest.runtime.android)}
    static let iosRuntimeArtifact = ${renderSwiftRuntimeArtifactMetadata(manifest.runtime.ios)}

    static let all: [GeneratedOfflineModelEntry] = [
${indent(models, 8)}
    ]

    static let packs: [GeneratedVoicePack] = [
${indent(packs, 8)}
    ]

    static func byID(_ id: String) -> GeneratedOfflineModelEntry? {
        all.first { $0.id == id }
    }

    static func packFor(_ language: String) -> [GeneratedOfflineModelEntry] {
        packs.first { $0.language == language }?.models ?? []
    }
}
`;
}

export function renderGeneratedOutputs(manifest) {
  return {
    [kotlinOutputPath]: renderKotlin(manifest),
    [swiftOutputPath]: renderSwift(manifest),
  };
}

export function writeGeneratedOutputs(manifest = readManifest()) {
  const outputs = renderGeneratedOutputs(manifest);
  for (const [outputPath, content] of Object.entries(outputs)) {
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });
    fs.writeFileSync(outputPath, content, "utf8");
  }
}

export function checkGeneratedOutputs(manifest = readManifest()) {
  const outputs = renderGeneratedOutputs(manifest);
  const drift = [];
  for (const [outputPath, expected] of Object.entries(outputs)) {
    const actual = fs.existsSync(outputPath) ? fs.readFileSync(outputPath, "utf8") : null;
    if (actual !== expected) {
      drift.push(outputPath);
    }
  }
  return drift;
}
