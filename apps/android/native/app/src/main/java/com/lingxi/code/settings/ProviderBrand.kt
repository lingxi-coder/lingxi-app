package com.lingxi.code.settings

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme

@Composable
internal fun ProviderBrandMark(providerId: String, size: Dp, fallback: @Composable () -> Unit) {
    val drawable = when (providerId) {
        "anthropic" -> R.drawable.provider_brand_claude
        "deepseek" -> R.drawable.provider_brand_deepseek
        "google", "gemini" -> R.drawable.provider_brand_gemini
        "github-copilot" -> R.drawable.provider_brand_githubcopilot
        "kimi", "kimi-code" -> R.drawable.provider_brand_kimi
        "minimax" -> R.drawable.provider_brand_minimax
        "qwen" -> R.drawable.provider_brand_qwen
        "openai", "openai-chatgpt" -> R.drawable.provider_brand_openai
        "openrouter" -> R.drawable.provider_brand_openrouter
        "xai" -> R.drawable.provider_brand_xai
        "zai", "glm-coding" -> R.drawable.provider_brand_zai
        else -> null
    }
    if (drawable == null) {
        fallback()
        return
    }
    val hasBrandColor = when (drawable) {
        R.drawable.provider_brand_claude, R.drawable.provider_brand_deepseek, R.drawable.provider_brand_gemini,
        R.drawable.provider_brand_minimax, R.drawable.provider_brand_qwen -> true
        else -> false
    }
    Box(Modifier.size(size), contentAlignment = Alignment.Center) {
        Image(
            painter = painterResource(drawable),
            contentDescription = null,
            modifier = Modifier.size(size * 0.7f),
            colorFilter = if (hasBrandColor) null else ColorFilter.tint(LingXiTheme.palette.text),
        )
    }
}

@Composable
internal fun ProviderMessageDialog(message: String, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        text = { Text(message) },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(android.R.string.ok)) } },
    )
}
