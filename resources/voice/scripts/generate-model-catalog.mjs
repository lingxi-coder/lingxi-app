#!/usr/bin/env node

import { readManifest, writeGeneratedOutputs } from "./model-catalog-lib.mjs";

const manifest = readManifest();
writeGeneratedOutputs(manifest);
