import AVFoundation
import UIKit

/// A foreground, user-started call owns a lease in the same audio service as chat.
@MainActor
final class LocalAppRecording {
    private let owner: IOSAudioOwner
    private var generation: UInt64 = 0
    private var continuation: CheckedContinuation<Bool, Never>?
    private weak var alert: UIAlertController?
    private var cancelled = false

    init(appID: String, runID: String) {
        owner = .ui(instanceID: "local-app:\(appID):\(runID):\(UUID())")
    }
    func cancel() {
        cancelled = true
        finish(false)
    }
    private func finish(_ accepted: Bool) {
        let pending = continuation
        continuation = nil
        alert?.dismiss(animated: false); alert = nil
        pending?.resume(returning: accepted)
    }
    private func prompt(presenter: UIViewController, recording: Bool) async -> Bool {
        guard !cancelled else { return false }
        return await withCheckedContinuation { pending in
            continuation = pending
            let dialog = UIAlertController(title: recording ? "正在录音" : "录音",
                message: recording ? "最长 30 秒。完成后将录音返回当前应用。" : "点开始后才会使用麦克风。进入后台或撤销权限会取消录音。", preferredStyle: .alert)
            dialog.addAction(UIAlertAction(title: "取消", style: .cancel) { [weak self] _ in self?.finish(false) })
            dialog.addAction(UIAlertAction(title: recording ? "完成录音" : "开始录音", style: .default) { [weak self] _ in self?.finish(true) })
            alert = dialog
            presenter.present(dialog, animated: true)
        }
    }
    private func perform(_ operation: IOSAudioOperation) async -> IOSAudioOperationResult {
        generation += 1
        let audio = IOSAudioService.shared
        return await audio.execute(.init(identity: .init(id: UUID().uuidString.lowercased(), generation: generation,
            serviceEpoch: audio.capabilityState().serviceEpoch), owner: owner, initiator: nil,
            timeoutBudgetMs: nil, maxPayloadBytes: min(audio.maximumPayloadBytes ?? maxAudioPayloadBytes(), 8 * 1024 * 1024), operation: operation))
    }
    func record(presenter: UIViewController, validate: @escaping @MainActor () async throws -> Void) async throws -> [String: String] {
        guard await prompt(presenter: presenter, recording: false) else { throw CancellationError() }
        guard await AVCaptureDevice.requestAccess(for: .audio) else { throw LocalAppServiceFailure("MICROPHONE_DENIED", "Microphone permission was not granted.") }
        try await validate()
        guard !cancelled, UIApplication.shared.applicationState == .active else { throw CancellationError() }
        let background = NotificationCenter.default.addObserver(forName: UIApplication.willResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.cancel() }
        }
        defer { NotificationCenter.default.removeObserver(background) }
        let result: [String: String]
        do {
            let started = await perform(.startRecording(sampleRateHz: 44_100, format: "audio/m4a"))
            let handle: String
            switch started {
            case .recordingStarted(let value): handle = value
            case .failed(let error): throw LocalAppServiceFailure(error.kind.rawValue, error.message)
            default: throw LocalAppServiceFailure("MICROPHONE_UNAVAILABLE", "Recording did not start.")
            }
            guard !cancelled, UIApplication.shared.applicationState == .active else { throw CancellationError() }
            let monitor = Task { @MainActor in
                do {
                    for _ in 0..<150 {
                        try await Task.sleep(for: .milliseconds(200))
                        guard UIApplication.shared.applicationState == .active else { throw CancellationError() }
                        try await validate()
                    }
                    self.finish(true)
                } catch { self.cancel() }
            }
            let accepted = await prompt(presenter: presenter, recording: true)
            monitor.cancel()
            guard accepted, !cancelled else { throw CancellationError() }
            switch await perform(.stopRecording(handle: handle)) {
            case .recording(let data, let mimeType):
                try await validate()
                result = ["mimeType": mimeType, "base64": data.base64EncodedString()]
            case .failed(let error): throw LocalAppServiceFailure(error.kind.rawValue, error.message)
            default: throw LocalAppServiceFailure("RECORDING_FAILED", "Recording did not finish.")
            }
        } catch {
            // An unstructured cleanup task still runs if the page task cancels.
            try await releaseOwner()
            throw error
        }
        try await releaseOwner()
        return result
    }
    private func releaseOwner() async throws {
        let result = await Task { @MainActor in await self.perform(.endOwner) }.value
        if case .failed(let error) = result { throw LocalAppServiceFailure("RECORDING_CLEANUP_FAILED", error.message) }
    }
}
