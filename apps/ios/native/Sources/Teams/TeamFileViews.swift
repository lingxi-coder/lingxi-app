import ImageIO
import PhotosUI
import QuickLook
import SwiftUI
import UniformTypeIdentifiers

// Files in a Team message (M3-4): choosing them, the unsent files above the message box, and the files of a
// message in the stream. The rules are in `TeamAttachments`; the uploads belong to `TeamsWorkspace`.

// MARK: - Reading what was chosen

/// Reads chosen documents and pictures into memory, off the main actor. Nothing is written to disk.
enum TeamFileReader {
    /// What the document picker offers. A file of another kind that gets through is refused with the reason.
    static let documentTypes: [UTType] = [.png, .jpeg, .pdf, .plainText, .commaSeparatedText, .log] + ["md", "markdown"].compactMap { UTType(filenameExtension: $0) }

    static func documents(_ urls: [URL]) async -> [TeamChosenFile] {
        await Task.detached(priority: .userInitiated) {
            urls.map { url in
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let name = url.lastPathComponent
                let type = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? ""
                // A file over the limit is refused before it is read.
                if let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize, size > TeamAttachments.fileBytes {
                    return TeamChosenFile(name: name, type: type, data: Data(), error: "ATTACHMENT_TOO_LARGE")
                }
                guard let data = try? Data(contentsOf: url) else { return TeamChosenFile(name: name, type: type, data: Data(), error: "ATTACHMENT_UNREADABLE") }
                return TeamChosenFile(name: name, type: type, data: data)
            }
        }.value
    }

    /// Pictures from the photo library. A PNG or JPEG within the limit is sent as it is; anything else (HEIC, a
    /// very large photo) is made into a JPEG of at most 4096 points a side.
    static func pictures(_ items: [PhotosPickerItem], now: Date = Date()) async -> [TeamChosenFile] {
        let stamp = now.formatted(.iso8601.year().month().day().dateSeparator(.dash)) + " " + now.formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits).second(.twoDigits)).replacingOccurrences(of: ":", with: ".")
        var files: [TeamChosenFile] = []
        for (index, item) in items.enumerated() {
            let base = "Photo \(stamp)" + (items.count > 1 ? " \(index + 1)" : "")
            guard let data = try? await item.loadTransferable(type: Data.self) else {
                files.append(TeamChosenFile(name: base + ".jpg", type: "image/jpeg", data: Data(), error: "ATTACHMENT_UNREADABLE")); continue
            }
            files.append(await Task.detached(priority: .userInitiated) {
                if let type = TeamAttachments.pictureType(data), data.count <= TeamAttachments.fileBytes {
                    return TeamChosenFile(name: base + (type == "image/png" ? ".png" : ".jpg"), type: type, data: data)
                }
                guard let jpeg = TeamPictures.jpeg(data, side: 4096) else { return TeamChosenFile(name: base + ".jpg", type: "image/jpeg", data: Data(), error: "ATTACHMENT_TYPE") }
                return TeamChosenFile(name: base + ".jpg", type: "image/jpeg", data: jpeg)
            }.value)
        }
        return files
    }
}

/// Picture work with ImageIO, which never holds a whole large photo decoded.
enum TeamPictures {
    /// The picture no larger than `side` pixels a side, upright.
    static func small(_ data: Data, side: Int) -> CGImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary) else { return nil }
        return CGImageSourceCreateThumbnailAtIndex(source, 0, [
            kCGImageSourceCreateThumbnailFromImageAlways: true, kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: side, kCGImageSourceShouldCacheImmediately: true,
        ] as CFDictionary)
    }

    static func jpeg(_ data: Data, side: Int) -> Data? {
        guard let image = small(data, side: side) else { return nil }
        let out = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(out, UTType.jpeg.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
        return CGImageDestinationFinalize(destination) ? out as Data : nil
    }
}

/// A file being looked at is written under the app's temporary directory for the preview and deleted when the
/// preview closes; whatever is left there is deleted when Teams starts or the account changes.
enum TeamFileStore {
    private static var root: URL { FileManager.default.temporaryDirectory.appendingPathComponent("TeamFiles", isDirectory: true) }

    static func write(_ data: Data, name: String) throws -> URL {
        let directory = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent(TeamAttachments.name(name))
        try data.write(to: url, options: [.atomic, .completeFileProtection])
        return url
    }

    static func remove(_ url: URL) { try? FileManager.default.removeItem(at: url.deletingLastPathComponent()) }
    static func purge() { try? FileManager.default.removeItem(at: root) }
}

// MARK: - The unsent files above the message box

/// The "add files" button of the message box: pictures from the photo library or documents from Files.
struct TeamAttachButton: View {
    let disabled: Bool
    let onChosen: ([TeamChosenFile]) -> Void

    @State private var showPhotos = false
    @State private var showDocuments = false
    @State private var photos: [PhotosPickerItem] = []
    @State private var reading = false

    var body: some View {
        Menu {
            Button("teams_file_add_photos", systemImage: "photo.on.rectangle") { showPhotos = true }
            Button("teams_file_add_documents", systemImage: "folder") { showDocuments = true }
        } label: {
            Group {
                if reading { ProgressView() } else { Image(systemName: "paperclip").font(.system(size: 18, weight: .medium)) }
            }
            .frame(width: 32, height: 32)
        }
        .disabled(disabled || reading)
        .accessibilityLabel(Text("teams_file_add"))
        .accessibilityHint(Text("teams_file_add_hint"))
        .accessibilityIdentifier("teams.composer.attach")
        .photosPicker(isPresented: $showPhotos, selection: $photos, maxSelectionCount: TeamAttachments.messageFiles, matching: .images)
        .onChange(of: photos) { _, items in
            guard !items.isEmpty else { return }
            photos = []
            read { await TeamFileReader.pictures(items) }
        }
        .fileImporter(isPresented: $showDocuments, allowedContentTypes: TeamFileReader.documentTypes, allowsMultipleSelection: true) { result in
            guard case let .success(urls) = result, !urls.isEmpty else { return }
            read { await TeamFileReader.documents(urls) }
        }
    }

    private func read(_ load: @escaping @Sendable () async -> [TeamChosenFile]) {
        reading = true
        Task {
            let files = await load()
            reading = false
            onChosen(files)
        }
    }
}

/// The files of the unsent message, each with where its upload stands and a button to take it out, and under
/// them the chosen files that were not taken, each with the reason.
struct TeamDraftFilesView: View {
    let files: [TeamDraftFile]
    let refusals: [TeamFileRefusal]
    let onRemove: (String) -> Void
    let onDismissRefusals: () -> Void

    var body: some View {
        if !files.isEmpty || !refusals.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(files) { file in row(file) }
                if !refusals.isEmpty {
                    HStack(alignment: .top, spacing: 8) {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(refusals) { refusal in
                                Text("\(Text(verbatim: refusal.name + ": ").fontWeight(.semibold))\(Text(LocalizedStringKey(TeamsErrors.key(refusal.code))))").font(.caption)
                            }
                        }
                        .foregroundStyle(.orange)
                        Spacer(minLength: 8)
                        Button(action: onDismissRefusals) { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                            .accessibilityLabel(Text("common_close"))
                    }
                    .accessibilityIdentifier("teams.composer.files.refused")
                }
            }
            .accessibilityIdentifier("teams.composer.files")
        }
    }

    private func row(_ file: TeamDraftFile) -> some View {
        HStack(spacing: 8) {
            Image(systemName: TeamFileIcon.name(file.mime)).foregroundStyle(.secondary).frame(width: 20).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(file.name).font(.footnote.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                switch file.state {
                case .uploading:
                    ProgressView(value: Double(file.sent), total: Double(max(file.byteLength, 1)))
                        .accessibilityLabel(Text("teams_file_uploading"))
                case .ready:
                    Text(verbatim: TeamAttachments.size(file.byteLength)).font(.caption2).foregroundStyle(.secondary)
                case .failed:
                    Text(LocalizedStringKey(TeamsErrors.key(file.error ?? "UPLOAD_FAILED"))).font(.caption2).foregroundStyle(.orange)
                }
            }
            Spacer(minLength: 8)
            Button { onRemove(file.uploadId) } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                .accessibilityLabel(Text("teams_file_remove \(file.name)"))
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("teams.composer.file.\(file.uploadId)")
    }
}

enum TeamFileIcon {
    static func name(_ mime: String) -> String {
        switch mime {
        case "image/png", "image/jpeg": "photo"
        case "application/pdf": "doc.richtext"
        default: "doc.text"
        }
    }
}

// MARK: - The files of a message

/// The files a message carries. A picture is shown in the stream; any file opens in a preview, from where it
/// can be shared or saved. The bytes are asked of the service each time the file is opened and the preview's
/// copy is deleted when it closes, so someone who can no longer read the conversation cannot open them here.
/// The sender can take a file back; everyone then sees that it was taken back.
struct TeamMessageFilesView: View {
    @Environment(TeamsWorkspace.self) private var workspace
    let message: TeamMessage
    let context: TeamConversationContext

    @State private var opening: String?
    @State private var preview: URL?
    /// Why a file could not be opened, by attachment id.
    @State private var failures: [String: String] = [:]
    @State private var takingBack: TeamMessageAttachment?

    private var mine: Bool { context.isMine(message) && context.participant }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(message.files) { file in
                if file.revoked == true { takenBack(file) } else { row(file) }
            }
        }
        .quickLookPreview($preview)
        .onChange(of: preview) { old, new in if new == nil, let old { TeamFileStore.remove(old) } }
        .confirmationDialog(Text("teams_file_take_back_confirm \(takingBack?.name ?? "")"), isPresented: Binding(get: { takingBack != nil }, set: { if !$0 { takingBack = nil } }),
                            titleVisibility: .visible, presenting: takingBack) { file in
            Button("teams_file_take_back", role: .destructive) {
                Task { await workspace.run { try await context.model.revokeAttachment(file.attachmentId) } }
            }
            Button("teams_cancel", role: .cancel) {}
        } message: { _ in
            Text("teams_file_take_back_detail")
        }
    }

    private func takenBack(_ file: TeamMessageAttachment) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "doc").foregroundStyle(.tertiary).frame(width: 20).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(file.name).font(.footnote.weight(.semibold)).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Text("teams_file_taken_back").font(.caption2).italic().foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("teams.message.file.\(file.attachmentId).revoked")
    }

    private func row(_ file: TeamMessageAttachment) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Button { open(file) } label: {
                VStack(alignment: .leading, spacing: 4) {
                    if file.isImage { TeamInlinePicture(file: file, model: context.model) }
                    HStack(spacing: 8) {
                        if opening == file.attachmentId { ProgressView().frame(width: 20) }
                        else { Image(systemName: TeamFileIcon.name(file.mime)).foregroundStyle(.secondary).frame(width: 20).accessibilityHidden(true) }
                        VStack(alignment: .leading, spacing: 1) {
                            Text(file.name).font(.footnote.weight(.semibold)).foregroundStyle(.primary).lineLimit(1).truncationMode(.middle)
                            Text(verbatim: TeamAttachments.size(file.byteLength)).font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                    .padding(.horizontal, 10).padding(.vertical, 6)
                    .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                }
            }
            .buttonStyle(.plain)
            .disabled(opening != nil)
            .accessibilityLabel(Text("teams_file_open \(file.name)"))
            .accessibilityIdentifier("teams.message.file.\(file.attachmentId)")
            .contextMenu {
                Button("common_open", systemImage: "eye") { open(file) }
                if mine { Button("teams_file_take_back", systemImage: "arrow.uturn.backward", role: .destructive) { takingBack = file } }
            }
            .accessibilityActions {
                if mine { Button("teams_file_take_back") { takingBack = file } }
            }
            if let code = failures[file.attachmentId] {
                Text(LocalizedStringKey(TeamsErrors.key(code))).font(.caption2).foregroundStyle(.orange)
            }
        }
    }

    private func open(_ file: TeamMessageAttachment) {
        guard opening == nil else { return }
        opening = file.attachmentId; failures[file.attachmentId] = nil
        let model = context.model
        Task {
            defer { opening = nil }
            do {
                let data = try await model.download(file)
                preview = try await Task.detached { try TeamFileStore.write(data, name: file.name) }.value
            } catch {
                failures[file.attachmentId] = (error as? TeamMessageFailure)?.code ?? "ATTACHMENT_UNREADABLE"
            }
        }
    }
}

/// A picture of a message in the stream, small. It is read through the service when the message comes into
/// view; the small copy is kept in memory by the team's model only, until the team is closed.
private struct TeamInlinePicture: View {
    let file: TeamMessageAttachment
    let model: TeamMessagingModel

    @State private var image: CGImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(decorative: image, scale: 1)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: 240, maxHeight: 240, alignment: .leading)
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            } else {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(Color(uiColor: .secondarySystemBackground))
                    .frame(width: 160, height: 110)
                    .overlay { if failed { Image(systemName: "photo").foregroundStyle(.tertiary) } else { ProgressView() } }
            }
        }
        .accessibilityHidden(true)
        .task(id: file.attachmentId) {
            if let known = model.pictures.object(forKey: file.attachmentId as NSString) { image = known; return }
            guard let data = try? await model.download(file),
                  let small = await Task.detached(priority: .utility, operation: { TeamPictures.small(data, side: 720) }).value else {
                failed = !Task.isCancelled; return
            }
            model.pictures.setObject(small, forKey: file.attachmentId as NSString)
            image = small
        }
    }
}
