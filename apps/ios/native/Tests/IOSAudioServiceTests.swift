import AVFoundation
import XCTest
@testable import LingxiCode

@MainActor
final class IOSAudioServiceTests: XCTestCase {
    func testFlowBargeInRecognitionUsesDuplexSessionPurpose() {
        XCTAssertEqual(
            IOSAudioService.recognitionSessionPurpose(for: .ui(instanceID: "flow-barge-in")),
            .flowDuplex
        )
        XCTAssertEqual(
            IOSAudioService.recognitionSessionPurpose(for: .ui(instanceID: "voice-capture")),
            .recognition
        )
    }

    func testCompletedOperationIdentityHistoryIsBounded() async {
        let service = IOSAudioService(serviceEpoch: 91, onInvalidation: { _ in })
        let owner = IOSAudioOwner.session(sessionID: "identity-history")
        let firstIdentity = identity(generation: 1, epoch: 91)
        let first = await service.execute(request(identity: firstIdentity, owner: owner, operation: .status(handle: nil)))
        guard case .status = first else { return XCTFail("initial status should succeed") }

        for generation in 2 ... 4_097 {
            let result = await service.execute(request(
                identity: identity(generation: UInt64(generation), epoch: 91),
                owner: owner,
                operation: .status(handle: nil)
            ))
            guard case .status = result else { return XCTFail("status should succeed while history rotates") }
        }
        let recycled = await service.execute(request(identity: firstIdentity, owner: owner, operation: .status(handle: nil)))
        guard case .status = recycled else { return XCTFail("completed identities must leave the bounded history") }
    }

    func testEndOwnerAllowsFreshLowerGenerationForSameStableOwner() async {
        let service = IOSAudioService(serviceEpoch: 41, onInvalidation: { _ in })
        let owner = IOSAudioOwner.session(sessionID: "stable-session")
        let endIdentity = identity(generation: 80, epoch: 41)
        let ended = await service.execute(request(
            identity: endIdentity,
            owner: owner,
            operation: .endOwner
        ))

        guard case .ownerEnded = ended else { return XCTFail("owner cleanup should complete") }

        let reopened = await service.execute(request(
            identity: identity(generation: 1, epoch: 41),
            owner: owner,
            operation: .status(handle: nil)
        ))
        guard case .status(recording: false, playing: false) = reopened else {
            return XCTFail("a stable owner can reopen after cleanup with a producer-local generation reset")
        }
    }

    func testEndOwnerFencesSameOwnerAdmissionsUntilCleanupDrains() async {
        let recorder = EndOwnerGateRecordingDriver()
        let service = IOSAudioService(recorder: recorder, serviceEpoch: 81, onInvalidation: { _ in })
        let owner = IOSAudioOwner.system(instanceID: "closing-app")
        let started = await service.execute(request(
            identity: identity(generation: 1, epoch: 81),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .recordingStarted = started else {
            return XCTFail("the initial owner recording should start")
        }

        let cleanup = Task { @MainActor in await service.endOwner(owner) }
        await recorder.waitUntilEndOwnerCleanupIsPending()

        let duringCleanup = await service.execute(request(
            identity: identity(generation: 2, epoch: 81),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .failed(let busy) = duringCleanup else {
            return XCTFail("same-owner work must be rejected while resource cleanup is suspended")
        }
        XCTAssertEqual(busy.kind, .busy)
        XCTAssertEqual(recorder.startCallCount, 1)

        await recorder.releaseEndOwnerCleanup()
        await cleanup.value

        let afterCleanup = await service.execute(request(
            identity: identity(generation: 3, epoch: 81),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .recordingStarted = afterCleanup else {
            return XCTFail("same-owner work may resume after cleanup has completely drained")
        }
        XCTAssertEqual(recorder.startCallCount, 2)
        await service.endOwner(owner)
    }

    func testCancelDuringPermissionAdmissionPreventsLateRecordingStart() async throws {
        let gate = DelayedMicrophonePermission()
        let recorder = VoiceImpl(microphoneAuthorization: { await gate.requestPermission() })
        let service = IOSAudioService(
            recorder: recorder,
            serviceEpoch: 52,
            onInvalidation: { _ in }
        )
        let owner = IOSAudioOwner.system(instanceID: "demo")
        let operationIdentity = identity(generation: 9, epoch: 52)
        let operation = Task { @MainActor in
            await service.execute(request(
                identity: operationIdentity,
                owner: owner,
                operation: .startRecording(sampleRateHz: 16_000, format: "audio/m4a")
            ))
        }

        await gate.waitUntilPermissionWasRequested()
        let pendingDiagnostics = await service.diagnostics()
        XCTAssertEqual(pendingDiagnostics.pendingOperations.count, 1)
        XCTAssertNil(pendingDiagnostics.activeLeasePurpose)
        XCTAssertFalse(pendingDiagnostics.leaseAwaitingOwnerCleanup)
        await service.cancel(operationIdentity)
        await gate.resolve(true)
        let result = await operation.value

        guard case .failed(let error) = result else {
            return XCTFail("a cancelled pending start must not produce a recording handle")
        }
        XCTAssertEqual(error.kind, .cancelled)
        XCTAssertFalse(recorder.isRecordingOwned(handle: nil, ownerID: owner.stableKey))
        let settledDiagnostics = await service.diagnostics()
        XCTAssertTrue(settledDiagnostics.pendingOperations.isEmpty)
        XCTAssertEqual(settledDiagnostics.lastOperation?.phase, "cancelled")
    }

    func testTimeoutDuringPermissionAdmissionSettlesWithoutWaitingForPrompt() async {
        let gate = DelayedMicrophonePermission()
        let recorder = VoiceImpl(microphoneAuthorization: { await gate.requestPermission() })
        let service = IOSAudioService(recorder: recorder, serviceEpoch: 53, onInvalidation: { _ in })
        let owner = IOSAudioOwner.ui(instanceID: "timeout-test")
        let task = Task { @MainActor in
            await service.execute(request(
                identity: identity(generation: 1, epoch: 53),
                owner: owner,
                timeoutBudgetMs: 10,
                operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
            ))
        }

        await gate.waitUntilPermissionWasRequested()
        let result = await task.value
        guard case .failed(let error) = result else {
            return XCTFail("permission admission timeout must be a terminal error")
        }
        XCTAssertEqual(error.kind, .timeout)

        await gate.resolve(true)
        await Task.yield()
        XCTAssertFalse(recorder.isRecordingOwned(handle: nil, ownerID: owner.stableKey))
    }

    func testAlreadyCancelledCallerCannotAdmitAnAudioOperation() async {
        let permissionGate = DelayedMicrophonePermission()
        let recorder = VoiceImpl(microphoneAuthorization: { await permissionGate.requestPermission() })
        let service = IOSAudioService(recorder: recorder, serviceEpoch: 54, onInvalidation: { _ in })
        let owner = IOSAudioOwner.ui(instanceID: "pre-cancelled")
        let executionGate = ExecutionGate()
        let task = Task { @MainActor in
            await executionGate.wait()
            return await service.execute(request(
                identity: identity(generation: 1, epoch: 54),
                owner: owner,
                operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
            ))
        }
        task.cancel()
        await executionGate.release()

        let result = await task.value
        guard case .failed(let error) = result else {
            return XCTFail("a cancelled caller must not begin a native operation")
        }
        XCTAssertEqual(error.kind, .cancelled)
        let permissionWasRequested = await permissionGate.wasRequested
        XCTAssertFalse(permissionWasRequested)
    }

    func testCancelListenDuringPermissionPromptReleasesServiceAdmission() async {
        let permission = DelayedSpeechAuthorization()
        let stt = SttImpl(speechAuthorization: { await permission.requestPermission() })
        let service = IOSAudioService(stt: stt, serviceEpoch: 80, onInvalidation: { _ in })
        let owner = IOSAudioOwner.ui(instanceID: "listen-permission-cancel")
        let route = systemPermissionRoute()
        let firstIdentity = identity(generation: 1, epoch: 80)
        let firstListen = Task { @MainActor in
            await service.execute(
                request(identity: firstIdentity, owner: owner, operation: .listen(language: nil)),
                routeResolution: route
            )
        }

        await permission.waitUntilRequestCount(1)
        await service.cancel(firstIdentity)
        guard case .failed(let cancelled) = await firstListen.value else {
            return XCTFail("cancelling a pending permission prompt should settle without awaiting the system sheet")
        }
        XCTAssertEqual(cancelled.kind, .cancelled)

        let secondIdentity = identity(generation: 2, epoch: 80)
        let secondListen = Task { @MainActor in
            await service.execute(
                request(identity: secondIdentity, owner: owner, operation: .listen(language: nil)),
                routeResolution: route
            )
        }
        await permission.waitUntilRequestCount(2)

        await permission.resolve(0, granted: true)
        for _ in 0 ..< 10 { await Task.yield() }
        let diagnostics = await service.diagnostics()
        XCTAssertEqual(diagnostics.activeListenOwner, owner.stableKey)

        let third = await service.execute(
            request(identity: identity(generation: 3, epoch: 80), owner: owner, operation: .listen(language: nil)),
            routeResolution: route
        )
        guard case .failed(let busy) = third else {
            return XCTFail("a late result from the cancelled listen must not clear the newer listen's admission")
        }
        XCTAssertEqual(busy.kind, .busy)

        await service.cancel(secondIdentity)
        await permission.resolve(1, granted: true)
        guard case .failed(let secondCancelled) = await secondListen.value else {
            return XCTFail("the replacement listen should also settle as cancelled")
        }
        XCTAssertEqual(secondCancelled.kind, .cancelled)
    }

    func testSttLatePermissionCallbackCannotClearReplacementAttempt() async {
        let permission = DelayedSpeechAuthorization()
        let stt = SttImpl(speechAuthorization: { await permission.requestPermission() })
        let route = systemPermissionRoute()

        let first = Task {
            try await stt.transcribe(
                language: "en-US",
                automaticEndpointAfterSilence: nil,
                routeResolution: route
            )
        }
        await permission.waitUntilRequestCount(1)
        first.cancel()
        stt.cancelRecognition()

        let second = Task {
            try await stt.transcribe(
                language: "en-US",
                automaticEndpointAfterSilence: nil,
                routeResolution: route
            )
        }
        await permission.waitUntilRequestCount(2)

        await permission.resolve(0, granted: true)
        if case .success = await first.result {
            XCTFail("the first cancelled attempt must not succeed")
        }

        do {
            _ = try await stt.transcribe(
                language: "en-US",
                automaticEndpointAfterSilence: nil,
                routeResolution: route
            )
            XCTFail("the second permission-pending attempt should still own recognition admission")
        } catch let failure as SpeechRecognitionError {
            guard case .Retriable = failure else {
                return XCTFail("expected a busy recognition attempt, got \(failure)")
            }
        } catch {
            XCTFail("expected a busy recognition attempt, got \(error)")
        }

        await permission.resolve(1, granted: false)
        guard case .failure(let failure) = await second.result,
              let audioFailure = failure as? AudioServiceFailure
        else {
            return XCTFail("denying the replacement permission request should end that attempt")
        }
        XCTAssertEqual(audioFailure.localizedDescription, AudioServiceFailure.permissionDenied.localizedDescription)
    }

    func testCancelArrivingBeforeExecuteAdmissionRetiresOnlyThatIdentity() async {
        let gate = DelayedMicrophonePermission()
        let recorder = VoiceImpl(microphoneAuthorization: { await gate.requestPermission() })
        let service = IOSAudioService(recorder: recorder, serviceEpoch: 55, onInvalidation: { _ in })
        let owner = IOSAudioOwner.ui(instanceID: "cancel-first")
        let cancelledIdentity = identity(generation: 90, epoch: 55)

        await service.cancel(cancelledIdentity)
        let cancelledResult = await service.execute(request(
            identity: cancelledIdentity,
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))

        guard case .failed(let cancelledError) = cancelledResult else {
            return XCTFail("a cancel queued before execute admission must prevent native work")
        }
        XCTAssertEqual(cancelledError.kind, .cancelled)
        let permissionWasRequested = await gate.wasRequested
        XCTAssertFalse(permissionWasRequested)

        let freshIdentity = identity(generation: 1, epoch: 55)
        let freshResult = await service.execute(request(
            identity: freshIdentity,
            owner: owner,
            operation: .status(handle: nil)
        ))
        guard case .status(recording: false, playing: false) = freshResult else {
            return XCTFail("the cancellation tombstone must be exact-identity scoped")
        }
    }

    func testLateRecordingStartCleanupCannotClearFreshOwner() async {
        let oldIdentity = identity(generation: 12, epoch: 56)
        let delayedStart = LateStartRecordingDriver(delayedIdentity: oldIdentity.id)
        let service = IOSAudioService(
            recorder: delayedStart,
            coordinator: VoiceAudioSessionCoordinator(sessionDriver: NoopAudioSessionDriver()),
            serviceEpoch: 56,
            onInvalidation: { _ in }
        )
        let oldOwner = IOSAudioOwner.system(instanceID: "old-app")
        let newOwner = IOSAudioOwner.session(sessionID: "new-session")
        let oldStart = Task { @MainActor in
            await service.execute(request(
                identity: oldIdentity,
                owner: oldOwner,
                operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
            ))
        }

        await delayedStart.waitUntilDelayedStartIsPending()
        await service.cancel(oldIdentity)
        guard case .failed(let cancelled) = await oldStart.value else {
            return XCTFail("the old start should settle as cancelled before its driver callback returns")
        }
        XCTAssertEqual(cancelled.kind, .cancelled)

        let newIdentity = identity(generation: 1, epoch: 56)
        let newStart = await service.execute(request(
            identity: newIdentity,
            owner: newOwner,
            operation: .startRecording(sampleRateHz: 16_000, format: "audio/m4a")
        ))
        guard case .recordingStarted(let newHandle) = newStart else {
            return XCTFail("a fresh owner should be able to start while the old permission callback is unresolved")
        }

        await delayedStart.releaseDelayedStart()
        await delayedStart.waitUntilStaleResultWasCleaned()

        let status = await service.execute(request(
            identity: identity(generation: 2, epoch: 56),
            owner: newOwner,
            operation: .status(handle: newHandle)
        ))
        guard case .status(recording: true, playing: false) = status else {
            return XCTFail("late cleanup from the old identity must leave the new owner's recording intact")
        }
    }

    func testRecorderRejectsUnsupportedContainerBeforeRequestingPermission() async {
        let gate = DelayedMicrophonePermission()
        let recorder = VoiceImpl(microphoneAuthorization: { await gate.requestPermission() })

        do {
            _ = try await recorder.startRecordingOwned(
                operationID: UUID().uuidString,
                ownerID: "ui:test",
                sampleRateHz: 16_000,
                format: "audio/wav",
                maximumBytes: 4096
            )
            XCTFail("the native recorder only returns the M4A container it can actually produce")
        } catch let failure as AudioServiceFailure {
            XCTAssertEqual(failure.localizedDescription, AudioServiceFailure.unsupported.localizedDescription)
        } catch {
            XCTFail("unexpected error: \(error)")
        }

        let permissionWasRequested = await gate.wasRequested
        XCTAssertFalse(permissionWasRequested)
    }

    func testRecorderAcceptsBothNativeM4AAliasesBeforePermissionResult() async {
        for format in ["m4a", "audio/m4a"] {
            let recorder = VoiceImpl(microphoneAuthorization: { false })
            do {
                _ = try await recorder.startRecordingOwned(
                    operationID: UUID().uuidString,
                    ownerID: "ui:m4a-alias",
                    sampleRateHz: 16_000,
                    format: format,
                    maximumBytes: 4096
                )
                XCTFail("permission denial should stop this recording")
            } catch let failure as AudioServiceFailure {
                guard case .permissionDenied = failure else {
                    return XCTFail("\(format) should be accepted and reach the permission check, got \(failure)")
                }
            } catch {
                XCTFail("unexpected error for \(format): \(error)")
            }
        }
    }

    func testNativeRecorderAutoFinishAllowsRestartAndKeepsOldFileReadable() async throws {
        let capture = RecorderInstanceCapture()
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: NoopAudioSessionDriver())
        let recorder = VoiceImpl(
            microphoneAuthorization: { true },
            coordinator: coordinator,
            startRecorder: { audioRecorder, _ in
                capture.append(audioRecorder)
                return true
            }
        )
        let firstHandle = try await recorder.startRecordingOwned(
            operationID: "bounded-first",
            ownerID: "ui:bounded-recorder",
            sampleRateHz: 16_000,
            format: "m4a",
            maximumBytes: 4096
        )
        let firstRecorder = try XCTUnwrap(capture.recorder(at: 0))
        try Data([0x41, 0x42]).write(to: try XCTUnwrap(firstRecorder.url))

        recorder.audioRecorderDidFinishRecording(firstRecorder, successfully: true)
        for _ in 0 ..< 100 {
            if await coordinator.activePurpose() == nil { break }
            try await Task.sleep(for: .milliseconds(2))
        }

        let secondHandle = try await recorder.startRecordingOwned(
            operationID: "bounded-second",
            ownerID: "ui:bounded-recorder",
            sampleRateHz: 16_000,
            format: "m4a",
            maximumBytes: 4096
        )
        XCTAssertNotEqual(secondHandle, firstHandle)

        let oldResult = try await recorder.stopRecordingOwned(
            handle: firstHandle,
            ownerID: "ui:bounded-recorder",
            maximumBytes: 4096
        )
        XCTAssertEqual(oldResult.audioBytes, Data([0x41, 0x42]))

        do {
            _ = try await recorder.startRecordingOwned(
                operationID: "bounded-third",
                ownerID: "ui:bounded-recorder",
                sampleRateHz: 16_000,
                format: "m4a",
                maximumBytes: 4096
            )
            XCTFail("stopping the old handle must leave the newer recording admitted")
        } catch let failure as AudioServiceFailure {
            XCTAssertEqual(failure.localizedDescription, AudioServiceFailure.busy.localizedDescription)
        }

        let secondRecorder = try XCTUnwrap(capture.recorder(at: 1))
        try Data([0x43, 0x44]).write(to: try XCTUnwrap(secondRecorder.url))
        let newResult = try await recorder.stopRecordingOwned(
            handle: secondHandle,
            ownerID: "ui:bounded-recorder",
            maximumBytes: 4096
        )
        XCTAssertEqual(newResult.audioBytes, Data([0x43, 0x44]))
    }

    func testAutoFinishedRecorderAdmitsNextRecordingAndRetainsOldStopResult() async throws {
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: NoopAudioSessionDriver())
        let recorder = AutoFinishingRecordingDriver(coordinator: coordinator)
        let service = IOSAudioService(
            recorder: recorder,
            coordinator: coordinator,
            serviceEpoch: 79,
            onInvalidation: { _ in }
        )
        let callback = IOSAudioServiceCallbackAdapter(service: service)
        let owner = IOSAudioOwner.ui(instanceID: "bounded-recording")
        let started = await service.execute(request(
            identity: identity(generation: 1, epoch: 79),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .recordingStarted(let handle) = started else {
            return XCTFail("the native recorder should return an owned handle")
        }
        XCTAssertEqual(readiness(.record, in: callback.capabilities()), .busy)

        await recorder.finishForDurationLimit()

        let purpose = await coordinator.activePurpose()
        XCTAssertNil(purpose, "duration-limited capture must release its audio-session lease")
        for _ in 0 ..< 100 {
            if readiness(.record, in: callback.capabilities()) != .busy { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertNotEqual(readiness(.record, in: callback.capabilities()), .busy)
        XCTAssertFalse(try service.status(owner: owner, handle: handle).recording)

        let secondStart = await service.execute(request(
            identity: identity(generation: 2, epoch: 79),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .recordingStarted(let secondHandle) = secondStart else {
            return XCTFail("a recorder that auto-finished must release admission for the next capture")
        }
        XCTAssertNotEqual(secondHandle, handle)
        XCTAssertEqual(readiness(.record, in: callback.capabilities()), .busy)

        let stopped = await service.execute(request(
            identity: identity(generation: 3, epoch: 79),
            owner: owner,
            operation: .stopRecording(handle: handle)
        ))
        guard case .recording(let data, let mimeType) = stopped else {
            return XCTFail("a bounded auto-stop should preserve its final file for StopRecording")
        }
        XCTAssertEqual(data, Data([0x41, 0x42]))
        XCTAssertEqual(mimeType, "audio/m4a")
        XCTAssertTrue(try service.status(owner: owner, handle: secondHandle).recording)

        let stoppedSecond = await service.execute(request(
            identity: identity(generation: 4, epoch: 79),
            owner: owner,
            operation: .stopRecording(handle: secondHandle)
        ))
        guard case .recording(let secondData, let secondMimeType) = stoppedSecond else {
            return XCTFail("stopping the new recording should leave its own file available")
        }
        XCTAssertEqual(secondData, Data([0x41, 0x42]))
        XCTAssertEqual(secondMimeType, "audio/m4a")
    }

    func testInvalidationWaitsForServiceCleanupBeforeLeaseCanBeReacquired() async throws {
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: NoopAudioSessionDriver())
        let resource = TestAudioResource()
        let service = IOSAudioService(
            coordinator: coordinator,
            serviceEpoch: 63,
            onInvalidation: { event in
                await resource.stop()
                await coordinator.release(event.lease)
            }
        )
        _ = await service.diagnostics()
        let lease = try await coordinator.acquire(.recording)

        for _ in 0 ..< 20 {
            if await coordinator.invalidationSubscriberCount() > 0 { break }
            await Task.yield()
        }

        await coordinator.suspendForBackground()
        await resource.waitUntilStopped()

        let resourceIsActive = await resource.isActive
        XCTAssertFalse(resourceIsActive)
        let oldLeaseIsStillCurrent = await coordinator.owns(lease)
        XCTAssertFalse(oldLeaseIsStillCurrent)
        let replacement = try await coordinator.acquire(.playback)
        await coordinator.release(replacement)
    }

    func testFailedRecordingFinalizationRetiresTheStoppedHandle() async throws {
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: NoopAudioSessionDriver())
        let recorder = AutoFinishingRecordingDriver(coordinator: coordinator)
        let service = IOSAudioService(
            recorder: recorder,
            coordinator: coordinator,
            serviceEpoch: 84,
            onInvalidation: { _ in }
        )
        let owner = IOSAudioOwner.session(sessionID: "failed-finalization")
        let started = await service.execute(request(
            identity: identity(generation: 1, epoch: 84),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .recordingStarted(let handle) = started else {
            return XCTFail("recording should start")
        }
        recorder.failNextStopOnce()

        let stopped = await service.execute(request(
            identity: identity(generation: 2, epoch: 84),
            owner: owner,
            operation: .stopRecording(handle: handle)
        ))
        guard case .failed = stopped else {
            return XCTFail("file finalization failure should be reported")
        }
        XCTAssertThrowsError(try service.status(owner: owner, handle: handle))
        let replacement = await service.execute(request(
            identity: identity(generation: 3, epoch: 84),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .recordingStarted = replacement else {
            return XCTFail("a failed file read must not keep the old recording registered")
        }
        await service.endOwner(owner)
    }

    func testStaleInvalidationDoesNotStopReplacementRecording() async throws {
        let coordinator = VoiceAudioSessionCoordinator(sessionDriver: NoopAudioSessionDriver())
        let recorder = AutoFinishingRecordingDriver(coordinator: coordinator)
        let service = IOSAudioService(
            recorder: recorder,
            coordinator: coordinator,
            serviceEpoch: 85,
            onInvalidation: { _ in }
        )
        let oldLease = try await coordinator.acquire(.recording)
        await coordinator.release(oldLease)
        let owner = IOSAudioOwner.session(sessionID: "replacement-recording")
        let started = await service.execute(request(
            identity: identity(generation: 1, epoch: 85),
            owner: owner,
            operation: .startRecording(sampleRateHz: 16_000, format: "m4a")
        ))
        guard case .recordingStarted(let handle) = started else {
            return XCTFail("replacement recording should start")
        }

        await service.handleInvalidation(.init(lease: oldLease, reason: .routeChange))
        XCTAssertTrue(try service.status(owner: owner, handle: handle).recording)
        await service.endOwner(owner)
    }

    func testCapabilitiesCallbackIsSafeFromBackgroundThread() async {
        let callback = IOSAudioServiceCallbackAdapter.shared
        let snapshot = await Task.detached {
            let capabilities = callback.capabilities()
            return (
                capabilities.serviceEpoch,
                capabilities.maxPayloadBytes,
                capabilities.supportedOperations.count,
                capabilities.readiness.count
            )
        }.value

        XCTAssertGreaterThan(snapshot.0, 0)
        XCTAssertEqual(snapshot.1, maxAudioPayloadBytes())
        XCTAssertEqual(snapshot.2, 6)
        XCTAssertEqual(snapshot.3, 6)
    }

    func testCapabilityCacheRefreshesAfterConfigurationChangeAndStaysStable() async throws {
        let defaultsName = "IOSAudioServiceCapabilityTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: defaultsName)!
        defaults.removePersistentDomain(forName: defaultsName)
        let store = AudioConfigurationStore(defaults: defaults)
        let service = IOSAudioService(
            configurationStore: store,
            serviceEpoch: 78,
            onInvalidation: { _ in }
        )
        let callback = IOSAudioServiceCallbackAdapter(service: service)
        let initial = callback.capabilities()

        var unavailableConfiguration = store.configuration
        unavailableConfiguration.speech = AudioSpeechPreference(
            source: AudioSource(rawValue: "future-provider")
        )
        _ = try store.save(unavailableConfiguration, expectedRevision: store.revision)

        var refreshed = callback.capabilities()
        for _ in 0 ..< 100 {
            refreshed = callback.capabilities()
            if refreshed.supportRevision > initial.supportRevision,
               refreshed.readiness.first(where: { $0.operation == .speak })?.state == .unavailable {
                break
            }
            try await Task.sleep(for: .milliseconds(5))
        }

        XCTAssertGreaterThan(refreshed.supportRevision, initial.supportRevision)
        XCTAssertEqual(
            refreshed.readiness.first(where: { $0.operation == .speak })?.state,
            .unavailable
        )
        let settledRevision = refreshed.supportRevision
        for _ in 0 ..< 100 {
            XCTAssertEqual(callback.capabilities().supportRevision, settledRevision)
        }
    }

    func testRawRecordingCapabilityIsBusyDuringListenOrPlaybackOwnership() {
        XCTAssertEqual(
            IOSAudioService.rawRecordingReadiness(
                microphoneReadiness: .ready,
                recordingOwned: false,
                listening: true,
                playback: false,
                systemRender: false
            ),
            .busy
        )
        XCTAssertEqual(
            IOSAudioService.rawRecordingReadiness(
                microphoneReadiness: .ready,
                recordingOwned: false,
                listening: false,
                playback: true,
                systemRender: false
            ),
            .busy
        )
        XCTAssertEqual(
            IOSAudioService.rawRecordingReadiness(
                microphoneReadiness: .needsPermission,
                recordingOwned: false,
                listening: false,
                playback: false,
                systemRender: false
            ),
            .needsPermission
        )
    }

    func testGeneratedCallbackPreservesOwnerScopedStatusResult() async {
        let service = IOSAudioService(serviceEpoch: 77, onInvalidation: { _ in })
        let callback = IOSAudioServiceCallbackAdapter(service: service)
        let request = AudioOperationRequestDto(
            identity: AudioOperationIdDto(
                id: UUID().uuidString.lowercased(),
                generation: 1,
                serviceEpoch: 77
            ),
            owner: .session(sessionId: "session-a"),
            initiator: AudioInitiatorDto(agentId: "agent", toolUseId: "tool", requestId: nil),
            timeoutBudgetMs: nil,
            maxPayloadBytes: maxAudioPayloadBytes(),
            operation: .status(handle: nil)
        )

        let result = await callback.execute(request: request)
        guard case let .status(status) = result else {
            return XCTFail("the adapter should preserve owner-scoped status as a typed result")
        }
        XCTAssertFalse(status.recording)
        XCTAssertFalse(status.playing)
        let diagnostics = await service.diagnostics()
        XCTAssertEqual(diagnostics.lastOperation?.operation, "status")
        XCTAssertEqual(diagnostics.lastOperation?.configurationRevision, diagnostics.configurationRevision)
        XCTAssertNil(diagnostics.activeLeasePurpose)
    }

    private func request(
        identity: IOSAudioOperationIdentity,
        owner: IOSAudioOwner,
        timeoutBudgetMs: UInt64? = nil,
        operation: IOSAudioOperation
    ) -> IOSAudioOperationRequest {
        IOSAudioOperationRequest(
            identity: identity,
            owner: owner,
            initiator: nil,
            timeoutBudgetMs: timeoutBudgetMs,
            maxPayloadBytes: 16_384,
            operation: operation
        )
    }

    private func identity(generation: UInt64, epoch: UInt64) -> IOSAudioOperationIdentity {
        IOSAudioOperationIdentity(
            id: UUID().uuidString.lowercased(),
            generation: generation,
            serviceEpoch: epoch
        )
    }

    private func systemPermissionRoute() -> AudioRouteResolution {
        AudioRouteResolution(
            requested: .init(source: .system, offlineModelId: nil, voice: nil),
            effective: .init(source: .system, modelId: nil, voiceId: nil),
            status: .permissionRequired,
            reason: "systemPermissionRequired",
            fallbackReason: nil
        )
    }

    private func readiness(
        _ operation: AudioOperationKindDto,
        in snapshot: AudioCapabilitySnapshotDto
    ) -> AudioReadinessStateDto? {
        snapshot.readiness.first(where: { $0.operation == operation })?.state
    }
}

private actor DelayedMicrophonePermission {
    private var permissionContinuation: CheckedContinuation<Bool, Never>?
    private var requestContinuation: CheckedContinuation<Void, Never>?
    private(set) var wasRequested = false

    func requestPermission() async -> Bool {
        wasRequested = true
        requestContinuation?.resume()
        requestContinuation = nil
        return await withCheckedContinuation { permissionContinuation = $0 }
    }

    func waitUntilPermissionWasRequested() async {
        if wasRequested { return }
        await withCheckedContinuation { requestContinuation = $0 }
    }

    func resolve(_ granted: Bool) {
        permissionContinuation?.resume(returning: granted)
        permissionContinuation = nil
    }
}

private actor DelayedSpeechAuthorization {
    private var permissionContinuations: [Int: CheckedContinuation<Bool, Never>] = [:]
    private var requestCountContinuations: [(Int, CheckedContinuation<Void, Never>)] = []
    private(set) var requestCount = 0

    func requestPermission() async -> Bool {
        let index = requestCount
        requestCount += 1
        let ready = requestCountContinuations.filter { $0.0 <= requestCount }
        requestCountContinuations.removeAll { $0.0 <= requestCount }
        ready.forEach { $0.1.resume() }
        return await withCheckedContinuation { permissionContinuations[index] = $0 }
    }

    func waitUntilRequestCount(_ expected: Int) async {
        if requestCount >= expected { return }
        await withCheckedContinuation { requestCountContinuations.append((expected, $0)) }
    }

    func resolve(_ index: Int, granted: Bool) {
        permissionContinuations.removeValue(forKey: index)?.resume(returning: granted)
    }
}

private final class RecorderInstanceCapture: @unchecked Sendable {
    private let lock = NSLock()
    private var recorders: [AVAudioRecorder] = []

    func append(_ recorder: AVAudioRecorder) {
        lock.withLock { recorders.append(recorder) }
    }

    func recorder(at index: Int) -> AVAudioRecorder? {
        lock.withLock { recorders.indices.contains(index) ? recorders[index] : nil }
    }
}

private actor TestAudioResource {
    private(set) var isActive = true
    private var stoppedContinuation: CheckedContinuation<Void, Never>?

    func stop() {
        isActive = false
        stoppedContinuation?.resume()
        stoppedContinuation = nil
    }

    func waitUntilStopped() async {
        if !isActive { return }
        await withCheckedContinuation { stoppedContinuation = $0 }
    }
}

private actor ExecutionGate {
    private var continuation: CheckedContinuation<Void, Never>?
    private var released = false

    func wait() async {
        if released { return }
        await withCheckedContinuation { continuation = $0 }
    }

    func release() {
        released = true
        continuation?.resume()
        continuation = nil
    }
}

private actor LateStartGate {
    private var releaseContinuation: CheckedContinuation<Void, Never>?
    private var pendingContinuation: CheckedContinuation<Void, Never>?
    private var isPending = false
    private var isReleased = false

    func blockUntilReleased() async {
        isPending = true
        pendingContinuation?.resume()
        pendingContinuation = nil
        if isReleased { return }
        await withCheckedContinuation { releaseContinuation = $0 }
    }

    func waitUntilPending() async {
        if isPending { return }
        await withCheckedContinuation { pendingContinuation = $0 }
    }

    func release() {
        isReleased = true
        releaseContinuation?.resume()
        releaseContinuation = nil
    }
}

private actor LateStartCleanupSignal {
    private var continuation: CheckedContinuation<Void, Never>?
    private var signalled = false

    func signal() {
        signalled = true
        continuation?.resume()
        continuation = nil
    }

    func wait() async {
        if signalled { return }
        await withCheckedContinuation { continuation = $0 }
    }
}

private final class LateStartRecordingDriver: AudioRecordingDriving, @unchecked Sendable {
    private struct Recording {
        let ownerID: String
        let operationID: String
    }

    private let lock = NSLock()
    private let delayedIdentity: String
    private let startGate = LateStartGate()
    private let cleanupSignal = LateStartCleanupSignal()
    private var recordings: [String: Recording] = [:]
    private var startedIdentities = Set<String>()
    private var cancelCounts: [String: Int] = [:]

    init(delayedIdentity: String) {
        self.delayedIdentity = delayedIdentity
    }

    func startRecordingOwned(
        operationID: String,
        ownerID: String,
        sampleRateHz _: UInt32,
        format _: String,
        maximumBytes _: UInt64
    ) async throws -> String {
        if operationID == delayedIdentity { await startGate.blockUntilReleased() }
        let handle = "handle-\(operationID)"
        lock.withLock {
            recordings[handle] = Recording(ownerID: ownerID, operationID: operationID)
            startedIdentities.insert(operationID)
        }
        return handle
    }

    func stopRecordingOwned(handle: String, ownerID: String, maximumBytes _: UInt64) async throws -> IOSAudioRecording {
        let recording = lock.withLock { () -> IOSAudioRecording? in
            guard recordings[handle]?.ownerID == ownerID else { return nil }
            recordings.removeValue(forKey: handle)
            return IOSAudioRecording(audioBytes: Data(), mimeType: "audio/m4a")
        }
        guard let recording else { throw AudioServiceFailure.notRecording }
        return recording
    }

    func isRecordingOwned(handle: String?, ownerID: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return recordings.contains { candidate, recording in
            recording.ownerID == ownerID && (handle == nil || handle == candidate)
        }
    }

    func cancel(startOperationID: String) async {
        let cancelledAfterLateStart = lock.withLock { () -> Bool in
            cancelCounts[startOperationID, default: 0] += 1
            let didStart = startedIdentities.contains(startOperationID)
            recordings = recordings.filter { $0.value.operationID != startOperationID }
            return didStart && cancelCounts[startOperationID, default: 0] >= 2
        }
        if startOperationID == delayedIdentity, cancelledAfterLateStart { await cleanupSignal.signal() }
    }

    func end(ownerID: String) async {
        lock.withLock { recordings = recordings.filter { $0.value.ownerID != ownerID } }
    }

    func stopAll() async {
        lock.withLock { recordings.removeAll() }
    }

    func waitUntilDelayedStartIsPending() async { await startGate.waitUntilPending() }
    func releaseDelayedStart() async { await startGate.release() }
    func waitUntilStaleResultWasCleaned() async { await cleanupSignal.wait() }
}

private final class EndOwnerGateRecordingDriver: AudioRecordingDriving, @unchecked Sendable {
    private struct Recording {
        let operationID: String
        let ownerID: String
    }

    private let lock = NSLock()
    private let cleanupGate = LateStartGate()
    private var recordings: [String: Recording] = [:]
    private var starts = 0

    var startCallCount: Int { lock.withLock { starts } }

    func startRecordingOwned(
        operationID: String,
        ownerID: String,
        sampleRateHz _: UInt32,
        format _: String,
        maximumBytes _: UInt64
    ) async throws -> String {
        let handle = UUID().uuidString
        lock.withLock {
            starts += 1
            recordings[handle] = Recording(operationID: operationID, ownerID: ownerID)
        }
        return handle
    }

    func stopRecordingOwned(handle: String, ownerID: String, maximumBytes _: UInt64) async throws -> IOSAudioRecording {
        let found = lock.withLock {
            guard recordings[handle]?.ownerID == ownerID else { return false }
            recordings.removeValue(forKey: handle)
            return true
        }
        guard found else { throw AudioServiceFailure.notRecording }
        return IOSAudioRecording(audioBytes: Data(), mimeType: "audio/m4a")
    }

    func isRecordingOwned(handle: String?, ownerID: String) -> Bool {
        lock.withLock {
            recordings.contains { candidate, recording in
                recording.ownerID == ownerID && (handle == nil || candidate == handle)
            }
        }
    }

    func cancel(startOperationID: String) async {
        lock.withLock {
            recordings = recordings.filter { $0.value.operationID != startOperationID }
        }
    }

    func end(ownerID: String) async {
        await cleanupGate.blockUntilReleased()
        lock.withLock { recordings = recordings.filter { $0.value.ownerID != ownerID } }
    }

    func stopAll() async {
        lock.withLock { recordings.removeAll() }
    }

    func waitUntilEndOwnerCleanupIsPending() async { await cleanupGate.waitUntilPending() }
    func releaseEndOwnerCleanup() async { await cleanupGate.release() }
}

private final class AutoFinishingRecordingDriver: AudioRecordingDriving, @unchecked Sendable {
    private struct Recording {
        let operationID: String
        let ownerID: String
        var isActive: Bool
        var lease: VoiceAudioSessionCoordinator.Lease?
    }

    private let lock = NSLock()
    private let coordinator: VoiceAudioSessionCoordinator
    private var onActivityChange: (@Sendable () -> Void)?
    private var activeHandle: String?
    private var recordings: [String: Recording] = [:]
    private var failNextStop = false

    init(coordinator: VoiceAudioSessionCoordinator) {
        self.coordinator = coordinator
    }

    func failNextStopOnce() {
        lock.withLock { failNextStop = true }
    }

    func installActivityChangeHandler(_ handler: (@Sendable () -> Void)?) {
        lock.withLock { onActivityChange = handler }
    }

    func startRecordingOwned(
        operationID: String,
        ownerID: String,
        sampleRateHz _: UInt32,
        format _: String,
        maximumBytes _: UInt64
    ) async throws -> String {
        let acquiredLease = try await coordinator.acquire(.recording)
        let startedHandle = UUID().uuidString
        lock.withLock {
            activeHandle = startedHandle
            recordings[startedHandle] = Recording(
                operationID: operationID,
                ownerID: ownerID,
                isActive: true,
                lease: acquiredLease
            )
        }
        return startedHandle
    }

    func stopRecordingOwned(handle: String, ownerID: String, maximumBytes _: UInt64) async throws -> IOSAudioRecording {
        let (found, acquiredLease) = lock.withLock { () -> (Bool, VoiceAudioSessionCoordinator.Lease?) in
            guard let recording = recordings[handle], recording.ownerID == ownerID else { return (false, nil) }
            recordings.removeValue(forKey: handle)
            if activeHandle == handle { activeHandle = nil }
            return (true, recording.lease)
        }
        guard found else { throw AudioServiceFailure.notRecording }
        if let acquiredLease { await coordinator.release(acquiredLease) }
        let shouldFail = lock.withLock { () -> Bool in
            let shouldFail = failNextStop
            failNextStop = false
            return shouldFail
        }
        if shouldFail { throw AudioServiceFailure.nativeFailure("recording file could not be read") }
        return IOSAudioRecording(audioBytes: Data([0x41, 0x42]), mimeType: "audio/m4a")
    }

    func isRecordingOwned(handle: String?, ownerID: String) -> Bool {
        lock.withLock {
            recordings.contains { candidate, recording in
                recording.isActive && recording.ownerID == ownerID && (handle == nil || candidate == handle)
            }
        }
    }

    func cancel(startOperationID: String) async {
        let acquiredLease = lock.withLock { () -> VoiceAudioSessionCoordinator.Lease? in
            guard let entry = recordings.first(where: { $0.value.operationID == startOperationID }) else { return nil }
            recordings.removeValue(forKey: entry.key)
            if activeHandle == entry.key { activeHandle = nil }
            return entry.value.lease
        }
        if let acquiredLease { await coordinator.release(acquiredLease) }
    }

    func end(ownerID: String) async {
        let startIDs = lock.withLock {
            recordings.values.filter { $0.ownerID == ownerID }.map(\.operationID)
        }
        for startID in startIDs { await cancel(startOperationID: startID) }
    }

    func stopAll() async {
        let startIDs = lock.withLock { recordings.values.map(\.operationID) }
        for startID in startIDs { await cancel(startOperationID: startID) }
    }

    func finishForDurationLimit() async {
        let (releasedLease, callback) = lock.withLock { () -> (VoiceAudioSessionCoordinator.Lease?, (@Sendable () -> Void)?) in
            guard let activeHandle, var recording = recordings[activeHandle] else {
                return (nil, onActivityChange)
            }
            recording.isActive = false
            let lease = recording.lease
            recording.lease = nil
            recordings[activeHandle] = recording
            self.activeHandle = nil
            return (lease, onActivityChange)
        }
        if let releasedLease { await coordinator.release(releasedLease) }
        callback?()
    }
}

private struct NoopAudioSessionDriver: VoiceAudioSessionDriving {
    func activate(_: VoiceAudioSessionCoordinator.Purpose) throws {}
    func deactivate() {}
}
