package com.lingxi.code.computeruse

import android.content.Context
import android.content.Intent
import com.lingxi.code.bindings.AndroidComputerUseHost
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

object ComputerUseFeatureProvider : ComputerUseFeature {
    private val unavailable = MutableStateFlow(
        ComputerUseUiState(lastError = "Google Play 版本不包含 Computer Use"),
    )
    private val noApproval = MutableStateFlow<ComputerUseApproval?>(null)
    private val unavailableConfiguration = MutableStateFlow(ComputerUseConfiguration())

    override val available: Boolean = false
    override val state: StateFlow<ComputerUseUiState> = unavailable
    override val pendingApproval: StateFlow<ComputerUseApproval?> = noApproval
    override val configuration: StateFlow<ComputerUseConfiguration> = unavailableConfiguration

    override fun attach(context: Context, onEmergencyStop: () -> Unit) = Unit

    override fun listLaunchableApps(context: Context): List<ComputerUseApp> = emptyList()

    override fun mediaProjectionRequest(context: Context): Intent? = null

    override fun start(
        context: Context,
        grants: List<ComputerUseGrant>,
        includeSystemUi: Boolean,
        projectionResultCode: Int?,
        projectionData: Intent?,
    ): Result<Unit> = Result.failure(
        IllegalStateException("Google Play 版本不包含 Computer Use"),
    )

    override fun stop(context: Context, reason: String) = Unit

    override fun resolveApproval(id: String, allowed: Boolean) = Unit

    override fun clearAudit(context: Context) = Unit

    override fun updateConfiguration(
        context: Context,
        configuration: ComputerUseConfiguration,
    ) = Unit

    override fun openAccessibilitySettings(context: Context) = Unit

    override fun engineHost(): AndroidComputerUseHost? = null
}
