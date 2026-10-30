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
        private final class CallbackFence: @unchecked Sendable {
            private let lock = NSLock()
            private var accepting = true
            private var active = 0
            private var waiters: [CheckedContinuation<Void, Never>] = []
            func enter() -> Bool {
                lock.withLock {
                    guard accepting else { return false }
                    active += 1
                    return true
                }
            }
            func leave() {
                let ready: [CheckedContinuation<Void, Never>] = lock.withLock {
                    active -= 1
                    guard active == 0 else { return [] }
                    let ready = waiters
                    waiters.removeAll()
                    return ready
                }
                ready.forEach { $0.resume() }
            }
            func close() { lock.withLock { accepting = false } }
            func drain() async {
                await withCheckedContinuation { continuation in
                    let done = lock.withLock {
                        if active == 0 { return true }
                        waiters.append(continuation)
                        return false
                    }
                    if done { continuation.resume() }
                }
            }
        }

        private struct CloseClaim {
            let ownerID: String
            let fence = CallbackFence()
            var recorder: AVAudioRecorder?
            var lease: VoiceAudioSessionCoordinator.Lease?
            var fileURL: URL?
            var discard = false
            var startSettled = false
            var confirmed = false
            var reclaimable = false
            var flight: (id: UUID, task: Task<Bool, Never>)?
        }
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
        private var closeClaims: [String: CloseClaim] = [:]
        private var confirmedClaimOrder: [String] = []
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
            let registered = lock.withLock {
                guard closeClaims[operationID] == nil else { return false }
                let candidates = confirmedClaimOrder.count
                for _ in 0 ..< candidates where closeClaims.count >= 1_024 {
                    let oldest = confirmedClaimOrder.removeFirst()
                    if closeClaims[oldest]?.confirmed == true, closeClaims[oldest]?.reclaimable == true {
                        closeClaims.removeValue(forKey: oldest)
                    } else {
                        confirmedClaimOrder.append(oldest)
                    }
                }
                guard closeClaims.count < 1_024 else { return false }
                closeClaims[operationID] = CloseClaim(ownerID: ownerID)
                return true
            }
            guard registered else { throw AudioServiceFailure.busy }

            do {
                guard ["m4a", "audio/m4a"].contains(format.lowercased()) else {
                    throw AudioServiceFailure.unsupported
                }
                guard [16_000, 22_050, 24_000, 32_000, 44_100, 48_000].contains(sampleRateHz),
                      maximumBytes > 0, maximumBytes <= UInt64(Int.max)
                else { throw AudioServiceFailure.invalidRequest }
                let admitted = lock.withLock {
                    guard recorder == nil, pendingStartOperationIDs.isEmpty else { return false }
                    pendingStartOperationIDs.insert(operationID)
                    pendingStartOwnerIDs[operationID] = ownerID
                    return true
                }
                guard admitted else { throw AudioServiceFailure.busy }
                let granted = await microphoneAuthorization()
                try Task.checkCancellation()
                guard granted else { throw AudioServiceFailure.permissionDenied }
                guard isPendingStart(operationID) else { throw CancellationError() }

                let lease: VoiceAudioSessionCoordinator.Lease
                switch await audioSessionCoordinator.acquireForMedia(.recording) {
                case .notAdmitted:
                    throw AudioServiceFailure.busy
                case let .acquired(acquired):
                    lease = acquired
                case let .activationFailed(attempted, error):
                    lock.withLock { closeClaims[operationID]?.lease = attempted }
                    throw AudioServiceFailure.nativeFailure("audio session: \(error.localizedDescription)")
                }
                lock.withLock { closeClaims[operationID]?.lease = lease }
                // An interruption may arrive between coordinator admission and
                // installing this claim. Reject a lease already invalidated in
                // that interval; later interruptions can match the claim.
                let leaseState = await audioSessionCoordinator.leaseStateForDiagnostics()
                guard !leaseState.awaitingOwnerCleanup, await audioSessionCoordinator.owns(lease) else {
                    throw CancellationError()
                }
                try Task.checkCancellation()
                guard isPendingStart(operationID) else { throw CancellationError() }

                let url = FileManager.default.temporaryDirectory
                    .appendingPathComponent("lingxi-voice-\(UUID().uuidString).m4a")
                lock.withLock { closeClaims[operationID]?.fileURL = url }
                let settings: [String: Any] = [
                    AVFormatIDKey: kAudioFormatMPEG4AAC,
                    AVSampleRateKey: Double(sampleRateHz),
                    AVNumberOfChannelsKey: 1,
                    AVEncoderBitRateKey: 32_000,
                    AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
                ]
                let rec = try AVAudioRecorder(url: url, settings: settings)
                lock.withLock {
                    closeClaims[operationID]?.recorder = rec
                }
                rec.delegate = self
                rec.isMeteringEnabled = true
                let duration = min(4 * 60 * 60, max(0.1, Double(maximumBytes) * 8 / 32_000 * 0.9))
                let handle = UUID().uuidString
                let installed = lock.withLock {
                    guard pendingStartOperationIDs.contains(operationID), recorder == nil else { return false }
                    recorder = rec
                    fileURL = url
                    audioLease = lease
                    recordingHandle = handle
                    recordingOwnerID = ownerID
                    startOperationID = operationID
                    recordingFailure = nil
                    return true
                }
                guard installed else {
                    throw CancellationError()
                }
                // Start outside the state lock: a delegate may arrive while
                // record() is in progress. Cancellation retains this claim and
                // waits for start to settle before closing its exact resources.
                let started = startRecorder(rec, duration)
                let startResult: Result<String, RecorderStartFailure> = lock.withLock {
                    closeClaims[operationID]?.startSettled = true
                    guard pendingStartOperationIDs.contains(operationID), recorder === rec else {
                        return .failure(.cancelled)
                    }
                    guard started else { return .failure(.failed) }
                    return .success(handle)
                }
                switch startResult {
                case .success(let handle): return handle
                case .failure(.cancelled): throw CancellationError()
                case .failure(.failed): throw AudioServiceFailure.nativeFailure("recorder failed to start")
                }
            } catch {
                lock.withLock { closeClaims[operationID]?.startSettled = true }
                await cancel(startOperationID: operationID)
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
            let (url, recordingFailure, operationID) = takeRecording(handle: handle, ownerID: ownerID)

            guard let url, let operationID else { throw AudioServiceFailure.notRecording }
            guard await closeClaim(operationID) else {
                throw AudioServiceFailure.nativeFailure("recording close is not confirmed")
            }
            defer { retireCloseReceipt(startOperationID: operationID) }
            lock.withLock {
                if finishedRecordings[handle]?.operationID == operationID {
                    finishedRecordings.removeValue(forKey: handle)
                }
            }
            defer { try? FileManager.default.removeItem(at: url) }

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
                throw AudioRecordingClosedError(underlying: failure)
            } catch {
                throw AudioRecordingClosedError(underlying: AudioServiceFailure.nativeFailure("read recording: \(error.localizedDescription)"))
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
            lock.withLock {
                closeClaims[startOperationID]?.discard = true
                pendingStartOperationIDs.remove(startOperationID)
                pendingStartOwnerIDs.removeValue(forKey: startOperationID)
                let matchesStarted = self.startOperationID == startOperationID
                if matchesStarted { _ = takeRecorderLocked(); return }
                guard let entry = finishedRecordings.first(where: { $0.value.operationID == startOperationID }) else {
                    return
                }
                finishedRecordings.removeValue(forKey: entry.key)
            }
            _ = await closeClaim(startOperationID)
        }

        func cancelWithReceipt(startOperationID: String) async -> Bool {
            await cancel(startOperationID: startOperationID)
            return await closeClaim(startOperationID)
        }

        func ownsRecordingLease(_ lease: VoiceAudioSessionCoordinator.Lease, startOperationID: String) -> Bool {
            lock.withLock { closeClaims[startOperationID]?.lease == lease }
        }

        func retireCloseReceipt(startOperationID: String) {
            lock.withLock {
                guard closeClaims[startOperationID]?.confirmed == true else { return }
                closeClaims[startOperationID]?.reclaimable = true
            }
        }

        func end(ownerID: String) async {
            let operations = lock.withLock {
                closeClaims.compactMap { $0.value.ownerID == ownerID ? $0.key : nil }
            }
            for operationID in operations { await cancel(startOperationID: operationID) }
        }

        func stopAll() async {
            let operations = lock.withLock { Array(closeClaims.keys) }
            for operationID in operations { await cancel(startOperationID: operationID) }
        }

        private func closeClaim(_ operationID: String) async -> Bool {
            let flight: (id: UUID, task: Task<Bool, Never>)? = lock.withLock {
                guard var claim = closeClaims[operationID] else { return nil }
                if claim.confirmed { return (UUID(), Task { true }) }
                guard claim.startSettled else { return nil }
                if let flight = claim.flight { return flight }
                claim.fence.close()
                let id = UUID()
                let rec = claim.recorder
                let lease = claim.lease
                let fence = claim.fence
                let coordinator = audioSessionCoordinator
                let task = Task {
                    rec?.delegate = nil
                    rec?.stop()
                    await fence.drain()
                    guard rec?.isRecording != true else { return false }
                    if let lease, !(await coordinator.releaseWithReceipt(lease)) { return false }
                    return true
                }
                claim.flight = (id, task)
                closeClaims[operationID] = claim
                return (id, task)
            }
            guard let flight else { return false }
            let result = await flight.task.value
            let discardURL: URL? = lock.withLock {
                // A previous successful close may have been followed by a
                // cancel that requests discarding its finished file.
                if let claim = closeClaims[operationID], claim.confirmed {
                    return claim.discard ? claim.fileURL : nil
                }
                guard var claim = closeClaims[operationID], claim.flight?.id == flight.id else { return nil }
                claim.flight = nil
                if result {
                    claim.confirmed = true
                    claim.recorder = nil
                    claim.lease = nil
                    confirmedClaimOrder.append(operationID)
                }
                closeClaims[operationID] = claim
                return result && claim.discard ? claim.fileURL : nil
            }
            if let discardURL { try? FileManager.default.removeItem(at: discardURL) }
            return result
        }

        private func isPendingStart(_ operationID: String) -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return pendingStartOperationIDs.contains(operationID)
        }

        private func takeRecording(handle: String, ownerID: String) -> (URL?, String?, String?) {
            lock.lock()
            defer { lock.unlock() }
            if recordingHandle == handle, recordingOwnerID == ownerID,
               let operationID = startOperationID, let url = fileURL {
                let failure = recordingFailure
                finishedRecordings[handle] = FinishedRecording(
                    operationID: operationID, ownerID: ownerID, fileURL: url, failure: failure
                )
                _ = takeRecorderLocked()
                return (url, failure, operationID)
            }
            guard let finished = finishedRecordings[handle], finished.ownerID == ownerID else {
                return (nil, nil, nil)
            }
            return (finished.fileURL, finished.failure, finished.operationID)
        }

        private func takeRecorderLocked() -> (
            AVAudioRecorder?,
            URL?,
            VoiceAudioSessionCoordinator.Lease?
        ) {
            let current = (recorder, fileURL, audioLease)
            let operationID = startOperationID
            recorder = nil
            fileURL = nil
            audioLease = nil
            recordingHandle = nil
            recordingOwnerID = nil
            startOperationID = nil
            recordingFailure = nil
            if let operationID {
                pendingStartOperationIDs.remove(operationID)
                pendingStartOwnerIDs.removeValue(forKey: operationID)
            }
            return current
        }

        func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
            lock.lock()
            guard self.recorder === recorder, let operationID = startOperationID,
                  let fence = closeClaims[operationID]?.fence, fence.enter() else {
                lock.unlock()
                return
            }
            defer { fence.leave() }
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
                _ = await closeClaim(operationID)
                activityChangeHandler?()
            }
        }

    }
#endif
