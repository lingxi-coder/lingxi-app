package com.lingxi.code.components

import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.lingxi.code.theme.LingXiTheme

/**
 * SectionHeader — the small uppercased, letter-spaced label that prefixes a
 * grouped section (ported from the label slot of the iOS `SettingsSection`).
 * Shared so settings groups, drawer sections and the appearance page all read
 * the same. The grouped card body itself lives in A6's settings components;
 * this is just the header text primitive.
 */
@Composable
fun SectionHeader(
    text: String,
    modifier: Modifier = Modifier,
) {
    val t = LingXiTheme.palette
    Text(
        text = text.uppercase(),
        color = t.text4,
        fontSize = 11.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 0.6.sp,
        modifier = modifier.padding(horizontal = 4.dp, vertical = 8.dp),
    )
}
