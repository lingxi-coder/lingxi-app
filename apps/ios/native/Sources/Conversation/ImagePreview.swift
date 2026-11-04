import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Shared by transcript images, composer attachments and session resources.
struct NativeImagePreview: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let source: String
    var mediaType: String = "image/png"
    var fallbackImage: UIImage? = nil
    var closeIdentifier = "image-preview.close"
    @State private var asset: PreviewImageAsset?
    @State private var failed = false
    @State private var exporting = false
    @State private var exportFailed = false
    @State private var scale: CGFloat = 1
    @State private var minimum: CGFloat = 0.1
    @State private var command = PreviewZoomCommand()

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Spacer()
                Button { exporting = true } label: { Image(systemName: "arrow.down.to.line") }
                    .disabled(asset == nil)
                    .accessibilityLabel(String(localized: "image_preview_save"))
                    .accessibilityIdentifier("image-preview.save")
                Button { dismiss() } label: { Image(systemName: "xmark") }
                    .keyboardShortcut(.cancelAction)
                    .accessibilityLabel(String(localized: "common_close"))
                    .accessibilityIdentifier(closeIdentifier)
            }
            .buttonStyle(PreviewCircleButtonStyle())
            .padding(.horizontal, 16).padding(.top, 12)

            ZStack {
                if let asset {
                    PreviewZoomView(image: asset.image, command: command, reduceMotion: reduceMotion) { newScale, newMinimum in
                        if abs(scale - newScale) > 0.0001 { scale = newScale }
                        if abs(minimum - newMinimum) > 0.0001 { minimum = newMinimum }
                    }
                } else if failed {
                    Text("image_preview_failed").foregroundStyle(.white)
                        .accessibilityIdentifier("image-preview.error")
                } else {
                    ProgressView().tint(.white).accessibilityLabel(String(localized: "image_preview_loading"))
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(12)

            HStack(spacing: 4) {
                Button { request(.out) } label: { Image(systemName: "minus") }
                    .disabled(asset == nil || scale <= minimum + 0.001)
                    .accessibilityLabel(String(localized: "image_preview_zoom_out"))
                    .accessibilityIdentifier("image-preview.zoom-out")
                Button { request(.fit) } label: {
                    Text(asset == nil ? "—" : percentage)
                        .font(.system(.subheadline, design: .default, weight: .medium))
                        .monospacedDigit().frame(minWidth: 68, minHeight: 44)
                }
                .buttonStyle(.plain)
                .disabled(asset == nil)
                .accessibilityLabel(String(localized: "image_preview_fit"))
                .accessibilityValue(percentage)
                .accessibilityIdentifier("image-preview.fit")
                Button { request(.in) } label: { Image(systemName: "plus") }
                    .disabled(asset == nil || scale >= 8)
                    .accessibilityLabel(String(localized: "image_preview_zoom_in"))
                    .accessibilityIdentifier("image-preview.zoom-in")
            }
            .buttonStyle(PreviewCircleButtonStyle(muted: true))
            .padding(4)
            .foregroundStyle(Color(white: 0.11))
            .background(Color(white: 0.98), in: Capsule())
            .padding(.bottom, 16)
        }
        .background(Color(white: 0.07).ignoresSafeArea())
        .preferredColorScheme(.dark)
        .toolbar(.hidden, for: .navigationBar)
        .accessibilityAction(.escape) { dismiss() }
        .task(id: source) {
            asset = nil
            failed = false
            do {
                let loaded = try await PreviewImageAsset.load(source: source, mediaType: mediaType, fallbackImage: fallbackImage)
                try Task.checkCancellation()
                asset = loaded
            } catch is CancellationError { }
            catch { if !Task.isCancelled { failed = true } }
        }
        .fileExporter(isPresented: $exporting, document: PreviewImageDocument(data: asset?.data ?? Data()),
                      contentType: asset?.contentType ?? .image, defaultFilename: "image") { result in
            if case .failure = result { exportFailed = true }
        }
        .alert("image_preview_save_failed", isPresented: $exportFailed) { Button("common_ok", role: .cancel) {} }
    }

    private func request(_ action: PreviewZoomAction) {
        command = PreviewZoomCommand(sequence: command.sequence + 1, action: action)
    }
    private var percentage: String { Double(scale).formatted(.percent.precision(.fractionLength(0))) }
}

private struct PreviewCircleButtonStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.isEnabled) private var enabled
    var muted = false
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 19, weight: .regular))
            .foregroundStyle(Color(white: 0.11))
            .frame(width: 44, height: 44)
            .background(Color(white: configuration.isPressed ? 0.82 : muted ? 0.91 : 0.98), in: Circle())
            .contentShape(Circle())
            .opacity(enabled ? 1 : 0.35)
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.95 : 1)
    }
}

private struct PreviewImageAsset {
    let image: UIImage
    let data: Data
    let contentType: UTType

    static func load(source: String, mediaType: String, fallbackImage: UIImage?) async throws -> PreviewImageAsset {
        let data: Data
        var type = UTType(mimeType: mediaType) ?? .image
        if source.hasPrefix("data:"), let comma = source.firstIndex(of: ",") {
            let embeddedType = String(source.dropFirst(5).prefix { $0 != ";" && $0 != "," })
            type = UTType(mimeType: embeddedType) ?? type
            let encoded = String(source[source.index(after: comma)...])
            data = try await Task.detached(priority: .userInitiated) {
                if let decoded = Data(base64Encoded: encoded, options: .ignoreUnknownCharacters), !decoded.isEmpty { return decoded }
                if let fallbackImage, let png = fallbackImage.pngData() { return png }
                throw CocoaError(.fileReadCorruptFile)
            }.value
            if encoded.isEmpty { type = .png }
        } else if let url = URL(string: source), url.isFileURL {
            data = try await Task.detached(priority: .userInitiated) { try Data(contentsOf: url) }.value
            type = UTType(filenameExtension: url.pathExtension) ?? type
        } else if let url = URL(string: source), ["https", "http"].contains(url.scheme?.lowercased() ?? "") {
            let (bytes, response) = try await URLSession.shared.data(from: url)
            guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else { throw URLError(.badServerResponse) }
            data = bytes
            type = response.mimeType.flatMap { UTType(mimeType: $0) } ?? type
        } else { throw CocoaError(.fileReadUnsupportedScheme) }
        return try await Task.detached(priority: .userInitiated) {
            guard let image = UIImage(data: data) else { throw CocoaError(.fileReadCorruptFile) }
            return PreviewImageAsset(image: image, data: data, contentType: type)
        }.value
    }
}

private struct PreviewImageDocument: FileDocument {
    static var readableContentTypes: [UTType] { [.image] }
    static var writableContentTypes: [UTType] {
        ["image/png", "image/jpeg", "image/gif", "image/heic", "image/heif", "image/webp", "image/tiff", "image/bmp", "image/jp2", "image/avif", "image/x-icon"]
            .compactMap { UTType(mimeType: $0) }
    }
    let data: Data
    init(data: Data) { self.data = data }
    init(configuration: ReadConfiguration) throws { data = configuration.file.regularFileContents ?? Data() }
    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper { FileWrapper(regularFileWithContents: data) }
}

enum PreviewZoomAction { case fit, `in`, out }
struct PreviewZoomCommand {
    var sequence = 0
    var action: PreviewZoomAction = .fit
}

private struct PreviewZoomView: UIViewRepresentable {
    let image: UIImage
    let command: PreviewZoomCommand
    let reduceMotion: Bool
    let onZoom: (CGFloat, CGFloat) -> Void

    func makeUIView(context: Context) -> PreviewImageScrollView {
        let view = PreviewImageScrollView(image: image)
        view.onZoom = onZoom
        return view
    }
    func updateUIView(_ view: PreviewImageScrollView, context: Context) {
        view.onZoom = onZoom
        view.reduceMotion = reduceMotion
        view.bouncesZoom = !reduceMotion
        view.apply(command)
    }
}

/// UIKit supplies interruptible pinch, inertia, bounce and accessible scrolling.
final class PreviewImageScrollView: UIScrollView, UIScrollViewDelegate {
    let imageView: UIImageView
    let sourceSize: CGSize
    var onZoom: ((CGFloat, CGFloat) -> Void)?
    var reduceMotion = false
    private(set) var fitScale: CGFloat = 1
    private var viewportSize: CGSize = .zero
    private var commandSequence = 0

    init(image: UIImage) {
        imageView = UIImageView(image: image)
        sourceSize = CGSize(width: image.size.width * image.scale, height: image.size.height * image.scale)
        super.init(frame: .zero)
        delegate = self
        backgroundColor = .clear
        contentInsetAdjustmentBehavior = .never
        showsHorizontalScrollIndicator = false
        showsVerticalScrollIndicator = false
        imageView.contentMode = .scaleAspectFit
        imageView.frame = CGRect(origin: .zero, size: sourceSize)
        addSubview(imageView)
        contentSize = sourceSize
        accessibilityIdentifier = "image-preview.canvas"
        imageView.isAccessibilityElement = true
        imageView.accessibilityLabel = String(localized: "chat_attached_image")
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(doubleTapped(_:)))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        if bounds.size != viewportSize, bounds.width > 0, bounds.height > 0 {
            viewportSize = bounds.size
            setZoomScale(1, animated: false)
            imageView.frame = CGRect(origin: .zero, size: sourceSize)
            contentSize = sourceSize
            fitScale = min(1, min(bounds.width / sourceSize.width, bounds.height / sourceSize.height))
            minimumZoomScale = min(0.1, fitScale)
            maximumZoomScale = 8
            setZoomScale(fitScale, animated: false)
        }
        centerImage()
    }
    func viewForZooming(in scrollView: UIScrollView) -> UIView? { imageView }
    func scrollViewDidZoom(_ scrollView: UIScrollView) { centerImage() }

    func apply(_ command: PreviewZoomCommand) {
        guard command.sequence != commandSequence else { return }
        commandSequence = command.sequence
        let steps: [CGFloat] = [0.1, 0.125, 0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4, 6, 8]
        let target: CGFloat
        switch command.action {
        case .fit: target = fitScale
        case .in: target = steps.first { $0 > zoomScale + 0.001 } ?? maximumZoomScale
        case .out: target = steps.last { $0 < zoomScale - 0.001 } ?? minimumZoomScale
        }
        setZoomScale(max(minimumZoomScale, min(maximumZoomScale, target)), animated: !reduceMotion)
    }

    @objc private func doubleTapped(_ gesture: UITapGestureRecognizer) {
        if zoomScale > fitScale + 0.001 { setZoomScale(fitScale, animated: !reduceMotion); return }
        let target: CGFloat = fitScale < 1 ? 1 : 2
        let point = gesture.location(in: imageView)
        let size = CGSize(width: bounds.width / target, height: bounds.height / target)
        zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height), animated: !reduceMotion)
    }

    private func centerImage() {
        let inset = UIEdgeInsets(top: max(0, (bounds.height - imageView.frame.height) / 2), left: max(0, (bounds.width - imageView.frame.width) / 2),
                                 bottom: max(0, (bounds.height - imageView.frame.height) / 2), right: max(0, (bounds.width - imageView.frame.width) / 2))
        if contentInset != inset { contentInset = inset }
        // Avoid publishing SwiftUI state during updateUIView/layout.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.onZoom?(self.zoomScale, self.minimumZoomScale)
        }
    }
}
