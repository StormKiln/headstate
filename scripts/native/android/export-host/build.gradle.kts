plugins { id("com.android.application") }
android {
    namespace = "com.headstate.nativecheck"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.headstate.nativecheck"
        minSdk = 24
        targetSdk = 36
        versionCode = 1
        versionName = "1"
    }
}
dependencies { implementation(project(":headstate-export")) }
