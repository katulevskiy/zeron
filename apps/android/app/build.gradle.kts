plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Rust outputs (docs/android.md § Build): the UniFFI core and the on-device
// runtime executables, each laid out as jniLibs/<abi>/lib*.so.
// `scripts/android/build-core.sh` writes the core; the Kotlin bindings it
// generates are committed under src/main/java/uniffi (like the iOS app's
// Core/Generated), so this build never needs cargo.
val repoRoot = rootProject.projectDir.parentFile.parentFile
val coreJniLibs = File(repoRoot, "target/android-core/jniLibs")
val runtimeJniLibs = File(repoRoot, "target/android-runtime/jniLibs")

android {
    namespace = "sh.zeron.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "sh.zeron.android"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.2.98"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    sourceSets["main"].jniLibs.srcDirs(coreJniLibs, runtimeJniLibs)

    packaging {
        // The runtime ships real executables as lib*.so; they must be
        // extracted to nativeLibraryDir (the only exec-allowed location).
        jniLibs.useLegacyPackaging = true
        jniLibs.keepDebugSymbols += "**/libzeron.so"
        jniLibs.keepDebugSymbols += "**/libproot*.so"
        // patchelf'd (SONAME/NEEDED rewritten) by fetch-proot.sh: AGP's strip
        // pass corrupts them — libproot then fails to link
        // ("cannot locate symbol talloc_enable_leak_report").
        jniLibs.keepDebugSymbols += "**/libtalloc.so"
        jniLibs.keepDebugSymbols += "**/libandroid-shmem.so"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    testOptions {
        // Pure-Kotlin unit tests only (formatters, reducers): Android stubs
        // return defaults instead of throwing.
        unitTests.isReturnDefaultValues = true
    }
}

kotlin { jvmToolchain(17) }

dependencies {
    implementation(project(":runtime"))
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    // Foundation only: the design language is Zeron's, not Material's.
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.animation:animation")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-process:2.8.7")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.browser:browser:1.8.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    // UniFFI's generated Kotlin binds through JNA.
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    // Desktop tool/file icons ship as SVG (same files as iOS).
    implementation("com.caverock:androidsvg-aar:1.4")
    debugImplementation("androidx.compose.ui:ui-tooling")

    testImplementation("junit:junit:4.13.2")
    // android.jar's org.json is a stub under unit tests; the Agents parsers need the real one.
    testImplementation("org.json:json:20240303")
}
