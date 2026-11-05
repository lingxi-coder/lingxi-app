plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}
android {
    namespace = "com.lingxi.code"
    compileSdk = libs.versions.compileSdk.get().toInt()
    defaultConfig { applicationId = "com.lingxi.teamsmembers.probe"; minSdk = 29; targetSdk = 35; versionCode = 1; versionName = "1.0" }
    buildFeatures { compose = true }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    sourceSets {
        getByName("main") {
            java.srcDirs(layout.buildDirectory.dir("teams-src").get().asFile)
            res.srcDir(layout.buildDirectory.dir("teams-res").get().asFile)
        }
        getByName("test") {
            java.srcDirs(layout.buildDirectory.dir("teams-tests").get().asFile)
        }
    }
}
kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }
val stageTeamsStrings by tasks.registering {
    val source = file("../../../../apps/android/native/app/src/main/res/values-en/strings.xml")
    inputs.file(source); outputs.dir(layout.buildDirectory.dir("teams-res"))
    doLast {
        val target = layout.buildDirectory.file("teams-res/values/strings.xml").get().asFile
        target.parentFile.mkdirs()
        target.writeText("<resources>\n" + source.readLines().filter { it.contains("<string ") }.joinToString("\n") + "\n</resources>\n")
    }
}
tasks.named("preBuild") { dependsOn(stageTeamsStrings) }
val stageTeamsSources = tasks.register<Sync>("stageTeamsSources") {
    from("../../../../apps/android/native/app/src/main/java") {
        include("com/lingxi/code/teams/**", "com/lingxi/code/navigation/DeepLink.kt", "com/lingxi/code/navigation/DeepLinkInbox.kt", "com/lingxi/code/push/PushRoute.kt")
    }
    from("Sources")
    into(layout.buildDirectory.dir("teams-src"))
}
val stageTeamsTests = tasks.register<Sync>("stageTeamsTests") {
    from("../../../../apps/android/native/app/src/test/java") {
        include("com/lingxi/code/teams/**", "com/lingxi/code/navigation/DeepLinkTest.kt", "com/lingxi/code/navigation/DeepLinkInboxTest.kt")
    }
    into(layout.buildDirectory.dir("teams-tests"))
}
tasks.named("preBuild") { dependsOn(stageTeamsSources, stageTeamsTests) }
dependencies {
    implementation(platform(libs.firebase.bom)); implementation(libs.firebase.auth); implementation(libs.firebase.firestore)
    implementation(platform(libs.androidx.compose.bom)); implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.activity.compose); implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose); implementation(libs.androidx.lifecycle.viewmodel.ktx)
    implementation(libs.kotlinx.coroutines.android)
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-play-services:1.11.0")
    testImplementation(libs.junit); testImplementation(libs.json); testImplementation(libs.kotlinx.coroutines.test)
}

// Current tab dependencies, scoped to the feature probe.
val sharedProbeSources = file("../../../../acceptance/mobile-team-bot-20261104/probes/android")
android.sourceSets.getByName("main").java.srcDir(sharedProbeSources)
val productRoot = file("../../../../apps/android/native/app/src/main/java/com/lingxi/code")
val tabSupport = files(
    productRoot.resolve("tabs/AppTab.kt"),
    productRoot.resolve("tabs/TabLogic.kt"),
    productRoot.resolve("theme/AvatarIdentity.kt"),
    productRoot.resolve("theme/AgentAvatar.kt"),
    productRoot.resolve("theme/PersonAvatar.kt"),
)
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    if (name == "compileDebugKotlin") source(tabSupport)
    if (name == "compileDebugUnitTestKotlin") source(file("../../../../apps/android/native/app/src/test/java/com/lingxi/code/tabs/SharedVectors.kt"))
}
val stageProbeAvatars = tasks.register<Sync>("stageProbeAvatars") {
    from("../../../../apps/android/native/app/src/main/res") { include("drawable*/agent_avatar_*") }
    into(layout.buildDirectory.dir("probe-avatar-res"))
}
android.sourceSets.getByName("main").res.srcDir(layout.buildDirectory.dir("probe-avatar-res").get().asFile)
tasks.named("preBuild") { dependsOn(stageProbeAvatars) }
dependencies { implementation(libs.androidx.datastore.preferences); implementation("androidx.compose.material:material-icons-extended") }

android.sourceSets.getByName("test").resources.srcDir("../../../../packages/bridge-client/fixtures")

tasks.withType<Test>().configureEach { workingDir(file("../../../../apps/android/native")) }
