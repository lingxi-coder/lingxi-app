import XCTest
@testable import LingxiCode

@MainActor
final class VoiceModelAudioReferenceTests: XCTestCase {
    func testCancelledDownloadFinishesBeforeRetryStarts() async throws {
        var continuations: [CheckedContinuation<Void, Never>] = []
        let store = VoiceModelStore(downloadOverride: { _ in
            await withCheckedContinuation { continuations.append($0) }
            throw CancellationError()
        })
        let model = try XCTUnwrap(GeneratedVoiceModelCatalog.all.first)
        store.download(model)
        for _ in 0..<100 where continuations.count < 1 { try await Task.sleep(for: .milliseconds(1)) }
        XCTAssertEqual(continuations.count, 1)
        store.cancel(model.id)
        store.download(model)
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(continuations.count, 1, "Retry must wait for old archive IO")
        XCTAssertEqual(store.state(for: model.id), .queued)
        continuations.first?.resume()
        for _ in 0..<100 where continuations.count < 2 { try await Task.sleep(for: .milliseconds(1)) }
        XCTAssertEqual(continuations.count, 2)
        XCTAssertEqual(store.state(for: model.id), .queued, "Old cancellation must not overwrite the retry")
        store.download(model)
        for _ in 0..<10 { await Task.yield() }
        XCTAssertEqual(continuations.count, 2, "Old cleanup must not remove the new task")
        store.cancel(model.id)
        store.download(model)
        continuations.last?.resume()
        for _ in 0..<100 where continuations.count < 3 { try await Task.sleep(for: .milliseconds(1)) }
        XCTAssertEqual(continuations.count, 3, "Retry task must remain cancellable")
        store.cancel(model.id)
        continuations.last?.resume()
    }

    func testActiveAudioReferencePreventsModelRemovalUntilReleased() throws {
        let store = VoiceModelStore()
        let model = try XCTUnwrap(GeneratedVoiceModelCatalog.all.first(where: { $0.kind == .tts }))
        let reference = store.retainForAudioUse(model.id)

        XCTAssertFalse(store.canRemove(model.id))
        XCTAssertFalse(store.remove(model))

        store.releaseAudioUse(reference)
        XCTAssertTrue(store.canRemove(model.id))
    }
}
