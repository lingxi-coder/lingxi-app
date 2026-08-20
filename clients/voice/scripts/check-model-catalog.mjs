#!/usr/bin/env node

import { checkGeneratedOutputs, readManifest } from "./model-catalog-lib.mjs";

const drift = checkGeneratedOutputs(readManifest());
if (drift.length > 0) {
  console.error("Generated voice catalog drift detected:");
  for (const outputPath of drift) {
    console.error(`- ${outputPath}`);
  }
  process.exit(1);
}
