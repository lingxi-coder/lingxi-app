// M8-P12 skeleton — the LingxiCode Android library module.
//
// The UniFFI-generated Kotlin bindings (package com.lingxi.code.bindings,
// produced in M9 by `uniffi-bindgen generate … --language kotlin`) are added to
// src/main/kotlin in M9 alongside the jniLibs (.so per ABI). This module wraps
// them with the ergonomic API + native callback-interface impls.
plugins {
    id("com.android.library")
    kotlin("android")
}

android {
    namespace = "com.lingxi.code"
    compileSdk = 34
    defaultConfig { minSdk = 26 }
}

dependencies {
    // net.java.dev.jna:jna (aar) is required by UniFFI-generated Kotlin — added in M9.
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.0")
}
