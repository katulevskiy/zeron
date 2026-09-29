plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

// scripts/android/fetch-rootfs.sh → assets/rootfs-<abi>.tar.gz
val repoRoot = projectDir.resolve("../../..").normalize()

android {
    namespace = "sh.zeron.runtime"
    compileSdk = 36
    defaultConfig { minSdk = 29 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets["main"].assets.srcDir(File(repoRoot, "target/android-runtime/assets"))
}

kotlin { jvmToolchain(17) }

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
}
