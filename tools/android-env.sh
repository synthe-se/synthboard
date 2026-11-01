# Sourced by the tools that reach the Android SDK, once `root` is set. Runs nothing by itself.

# ANDROID_HOME wins, as it does for Gradle. Otherwise, Android Studio's default for this OS.
if [ -n "${ANDROID_HOME:-}" ]; then
    android_sdk=$ANDROID_HOME
elif [ "$(uname -s)" = Darwin ]; then
    android_sdk=$HOME/Library/Android/sdk
else
    android_sdk=$HOME/Android/Sdk
fi

android_setting() { sed -n "s/^synthboard\.$1=//p" "$root/android/gradle.properties"; }

# The newest build tools, as the shell sorts their version directories.
android_build_tools() { ls -d "$android_sdk"/build-tools/* | tail -1; }

android_ndk() { printf '%s\n' "$android_sdk/ndk/$(android_setting ndkVersion)"; }

# An NDK install holds one host's tree, so the host tag is found here rather than named a
# second time: `android/app/build.gradle.kts` is the one place that picks it from the OS.
android_ndk_bin() { ls -d "$(android_ndk)"/toolchains/llvm/prebuilt/*/bin; }
