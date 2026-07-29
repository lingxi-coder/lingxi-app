package com.lingxi.code.computeruse

import android.content.Context
import android.content.Intent
import com.lingxi.code.bindings.AndroidComputerUseHost
import kotlinx.coroutines.flow.StateFlow

enum class ComputerUseTier {
    Read,
    Click,
    Full,
}

data class ComputerUseApp(
    val packageName: String,
    val label: String,
    val systemUi: Boolean = false,
)

data class ComputerUseGrant(
    val packageName: String,
    val label: String,
    val tier: ComputerUseTier,
    val systemUi: Boolean = false,
)

enum class ComputerUseCaptureMode {
    None,
    Accessibility,
    MediaProjection,
}

enum class ComputerUseSessionState {
    Inactive,
    Starting,
    Active,
    AwaitingApproval,
    Stopping,
}

data class ComputerUseUiState(
    val serviceEnabled: Boolean = false,
    val sessionState: ComputerUseSessionState = ComputerUseSessionState.Inactive,
    val captureMode: ComputerUseCaptureMode = ComputerUseCaptureMode.None,
    val activePackage: String? = null,
    val grants: List<ComputerUseGrant> = emptyList(),
    val sessionStartedAtMs: Long? = null,
    val expiresAtMs: Long? = null,
    val lastError: String? = null,
)

data class ComputerUseApproval(
    val id: String,
    val targetPackage: String,
    val action: String,
    val summary: String,
    val expiresAtMs: Long,
)

data class ComputerUseConfiguration(
    val listenEnabled: Boolean = false,
    val speakEnabled: Boolean = true,
    val maxListenSeconds: Int = 15,
)

interface ComputerUseFeature {
    val available: Boolean
    val state: StateFlow<ComputerUseUiState>
    val pendingApproval: StateFlow<ComputerUseApproval?>
    val configuration: StateFlow<ComputerUseConfiguration>

    fun attach(context: Context, onEmergencyStop: () -> Unit)

    fun listLaunchableApps(context: Context): List<ComputerUseApp>

    fun mediaProjectionRequest(context: Context): Intent?

    fun start(
        context: Context,
        grants: List<ComputerUseGrant>,
        includeSystemUi: Boolean,
        projectionResultCode: Int? = null,
        projectionData: Intent? = null,
    ): Result<Unit>

    fun stop(context: Context, reason: String = "user")

    fun resolveApproval(id: String, allowed: Boolean)

    fun clearAudit(context: Context)

    fun updateConfiguration(context: Context, configuration: ComputerUseConfiguration)

    fun openAccessibilitySettings(context: Context)

    fun engineHost(): AndroidComputerUseHost?
}
