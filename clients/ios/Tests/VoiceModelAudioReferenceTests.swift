import XCTest
@testable import LingxiCode

@MainActor
final class VoiceModelAudioReferenceTests: XCTestCase {
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
