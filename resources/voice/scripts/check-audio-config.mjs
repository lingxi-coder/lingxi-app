#!/usr/bin/env node

import { checkGeneratedAudioConfiguration, readAudioConfigurationSchema } from "./audio-config-lib.mjs";

const drift = checkGeneratedAudioConfiguration(readAudioConfigurationSchema());
if (drift.length > 0) {
  console.error("Generated audio configuration drift detected:");
  for (const outputPath of drift) console.error(`- ${outputPath}`);
  process.exit(1);
}
