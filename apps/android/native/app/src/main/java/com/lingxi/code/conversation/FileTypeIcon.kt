package com.lingxi.code.conversation

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import java.util.Locale

// Material Icon Theme (MIT); see apps/electron/src/renderer/assets/file-icons/LICENSE.
private val fileTypeExtensions = mapOf(
    "swift" to R.drawable.file_type_swift,
    "ts" to R.drawable.file_type_typescript,
    "mts" to R.drawable.file_type_typescript,
    "cts" to R.drawable.file_type_typescript,
    "tsx" to R.drawable.file_type_react,
    "jsx" to R.drawable.file_type_react,
    "js" to R.drawable.file_type_javascript,
    "mjs" to R.drawable.file_type_javascript,
    "cjs" to R.drawable.file_type_javascript,
    "py" to R.drawable.file_type_python,
    "rs" to R.drawable.file_type_rust,
    "go" to R.drawable.file_type_go,
    "java" to R.drawable.file_type_java,
    "kt" to R.drawable.file_type_kotlin,
    "kts" to R.drawable.file_type_kotlin,
    "c" to R.drawable.file_type_c,
    "h" to R.drawable.file_type_c,
    "cc" to R.drawable.file_type_cpp,
    "cpp" to R.drawable.file_type_cpp,
    "cxx" to R.drawable.file_type_cpp,
    "hpp" to R.drawable.file_type_cpp,
    "cs" to R.drawable.file_type_csharp,
    "yml" to R.drawable.file_type_yaml,
    "yaml" to R.drawable.file_type_yaml,
    "json" to R.drawable.file_type_json,
    "jsonc" to R.drawable.file_type_json,
    "html" to R.drawable.file_type_html,
    "htm" to R.drawable.file_type_html,
    "css" to R.drawable.file_type_css,
    "md" to R.drawable.file_type_markdown,
    "mdx" to R.drawable.file_type_markdown,
    "xml" to R.drawable.file_type_xml,
    "toml" to R.drawable.file_type_toml,
    "dart" to R.drawable.file_type_dart,
    "php" to R.drawable.file_type_php,
    "rb" to R.drawable.file_type_ruby,
    "lua" to R.drawable.file_type_lua,
    "scala" to R.drawable.file_type_scala,
    "ex" to R.drawable.file_type_elixir,
    "exs" to R.drawable.file_type_elixir,
    "vue" to R.drawable.file_type_vue,
    "svelte" to R.drawable.file_type_svelte,
    "graphql" to R.drawable.file_type_graphql,
    "gql" to R.drawable.file_type_graphql,
    "tf" to R.drawable.file_type_terraform,
    "ps1" to R.drawable.file_type_powershell,
)

internal fun fileTypeIconResource(path: String): Int {
    val name = path.substringAfterLast('/').substringAfterLast('\\').lowercase(Locale.ROOT)
    if (name == "dockerfile" || name.startsWith("dockerfile.")) return R.drawable.file_type_docker
    if (name in setOf(".gitignore", ".gitattributes", ".gitmodules")) return R.drawable.file_type_git
    return fileTypeExtensions[name.substringAfterLast('.', "")] ?: R.drawable.file_type_file
}

@Composable
internal fun FileTypeIcon(path: String) {
    Image(painter = painterResource(fileTypeIconResource(path)), contentDescription = null, modifier = Modifier.size(18.dp))
}
