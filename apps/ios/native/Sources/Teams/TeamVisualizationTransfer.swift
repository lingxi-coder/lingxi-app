import CryptoKit
import Foundation

@MainActor
enum TeamVisualizationTransfer {
    static let maximumBytes = 50 * 1024 * 1024
    static let partBytes = 2 * 1024 * 1024
    static let editableChars = 128 * 1024
    typealias Request = ([String: Any]) async throws -> [String: Any]

    struct Prepared {
        let manifest: [String: Any]
        let payload: Data
    }

    private static func digest(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    static func prepare(_ bundle: [String: Any]) throws -> Prepared {
        guard let title = bundle["title"] as? String, let html = bundle["html"] as? String,
              !html.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw TeamMessageFailure(code: "INVALID_DATA")
        }
        let htmlBytes = html.utf8.count
        let assets = bundle["assets"] as? [String: [String: String]] ?? [:]
        let assetValues = try assets.keys.sorted().map { name -> (String, String, String) in
            guard let value = assets[name], let mime = value["mime"], let data = value["data"] else {
                throw TeamMessageFailure(code: "INVALID_DATA")
            }
            return (name, mime, data)
        }
        let bytes = assetValues.reduce(htmlBytes) { $0 + $1.2.utf8.count }
        guard bytes <= maximumBytes else { throw TeamMessageFailure(code: "VISUALIZATION_TOO_LARGE") }
        var payload = Data()
        payload.reserveCapacity(bytes)
        payload.append(contentsOf: html.utf8)
        var manifestAssets: [[String: Any]] = []
        for (name, mime, data) in assetValues {
            manifestAssets.append(["name": name, "mime": mime, "bytes": data.utf8.count])
            payload.append(contentsOf: data.utf8)
        }
        let parts = stride(from: 0, to: bytes, by: partBytes).map { start in
            digest(payload[start..<min(start + partBytes, bytes)])
        }
        return Prepared(manifest: ["title": title, "htmlBytes": htmlBytes, "assets": manifestAssets, "parts": parts], payload: payload)
    }

    static func upload(_ transfer: Prepared, plan: [String: Any], request: Request) async throws {
        guard let message = plan["message"] as? String, let version = plan["version"] as? String,
              let missing = plan["missing"] as? [Int], let parts = transfer.manifest["parts"] as? [String],
              missing.count <= parts.count, missing.allSatisfy(parts.indices.contains) else {
            throw TeamMessageFailure(code: "INVALID_DATA")
        }
        for index in missing {
            try Task.checkCancellation()
            let start = index * partBytes
            let chunk = transfer.payload[start..<min(start + partBytes, transfer.payload.count)]
            _ = try await request(["action": "putPart", "message": message, "version": version, "index": index,
                                   "data": chunk.base64EncodedString()])
        }
    }

    static func open(_ request: Request) async throws -> [String: Any] {
        let opened = try await request(["action": "open"])
        guard let manifest = opened["package"] as? [String: Any], let title = manifest["title"] as? String,
              let htmlBytes = manifest["htmlBytes"] as? Int, (1...maximumBytes).contains(htmlBytes),
              let assets = manifest["assets"] as? [[String: Any]], assets.count <= 32,
              let parts = manifest["parts"] as? [String], parts.count <= maximumBytes / partBytes,
              let state = opened["state"] else { throw TeamMessageFailure(code: "INVALID_DATA") }
        var total = htmlBytes
        for asset in assets {
            guard let bytes = asset["bytes"] as? Int, (0...(maximumBytes - total)).contains(bytes) else {
                throw TeamMessageFailure(code: "INVALID_DATA")
            }
            total += bytes
        }
        guard parts.count == (total + partBytes - 1) / partBytes else { throw TeamMessageFailure(code: "INVALID_DATA") }
        var payload = Data()
        payload.reserveCapacity(total)
        for (index, expected) in parts.enumerated() {
            try Task.checkCancellation()
            let value = try await request(["action": "readPart", "index": index])
            let expectedSize = min(partBytes, total - payload.count)
            guard let encoded = value["data"] as? String, encoded.utf8.count == (expectedSize + 2) / 3 * 4,
                  let bytes = Data(base64Encoded: encoded), bytes.count == expectedSize, digest(bytes) == expected else {
                throw TeamMessageFailure(code: "INVALID_DATA")
            }
            payload.append(bytes)
        }
        guard let html = String(data: payload[..<htmlBytes], encoding: .utf8),
              !html.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw TeamMessageFailure(code: "INVALID_DATA")
        }
        var bundleAssets: [String: [String: String]] = [:]
        var start = htmlBytes
        for asset in assets {
            guard let name = asset["name"] as? String, let mime = asset["mime"] as? String,
                  let count = asset["bytes"] as? Int, bundleAssets[name] == nil,
                  let data = String(data: payload[start..<(start + count)], encoding: .ascii) else {
                throw TeamMessageFailure(code: "INVALID_DATA")
            }
            bundleAssets[name] = ["mime": mime, "data": data]
            start += count
        }
        return ["bundle": ["title": title, "html": html, "assets": bundleAssets], "state": state]
    }
}
