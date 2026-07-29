package com.lingxi.code.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

private data class SourceDisclosure(
    val component: String,
    val version: String,
    val license: String,
    val sourceUrl: String,
)

private val androidSourceDisclosures = listOf(
    SourceDisclosure(
        component = "OpenMinis Android Shell / PTY",
        version = "9cf3a855fecd27bb5735b84cacbd56852a3ab8dd",
        license = "GPL-3.0-only",
        sourceUrl = "https://github.com/OpenMinis/OpenMinis/tree/9cf3a855fecd27bb5735b84cacbd56852a3ab8dd",
    ),
    SourceDisclosure(
        component = "OpenMinis PRoot fork",
        version = "8cf13e997cdc9472997aae19df8050c073c9a86c",
        license = "GPL-2.0-or-later（本组合选择 GPLv3）",
        sourceUrl = "https://github.com/OpenMinis/proot/tree/8cf13e997cdc9472997aae19df8050c073c9a86c",
    ),
    SourceDisclosure(
        component = "talloc",
        version = "2.4.2",
        license = "LGPL-3.0-or-later",
        sourceUrl = "https://download.samba.org/pub/talloc/talloc-2.4.2.tar.gz",
    ),
    SourceDisclosure(
        component = "Alpine Linux minirootfs",
        version = "3.21.3 · arm64-v8a / x86_64",
        license = "各软件包许可证（清单和 SBOM 随发行物提供）",
        sourceUrl = "https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/",
    ),
)

@Composable
fun OpenSourceLicensesPage(modifier: Modifier = Modifier) {
    val uriHandler = LocalUriHandler.current
    LazyColumn(
        modifier = modifier.padding(horizontal = 20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item("summary") {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    "Android 组合发行物",
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    "本 Android 组合产品按 GNU GPLv3 分发。原有 MIT、Apache、LGPL " +
                        "及 Alpine 软件包继续保留各自声明。构建产物同时携带 NOTICE、" +
                        "SPDX SBOM、固定摘要和对应源码信息。",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        items(androidSourceDisclosures, key = SourceDisclosure::component) { disclosure ->
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { uriHandler.openUri(disclosure.sourceUrl) }
                    .padding(vertical = 8.dp),
                verticalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                Text(disclosure.component, fontWeight = FontWeight.SemiBold)
                Text(disclosure.license, style = MaterialTheme.typography.bodySmall)
                Text(
                    disclosure.version,
                    style = MaterialTheme.typography.labelSmall,
                    fontFamily = FontFamily.Monospace,
                )
                Text(
                    "查看对应源码",
                    color = MaterialTheme.colorScheme.primary,
                    style = MaterialTheme.typography.labelLarge,
                )
            }
        }
        item("offer") {
            Text(
                "若应用内链接暂时不可用，请以发行包内 NOTICE 与 SBOM 所列固定版本、" +
                    "哈希和源码地址为准。",
                modifier = Modifier.padding(bottom = 28.dp),
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}
