#!/usr/bin/env node

import { readAudioConfigurationSchema, writeGeneratedAudioConfiguration } from "./audio-config-lib.mjs";

writeGeneratedAudioConfiguration(readAudioConfigurationSchema());
