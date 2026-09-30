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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.lingxi.code.R

private data class SourceDisclosure(
    val component: String,
    val version: String,
    val license: String,
    val sourceUrl: String,
)

@Composable
private fun androidSourceDisclosures(): List<SourceDisclosure> = listOf(
    SourceDisclosure(
        component = "OpenMinis Android Shell / PTY",
        version = "9cf3a855fecd27bb5735b84cacbd56852a3ab8dd",
        license = "GPL-3.0-only",
        sourceUrl = "https://github.com/OpenMinis/OpenMinis/tree/9cf3a855fecd27bb5735b84cacbd56852a3ab8dd",
    ),
    SourceDisclosure(
        component = "OpenMinis PRoot fork",
        version = "8cf13e997cdc9472997aae19df8050c073c9a86c",
        license = stringResource(R.string.oss_license_proot),
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
        license = stringResource(R.string.oss_license_alpine_packages),
        sourceUrl = "https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/",
    ),
)

@Composable
fun OpenSourceLicensesPage(modifier: Modifier = Modifier) {
    val uriHandler = LocalUriHandler.current
    val disclosures = androidSourceDisclosures()
    LazyColumn(
        modifier = modifier.padding(horizontal = 20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item("summary") {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    stringResource(R.string.oss_android_bundle_title),
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    stringResource(R.string.oss_android_bundle_body),
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        items(disclosures, key = SourceDisclosure::component) { disclosure ->
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
                    stringResource(R.string.oss_view_source),
                    color = MaterialTheme.colorScheme.primary,
                    style = MaterialTheme.typography.labelLarge,
                )
            }
        }
        item("offer") {
            Text(
                stringResource(R.string.oss_offer_note),
                modifier = Modifier.padding(bottom = 28.dp),
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}
