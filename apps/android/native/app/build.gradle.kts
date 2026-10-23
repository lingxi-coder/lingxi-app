import java.util.Properties
import groovy.json.JsonSlurper
import com.google.firebase.crashlytics.buildtools.gradle.CrashlyticsExtension

val voiceManifest = rootProject.file("../../../resources/voice/models.json")
val voiceRuntime = JsonSlurper().parse(voiceManifest) as Map<*, *>
val sherpaRuntimeVersion = (voiceRuntime["runtime"] as Map<*, *>)["version"] as String
val mobileLinuxContract = JsonSlurper().parse(rootProject.file("../../../docs/mobile-linux/mobile-linux-native-pins.json")) as Map<*, *>
val mobileLinuxSdkVersion = mobileLinuxContract["sdk_version"] as String

plugins {
    alias(libs.plugins.google.services)
    alias(libs.plugins.android.application)
    alias(libs.plugins.firebase.crashlytics)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

// Local release credentials stay in the ignored .signing directory. CI may
// supply this file separately; a checkout without it keeps release unsigned.
val localReleaseSigningFile = rootProject.file(".signing/release-signing.properties")
val localReleaseSigning = localReleaseSigningFile.takeIf { it.isFile }?.let { file ->
    Properties().apply { file.inputStream().use { load(it) } }
}

// Release verification may build one architecture explicitly; never package a
// stale second JNI library while waiting for its current SDK build.
val targetAbis = providers.gradleProperty("lingxiTargetAbis").orNull
    ?.split(',')?.map { it.trim() } ?: listOf("arm64-v8a", "x86_64")
require(targetAbis.isNotEmpty() && targetAbis.all { it in setOf("arm64-v8a", "x86_64") }) {
    "lingxiTargetAbis must contain arm64-v8a, x86_64, or both"
}

android {
    namespace = "com.lingxi.code"
    compileSdk = libs.versions.compileSdk.get().toInt()

    defaultConfig {
        applicationId = "com.lingxi.code"
        minSdk = libs.versions.minSdk.get().toInt()
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
            abiFilters += targetAbis
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
            configure<CrashlyticsExtension> {
                unstrippedNativeLibsDir = file("build/nativeSymbols/play")
            }
        }
        create("direct") {
            dimension = "distribution"
            applicationIdSuffix = ".direct"
            // "direct" is the Full distribution. Runtime permission gates
            // still apply; this flag only describes compiled capabilities.
            buildConfigField("String", "DISTRIBUTION_CHANNEL", "\"full\"")
            buildConfigField("boolean", "MOBILE_LINUX_FULL", "true")
            buildConfigField("boolean", "HIGH_RISK_NATIVE_OFFLOADS", "true")
            configure<CrashlyticsExtension> {
                unstrippedNativeLibsDir = file("build/nativeSymbols/direct")
            }
        }
    }

    signingConfigs {
        localReleaseSigning?.let { credentials ->
            create("localRelease") {
                storeFile = localReleaseSigningFile.parentFile.resolve(credentials.getProperty("storeFile"))
                storePassword = credentials.getProperty("storePassword")
                keyAlias = credentials.getProperty("keyAlias")
                keyPassword = credentials.getProperty("keyPassword")
                storeType = "PKCS12"
            }
        }
        getByName("debug") {
            storeFile = rootProject.file(".signing/lingxi-5e4b2-debug.keystore")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
            applicationIdSuffix = ".debug"
            manifestPlaceholders["crashlyticsCollectionEnabled"] = false
        }
        release {
            if (localReleaseSigning != null) signingConfig = signingConfigs.getByName("localRelease")
            manifestPlaceholders["crashlyticsCollectionEnabled"] = true
            configure<CrashlyticsExtension> {
                nativeSymbolUploadEnabled = true
            }
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
        // Android PRoot helpers ship as lib*.so under
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
        getByName("test").resources.srcDir(rootProject.file("../../../resources/voice"))
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
        description = "Verify MobileLinux ELF artifacts for the selected ABIs ($distribution)."
        commandLine(
            rootProject.file("scripts/verify-mobile-linux-native.sh").absolutePath,
            "--variant",
            distribution,
            "--abis", targetAbis.joinToString(","),
        )
    }
    listOf("debug", "release").forEach { buildType ->
        val variantName = capitalized + buildType.replaceFirstChar(Char::uppercaseChar)
        val verifyApk = tasks.register<Exec>("verify${variantName}MobileLinuxApk") {
            group = "verification"
            description = "Check the final APK's native bytes and extraction packaging ($variantName)."
            commandLine(
                "python3", rootProject.file("scripts/verify-mobile-linux-apk.py").absolutePath,
                "--apk-dir", layout.buildDirectory.dir("outputs/apk/$distribution/$buildType").get().asFile,
                "--source", layout.buildDirectory.dir("mobileLinuxNative/$distribution/native-support").get().asFile,
                "--jni-root", file("src/$distribution/jniLibs"),
                "--assets-source", layout.buildDirectory.dir("generated/mobileLinux/$distribution/assets/mobile-linux").get().asFile,
                "--abis", targetAbis.joinToString(","),
            )
        }
        tasks.matching { it.name == "pre${variantName}Build" }.configureEach {
            dependsOn("verify${capitalized}MobileLinuxNative")
        }
        tasks.matching { it.name == "assemble$variantName" }.configureEach {
            finalizedBy(verifyApk)
        }
    }
}

dependencies {
    implementation(platform(libs.firebase.bom))
    implementation(libs.firebase.auth)
    implementation(libs.firebase.messaging)
    implementation(libs.firebase.installations)
    // Includes JVM/ANR reporting and the native signal handler for Rust/JNI.
    implementation(libs.firebase.crashlytics.ndk)
    implementation(libs.androidx.credentials)
    implementation(libs.androidx.credentials.play.services.auth)
    implementation(libs.googleid)
    implementation(libs.kotlinx.coroutines.play.services)
    // Installer + native helpers only: libandroid_aar.so already owns Rust FFI.
    implementation("io.github.lingxi-coder:mobile-linux-installer:$mobileLinuxSdkVersion")
    implementation("io.github.lingxi-coder:mobile-linux-native-support:$mobileLinuxSdkVersion")
    val composeBom = platform(libs.androidx.compose.bom)
    implementation(composeBom)
    androidTestImplementation(composeBom)

    // Coroutines — required by the extracted device-audio layer
    // (STT/TTS providers use suspendCancellableCoroutine + Flow) and by the
    // generated UniFFI bindings (async callback interfaces).
    implementation(libs.kotlinx.coroutines.android)

    // UniFFI Kotlin runtime — the generated bindings in
    // com/lingxi/code/bindings/android_aar.kt (one merged file for all four
    // crates) load the Rust cdylib (jniLibs/<abi>/libandroid_aar.so) through
    // JNA. The @aar classifier pulls JNA's bundled native libs so Native.load
    // resolves on-device.
    implementation("net.java.dev.jna:jna:${libs.versions.jna.get()}@aar")

    // Core / lifecycle
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    // SavedStateHandle + createSavedStateHandle() — the transcript / draft /
    // active session survive process death (low-memory kill while backgrounded).
    implementation(libs.androidx.lifecycle.viewmodel.savedstate)

    // Inline-visualization WebView security boundary: document-start bridge
    // injection and origin-scoped, main-frame-only WebMessageListener delivery.
    implementation(libs.androidx.webkit)

    // Activity + Compose
    implementation(libs.androidx.activity.compose)

    // Compose UI
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.material.icons.extended)

    // Navigation
    implementation(libs.androidx.navigation.compose)

    // DataStore (preferences) for persisted theme / accent
    implementation(libs.androidx.datastore.preferences)

    // Persistent Android cron execution. AlarmManager only provides the precise
    // wake-up signal; WorkManager owns network constraints, retries, process
    // recovery, and the serialized execution queue.
    implementation(libs.androidx.workmanager.ktx)

    // Offline voice models: tar.bz2 extraction for the sherpa-onnx packs the
    // setup wizard downloads (pure-Java bzip2 + tar; no native dependency).
    implementation(libs.commons.compress)

    // sherpa-onnx offline voice runtime (vendored AAR in app/libs/, via the
    // flatDir repo in settings.gradle.kts). Carries the com.k2fsa.sherpa.onnx
    // Kotlin API + the static-linked onnxruntime JNI .so for on-device STT/TTS.
    implementation(group = "", name = "sherpa-onnx-static-link-onnxruntime-$sherpaRuntimeVersion", ext = "aar")

    // Secure key store — the Anthropic API key + base URL are encrypted at rest
    // via EncryptedSharedPreferences (AES-256 GCM, key wrapped by the Android
    // Keystore). SHIP-BLOCKER #1: a shipped app has no process env, so the key
    // must live in an encrypted on-device store, never plain DataStore/prefs.
    implementation(libs.androidx.security.crypto)

    // Unit tests
    testImplementation(libs.junit)
    // Coroutine/Flow test harness — runTest + UnconfinedTestDispatcher drive the
    // engine reply-stream ordering tests (subscribe-before-submit, terminal
    // completion) on the plain JVM with virtual time.
    testImplementation(libs.kotlinx.coroutines.test)
    // Real org.json for JVM unit tests: `isReturnDefaultValues = true` makes the
    // android.jar JSONObject stub return null everywhere, so the per-scope
    // conversation-state store (ScopeStateStore) could not be exercised at all.
    // The real artifact shadows the stub on the unit-test classpath only.
    testImplementation(libs.json)

    // Instrumented + Compose UI tests
    androidTestImplementation(libs.androidx.junit)
    androidTestImplementation(libs.androidx.espresso.core)
    androidTestImplementation(libs.androidx.compose.ui.test.junit4)
    androidTestImplementation(libs.androidx.workmanager.testing)

    // Debug tooling
    debugImplementation(libs.androidx.compose.ui.tooling)
    debugImplementation(libs.androidx.compose.ui.test.manifest)
}
