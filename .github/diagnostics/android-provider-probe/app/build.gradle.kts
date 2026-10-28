plugins { id("com.android.application") }
android {
    namespace = "com.lingxi.code"
    compileSdk = 37
    defaultConfig { applicationId = "com.lingxi.code"; minSdk = 26; targetSdk = 35; testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner" }
    flavorDimensions += "distribution"
    productFlavors {
        create("play") { dimension = "distribution" }
        create("direct") { dimension = "distribution"; applicationIdSuffix = ".direct" }
    }
    buildTypes { debug { applicationIdSuffix = ".debug" } }
}

dependencies { androidTestImplementation("androidx.test:runner:1.7.0"); androidTestImplementation("androidx.test.ext:junit:1.3.0") }
