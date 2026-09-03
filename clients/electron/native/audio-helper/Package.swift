// swift-tools-version: 5.9

import PackageDescription

let package = Package(
  name: "LingXiAudioHelper",
  platforms: [
    .macOS(.v13),
  ],
  products: [
    .executable(name: "LingXiAudioHelper", targets: ["LingXiAudioHelper"]),
  ],
  dependencies: [
    .package(url: "https://github.com/k2-fsa/sherpa-onnx", exact: "1.13.6"),
  ],
  targets: [
    .executableTarget(
      name: "LingXiAudioHelper",
      dependencies: [
        .product(name: "sherpa-onnx", package: "sherpa-onnx"),
      ],
      path: ".",
      exclude: [
        "Package.swift",
        "Package.resolved",
        "Tests",
      ],
      sources: [
        "AudioHelperMain.swift",
        "GeneratedVoiceModels.swift",
      ],
      linkerSettings: [
        .linkedFramework("AVFoundation"),
        .linkedFramework("Speech"),
        .linkedFramework("CoreMedia"),
      ]
    ),
    .testTarget(
      name: "LingXiAudioHelperTests",
      dependencies: ["LingXiAudioHelper"],
      path: "Tests/LingXiAudioHelperTests"
    ),
  ]
)
