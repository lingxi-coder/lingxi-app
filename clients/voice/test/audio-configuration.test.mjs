import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  audioConfigurationDefaults,
  isAudioFallbackAllowed,
  migrateLegacyAudioConfiguration,
  normalizeAudioConfiguration,
  resolveAudioRoute,
} from "../scripts/audio-config-lib.mjs";
import { checkGeneratedAudioConfiguration } from "../scripts/audio-config-lib.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const fixture = JSON.parse(fs.readFileSync(path.join(root, "audio-config-fixtures.json"), "utf8"));

test("v3 defaults, normalization, and legacy migration match shared fixtures", () => {
  assert.deepEqual(audioConfigurationDefaults(), fixture.normalization[0].expected);
  for (const item of fixture.normalization) {
    assert.deepEqual(normalizeAudioConfiguration(item.input), item.expected, item.name);
  }
  for (const item of fixture.migrations) {
    assert.deepEqual(migrateLegacyAudioConfiguration(item.input), item.expected, item.name);
  }
});

test("route resolution matches shared deterministic requested/effective fixtures", () => {
  for (const item of fixture.routes) {
    assert.deepEqual(resolveAudioRoute(item.input), item.expected, item.name);
  }
});

test("only pre-start permission and unavailability failures can fall back", () => {
  for (const failure of ["permission", "unavailable"]) {
    assert.equal(isAudioFallbackAllowed(failure, false), true);
    assert.equal(isAudioFallbackAllowed(failure, true), false);
  }
  for (const failure of ["busy", "cancelled", "timeout", "invalidRequest", "noSpeech", "nativeFailure"]) {
    assert.equal(isAudioFallbackAllowed(failure, false), false, failure);
  }
});

test("Swift, Kotlin, and TypeScript generated helpers match their bounded templates", () => {
  assert.deepEqual(checkGeneratedAudioConfiguration(), []);
});
