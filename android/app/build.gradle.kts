plugins {
    id("com.android.application")
}

android {
    namespace = "app.synthboard"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "app.synthboard"
        minSdk = 34
        targetSdk = 36
        versionCode = 1
        versionName = "0.0.0"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildFeatures {
        buildConfig = false
    }
}

val generated = layout.buildDirectory.dir("boltffi")

val boltffiPack = tasks.register<Exec>("boltffiPack") {
    description = "Builds the Rust library and generates its Kotlin bindings."
    workingDir = rootDir.parentFile.resolve("crates/synthboard-ffi")
    environment(
        "ANDROID_NDK_HOME",
        androidComponents.sdkComponents.sdkDirectory.get().asFile.resolve("ndk/${android.ndkVersion}").path,
    )
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
