plugins {
    id("com.android.application")
    id("io.github.takahirom.roborazzi")
}

android {
    namespace = "app.synthboard"
    compileSdk = 37
    ndkVersion = "28.2.13676358"

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

val generated = layout.buildDirectory.dir("boltffi")

val boltffiPack = tasks.register<Exec>("boltffiPack") {
    description = "Builds the Rust library and generates its Kotlin bindings."
    workingDir = rootDir.parentFile.resolve("crates/synthboard-ffi")
    environment(
        "ANDROID_NDK_HOME",
        androidComponents.sdkComponents.sdkDirectory.get().asFile.resolve("ndk/${android.ndkVersion}").path,
    )
    // Keeps this machine's paths out of the shipped library.
    environment("RUSTFLAGS", "--remap-path-prefix=${System.getProperty("user.home")}=~")
    commandLine("boltffi", "pack", "android", "--release")
    outputs.dir(generated)
    outputs.upToDateWhen { false }
}

android.sourceSets.getByName("main").apply {
    jniLibs.directories.add(generated.get().dir("jniLibs").asFile.path)
    kotlin.directories.add(generated.get().dir("kotlin").asFile.path)
}

tasks.named("preBuild") {
    dependsOn(boltffiPack)
}
