import Foundation
import Security

// These module-local SecItem functions replace every Keychain call made by the
// production CredentialStore in this test binary. No real Keychain is accessed.
final class FakeKeychain: @unchecked Sendable {
    private let lock = NSLock()
    private var payload: Data? = Data("old-fake-secret".utf8)
    private var mutations = 0
    private var blockNextRead = true
    let readStarted = DispatchSemaphore(value: 0)
    let releaseRead = DispatchSemaphore(value: 0)
    let deleteStarted = DispatchSemaphore(value: 0)

    func read(_ query: CFDictionary, _ result: UnsafeMutablePointer<CFTypeRef?>?) -> OSStatus {
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
        let attributes = query as NSDictionary
        if attributes[kSecReturnAttributes] as? Bool == true {
            result?.pointee = [[kSecAttrAccount as String: "anthropic"]] as CFArray
        } else {
            result?.pointee = value as CFData
        }
        return errSecSuccess
    }

    func snapshot() -> (Data?, Int) {
        lock.lock(); defer { lock.unlock() }
        return (payload, mutations)
    }

    func delete() -> OSStatus {
        deleteStarted.signal()
        lock.lock()
        defer { lock.unlock() }
        mutations += 1
        payload = nil
        return errSecSuccess
    }

    func update(_ attributes: CFDictionary, insert: Bool) -> OSStatus {
        lock.lock()
        defer { lock.unlock() }
        mutations += 1
        if !insert && payload == nil { return errSecItemNotFound }
        payload = (attributes as NSDictionary)[kSecValueData] as? Data
        return errSecSuccess
    }
}

let fakeKeychain = FakeKeychain()

func SecItemCopyMatching(_ query: CFDictionary, _ result: UnsafeMutablePointer<CFTypeRef?>?) -> OSStatus {
    fakeKeychain.read(query, result)
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
            BrokerRequest(op: op, service: service, account: "anthropic", payload: payload)
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
        let beforeRefresh = fakeKeychain.snapshot()
        for _ in 0..<3 {
            let listed = await store.handle(BrokerRequest(op: "list", service: service, account: nil, payload: nil))
            let anthropicPreview = await store.handle(BrokerRequest(op: "preview", service: service, account: "anthropic", payload: nil))
            let contains = await store.handle(BrokerRequest(op: "contains", service: service, account: "anthropic", payload: nil))
            precondition(listed.ok && listed.accounts == ["anthropic"])
            precondition(anthropicPreview.ok && anthropicPreview.present == true)
            precondition(contains.ok && contains.present == true)
        }
        let afterRefresh = fakeKeychain.snapshot()
        precondition(beforeRefresh.0 == afterRefresh.0 && beforeRefresh.1 == afterRefresh.1,
                     "Anthropic status refresh must not modify or rewrite Keychain records")
        print("Anthropic status/list/preview checks remained read-only")
        let deletedAgain = await store.handle(request("delete"))
        let preview = await store.handle(request("preview"))
        precondition(deletedAgain.ok && preview.present == false && preview.payload == nil)
        print("CredentialStore serialized read/delete and cache replacement checks passed")
    }
}
