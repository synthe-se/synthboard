plugins {
    id("com.android.application")
    id("io.github.takahirom.roborazzi")
}

android {
    namespace = "app.synthboard"
    compileSdk = 37
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "app.synthboard"
        minSdk = 34
        targetSdk = 37
        versionCode = 1
        versionName = "0.0.0"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildFeatures {
        buildConfig = false
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
    }

    buildTypes {
        debug {
            // A debuggable build must never be able to replace the real keyboard.
            applicationIdSuffix = ".debug"
        }
        release {
            // Left unsigned here: `tools/sign-release.sh` signs with the key
            // that only exists on the encrypted volume.
            signingConfig = null
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
}

dependencies {
    implementation("androidx.autofill:autofill:1.3.0")
    testImplementation("androidx.test:core:1.7.0")
    testImplementation("androidx.test.ext:junit:1.3.0")
    testImplementation("io.github.takahirom.roborazzi:roborazzi:1.77.0")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.17")
}

val generated = layout.buildDirectory.dir("native")
val repository = rootDir.parentFile

val nativeLibrary = tasks.register<Exec>("nativeLibrary") {
    description = "Builds the Rust library for the phone."
    workingDir = repository
    val toolchain = androidComponents.sdkComponents.sdkDirectory.get().asFile
        .resolve("ndk/${android.ndkVersion}/toolchains/llvm/prebuilt/darwin-x86_64/bin")
    environment("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", toolchain.resolve("aarch64-linux-android34-clang").path)
    // Page size for current phones, and no path of this machine in the library.
    environment(
        "RUSTFLAGS",
        "-Clink-arg=-Wl,-z,max-page-size=16384 -Clink-arg=-lc++abi -Clink-arg=-Wl,--wrap=free " +
            "--remap-path-prefix=${System.getProperty("user.home")}=~",
    )
    // ONNX Runtime, static and built by `tools/build-ort.sh`.
    environment("ORT_LIB_PATH", repository.resolve("data/raw/ort/android").path)
    environment("ORT_CXX_STDLIB", "c++_static")
    commandLine(
        "cargo", "build", "--release", "--target", "aarch64-linux-android",
        "-p", "synthboard-ffi", "--lib", "--features", "onnx",
    )
    outputs.upToDateWhen { false }
}

val nativeCopy = tasks.register<Copy>("nativeCopy") {
    dependsOn(nativeLibrary)
    from(repository.resolve("target/aarch64-linux-android/release/libsynthboard_ffi.so"))
    into(generated.map { it.dir("jniLibs/arm64-v8a") })
}

val wireTypes = tasks.register<Exec>("wireTypes") {
    description = "Generates the Kotlin side of the wire from the Rust types."
    workingDir = repository
    val output = generated.get().file("kotlin/WireTypes.kt").asFile
    doFirst { output.parentFile.mkdirs() }
    commandLine("cargo", "run", "-q", "-p", "synthboard-ffi", "--bin", "kotlin_wire", "--", output.path)
    outputs.upToDateWhen { false }
}

android.sourceSets.getByName("main").apply {
    jniLibs.directories.add(generated.get().dir("jniLibs").asFile.path)
    kotlin.directories.add(generated.get().dir("kotlin").asFile.path)
}

tasks.named("preBuild") {
    dependsOn(nativeCopy, wireTypes)
}
