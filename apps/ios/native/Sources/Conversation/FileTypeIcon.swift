import SwiftUI

// Material Icon Theme (MIT); see apps/electron/src/renderer/assets/file-icons/LICENSE.
private let fileTypeExtensions: [String: String] = [
        "swift": "swift",
        "ts": "typescript",
        "mts": "typescript",
        "cts": "typescript",
        "tsx": "react",
        "jsx": "react",
        "js": "javascript",
        "mjs": "javascript",
        "cjs": "javascript",
        "py": "python",
        "rs": "rust",
        "go": "go",
        "java": "java",
        "kt": "kotlin",
        "kts": "kotlin",
        "c": "c",
        "h": "c",
        "cc": "cpp",
        "cpp": "cpp",
        "cxx": "cpp",
        "hpp": "cpp",
        "cs": "csharp",
        "yml": "yaml",
        "yaml": "yaml",
        "json": "json",
        "jsonc": "json",
        "html": "html",
        "htm": "html",
        "css": "css",
        "md": "markdown",
        "mdx": "markdown",
        "xml": "xml",
        "toml": "toml",
        "dart": "dart",
        "php": "php",
        "rb": "ruby",
        "lua": "lua",
        "scala": "scala",
        "ex": "elixir",
        "exs": "elixir",
        "vue": "vue",
        "svelte": "svelte",
        "graphql": "graphql",
        "gql": "graphql",
        "tf": "terraform",
        "ps1": "powershell",
]

func fileTypeIconName(_ path: String) -> String {
    let name = path.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last.map(String.init)?.lowercased() ?? ""
    if name == "dockerfile" || name.hasPrefix("dockerfile.") { return "docker" }
    if [".gitignore", ".gitattributes", ".gitmodules"].contains(name) { return "git" }
    let ext = name.contains(".") ? String(name.split(separator: ".", omittingEmptySubsequences: false).last ?? "") : ""
    return fileTypeExtensions[ext] ?? "file"
}

struct FileTypeIcon: View {
    let path: String
    var body: some View {
        Image("FileType_\(fileTypeIconName(path))")
            .resizable().aspectRatio(contentMode: .fit)
            .frame(width: 18, height: 18)
            .accessibilityHidden(true)
    }
}
