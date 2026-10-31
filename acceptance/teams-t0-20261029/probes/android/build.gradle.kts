plugins { alias(libs.plugins.android.application) }

android {
    namespace = "com.lingxi.teamsspike"
    compileSdk = libs.versions.compileSdk.get().toInt()
    defaultConfig {
        applicationId = "com.lingxi.teamsspike"
        minSdk = libs.versions.minSdk.get().toInt()
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
    }
    flavorDimensions += "sdk"
    productFlavors {
        create("baseline") { dimension = "sdk" }
        create("firestore") { dimension = "sdk"; applicationIdSuffix = ".firestore" }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
dependencies {
    implementation(platform(libs.firebase.bom))
    implementation(libs.firebase.auth)
    implementation(libs.firebase.messaging)
    implementation(libs.firebase.installations)
    implementation(libs.firebase.crashlytics.ndk)
    add("firestoreImplementation", libs.firebase.firestore)
}
