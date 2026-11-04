package com.lingxi.teamsmembers
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import com.lingxi.code.teams.TeamsScreen

/** Compile-only fixture for actual product sources, not a signed product. */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { MaterialTheme { TeamsScreen(null, {}, {}, {}) } }
    }
}
