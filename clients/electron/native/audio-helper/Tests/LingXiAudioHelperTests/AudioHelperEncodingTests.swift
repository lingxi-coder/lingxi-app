import Foundation
import Testing
@testable import LingXiAudioHelper

struct AudioHelperEncodingTests {
    @Test
    func foregroundPermissionArgumentsAreStrictlyBounded() throws {
        #expect(try permissionRequestFromArguments(["LingXiAudioHelper", "--jsonl"]) == nil)
        #expect(try permissionRequestFromArguments([
            "LingXiAudioHelper",
            "--request-permissions",
            "microphone,speech",
        ]) == Set(["microphone", "speech"]))
        #expect(throws: HelperError.self) {
            try permissionRequestFromArguments([
                "LingXiAudioHelper",
                "--request-permissions",
                "camera",
            ])
        }
    }

    @Test
    func archivePathRejectsTraversal() {
        #expect(validateArchivePath("model/tokens.txt"))
        #expect(!validateArchivePath("../escape"))
        #expect(!validateArchivePath("model/../escape"))
        #expect(!validateArchivePath("/absolute/path"))
    }

    @Test
    func recordingPayloadHonorsWaveAndM4aFormats() throws {
        let samples = [Float](repeating: 0, count: 1_600)
        let wave = try recordingPayload(samples: samples, sampleRate: 16_000, format: "wav")
        let waveData = try #require(Data(base64Encoded: wave.audioBase64))
        #expect(wave.mimeType == "audio/wav")
        #expect(String(data: waveData.prefix(4), encoding: .ascii) == "RIFF")
        #expect(waveData.withUnsafeBytes { $0.load(fromByteOffset: 24, as: UInt32.self) }.littleEndian == 16_000)

        let m4a = try recordingPayload(samples: samples, sampleRate: 16_000, format: "m4a")
        let m4aData = try #require(Data(base64Encoded: m4a.audioBase64))
        #expect(m4a.mimeType == "audio/mp4")
        #expect(String(data: m4aData.subdata(in: 4..<8), encoding: .ascii) == "ftyp")
    }

    @Test
    func missingOfflineVoiceFallsBackToSystemWithoutLosingRequestedValue() throws {
        let root = FileManager.default.temporaryDirectory
            .appending(path: "lingxi-missing-voice-\(UUID().uuidString)")
        let resolved = try resolvePlaybackVoice(
            "sherpa:missing:model-voice",
            language: "en-US",
            root: root
        )
        #expect(resolved.snapshot.requestedVoiceSelection == "sherpa:missing:model-voice")
        #expect(resolved.snapshot.effectiveVoiceId.hasPrefix("system:"))
    }

    @Test
    func helperSnapshotOmitsInternalPaths() throws {
        let snapshot = HelperSnapshot(
            helper: .init(state: "running", message: nil),
            permissions: .init(microphone: "granted", speech: "authorized"),
            owner: nil,
            activity: "idle",
            localeTag: "en-US",
            recognizerAvailable: true,
            recognition: nil,
            playback: nil,
            voices: [
                .init(
                    id: "system:default",
                    label: "System Default",
                    languageTag: "en-US",
                    source: "system",
                    familyId: "system",
                    isDefault: true,
                    networkRequired: false
                ),
            ],
            models: [
                .init(modelId: "sherpa.moonshine-tiny-en", state: .ready),
            ]
        )

        let encoded = try JSONEncoder().encode(snapshot)
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["storageRoot"] == nil)
        let helper = try #require(object["helper"] as? [String: Any])
        #expect(helper["path"] == nil)
    }

    @Test
    func modelStateEncodesSharedKeys() throws {
        let snapshot = HelperModelSnapshot(
            modelId: "sherpa.kitten-nano-en",
            state: .failed("checksum mismatch")
        )
        let encoded = try JSONEncoder().encode(snapshot)
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["modelId"] as? String == "sherpa.kitten-nano-en")
        let state = try #require(object["state"] as? [String: Any])
        #expect(state["type"] as? String == "failed")
        #expect(state["message"] as? String == "checksum mismatch")
    }
}
