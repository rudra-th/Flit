import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The release key is never in the repository: CI holds it base64-encoded in
// secrets, and a local build can point FLIT_KEYSTORE at the same file. When it
// is absent the release build is unsigned, which is a test artifact, not
// something to publish.
val releaseKey = System.getenv("FLIT_KEYSTORE")
val releaseKeyPassword = System.getenv("FLIT_KEYSTORE_PASSWORD")
val releaseKeyAlias = System.getenv("FLIT_KEY_ALIAS")
val releaseKeyPassword2 = System.getenv("FLIT_KEY_PASSWORD")
val hasReleaseKey = listOf(
    releaseKey,
    releaseKeyPassword,
    releaseKeyAlias,
    releaseKeyPassword2,
).all { !it.isNullOrBlank() }

android {
    namespace = "dev.flit.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.flit.android"
        minSdk = 26
        targetSdk = 36
        versionCode = 2
        versionName = "0.1.2"

        // The two ABIs CI compiles. JNA's aar carries mips and armeabi stubs;
        // shipping them would claim an install the engine cannot serve.
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    signingConfigs {
        create("release") {
            if (hasReleaseKey) {
                storeFile = file(releaseKey!!)
                storePassword = releaseKeyPassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword2
            }
        }
    }

    buildTypes {
        release {
            // The APK is an unsigned test artifact unless the release key is
            // present. A tag run always supplies it, so the published APK keeps
            // one identity and upgrades over the previous install.
            isMinifyEnabled = false
            if (hasReleaseKey) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        // libflit_mobile.so arrives through jniLibs from `cargo ndk`; JNA ships
        // its own .so files the same way. Both must stay loadable as-is.
        jniLibs {
            useLegacyPackaging = false
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.06.01")
    implementation(composeBom)

    implementation("androidx.core:core-ktx:1.16.0")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")

    // The generated Kotlin bindings borrow JNA's cleaner to free Rust objects;
    // everything else in JNA is only used by the FFI caller machinery.
    implementation("net.java.dev.jna:jna:5.14.0@aar")

    debugImplementation("androidx.compose.ui:ui-tooling")
}
