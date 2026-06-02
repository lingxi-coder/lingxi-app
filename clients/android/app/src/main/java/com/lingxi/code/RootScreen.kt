package com.lingxi.code

import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.tooling.preview.Preview
import com.lingxi.code.theme.LingXiTheme

/**
 * Root composable for the app shell.
 *
 * A1 scaffold: a placeholder so the empty app builds and runs. Subsequent
 * phases replace the body with the real shell — a `ModalNavigationDrawer`
 * (对话/项目/定时), the conversation `Scaffold`, the settings nav graph, and the
 * voice-flow overlay — all driven by hoisted state / a ViewModel.
 */
@Composable
fun RootScreen() {
    Scaffold(modifier = Modifier.fillMaxSize()) { innerPadding ->
        Text(
            text = "LingXi Code",
            modifier = Modifier
                .padding(innerPadding)
                .fillMaxSize()
                .wrapContentSize(Alignment.Center),
        )
    }
}

@Preview(showBackground = true)
@Composable
private fun RootScreenPreview() {
    LingXiTheme {
        RootScreen()
    }
}
