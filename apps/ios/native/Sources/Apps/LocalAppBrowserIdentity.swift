import Foundation
import CryptoKit

/// Browser identity belongs to an installation, never a release, run or account.
enum LocalAppBrowserIdentity {
    static func storeID(_ appID: String) -> UUID {
        let bytes = Array(SHA256.hash(data: Data(("lingxi.local-app.v2:" + appID).utf8)).prefix(16))
        return UUID(uuid: (bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]))
    }
    static func matches(_ url: URL?, origin: URL) -> Bool {
        guard let url else { return false }
        return url.scheme == "http" && url.host == "127.0.0.1" && url.port == origin.port && url.user == nil && url.password == nil
    }
}
