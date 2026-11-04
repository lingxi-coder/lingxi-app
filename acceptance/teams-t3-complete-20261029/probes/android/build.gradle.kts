plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}
layout.buildDirectory.set(file("../../../../output/teams-t3-android/build"))
android {
    namespace = "com.lingxi.code"
    compileSdk = libs.versions.compileSdk.get().toInt()
    defaultConfig {
        applicationId = "com.lingxi.teamst3.probe"
        minSdk = 29; targetSdk = 35; versionCode = 1; versionName = "1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    buildFeatures { compose = true }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    sourceSets {
        getByName("main") {
            // All actual Teams sources, unchanged. No native engine/JNI sources.
            java.srcDirs("../../../../apps/android/native/app/src/main/java/com/lingxi/code/teams", "Sources")
            res.srcDir("../../../../output/teams-t3-android/resources")
        }
        getByName("test") { java.srcDirs("../../../../apps/android/native/app/src/test/java/com/lingxi/code/teams") }
        getByName("androidTest") { java.srcDirs("Tests") }
    }
    testOptions { unitTests.isReturnDefaultValues = true }
}
kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }
// Existing TeamInvitationTest exercises canonical cold-start/push routing.
// Compile these exact production files into the JVM test variant only; adding
// the whole product Java root would also pull unrelated native app dependencies.
val canonicalInvitationSources = files(
    "../../../../apps/android/native/app/src/main/java/com/lingxi/code/navigation/DeepLink.kt",
    "../../../../apps/android/native/app/src/main/java/com/lingxi/code/navigation/DeepLinkInbox.kt",
    "../../../../apps/android/native/app/src/main/java/com/lingxi/code/push/PushRoute.kt",
)
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    if (name == "compileDebugUnitTestKotlin") source(canonicalInvitationSources)
}
val prepareProbe by tasks.registering(Exec::class) {
    commandLine("python3", file("prepare.py"))
    inputs.dir("../../../../apps/android/native/app/src/main/java/com/lingxi/code/teams")
    inputs.file("../../../../apps/android/native/app/src/main/res/values-en/strings.xml")
    inputs.files(canonicalInvitationSources)
    outputs.file("../../../../output/teams-t3-android/resources/values/strings.xml")
    outputs.file("../../../../output/teams-t3-android/provenance.json")
}
tasks.named("preBuild") { dependsOn(prepareProbe) }
dependencies {
    implementation(platform(libs.firebase.bom)); implementation(libs.firebase.auth); implementation(libs.firebase.firestore)
    implementation(platform(libs.androidx.compose.bom)); implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.activity.compose); implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose); implementation(libs.androidx.lifecycle.viewmodel.ktx)
    implementation(libs.kotlinx.coroutines.android); implementation(libs.kotlinx.coroutines.play.services)
    testImplementation(libs.junit); testImplementation(libs.json); testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.androidx.lifecycle.viewmodel.savedstate)
    androidTestImplementation(platform(libs.androidx.compose.bom))
    androidTestImplementation(libs.androidx.compose.ui.test.junit4)
    androidTestImplementation(libs.androidx.junit); androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.espresso.core)
    debugImplementation(libs.androidx.compose.ui.test.manifest)
}
