// VoiceImpl.swift — iOS native mic-recorder capability (parity with Android
// RecorderController.kt).
//
// The AudioService's raw recording driver. Capture returns AAC/m4a from
// `AVAudioRecorder`; live speech recognition stays in `SttImpl`.

import Foundation

#if canImport(AVFoundation)
    import AVFoundation

    /// Native mic recorder over `AVAudioRecorder` producing AAC/m4a.
    final class VoiceImpl: NSObject, AudioRecordingDriving, AVAudioRecorderDelegate, @unchecked Sendable {
        private enum RecorderStartFailure: Error {
            case cancelled
            case failed
        }

        private struct FinishedRecording {
            let operationID: String
            let ownerID: String
            let fileURL: URL
            let failure: String?
        }

        private let lock = NSLock()
        private let microphoneAuthorization: @Sendable () async -> Bool
        private let audioSessionCoordinator: VoiceAudioSessionCoordinator
        private let startRecorder: (AVAudioRecorder, TimeInterval) -> Bool
        private var recorder: AVAudioRecorder?
        private var fileURL: URL?
        private var audioLease: VoiceAudioSessionCoordinator.Lease?
        private var recordingHandle: String?
        private var recordingOwnerID: String?
        private var startOperationID: String?
        private var finishedRecordings: [String: FinishedRecording] = [:]
        private var pendingStartOperationIDs: Set<String> = []
        private var pendingStartOwnerIDs: [String: String] = [:]
        private var recordingFailure: String?
        private var activityChangeHandler: (@Sendable () -> Void)?

        init(
            microphoneAuthorization: @escaping @Sendable () async -> Bool = {
            await AVAudioApplication.requestRecordPermission()
            },
            coordinator: VoiceAudioSessionCoordinator? = nil,
            startRecorder: @escaping (AVAudioRecorder, TimeInterval) -> Bool = { recorder, duration in
                recorder.record(forDuration: duration)
            }
        ) {
            self.microphoneAuthorization = microphoneAuthorization
            audioSessionCoordinator = coordinator ?? .shared
            self.startRecorder = startRecorder
            super.init()
        }

        func installActivityChangeHandler(_ handler: (@Sendable () -> Void)?) {
            lock.lock()
            activityChangeHandler = handler
            lock.unlock()
        }

        func startRecordingOwned(
            operationID: String,
            ownerID: String,
            sampleRateHz: UInt32,
            format: String,
            maximumBytes: UInt64
        ) async throws -> String {
            guard ["m4a", "audio/m4a"].contains(format.lowercased()) else {
                throw AudioServiceFailure.unsupported
            }
            guard [16_000, 22_050, 24_000, 32_000, 44_100, 48_000].contains(sampleRateHz),
                  maximumBytes > 0,
                  maximumBytes <= UInt64(Int.max)
            else { throw AudioServiceFailure.invalidRequest }

            let admitted = lock.withLock {
                guard recorder == nil, pendingStartOperationIDs.isEmpty else { return false }
                pendingStartOperationIDs.insert(operationID)
                pendingStartOwnerIDs[operationID] = ownerID
                return true
            }
            guard admitted else { throw AudioServiceFailure.busy }

            do {
                let granted = await microphoneAuthorization()
                try Task.checkCancellation()
                guard granted else { throw AudioServiceFailure.permissionDenied }
                guard isPendingStart(operationID) else { throw CancellationError() }

                let lease: VoiceAudioSessionCoordinator.Lease
                do {
                    lease = try await audioSessionCoordinator.acquire(.recording)
                } catch is VoiceAudioSessionCoordinator.CoordinationError {
                    throw AudioServiceFailure.busy
                } catch {
                    throw AudioServiceFailure.nativeFailure("audio session: \(error.localizedDescription)")
                }
                do {
                    try Task.checkCancellation()
                    guard isPendingStart(operationID) else { throw CancellationError() }

                    let url = FileManager.default.temporaryDirectory
                        .appendingPathComponent("lingxi-voice-\(UUID().uuidString).m4a")
                    let settings: [String: Any] = [
                        AVFormatIDKey: kAudioFormatMPEG4AAC,
                        AVSampleRateKey: Double(sampleRateHz),
                        AVNumberOfChannelsKey: 1,
                        AVEncoderBitRateKey: 32_000,
                        AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
                    ]
                    let rec = try AVAudioRecorder(url: url, settings: settings)
                    rec.delegate = self
                    rec.isMeteringEnabled = true
                    // Rate-limit file growth before it reaches the transport
                    // ceiling; the final file-size check remains authoritative.
                    let byteLimit = Double(maximumBytes)
                    let duration = min(4 * 60 * 60, max(0.1, byteLimit * 8 / 32_000 * 0.9))

                    let startResult: Result<String, RecorderStartFailure> = lock.withLock {
                        guard pendingStartOperationIDs.contains(operationID), recorder == nil else {
                            return .failure(.cancelled)
                        }
                        guard startRecorder(rec, duration) else { return .failure(.failed) }
                        let handle = UUID().uuidString
                        recorder = rec
                        fileURL = url
                        audioLease = lease
                        recordingHandle = handle
                        recordingOwnerID = ownerID
                        startOperationID = operationID
                        recordingFailure = nil
                        return .success(handle)
                    }
                    switch startResult {
                    case .success(let handle): return handle
                    case .failure(.cancelled):
                        try? FileManager.default.removeItem(at: url)
                        throw CancellationError()
                    case .failure(.failed):
                        try? FileManager.default.removeItem(at: url)
                        throw AudioServiceFailure.nativeFailure("recorder failed to start")
                    }
                } catch {
                    await audioSessionCoordinator.release(lease)
                    throw error
                }
            } catch {
                lock.withLock {
                    pendingStartOperationIDs.remove(operationID)
                    pendingStartOwnerIDs.removeValue(forKey: operationID)
                }
                if error is CancellationError { throw error }
                if let failure = error as? AudioServiceFailure { throw failure }
                throw AudioServiceFailure.nativeFailure(error.localizedDescription)
            }
        }

        func stopRecordingOwned(
            handle: String,
            ownerID: String,
            maximumBytes: UInt64
        ) async throws -> IOSAudioRecording {
            let (rec, url, lease, recordingFailure) = takeRecording(handle: handle, ownerID: ownerID)

            guard let url else { throw AudioServiceFailure.notRecording }
            defer { try? FileManager.default.removeItem(at: url) }
            rec?.stop()
            if let lease { await audioSessionCoordinator.release(lease) }

            do {
                if let recordingFailure {
                    throw AudioServiceFailure.nativeFailure(recordingFailure)
                }
                let values = try url.resourceValues(forKeys: [.fileSizeKey])
                guard let fileSize = values.fileSize else {
                    throw AudioServiceFailure.nativeFailure("recording file size is unavailable")
                }
                guard fileSize > 0 else {
                    throw AudioServiceFailure.nativeFailure("recording produced no media")
                }
                guard UInt64(fileSize) <= maximumBytes else {
                    throw AudioServiceFailure.mediaTooLarge
                }
                let data = try Data(contentsOf: url)
                guard !data.isEmpty else {
                    throw AudioServiceFailure.nativeFailure("recording produced no media")
                }
                return IOSAudioRecording(audioBytes: data, mimeType: "audio/m4a")
            } catch let failure as AudioServiceFailure {
                throw failure
            } catch {
                throw AudioServiceFailure.nativeFailure("read recording: \(error.localizedDescription)")
            }
        }

        func recordingLevel(handle: String, ownerID: String) -> Float? {
            lock.withLock {
                guard recordingHandle == handle, recordingOwnerID == ownerID, let recorder, recorder.isRecording else { return nil }
                recorder.updateMeters()
                return recorder.averagePower(forChannel: 0)
            }
        }

        func isRecordingOwned(handle: String?, ownerID: String) -> Bool {
            lock.lock()
            defer { lock.unlock() }
            guard recordingOwnerID == ownerID,
                  handle == nil || recordingHandle == handle
            else { return false }
            return recorder?.isRecording ?? false
        }

        func cancel(startOperationID: String) async {
            let current = lock.withLock { () -> (AVAudioRecorder?, URL?, VoiceAudioSessionCoordinator.Lease?) in
                pendingStartOperationIDs.remove(startOperationID)
                pendingStartOwnerIDs.removeValue(forKey: startOperationID)
                let matchesStarted = self.startOperationID == startOperationID
                if matchesStarted { return takeRecorderLocked() }
                guard let entry = finishedRecordings.first(where: { $0.value.operationID == startOperationID }) else {
                    return (nil, nil, nil)
                }
                finishedRecordings.removeValue(forKey: entry.key)
                return (nil, entry.value.fileURL, nil)
            }
            await finishAndDiscard(current)
        }

        func end(ownerID: String) async {
            let (current, pending, finishedURLs) = lock.withLock { () -> (
                (AVAudioRecorder?, URL?, VoiceAudioSessionCoordinator.Lease?), [String], [URL]
            ) in
                let pending = pendingStartOwnerIDs.compactMap { operationID, pendingOwner in
                    pendingOwner == ownerID ? operationID : nil
                }
                let current = recordingOwnerID == ownerID ? takeRecorderLocked() : (nil, nil, nil)
                let finished = finishedRecordings.filter { $0.value.ownerID == ownerID }
                finished.keys.forEach { finishedRecordings.removeValue(forKey: $0) }
                return (current, pending, finished.values.map(\.fileURL))
            }
            for operationID in pending { await cancel(startOperationID: operationID) }
            await finishAndDiscard(current)
            finishedURLs.forEach { try? FileManager.default.removeItem(at: $0) }
        }

        func stopAll() async {
            let (current, pending, finishedURLs) = lock.withLock { () -> (
                (AVAudioRecorder?, URL?, VoiceAudioSessionCoordinator.Lease?), Set<String>, [URL]
            ) in
                let pending = pendingStartOperationIDs
                let current = takeRecorderLocked()
                let finishedURLs = finishedRecordings.values.map(\.fileURL)
                finishedRecordings.removeAll()
                return (current, pending, finishedURLs)
            }
            for operationID in pending { await cancel(startOperationID: operationID) }
            await finishAndDiscard(current)
            finishedURLs.forEach { try? FileManager.default.removeItem(at: $0) }
        }

        private func finishAndDiscard(_ current: (
            AVAudioRecorder?, URL?, VoiceAudioSessionCoordinator.Lease?
        )) async {
            current.0?.stop()
            if let url = current.1 { try? FileManager.default.removeItem(at: url) }
            if let lease = current.2 { await audioSessionCoordinator.release(lease) }
        }

        private func isPendingStart(_ operationID: String) -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return pendingStartOperationIDs.contains(operationID)
        }

        private func takeRecording(handle: String, ownerID: String) -> (
            AVAudioRecorder?,
            URL?,
            VoiceAudioSessionCoordinator.Lease?,
            String?
        ) {
            lock.lock()
            defer { lock.unlock() }
            if recordingHandle == handle, recordingOwnerID == ownerID {
                let failure = recordingFailure
                let current = takeRecorderLocked()
                return (current.0, current.1, current.2, failure)
            }
            guard let finished = finishedRecordings[handle], finished.ownerID == ownerID else {
                return (nil, nil, nil, nil)
            }
            finishedRecordings.removeValue(forKey: handle)
            return (nil, finished.fileURL, nil, finished.failure)
        }

        private func takeRecorderLocked() -> (
            AVAudioRecorder?,
            URL?,
            VoiceAudioSessionCoordinator.Lease?
        ) {
            let current = (recorder, fileURL, audioLease)
            recorder = nil
            fileURL = nil
            audioLease = nil
            recordingHandle = nil
            recordingOwnerID = nil
            startOperationID = nil
            recordingFailure = nil
            pendingStartOperationIDs.removeAll()
            pendingStartOwnerIDs.removeAll()
            return current
        }

        func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
            lock.lock()
            guard self.recorder === recorder else {
                lock.unlock()
                return
            }
            let lease = audioLease
            if let handle = recordingHandle,
               let ownerID = recordingOwnerID,
               let operationID = startOperationID,
               let fileURL {
                let failure = flag ? recordingFailure : "recorder stopped before producing a valid file"
                finishedRecordings[handle] = FinishedRecording(
                    operationID: operationID,
                    ownerID: ownerID,
                    fileURL: fileURL,
                    failure: failure
                )
            }
            if let startOperationID {
                pendingStartOperationIDs.remove(startOperationID)
                pendingStartOwnerIDs.removeValue(forKey: startOperationID)
            }
            self.recorder = nil
            fileURL = nil
            audioLease = nil
            recordingHandle = nil
            recordingOwnerID = nil
            startOperationID = nil
            recordingFailure = nil
            let activityChangeHandler = self.activityChangeHandler
            lock.unlock()
            Task {
                if let lease { await audioSessionCoordinator.release(lease) }
                activityChangeHandler?()
            }
        }

    }
#endif
