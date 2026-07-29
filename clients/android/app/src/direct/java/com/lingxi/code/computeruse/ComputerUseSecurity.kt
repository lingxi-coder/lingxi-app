package com.lingxi.code.computeruse

import android.view.accessibility.AccessibilityNodeInfo

internal enum class ComputerUseRisk {
    Normal,
    ConfirmEveryTime,
    Blocked,
}

internal object ComputerUseSecurity {
    private val protectedPackagePrefixes = listOf(
        "com.lingxi.code",
        "com.android.packageinstaller",
        "com.google.android.packageinstaller",
        "com.android.permissioncontroller",
        "com.google.android.permissioncontroller",
    )

    private val hardBlockedText = Regex(
        "(password|passcode|密码|口令|验证码|otp|one.?time|生物识别|指纹|face.?id|" +
            "支付密码|转账确认|确认付款|security key|安全密钥|unknown sources|未知来源|" +
            "accessibility|无障碍|device admin|设备管理|vpn)",
        RegexOption.IGNORE_CASE,
    )

    private val highRiskText = Regex(
        "(发送|发布|提交|拨打|呼叫|删除|清除|恢复出厂|转账|付款|购买|send|post|publish|" +
            "submit|call|delete|remove|factory reset|transfer|pay|purchase|install|uninstall)",
        RegexOption.IGNORE_CASE,
    )

    private val sensitiveFieldText = Regex(
        "((^|[^a-z0-9])(password|passcode|pin|otp|cvv|cvc|card[ _-]?number|" +
            "security[ _-]?code|id[ _-]?number)([^a-z0-9]|$)|" +
            "验证码|密码|口令|银行卡|安全码|身份证)",
        RegexOption.IGNORE_CASE,
    )

    fun isHardBlockedPackage(packageName: String): Boolean =
        protectedPackagePrefixes.any { packageName == it || packageName.startsWith("$it.") }

    fun isHardBlockedSurface(
        packageName: String,
        visibleText: String,
        containsSensitiveNode: Boolean = false,
        scanTruncated: Boolean = false,
    ): Boolean =
        isHardBlockedPackage(packageName) ||
            containsSensitiveNode ||
            scanTruncated ||
            hardBlockedText.containsMatchIn(visibleText)

    fun isSensitiveNode(node: AccessibilityNodeInfo): Boolean {
        if (node.isPassword) return true
        val identity = buildString {
            append(node.viewIdResourceName.orEmpty())
            append(' ')
            append(node.hintText?.toString().orEmpty())
            append(' ')
            append(node.contentDescription?.toString().orEmpty())
        }
        return isSensitiveIdentity(identity)
    }

    internal fun isSensitiveIdentity(identity: String): Boolean =
        sensitiveFieldText.containsMatchIn(identity)

    fun riskFor(
        actionType: String,
        nodeSummary: String,
        semanticsReliable: Boolean = true,
    ): ComputerUseRisk {
        if (hardBlockedText.containsMatchIn(nodeSummary)) return ComputerUseRisk.Blocked
        if (highRiskText.containsMatchIn(nodeSummary)) {
            return ComputerUseRisk.ConfirmEveryTime
        }
        if (!semanticsReliable && actionType in setOf("tap", "long_press", "enter")) {
            return ComputerUseRisk.ConfirmEveryTime
        }
        return ComputerUseRisk.Normal
    }

    /**
     * The Direct build deliberately exposes the same host to foreground and
     * background engines. Access is still session-scoped: grants live only in
     * memory and every call must observe both an active state and a non-empty
     * grant set.
     */
    fun hasActiveAuthorization(
        sessionState: ComputerUseSessionState,
        hasGrants: Boolean,
    ): Boolean =
        hasGrants &&
            sessionState in setOf(
                ComputerUseSessionState.Active,
                ComputerUseSessionState.AwaitingApproval,
            )

    /**
     * Accessibility events for the visible software keyboard are attributed to
     * the IME package rather than to the allowed app behind it. The IME remains
     * non-controllable, but its transient window must not be mistaken for an
     * app authorization escape.
     */
    fun isConfiguredInputMethodPackage(
        packageName: String,
        configuredInputMethod: String?,
    ): Boolean =
        configuredInputMethod
            ?.substringBefore('/')
            ?.trim()
            ?.takeIf(String::isNotEmpty) == packageName

    fun requiredTier(actionType: String): ComputerUseTier = when (actionType) {
        "screenshot", "ui_tree", "find", "inspect", "wait_for", "wait_idle" ->
            ComputerUseTier.Read
        "tap", "long_press", "scroll", "swipe", "pinch" -> ComputerUseTier.Click
        else -> ComputerUseTier.Full
    }
}
