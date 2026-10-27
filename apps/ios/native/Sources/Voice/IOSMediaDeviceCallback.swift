import AVFoundation
import CoreVideo
import Foundation

func iosMediaIntentAllowsPlayback(_ intent: MediaSessionIntentDto) -> Bool {
    intent == .conversation
}

func iosMediaAudioSessionPurpose(_ intent: MediaSessionIntentDto) -> VoiceAudioSessionCoordinator.Purpose {
    iosMediaIntentAllowsPlayback(intent) ? .flowDuplex : .recognition
}

/// The single app-scoped callback passed to every native engine handle. Its
/// sessions are owner-bound by the opaque lease ID returned from `open` and
/// all requests are serialized through the main-actor service below.
final class IOSMediaDeviceCallbackAdapter: IosMediaDevicePorts, @unchecked Sendable {
    @MainActor static let shared = IOSMediaDeviceCallbackAdapter(service: .shared)

    private let service: IOSMediaDeviceService

    @MainActor
    init(service: IOSMediaDeviceService) {
        self.service = service
    }

    func supportedSessionModes() -> [MediaSessionModeDto] {
        [.audio, .video, .audioVideo]
    }

    func request(request: MediaDeviceRequestEnvelopeDto) async -> MediaDeviceResponseDto {
        await service.handle(request)
    }
}

@MainActor
final class IOSMediaDeviceService {
    static let shared = IOSMediaDeviceService()

    private struct OpenSignature: Equatable {
        let ownerKey: String
        let mode: MediaSessionModeDto
        let intent: MediaSessionIntentDto
        let audioSourceID: String?
        let videoSourceID: String?
        let timeoutBudgetMs: UInt64?
        let maxPayloadBytes: UInt64
    }

    private struct OpenLease {
        let leaseID: String
        let signature: OpenSignature
    }

    private var sessions: [String: IOSMediaDeviceSession] = [:]
    private var cameraLeaseID: String?
    private var leasesByOpenKey: [String: OpenLease] = [:]
    private var openKeysByLease: [String: String] = [:]
    private var openingKeys: Set<String> = []

    func handle(_ envelope: MediaDeviceRequestEnvelopeDto) async -> MediaDeviceResponseDto {
        let result: MediaDeviceResultDto
        do {
            try Task.checkCancellation()
            switch envelope.request {
            case let .open(request):
                result = .opened(lease: try await open(request))
            case let .nextInput(leaseId):
                guard let session = sessions[leaseId] else { throw IOSMediaDeviceFailure.notFound }
                result = .input(frame: try await session.nextInput())
            case let .writeAudio(request):
                guard let session = sessions[request.leaseId] else { throw IOSMediaDeviceFailure.notFound }
                result = .audioWritten(receipt: try session.writeAudio(request.frame))
            case let .flushAudio(request):
                guard let session = sessions[request.leaseId] else { throw IOSMediaDeviceFailure.notFound }
                result = .audioFlushed(receipt: try await session.flushAudio())
            case let .interruptAudio(request):
                guard let session = sessions[request.leaseId] else { throw IOSMediaDeviceFailure.notFound }
                result = .audioInterrupted(receipt: try session.interruptAudio())
            case let .writeVideo(request):
                guard sessions[request.leaseId] != nil else { throw IOSMediaDeviceFailure.notFound }
                // There is no iOS media-video rendering surface in the app
                // yet. Never report success for decoded pixels we did not
                // display.
                throw IOSMediaDeviceFailure.unsupported
            case let .update(request):
                guard let session = sessions[request.leaseId] else { throw IOSMediaDeviceFailure.notFound }
                result = .updated(status: try await session.update(request.update))
            case let .stop(leaseId):
                if let session = sessions.removeValue(forKey: leaseId) {
                    if let openKey = openKeysByLease.removeValue(forKey: leaseId) {
                        leasesByOpenKey.removeValue(forKey: openKey)
                    }
                    await session.stop()
                    if cameraLeaseID == leaseId { cameraLeaseID = nil }
                }
                result = .stopped
            }
        } catch {
            result = .error(error: Self.errorDto(error))
        }
        return MediaDeviceResponseDto(requestId: envelope.requestId, result: result)
    }

    private func open(_ request: MediaDeviceOpenRequestDto) async throws -> MediaDeviceLeaseDto {
        guard request.maxPayloadBytes > 0 else { throw IOSMediaDeviceFailure.invalidRequest }
        let ownerKey: String
        switch request.owner {
        case let .session(sessionId): ownerKey = "session:\(sessionId)"
        case let .ui(instanceId): ownerKey = "ui:\(instanceId)"
        case let .system(instanceId): ownerKey = "system:\(instanceId)"
        }
        let wantsAudio = request.request.mode != .video
        let wantsVideo = request.request.mode != .audio
        let operation = request.identity
        let openKey = "\(ownerKey)|\(operation.id)|\(operation.generation)|\(operation.serviceEpoch)"
        let signature = OpenSignature(
            ownerKey: ownerKey,
            mode: request.request.mode,
            intent: request.request.intent,
            audioSourceID: request.request.audioSource?.id,
            videoSourceID: request.request.videoSource?.id,
            timeoutBudgetMs: request.timeoutBudgetMs,
            maxPayloadBytes: request.maxPayloadBytes
        )
        if let existing = leasesByOpenKey[openKey], sessions[existing.leaseID] != nil {
            guard existing.signature == signature else { throw IOSMediaDeviceFailure.invalidRequest }
            return MediaDeviceLeaseDto(leaseId: existing.leaseID)
        }
        guard !openingKeys.contains(openKey) else { throw IOSMediaDeviceFailure.busy }
        openingKeys.insert(openKey)
        defer { openingKeys.remove(openKey) }
        let audioSourceID = request.request.audioSource?.id
        let videoSourceID = request.request.videoSource?.id
        if wantsAudio, audioSourceID != "media-runtime:microphone" {
            throw IOSMediaDeviceFailure.invalidRequest
        }
        if wantsVideo, videoSourceID != "media-runtime:camera" {
            throw IOSMediaDeviceFailure.invalidRequest
        }

        let leaseID = UUID().uuidString.lowercased()
        if wantsVideo {
            guard cameraLeaseID == nil else { throw IOSMediaDeviceFailure.busy }
            // Reserve before awaiting permission/configuration so two opens
            // cannot start concurrent AVCaptureSessions.
            cameraLeaseID = leaseID
        }
        let session = IOSMediaDeviceSession(
            leaseID: leaseID,
            ownerKey: ownerKey,
            intent: request.request.intent,
            audioSourceID: audioSourceID,
            videoSourceID: videoSourceID,
            maxPayloadBytes: request.maxPayloadBytes
        )
        do {
            try await session.start(audio: wantsAudio, video: wantsVideo)
            try Task.checkCancellation()
            sessions[leaseID] = session
            leasesByOpenKey[openKey] = OpenLease(
                leaseID: leaseID,
                signature: signature
            )
            openKeysByLease[leaseID] = openKey
            return MediaDeviceLeaseDto(leaseId: leaseID)
        } catch {
            await session.stop()
            if cameraLeaseID == leaseID { cameraLeaseID = nil }
            throw error
        }
    }

    private static func errorDto(_ error: Error) -> MediaErrorDto {
        let kind: MediaErrorKindDto
        if let media = error as? IOSMediaDeviceFailure {
            switch media {
            case .permissionDenied: kind = .permissionDenied
            case .busy: kind = .busy
            case .cancelled: kind = .cancelled
            case .timeout: kind = .timeout
            case .noData: kind = .noData
            case .notFound: kind = .notFound
            case .unavailable: kind = .unavailable
            case .unsupported: kind = .unsupported
            case .invalidRequest: kind = .invalidRequest
            case .mediaTooLarge: kind = .mediaTooLarge
            case .nativeFailure: kind = .nativeFailure
            }
        } else if let audio = error as? AudioServiceFailure {
            switch audio {
            case .permissionDenied: kind = .permissionDenied
            case .busy: kind = .busy
            case .cancelled: kind = .cancelled
            case .timeout: kind = .timeout
            case .noSpeech: kind = .noData
            case .notRecording: kind = .notFound
            case .unavailable: kind = .unavailable
            case .unsupported, .modelMissing, .voiceMissing: kind = .unsupported
            case .invalidRequest: kind = .invalidRequest
            case .synthesisFailed, .nativeFailure: kind = .nativeFailure
            case .mediaTooLarge: kind = .mediaTooLarge
            }
        } else if error is CancellationError {
            kind = .cancelled
        } else if let coordination = error as? VoiceAudioSessionCoordinator.CoordinationError {
            switch coordination {
            case .busy:
                kind = .busy
            }
        } else {
            kind = .nativeFailure
        }
        let message = error.localizedDescription.isEmpty ? "Native media operation failed." : error.localizedDescription
        var safeMessage = ""
        for scalar in message.unicodeScalars {
            let value = String(scalar)
            guard safeMessage.utf8.count + value.utf8.count <= 1_024 else { break }
            safeMessage.append(value)
        }
        return MediaErrorDto(kind: kind, message: safeMessage)
    }
}

@MainActor
private final class IOSMediaDeviceSession {
    let leaseID: String
    let ownerKey: String
    private let allowsAudioPlayback: Bool
    private let audioSessionPurpose: VoiceAudioSessionCoordinator.Purpose
    let audioSourceID: String?
    let videoSourceID: String?
    let maxPayloadBytes: UInt64

    private let inputQueue = IOSMediaInputQueue()
    private var audioDevice: IOSRealtimeAudioDevice?
    private var audioInputTask: Task<Void, Never>?
    private var invalidationTask: Task<Void, Never>?
    private var camera: IOSMediaCameraCapture?
    private var stopped = false
    private var audioEnabled = false
    private var videoEnabled = false
    private var sequence: UInt64 = 0

    init(leaseID: String, ownerKey: String, intent: MediaSessionIntentDto, audioSourceID: String?, videoSourceID: String?, maxPayloadBytes: UInt64) {
        self.leaseID = leaseID
        self.ownerKey = ownerKey
        self.allowsAudioPlayback = iosMediaIntentAllowsPlayback(intent)
        self.audioSessionPurpose = iosMediaAudioSessionPurpose(intent)
        self.audioSourceID = audioSourceID
        self.videoSourceID = videoSourceID
        self.maxPayloadBytes = maxPayloadBytes
    }

    func start(audio: Bool, video: Bool) async throws {
        if video { try await setVideoEnabled(true) }
        if audio {
            await observeAudioLeaseInvalidations()
            try await setAudioEnabled(true)
        }
    }

    func nextInput() async throws -> MediaFrameDto? {
        try await inputQueue.next()
    }

    func writeAudio(_ frame: MediaPcmFrameDto) throws -> MediaRenderReceiptDto {
        guard allowsAudioPlayback else { throw IOSMediaDeviceFailure.unsupported }
        guard audioEnabled, let audioDevice else { throw IOSMediaDeviceFailure.unavailable }
        guard UInt64(frame.pcm16le.count) <= maxPayloadBytes else { throw IOSMediaDeviceFailure.mediaTooLarge }
        do {
            return try audioDevice.enqueueMediaOutput(
                pcm16le: Data(frame.pcm16le),
                sampleRateHz: frame.sampleRateHz,
                channels: frame.channels
            )
        } catch {
            throw error
        }
    }

    func flushAudio() async throws -> MediaRenderReceiptDto {
        guard allowsAudioPlayback else { throw IOSMediaDeviceFailure.unsupported }
        guard audioEnabled, let audioDevice else { throw IOSMediaDeviceFailure.unavailable }
        return try await audioDevice.flushMediaOutput()
    }

    func interruptAudio() throws -> MediaRenderReceiptDto {
        guard allowsAudioPlayback else { throw IOSMediaDeviceFailure.unsupported }
        guard audioEnabled, let audioDevice else { throw IOSMediaDeviceFailure.unavailable }
        return audioDevice.interruptMediaOutput()
    }

    func update(_ update: MediaSessionUpdateDto) async throws -> MediaSessionStatusDto {
        guard !stopped else { throw IOSMediaDeviceFailure.cancelled }
        switch update {
        case let .setEnabled(kind, enabled):
            switch kind {
            case .audio:
                try await setAudioEnabled(enabled)
            case .video:
                try await setVideoEnabled(enabled)
            case .image:
                throw IOSMediaDeviceFailure.unsupported
            }
        case let .selectSource(kind, source):
            switch kind {
            case .audio:
                guard source.id == audioSourceID else { throw IOSMediaDeviceFailure.unsupported }
            case .video:
                guard source.id == videoSourceID else { throw IOSMediaDeviceFailure.unsupported }
            case .image: throw IOSMediaDeviceFailure.unsupported
            }
        case let .setPlaybackGain(gain):
            guard allowsAudioPlayback else { throw IOSMediaDeviceFailure.unsupported }
            guard let audioDevice else { throw IOSMediaDeviceFailure.unavailable }
            try audioDevice.setMediaPlaybackGain(gain)
        }
        return status
    }

    func stop() async {
        guard !stopped else { return }
        stopped = true
        invalidationTask?.cancel()
        invalidationTask = nil
        audioInputTask?.cancel()
        audioInputTask = nil
        if let audioDevice { await audioDevice.stop() }
        audioDevice = nil
        if let camera { await camera.stop() }
        camera = nil
        audioEnabled = false
        videoEnabled = false
        inputQueue.finish()
    }

    private var status: MediaSessionStatusDto {
        MediaSessionStatusDto(
            state: .ready,
            audioEnabled: audioEnabled,
            videoEnabled: videoEnabled
        )
    }

    private func setAudioEnabled(_ enabled: Bool) async throws {
        guard !stopped else { throw IOSMediaDeviceFailure.cancelled }
        guard enabled != audioEnabled else { return }
        if enabled {
            let device = IOSRealtimeAudioDevice(coordinator: .shared)
            audioDevice = device
            let stream: AsyncThrowingStream<IOSRealtimeAudioInputFrame, Error>
            do {
                stream = try await device.startTimestamped(
                    inputSampleRateHz: 24_000,
                    outputSampleRateHz: 24_000,
                    audioSessionPurpose: audioSessionPurpose
                )
                try Task.checkCancellation()
                guard !stopped else { throw IOSMediaDeviceFailure.cancelled }
            } catch {
                if audioDevice === device { audioDevice = nil }
                await device.stop()
                throw error
            }
            audioEnabled = true
            audioInputTask = Task { @MainActor [weak self] in
                do {
                    for try await audioFrame in stream {
                        guard let self, self.audioEnabled, !Task.isCancelled else { return }
                        guard UInt64(audioFrame.pcm16le.count) <= self.maxPayloadBytes else {
                            self.inputQueue.fail(IOSMediaDeviceFailure.mediaTooLarge)
                            return
                        }
                        self.sequence &+= 1
                        let frame = MediaFrameDto(
                            source: MediaSourceRefDto(id: self.audioSourceID ?? "media-runtime:microphone"),
                            sequence: self.sequence,
                            capturedAtUnixMs: audioFrame.capturedAtUnixMs,
                            payload: .audio(MediaPcmFrameDto(
                                sampleRateHz: 24_000,
                                channels: 1,
                                pcm16le: audioFrame.pcm16le
                            ))
                        )
                        try self.inputQueue.enqueue(frame, replacingVideoWhenFull: false)
                    }
                } catch is CancellationError {
                } catch {
                    self?.inputQueue.fail(error)
                }
            }
        } else {
            audioEnabled = false
            audioInputTask?.cancel()
            audioInputTask = nil
            inputQueue.removeAudioFrames()
            if let audioDevice { await audioDevice.stop() }
            audioDevice = nil
        }
    }

    private func observeAudioLeaseInvalidations() async {
        guard invalidationTask == nil else { return }
        let events = await VoiceAudioSessionCoordinator.shared.invalidationEvents()
        invalidationTask = Task { @MainActor [weak self] in
            for await event in events {
                guard let self else { return }
                if let device = self.audioDevice, await device.owns(event) {
                    await self.stop()
                    return
                }
            }
        }
    }

    private func setVideoEnabled(_ enabled: Bool) async throws {
        guard !stopped else { throw IOSMediaDeviceFailure.cancelled }
        guard enabled != videoEnabled else { return }
        if enabled {
            let camera = IOSMediaCameraCapture(
                maxPayloadBytes: maxPayloadBytes,
                onFrame: { [weak self] frame in
                    Task { @MainActor [weak self] in
                        guard let self, self.videoEnabled, !Task.isCancelled else { return }
                        self.sequence &+= 1
                        let mediaFrame = MediaFrameDto(
                            source: MediaSourceRefDto(id: self.videoSourceID ?? "media-runtime:camera"),
                            sequence: self.sequence,
                            capturedAtUnixMs: frame.capturedAtUnixMs,
                            payload: .video(.rgba8(
                                widthPx: frame.widthPx,
                                heightPx: frame.heightPx,
                                strideBytes: frame.widthPx * 4,
                                bytes: Data(frame.rgbaBytes)
                            ))
                        )
                        do { try self.inputQueue.enqueue(mediaFrame, replacingVideoWhenFull: true) }
                        catch { self.inputQueue.fail(error) }
                    }
                },
                onFailure: { [weak self] error in
                    Task { @MainActor [weak self] in
                        guard let self else { return }
                        self.inputQueue.fail(error)
                        await self.camera?.stop()
                        self.camera = nil
                        self.videoEnabled = false
                    }
                }
            )
            do {
                try await camera.start()
                try Task.checkCancellation()
                guard !stopped else { throw IOSMediaDeviceFailure.cancelled }
            } catch {
                await camera.stop()
                throw error
            }
            self.camera = camera
            videoEnabled = true
        } else {
            videoEnabled = false
            if let camera { await camera.stop() }
            camera = nil
            inputQueue.removeVideoFrames()
        }
    }

}

@MainActor
private final class IOSMediaInputQueue {
    private struct Waiter {
        let id: UUID
        let continuation: CheckedContinuation<MediaFrameDto?, Error>
    }

    private var frames: [MediaFrameDto] = []
    private var waiters: [Waiter] = []
    private var terminalError: Error?
    private var ended = false

    func next() async throws -> MediaFrameDto? {
        let id = UUID()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                if Task.isCancelled {
                    continuation.resume(throwing: CancellationError())
                } else if !frames.isEmpty {
                    continuation.resume(returning: frames.removeFirst())
                } else if let terminalError {
                    continuation.resume(throwing: terminalError)
                } else if ended {
                    continuation.resume(returning: nil)
                } else {
                    waiters.append(Waiter(id: id, continuation: continuation))
                }
            }
        } onCancel: { [weak self] in
            Task { @MainActor [weak self] in self?.cancelWaiter(id) }
        }
    }

    func enqueue(_ frame: MediaFrameDto, replacingVideoWhenFull: Bool) throws {
        guard !ended else { throw IOSMediaDeviceFailure.cancelled }
        if !waiters.isEmpty {
            waiters.removeFirst().continuation.resume(returning: frame)
            return
        }
        if replacingVideoWhenFull, frames.count >= 4,
           let oldVideo = frames.firstIndex(where: { if case .video = $0.payload { return true }; return false }) {
            frames.remove(at: oldVideo)
        }
        guard frames.count < 64 else { throw IOSMediaDeviceFailure.mediaTooLarge }
        frames.append(frame)
    }

    func removeAudioFrames() {
        frames.removeAll { if case .audio = $0.payload { return true }; return false }
    }

    func removeVideoFrames() {
        frames.removeAll { if case .video = $0.payload { return true }; return false }
    }

    func fail(_ error: Error) {
        guard !ended else { return }
        terminalError = error
        ended = true
        let pending = waiters
        waiters.removeAll()
        for waiter in pending { waiter.continuation.resume(throwing: error) }
    }

    func finish() {
        guard !ended else { return }
        ended = true
        frames.removeAll()
        let pending = waiters
        waiters.removeAll()
        for waiter in pending { waiter.continuation.resume(returning: nil) }
    }

    private func cancelWaiter(_ id: UUID) {
        guard let index = waiters.firstIndex(where: { $0.id == id }) else { return }
        let waiter = waiters.remove(at: index)
        waiter.continuation.resume(throwing: CancellationError())
    }
}

private struct IOSMediaCapturedVideoFrame: Sendable {
    let widthPx: UInt32
    let heightPx: UInt32
    let rgbaBytes: [UInt8]
    let capturedAtUnixMs: UInt64
}

/// Camera input uses one bounded AVCaptureSession and never acknowledges
/// decoded video output because iOS currently has no corresponding renderer.
/// All AVFoundation session objects are touched on `captureQueue`; callbacks
/// only hand off copied Sendable RGBA bytes.
private final class IOSMediaCameraCapture: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate, @unchecked Sendable {
    private let captureSession = AVCaptureSession()
    private let captureQueue = DispatchQueue(label: "com.lingxi.media.camera", qos: .userInitiated)
    private let maxPayloadBytes: UInt64
    private let onFrame: @Sendable (IOSMediaCapturedVideoFrame) -> Void
    private let onFailure: @Sendable (Error) -> Void
    private var lastFrameTime: TimeInterval = 0
    private var failed = false

    init(
        maxPayloadBytes: UInt64,
        onFrame: @escaping @Sendable (IOSMediaCapturedVideoFrame) -> Void,
        onFailure: @escaping @Sendable (Error) -> Void
    ) {
        self.maxPayloadBytes = maxPayloadBytes
        self.onFrame = onFrame
        self.onFailure = onFailure
    }

    func start() async throws {
        try await requestCameraAuthorization()
        try Task.checkCancellation()
        try await withCheckedThrowingContinuation { continuation in
            captureQueue.async { [self] in
                var configurationOpen = false
                do {
                    guard !captureSession.isRunning else { throw IOSMediaDeviceFailure.busy }
                    captureSession.beginConfiguration()
                    configurationOpen = true
                    let preset: AVCaptureSession.Preset
                    if maxPayloadBytes >= 640 * 480 * 4 {
                        preset = .vga640x480
                    } else if maxPayloadBytes >= 352 * 288 * 4 {
                        preset = .cif352x288
                    } else {
                        throw IOSMediaDeviceFailure.mediaTooLarge
                    }
                    guard captureSession.canSetSessionPreset(preset) else { throw IOSMediaDeviceFailure.unavailable }
                    captureSession.sessionPreset = preset
                    guard let device = AVCaptureDevice.default(for: .video) else { throw IOSMediaDeviceFailure.unavailable }
                    let input = try AVCaptureDeviceInput(device: device)
                    guard captureSession.canAddInput(input) else { throw IOSMediaDeviceFailure.unavailable }
                    captureSession.addInput(input)
                    let output = AVCaptureVideoDataOutput()
                    output.alwaysDiscardsLateVideoFrames = true
                    output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
                    output.setSampleBufferDelegate(self, queue: captureQueue)
                    guard captureSession.canAddOutput(output) else { throw IOSMediaDeviceFailure.unavailable }
                    captureSession.addOutput(output)
                    captureSession.commitConfiguration()
                    configurationOpen = false
                    captureSession.startRunning()
                    guard captureSession.isRunning else { throw IOSMediaDeviceFailure.unavailable }
                    continuation.resume()
                } catch {
                    if configurationOpen { captureSession.commitConfiguration() }
                    if captureSession.isRunning { captureSession.stopRunning() }
                    captureSession.inputs.forEach(captureSession.removeInput)
                    captureSession.outputs.forEach(captureSession.removeOutput)
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    func stop() async {
        await withCheckedContinuation { continuation in
            captureQueue.async { [self] in
                if captureSession.isRunning { captureSession.stopRunning() }
                captureSession.outputs.forEach(captureSession.removeOutput)
                captureSession.inputs.forEach(captureSession.removeInput)
                continuation.resume()
            }
        }
    }

    private func requestCameraAuthorization() async throws {
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: return
        case .notDetermined:
            let granted = await withCheckedContinuation { continuation in
                AVCaptureDevice.requestAccess(for: .video) { continuation.resume(returning: $0) }
            }
            guard granted else { throw IOSMediaDeviceFailure.permissionDenied }
        default: throw IOSMediaDeviceFailure.permissionDenied
        }
    }

    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        guard !failed else { return }
        let now = Date().timeIntervalSince1970
        guard now - lastFrameTime >= 0.2 else { return }
        lastFrameTime = now
        guard let pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer) else {
            fail(IOSMediaDeviceFailure.nativeFailure)
            return
        }
        guard CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly) == kCVReturnSuccess else {
            fail(IOSMediaDeviceFailure.nativeFailure)
            return
        }
        defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
        guard let source = CVPixelBufferGetBaseAddress(pixelBuffer) else {
            fail(IOSMediaDeviceFailure.nativeFailure)
            return
        }
        let width = CVPixelBufferGetWidth(pixelBuffer)
        let height = CVPixelBufferGetHeight(pixelBuffer)
        let sourceStride = CVPixelBufferGetBytesPerRow(pixelBuffer)
        let destinationStride = width * 4
        let totalBytes = destinationStride * height
        guard UInt64(totalBytes) <= maxPayloadBytes else {
            fail(IOSMediaDeviceFailure.mediaTooLarge)
            return
        }
        let sourceBytes = source.assumingMemoryBound(to: UInt8.self)
        var rgba = [UInt8](repeating: 0, count: totalBytes)
        for y in 0 ..< height {
            let sourceRow = sourceBytes.advanced(by: y * sourceStride)
            let destinationRow = y * destinationStride
            for x in 0 ..< width {
                let sourceOffset = x * 4
                let destinationOffset = destinationRow + sourceOffset
                rgba[destinationOffset] = sourceRow[sourceOffset + 2]
                rgba[destinationOffset + 1] = sourceRow[sourceOffset + 1]
                rgba[destinationOffset + 2] = sourceRow[sourceOffset]
                rgba[destinationOffset + 3] = sourceRow[sourceOffset + 3]
            }
        }
        onFrame(IOSMediaCapturedVideoFrame(
            widthPx: UInt32(width),
            heightPx: UInt32(height),
            rgbaBytes: rgba,
            capturedAtUnixMs: UInt64(max(0, now * 1_000))
        ))
    }

    private func fail(_ error: Error) {
        guard !failed else { return }
        failed = true
        onFailure(error)
    }
}

private enum IOSMediaDeviceFailure: Error, LocalizedError {
    case permissionDenied
    case busy
    case cancelled
    case timeout
    case noData
    case notFound
    case unavailable
    case unsupported
    case invalidRequest
    case mediaTooLarge
    case nativeFailure

    var errorDescription: String? {
        switch self {
        case .permissionDenied: "Media permission was denied."
        case .busy: "Another media session owns the device."
        case .cancelled: "The media operation was cancelled."
        case .timeout: "The media operation timed out."
        case .noData: "No media frame is available."
        case .notFound: "The media lease is no longer active."
        case .unavailable: "The requested media device is unavailable."
        case .unsupported: "The requested native media operation is unsupported."
        case .invalidRequest: "The native media request is invalid."
        case .mediaTooLarge: "The media frame exceeds the supported payload size."
        case .nativeFailure: "The native media operation failed."
        }
    }
}
