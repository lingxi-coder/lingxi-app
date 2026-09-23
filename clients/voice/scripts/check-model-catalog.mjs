#!/usr/bin/env node

import { checkGeneratedOutputs, readManifest } from "./model-catalog-lib.mjs";
import { checkGeneratedAudioConfiguration, readAudioConfigurationSchema } from "./audio-config-lib.mjs";

const drift = [
  ...checkGeneratedOutputs(readManifest()),
  ...checkGeneratedAudioConfiguration(readAudioConfigurationSchema()),
];
if (drift.length > 0) {
  console.error("Generated voice/audio configuration drift detected:");
  for (const outputPath of drift) {
    console.error(`- ${outputPath}`);
  }
  process.exit(1);
}
