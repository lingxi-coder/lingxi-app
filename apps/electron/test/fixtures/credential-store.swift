import Foundation
import Security

// These module-local SecItem functions replace every Keychain call made by the
// production CredentialStore in this test binary. No real Keychain is accessed.
final class FakeKeychain: @unchecked Sendable {
    private let lock = NSLock()
    private var payload: Data? = Data("old-fake-secret".utf8)
    private var blockNextRead = true
    let readStarted = DispatchSemaphore(value: 0)
    let releaseRead = DispatchSemaphore(value: 0)
    let deleteStarted = DispatchSemaphore(value: 0)

    func read(_ result: UnsafeMutablePointer<CFTypeRef?>?) -> OSStatus {
        lock.lock()
        let value = payload
        let shouldBlock = blockNextRead
        blockNextRead = false
        lock.unlock()
        if shouldBlock {
            readStarted.signal()
            precondition(releaseRead.wait(timeout: .now() + 5) == .success, "test read was never released")
        }
        guard let value else { return errSecItemNotFound }
        result?.pointee = value as CFData
        return errSecSuccess
    }

    func delete() -> OSStatus {
        deleteStarted.signal()
        lock.lock()
        defer { lock.unlock() }
        payload = nil
        return errSecSuccess
    }

    func update(_ attributes: CFDictionary, insert: Bool) -> OSStatus {
        lock.lock()
        defer { lock.unlock() }
        if !insert && payload == nil { return errSecItemNotFound }
        payload = (attributes as NSDictionary)[kSecValueData] as? Data
        return errSecSuccess
    }
}

let fakeKeychain = FakeKeychain()

func SecItemCopyMatching(_ query: CFDictionary, _ result: UnsafeMutablePointer<CFTypeRef?>?) -> OSStatus {
    fakeKeychain.read(result)
}
func SecItemDelete(_ query: CFDictionary) -> OSStatus { fakeKeychain.delete() }
func SecItemUpdate(_ query: CFDictionary, _ attributes: CFDictionary) -> OSStatus {
    fakeKeychain.update(attributes, insert: false)
}
func SecItemAdd(_ query: CFDictionary, _ result: UnsafeMutablePointer<CFTypeRef?>?) -> OSStatus {
    fakeKeychain.update(query, insert: true)
}

func waitForSignal(_ semaphore: DispatchSemaphore, seconds: Double) async -> Bool {
    await withCheckedContinuation { continuation in
        DispatchQueue.global().async {
            continuation.resume(returning: semaphore.wait(timeout: .now() + seconds) == .success)
        }
    }
}

@main
struct CredentialStoreRegression {
    static func main() async {
        let store = CredentialStore(manifest: BrokerManifest(version: "test", protocolVersion: 1, channel: "development"))
        let service = "com.lingxi.provider-credentials.v1.development"
        func request(_ op: String, payload: String? = nil) -> BrokerRequest {
            BrokerRequest(op: op, service: service, account: "openai", payload: payload)
        }
        let retrieval = Task.detached { await store.handle(request("retrieve")) }
        let readStarted = await waitForSignal(fakeKeychain.readStarted, seconds: 5)
        precondition(readStarted, "retrieve did not enter fake Keychain")
        let deletionRequested = DispatchSemaphore(value: 0)
        let deletion = Task.detached {
            deletionRequested.signal()
            return await store.handle(request("delete"))
        }
        let requested = await waitForSignal(deletionRequested, seconds: 5)
        precondition(requested)
        let deleteEnteredBeforeReadFinished = await waitForSignal(fakeKeychain.deleteStarted, seconds: 0.1)
        fakeKeychain.releaseRead.signal()
        let beforeDelete = await retrieval.value
        let deleteResult = await deletion.value
        let afterDelete = await store.handle(request("retrieve"))
        precondition(!deleteEnteredBeforeReadFinished, "delete interleaved with the Keychain/cache read transaction")
        precondition(beforeDelete.ok && beforeDelete.payload == "old-fake-secret")
        precondition(deleteResult.ok)
        precondition(afterDelete.ok && afterDelete.present == false && afterDelete.payload == nil,
                     "a completed delete left the old credential in cache")

        let stored = await store.handle(request("store", payload: "new-fake-secret"))
        let replacement = await store.handle(request("retrieve"))
        precondition(stored.ok && replacement.payload == "new-fake-secret")
        let deletedAgain = await store.handle(request("delete"))
        let preview = await store.handle(request("preview"))
        precondition(deletedAgain.ok && preview.present == false && preview.payload == nil)
        print("CredentialStore serialized read/delete and cache replacement checks passed")
    }
}
