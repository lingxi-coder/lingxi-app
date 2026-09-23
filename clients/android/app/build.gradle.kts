import groovy.json.JsonSlurper

val voiceManifest = rootProject.file("../voice/models.json")
val voiceRuntime = JsonSlurper().parse(voiceManifest) as Map<*, *>
val sherpaRuntimeVersion = (voiceRuntime["runtime"] as Map<*, *>)["version"] as String

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.lingxi.code"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.lingxi.code"
        minSdk = 26
        // Explicit targetSdk (documented intent; AGP's unset-default is ambiguous
        // across versions). Required so the Android 13/14 foreground-service-type,
        // exact-alarm, and runtime-notification semantics the cron subsystem relies
        // on are actually enforced (and so the build is Play-uploadable).
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            // MobileLinux/PTY/legacy shell artifacts are release-gated for
            // exactly these two architectures.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
        vectorDrawables {
            useSupportLibrary = true
        }
    }

    flavorDimensions += "distribution"
    productFlavors {
        create("play") {
            dimension = "distribution"
            // "play" is the Store distribution described by the MobileLinux
            // plan. High-risk offloads are absent at compile time.
            buildConfigField("String", "DISTRIBUTION_CHANNEL", "\"store\"")
            buildConfigField("boolean", "MOBILE_LINUX_FULL", "false")
            buildConfigField("boolean", "HIGH_RISK_NATIVE_OFFLOADS", "false")
        }
        create("direct") {
            dimension = "distribution"
            applicationIdSuffix = ".direct"
            // "direct" is the Full distribution. Runtime permission gates
            // still apply; this flag only describes compiled capabilities.
            buildConfigField("String", "DISTRIBUTION_CHANNEL", "\"full\"")
            buildConfigField("boolean", "MOBILE_LINUX_FULL", "true")
            buildConfigField("boolean", "HIGH_RISK_NATIVE_OFFLOADS", "true")
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
            applicationIdSuffix = ".debug"
        }
        release {
            // Enable only after the minified release variant passes the
            // UniFFI/JNI device smoke documented in the optimizer checklist.
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
        buildConfig = true
    }

    androidResources {
        // Rootfs archives are already compressed and must remain byte-for-byte
        // identical to the SHA-256 recorded in rootfs-manifest.json.
        noCompress += listOf("gz", "zst")
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
        // P5a (Android bundled shell): mksh/toybox ship as lib*.so under
        // jniLibs/<abi> and MUST be EXTRACTED to nativeLibraryDir so they exist
        // as real, executable files on disk — Android 10+ W^X only permits
        // `execve` of files there. Uncompressed-in-APK (the AGP default since
        // 4.2) leaves them mmap'd inside the APK with no on-disk path to exec,
        // so force legacy (extracting) packaging. The Play "uncompressed native
        // libs" size note is the accepted cost (see plan Risks).
        jniLibs {
            useLegacyPackaging = true
        }
    }

    sourceSets {
        getByName("test").resources.srcDir("../../voice")
        // Native libraries are built independently so the Play artifact never
        // links the Direct-only android_use implementation.
        getByName("main").jniLibs.setSrcDirs(emptyList<String>())
        getByName("play").jniLibs.srcDir("src/play/jniLibs")
        getByName("direct").jniLibs.srcDir("src/direct/jniLibs")
        // Source-built rootfs archives and corresponding-source notices are
        // staged under build/ (gitignored), never copied from reference
        // binaries into the repository.
        getByName("play").assets.srcDir("build/generated/mobileLinux/play/assets")
        getByName("direct").assets.srcDir("build/generated/mobileLinux/direct/assets")
    }

    testOptions {
        // JVM unit tests only touch pure-Kotlin code (Color value-class, mock
        // data); returning defaults for any stray android.jar stub keeps them
        // off an emulator. Instrumented (androidTest) UI tests need a device.
        unitTests.isReturnDefaultValues = true
    }
}

listOf("play", "direct").forEach { distribution ->
    val capitalized = distribution.replaceFirstChar(Char::uppercaseChar)
    tasks.register<Exec>("verify${capitalized}MobileLinuxNative") {
        group = "verification"
        description = "Verify MobileLinux ELF artifacts for both supported ABIs ($distribution)."
        commandLine(
            rootProject.file("scripts/verify-mobile-linux-native.sh").absolutePath,
            "--variant",
            distribution,
        )
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.05.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    // Coroutines — required by the extracted device-audio layer
    // (STT/TTS providers use suspendCancellableCoroutine + Flow) and by the
    // generated UniFFI bindings (async callback interfaces).
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")

    // UniFFI Kotlin runtime — the generated bindings in
    // com/lingxi/code/bindings/android_aar.kt (one merged file for all four
    // crates) load the Rust cdylib (jniLibs/<abi>/libandroid_aar.so) through
    // JNA. The @aar classifier pulls JNA's bundled native libs so Native.load
    // resolves on-device.
    implementation("net.java.dev.jna:jna:5.19.0@aar")

    // Core / lifecycle
    implementation("androidx.core:core-ktx:1.19.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.10.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    // SavedStateHandle + createSavedStateHandle() — the transcript / draft /
    // active session survive process death (low-memory kill while backgrounded).
    implementation("androidx.lifecycle:lifecycle-viewmodel-savedstate:2.10.0")

    // Local-app WebView security boundary: document-start bridge injection and
    // origin-scoped, main-frame-only WebMessageListener delivery.
    implementation("androidx.webkit:webkit:1.12.1")

    // Activity + Compose
    implementation("androidx.activity:activity-compose:1.13.0")

    // Compose UI
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    // Navigation
    implementation("androidx.navigation:navigation-compose:2.9.8")

    // DataStore (preferences) for persisted theme / accent
    implementation("androidx.datastore:datastore-preferences:1.2.1")

    // Persistent Android cron execution. AlarmManager only provides the precise
    // wake-up signal; WorkManager owns network constraints, retries, process
    // recovery, and the serialized execution queue.
    implementation("androidx.work:work-runtime-ktx:2.11.2")

    // Offline voice models: tar.bz2 extraction for the sherpa-onnx packs the
    // setup wizard downloads (pure-Java bzip2 + tar; no native dependency).
    implementation("org.apache.commons:commons-compress:1.27.1")

    // sherpa-onnx offline voice runtime (vendored AAR in app/libs/, via the
    // flatDir repo in settings.gradle.kts). Carries the com.k2fsa.sherpa.onnx
    // Kotlin API + the static-linked onnxruntime JNI .so for on-device STT/TTS.
    implementation(group = "", name = "sherpa-onnx-static-link-onnxruntime-$sherpaRuntimeVersion", ext = "aar")

    // Secure key store — the Anthropic API key + base URL are encrypted at rest
    // via EncryptedSharedPreferences (AES-256 GCM, key wrapped by the Android
    // Keystore). SHIP-BLOCKER #1: a shipped app has no process env, so the key
    // must live in an encrypted on-device store, never plain DataStore/prefs.
    implementation("androidx.security:security-crypto:1.1.0")

    // Unit tests
    testImplementation("junit:junit:4.13.2")
    // Coroutine/Flow test harness — runTest + UnconfinedTestDispatcher drive the
    // engine reply-stream ordering tests (subscribe-before-submit, terminal
    // completion) on the plain JVM with virtual time.
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.11.0")
    // Real org.json for JVM unit tests: `isReturnDefaultValues = true` makes the
    // android.jar JSONObject stub return null everywhere, so the per-scope
    // conversation-state store (ScopeStateStore) could not be exercised at all.
    // The real artifact shadows the stub on the unit-test classpath only.
    testImplementation("org.json:json:20240303")

    // Instrumented + Compose UI tests
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.work:work-testing:2.11.2")

    // Debug tooling
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
