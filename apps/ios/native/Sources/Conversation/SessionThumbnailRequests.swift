import Foundation
import Nuke

enum SessionThumbnailRequests {
    static func request(for image: MessageImage, rowKey: UUID, index: Int, loader: SessionImageLoader?) -> Nuke.ImageRequest? {
        if let remote = loader?.request(for: image, thumbnail: true) { return remote }
        let source = image.url
        let options: [Nuke.ImageRequest.UserInfoKey: Any] = [
            .thumbnailKey: Nuke.ImageRequest.ThumbnailOptions(maxPixelSize: 348),
        ]
        if source.hasPrefix("data:") {
            return Nuke.ImageRequest(id: "inline:\(rowKey):\(index)", data: {
                guard let comma = source.firstIndex(of: ","),
                      let bytes = Data(base64Encoded: String(source[source.index(after: comma)...]), options: .ignoreUnknownCharacters)
                else { throw CocoaError(.fileReadCorruptFile) }
                return bytes
            }, options: [.disableDiskCache], userInfo: options)
        }
        guard let url = URL(string: source), let scheme = url.scheme,
              ["https", "http", "file"].contains(scheme.lowercased()) else { return nil }
        return Nuke.ImageRequest(url: url, userInfo: options)
    }
}

struct SessionThumbnailLayout: Equatable {
    struct ImageRow: Equatable {
        let key: UUID
        let images: [MessageImage]
    }

    let indexesByID: [String: Int]
    let imagesByIndex: [Int: ImageRow]

    func rows(near visible: Set<String>) -> [ImageRow] {
        let indexes = visible.compactMap { indexesByID[$0] }
        guard let first = indexes.min(), let last = indexes.max() else { return [] }
        return (1...4).flatMap { distance in
            [imagesByIndex[first - distance], imagesByIndex[last + distance]].compactMap { $0 }
        }
    }
}

@MainActor
final class SessionThumbnailPrefetcher {
    private var prefetcher: ImagePrefetcher?
    private var visible = Set<String>()
    private var requests: [String: Nuke.ImageRequest] = [:]
    private var update: Task<Void, Never>?
    private var findRequests: ((Set<String>) -> [Nuke.ImageRequest])?

    func setVisible(_ id: String, _ isVisible: Bool) {
        let changed = isVisible ? visible.insert(id).inserted : visible.remove(id) != nil
        if changed { refresh() }
    }

    func updateLookup(_ findRequests: ((Set<String>) -> [Nuke.ImageRequest])?) {
        self.findRequests = findRequests
        refresh()
    }

    private func refresh() {
        update?.cancel()
        guard !visible.isEmpty, let findRequests else {
            stopRequests()
            return
        }
        update = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(64)) } catch { return }
            guard let self, !Task.isCancelled else { return }
            self.update = nil
            let next = findRequests(self.visible)
            var keyed: [String: Nuke.ImageRequest] = [:]
            for request in next {
                if let key = request.imageId { keyed[key] = request }
            }
            let removed = self.requests.filter { keyed[$0.key] == nil }.map(\.value)
            let added = next.filter { request in
                guard let key = request.imageId else { return false }
                return self.requests[key] == nil
            }
            self.requests = keyed
            if !removed.isEmpty { self.prefetcher?.stopPrefetching(with: removed) }
            if !added.isEmpty {
                let prefetcher = self.prefetcher ?? ImagePrefetcher(destination: .memoryCache, maxConcurrentRequestCount: 1)
                self.prefetcher = prefetcher
                prefetcher.startPrefetching(with: added)
            }
        }
    }

    func stopRequests() {
        update?.cancel()
        update = nil
        requests.removeAll()
        prefetcher?.stopPrefetching()
    }

    func reset() {
        visible.removeAll()
        stopRequests()
    }
}
