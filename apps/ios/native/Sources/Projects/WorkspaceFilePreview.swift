import SwiftUI
import QuickLook

struct WorkspaceFilePreview: View {
    let repository: WorkspaceDirectoryRepository
    let snapshot: WorkspaceFileSnapshot
    @Environment(\.dismiss) private var dismiss
    @State private var text: String?
    @State private var error: String?
    @State private var loading = true
    @State private var share = false
    private var textFile: Bool {
        let ext = snapshot.url.pathExtension.lowercased()
        return ["txt", "md", "swift", "kt", "rs", "py", "json", "yaml", "yml", "html", "htm", "xhtml", "css", "js", "ts", "tsx", "jsx", "xml", "svg", "sh", "toml", "csv", "log", "ini", "conf", "c", "h", "cpp", "java", "sql"].contains(ext) || ["Dockerfile", "Makefile", ".gitignore"].contains(snapshot.url.lastPathComponent)
    }
    private var nativeFile: Bool { ["pdf", "jpg", "jpeg", "png", "gif", "heic", "webp", "bmp", "tiff", "mp4", "mov", "m4v", "mp3", "wav", "m4a", "aac", "flac", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "rtf", "pages", "numbers", "key"].contains(snapshot.url.pathExtension.lowercased()) }
    var body: some View {
        if nativeFile && QLPreviewController.canPreview(snapshot.url as NSURL) {
            WorkspaceQuickLook(url: snapshot.url, onClose: { dismiss() })
        } else { fallbackContent }
    }
    private var fallbackContent: some View {
        NavigationStack {
            Group {
                if textFile {
                    if loading { ProgressView() }
                    else if let text {
                        WorkspaceReadOnlyText(text: text).accessibilityIdentifier("files.text-preview")
                    } else { ContentUnavailableView(error ?? FilesStrings.text("unsupported"), systemImage: "doc.questionmark") }
                } else {
                    VStack {
                        ContentUnavailableView(FilesStrings.text("unsupported"), systemImage: "doc.questionmark", description: Text(snapshot.url.lastPathComponent))
                        Button(FilesStrings.text("open_with")) { share = true }.padding()
                    }
                }
            }
            .navigationTitle(snapshot.url.lastPathComponent).navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("common_close") { dismiss() }.accessibilityIdentifier("files.preview.close") }
                ToolbarItem(placement: .primaryAction) { Button { share = true } label: { Label(FilesStrings.text("share"), systemImage: "square.and.arrow.up") } }
            }
            .task {
                if textFile {
                    do { text = try await repository.textPreview(snapshot) }
                    catch { self.error = error.localizedDescription }
                }
                loading = false
            }
            .sheet(isPresented: $share) { WorkspaceShareSheet(url: snapshot.url) }
        }
    }
}
private struct WorkspaceQuickLook: UIViewControllerRepresentable {
    let url: URL
    let onClose: () -> Void
    func makeCoordinator() -> Coordinator { Coordinator(url, onClose: onClose) }
    func makeUIViewController(context: Context) -> UINavigationController {
        let controller = QLPreviewController()
        controller.dataSource = context.coordinator; controller.delegate = context.coordinator
        let close = UIBarButtonItem(title: FilesStrings.text("close"), style: .plain, target: context.coordinator, action: #selector(Coordinator.close))
        close.accessibilityIdentifier = "files.preview.close"
        controller.navigationItem.leftBarButtonItem = close
        return UINavigationController(rootViewController: controller)
    }
    func updateUIViewController(_ controller: UINavigationController, context: Context) {}
    final class Coordinator: NSObject, QLPreviewControllerDataSource, QLPreviewControllerDelegate {
        let url: URL
        let onClose: () -> Void
        init(_ url: URL, onClose: @escaping () -> Void) { self.url = url; self.onClose = onClose }
        @objc func close() { onClose() }
        func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
        func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> any QLPreviewItem { url as NSURL }
        func previewController(_ controller: QLPreviewController, editingModeFor previewItem: any QLPreviewItem) -> QLPreviewItemEditingMode { .disabled }
    }
}
struct WorkspaceShareSheet: UIViewControllerRepresentable {
    let urls: [URL]
    init(url: URL) { urls = [url] }
    init(urls: [URL]) { self.urls = urls }
    func makeUIViewController(context: Context) -> UIActivityViewController { UIActivityViewController(activityItems: urls, applicationActivities: nil) }
    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

private struct WorkspaceReadOnlyText: UIViewRepresentable {
    let text: String
    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false; view.isSelectable = true; view.backgroundColor = .clear
        view.adjustsFontForContentSizeCategory = true
        view.font = UIFontMetrics(forTextStyle: .body).scaledFont(for: .monospacedSystemFont(ofSize: 16, weight: .regular))
        view.textContainerInset = UIEdgeInsets(top: 16, left: 12, bottom: 16, right: 12)
        view.accessibilityIdentifier = "files.text-preview"
        return view
    }
    func updateUIView(_ view: UITextView, context: Context) { if view.text != text { view.text = text } }
}
