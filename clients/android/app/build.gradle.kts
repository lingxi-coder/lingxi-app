plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.lingxi.code"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.lingxi.code"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        vectorDrawables {
            useSupportLibrary = true
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
            applicationIdSuffix = ".debug"
        }
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }

    testOptions {
        // JVM unit tests only touch pure-Kotlin code (Color value-class, mock
        // data); returning defaults for any stray android.jar stub keeps them
        // off an emulator. Instrumented (androidTest) UI tests need a device.
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.05.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    // Coroutines — required by the extracted device-audio layer
    // (STT/TTS providers use suspendCancellableCoroutine + Flow) and by the
    // generated UniFFI bindings (async callback interfaces).
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")

    // UniFFI Kotlin runtime — the generated bindings in
    // com/lingxi/code/bindings/android_aar.kt (one merged file for all four
    // crates) load the Rust cdylib (jniLibs/<abi>/libandroid_aar.so) through
    // JNA. The @aar classifier pulls JNA's bundled native libs so Native.load
    // resolves on-device.
    implementation("net.java.dev.jna:jna:5.14.0@aar")

    // Core / lifecycle
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.9.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.0")

    // Activity + Compose
    implementation("androidx.activity:activity-compose:1.10.1")

    // Compose UI
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    // Navigation
    implementation("androidx.navigation:navigation-compose:2.9.6")

    // DataStore (preferences) for persisted theme / accent
    implementation("androidx.datastore:datastore-preferences:1.1.7")

    // Unit tests
    testImplementation("junit:junit:4.13.2")
    // Coroutine/Flow test harness — runTest + UnconfinedTestDispatcher drive the
    // engine reply-stream ordering tests (subscribe-before-submit, terminal
    // completion) on the plain JVM with virtual time.
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.9.0")

    // Instrumented + Compose UI tests
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.6.1")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")

    // Debug tooling
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
