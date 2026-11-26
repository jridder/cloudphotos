plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
    id("com.chaquo.python")
}

android {
    namespace = "com.jridder.cloudphotos"
    compileSdk {
        version = release(37) { minorApiLevel = 1 }
    }

    defaultConfig {
        applicationId = "com.jridder.cloudphotos"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            // Phones, and the x86_64 emulator.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Sideloaded: sign with the debug key until there's a release key.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

// The same helper the desktop app runs (../../helper/photos_helper.py),
// here inside the app through Chaquopy.
chaquopy {
    defaultConfig {
        version = "3.13"
        // Chaquopy needs a Python of the same version on the build machine.
        buildPython(
            System.getenv("CHAQUOPY_BUILD_PYTHON")
                ?: listOf(
                    "${System.getProperty("user.home")}/.local/share/uv/python/cpython-3.13-linux-x86_64-gnu/bin/python3.13",
                    "/usr/bin/python3.13",
                ).firstOrNull { File(it).exists() }
                ?: "python3.13"
        )
        pip {
            // pydantic-core has no Android wheels anywhere; wheels/build-pydantic-core.sh
            // cross-compiles them. Chaquopy's cryptography stops at 42.0.8, which has
            // everything pyicloud uses, but pyicloud asks for >=44, so dependencies
            // are pinned here (the set icloud-linux runs) instead of resolved.
            options("--find-links", "${rootDir}/wheels/dist")
            options("--no-deps")
            install("pyicloud==2026.10.1")
            install("certifi==2026.10.23")
            install("cryptography==42.0.8")
            install("cffi==1.17.1")
            install("chaquopy-libffi==3.3")
            install("pycparser==3.12")
            install("typing_extensions==4.16.0")
            install("fido2==2.2.1")
            install("keyring==25.7.0")
            install("jaraco.classes==3.4.0")
            install("jaraco.functools==4.6.0")
            install("jaraco.context==6.1.2")
            install("more-itertools==11.2.1")
            install("keyrings.alt==5.0.2")
            install("protobuf==7.36.2")
            install("pydantic==2.14.2")
            install("pydantic-core==2.50.2")
            install("annotated-types==0.8.0")
            install("typing-inspection==0.4.4")
            install("requests==2.34.2")
            install("charset-normalizer==3.5.2")
            install("idna==3.20")
            install("urllib3==2.8.0")
            install("srp==1.0.22")
            install("six==1.17.0")
            install("tinyhtml==1.3.0")
            install("tzlocal==5.3.1")
        }
    }
    sourceSets {
        getByName("main") {
            srcDir("../../helper")
        }
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.11.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.11.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.1")
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("io.coil-kt.coil3:coil-compose:3.6.3")
    implementation("androidx.media3:media3-exoplayer:1.11.1")
    implementation("androidx.media3:media3-ui:1.11.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
