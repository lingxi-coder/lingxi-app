import assert from "node:assert/strict";
import test from "node:test";
import {
  checkGeneratedOutputs,
  packTotalBytes,
  readManifest,
  resolvePackModels,
} from "../scripts/model-catalog-lib.mjs";

test("manifest matches the seeded Android sherpa catalog", () => {
  const manifest = readManifest();
  const ids = manifest.models.map((model) => model.id);
  assert.deepEqual(ids, [
    "sherpa.zipformer-zh-14m-mobile",
    "sherpa.moonshine-tiny-en",
    "sherpa.melo-zh-en",
    "sherpa.kitten-nano-en",
  ]);
  assert.equal(manifest.runtime.version, "1.13.2");
  assert.deepEqual(manifest.runtime.android, {
    name: "sherpa-onnx-static-link-onnxruntime-1.13.2.aar",
    sizeBytes: 38208264,
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-static-link-onnxruntime-1.13.2.aar",
    sha256: "9b2a290b8c7f31bd0aba35abb4628e87fe8d0eb71796a98aa12f3acd089ceaed",
  });
  assert.deepEqual(manifest.runtime.ios, {
    name: "sherpa-onnx-v1.13.2-ios.tar.bz2",
    sizeBytes: 77611169,
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.2/sherpa-onnx-v1.13.2-ios.tar.bz2",
    sha256: "2886a04df4f8d5066c6c8b6e712278d65d7b60fc9e45990223df50262861d38b",
  });
  assert.deepEqual(
    manifest.models.map((model) => [model.id, model.sourceUrl, model.sha256, model.license]),
    [
      [
        "sherpa.zipformer-zh-14m-mobile",
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-zh-14M-2023-02-23-mobile.tar.bz2",
        "d394cab72b17f788b8b09ffc610f5f070e610fecf022eedca2eae9e38be4f20a",
        "Apache-2.0",
      ],
      [
        "sherpa.moonshine-tiny-en",
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-int8.tar.bz2",
        "d5fe6ec4334fef36255b2a4010412cad4c007e33103fec62fb5d17cad88086f2",
        "MIT",
      ],
      [
        "sherpa.melo-zh-en",
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2",
        "e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514",
        "MIT",
      ],
      [
        "sherpa.kitten-nano-en",
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kitten-nano-en-v0_2-fp16.tar.bz2",
        "0345a8a2f4a710cb8f7912c9a731ded8b3e1e69b33a871efa95c2e64651518fe",
        "Apache-2.0",
      ],
    ],
  );
  assert.deepEqual(
    manifest.models.map((model) => model.runtimeParams),
    [
      { type: "asr-online-transducer", numThreads: 2, decoding: "greedy_search" },
      { type: "asr-offline-moonshine", numThreads: 2 },
      { type: "tts-vits", numThreads: 2 },
      { type: "tts-kitten", numThreads: 2 },
    ],
  );

  const chinesePack = manifest.packs.find((pack) => pack.language === "zh");
  const englishPack = manifest.packs.find((pack) => pack.language === "en");
  assert.ok(chinesePack);
  assert.ok(englishPack);
  assert.deepEqual(chinesePack.modelIds, [
    "sherpa.zipformer-zh-14m-mobile",
    "sherpa.melo-zh-en",
  ]);
  assert.deepEqual(englishPack.modelIds, [
    "sherpa.moonshine-tiny-en",
    "sherpa.kitten-nano-en",
  ]);
  assert.equal(packTotalBytes(manifest, chinesePack), 221351135);
  assert.equal(packTotalBytes(manifest, englishPack), 134187246);
  assert.deepEqual(
    resolvePackModels(manifest, chinesePack).map((model) => model.requiredDirectories),
    [[], ["dict"]],
  );
  assert.deepEqual(
    resolvePackModels(manifest, englishPack).map((model) => model.requiredDirectories),
    [[], ["espeak-ng-data"]],
  );
});

test("generated Kotlin and Swift catalogs stay in sync with the manifest", () => {
  const drift = checkGeneratedOutputs(readManifest());
  assert.deepEqual(drift, []);
});
