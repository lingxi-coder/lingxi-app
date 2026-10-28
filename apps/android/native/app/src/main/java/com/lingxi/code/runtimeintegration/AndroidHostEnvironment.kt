package com.lingxi.code.runtimeintegration

import android.content.Context
import android.content.res.Configuration
import android.os.Build
import com.lingxi.code.bindings.android.AndroidDeviceClassFfi
import com.lingxi.code.bindings.android.AndroidExecutionTargetFfi
import com.lingxi.code.bindings.android.AndroidHostEnvironmentFfi
import com.lingxi.code.bindings.android.AndroidLaunchModeFfi

internal fun androidDeviceClass(smallestScreenWidthDp: Int): AndroidDeviceClassFfi = when {
    smallestScreenWidthDp <= Configuration.SMALLEST_SCREEN_WIDTH_DP_UNDEFINED ->
        AndroidDeviceClassFfi.UNKNOWN
    smallestScreenWidthDp >= 600 -> AndroidDeviceClassFfi.TABLET
    else -> AndroidDeviceClassFfi.PHONE
}

internal fun androidExecutionTarget(
    fingerprint: String,
    model: String,
    manufacturer: String,
    brand: String,
    device: String,
    product: String,
    hardware: String,
): AndroidExecutionTargetFfi {
    val values = listOf(fingerprint, model, manufacturer, brand, device, product, hardware)
    if (values.all { it.isBlank() || it.equals("unknown", ignoreCase = true) }) {
        return AndroidExecutionTargetFfi.UNKNOWN
    }

    val emulator = fingerprint.startsWith("generic", ignoreCase = true) ||
        model.contains("google_sdk", ignoreCase = true) ||
        model.contains("emulator", ignoreCase = true) ||
        model.contains("android sdk built for", ignoreCase = true) ||
        manufacturer.contains("genymotion", ignoreCase = true) ||
        hardware.equals("goldfish", ignoreCase = true) ||
        hardware.equals("ranchu", ignoreCase = true) ||
        product.contains("sdk_gphone", ignoreCase = true) ||
        product.contains("google_sdk", ignoreCase = true) ||
        product.contains("emulator", ignoreCase = true) ||
        product.contains("simulator", ignoreCase = true) ||
        (brand.startsWith("generic", ignoreCase = true) &&
            device.startsWith("generic", ignoreCase = true))

    return if (emulator) AndroidExecutionTargetFfi.EMULATOR
    else AndroidExecutionTargetFfi.PHYSICAL_DEVICE
}

internal fun androidHostOsVersion(release: String, apiLevel: Int): String? {
    val normalizedRelease = release.trim()
    return when {
        normalizedRelease.isNotEmpty() && apiLevel > 0 -> "$normalizedRelease (API $apiLevel)"
        normalizedRelease.isNotEmpty() -> normalizedRelease
        apiLevel > 0 -> "API $apiLevel"
        else -> null
    }
}

internal fun androidHostEnvironment(
    context: Context,
    launchMode: AndroidLaunchModeFfi,
): AndroidHostEnvironmentFfi = AndroidHostEnvironmentFfi(
    hostOsVersion = androidHostOsVersion(Build.VERSION.RELEASE, Build.VERSION.SDK_INT),
    deviceClass = androidDeviceClass(context.resources.configuration.smallestScreenWidthDp),
    executionTarget = androidExecutionTarget(
        fingerprint = Build.FINGERPRINT,
        model = Build.MODEL,
        manufacturer = Build.MANUFACTURER,
        brand = Build.BRAND,
        device = Build.DEVICE,
        product = Build.PRODUCT,
        hardware = Build.HARDWARE,
    ),
    launchMode = launchMode,
)
