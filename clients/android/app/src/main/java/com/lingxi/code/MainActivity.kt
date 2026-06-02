package com.lingxi.code

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.lingxi.code.theme.LingXiTheme

/**
 * The single Activity host. Edge-to-edge, Compose-only — everything else
 * (drawer, conversation, settings nav graph, voice overlay) is composed under
 * [RootScreen]. This is the A1 entry point; later phases fill in [RootScreen].
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            LingXiTheme {
                RootScreen()
            }
        }
    }
}
