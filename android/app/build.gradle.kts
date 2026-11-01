plugins {
    id("com.android.application")
    id("io.github.takahirom.roborazzi")
}

val minimumSdk = providers.gradleProperty("synthboard.minSdk").get().toInt()
val targetedSdk = providers.gradleProperty("synthboard.targetSdk").get().toInt()

android {
    namespace = "app.synthboard"
    compileSdk = targetedSdk
    ndkVersion = providers.gradleProperty("synthboard.ndkVersion").get()

    defaultConfig {
        applicationId = "app.synthboard"
        minSdk = minimumSdk
        targetSdk = targetedSdk
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

// The NDK's tree for this host. It keeps the x86_64 name on Apple silicon, where it is universal.
val ndkHostTag = System.getProperty("os.name").lowercase().let { os ->
    when {
        os.startsWith("mac") -> "darwin-x86_64"
        os.startsWith("linux") -> "linux-x86_64"
        else -> error("No NDK host tag for $os: the native build runs on macOS and Linux.")
    }
}

val generated = layout.buildDirectory.dir("native")
val repository = rootDir.parentFile

val nativeLibrary = tasks.register<Exec>("nativeLibrary") {
    description = "Builds the Rust library for the phone."
    workingDir = repository
    val toolchain = androidComponents.sdkComponents.sdkDirectory.get().asFile
        .resolve("ndk/${android.ndkVersion}/toolchains/llvm/prebuilt/$ndkHostTag/bin")
    environment("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", toolchain.resolve("aarch64-linux-android$minimumSdk-clang").path)
    // A rebuilt standard library leaves out the helpers that outlined atomics and emulated
    // thread locals call, which the shipped one carries, so they come from the NDK runtime.
    val builtins = toolchain.parentFile.resolve("lib/clang").listFiles().orEmpty().single()
        .resolve("lib/linux/libclang_rt.builtins-aarch64-android.a")
    // Page size for current phones, and no path of this machine in the library.
    environment(
        "RUSTFLAGS",
        "-Clink-arg=-Wl,-z,max-page-size=16384 -Clink-arg=-lc++abi -Clink-arg=-Wl,--wrap=free " +
            "-Clink-arg=-Wl,-z,bti-report=error -Clink-arg=$builtins -Zbranch-protection=bti,pac-ret " +
            "--remap-path-prefix=${System.getProperty("user.home")}=~",
    )
    // BTI and PAC need the standard library rebuilt with them, which only nightly flags do.
    // This keeps the stable compiler and turns those two flags on, rather than ship a nightly one.
    environment("RUSTC_BOOTSTRAP", "1")
    // ONNX Runtime, static and built by `tools/build-ort.sh`.
    environment("ORT_LIB_PATH", repository.resolve("data/raw/ort/android").path)
    environment("ORT_CXX_STDLIB", "c++_static")
    commandLine(
        "cargo", "build", "--release", "--target", "aarch64-linux-android",
        "-p", "synthboard-ffi", "--lib", "--features", "onnx", "-Zbuild-std=std",
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
