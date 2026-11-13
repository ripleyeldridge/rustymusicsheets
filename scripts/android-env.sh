#!/usr/bin/env bash
# Source this file to configure the locally installed Android toolchain.
rms_android_env() {
    local sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
    local ndk="${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-$sdk/ndk/28.2.13676358}}"
    local java="${JAVA_HOME:-}"
    local host
    case "$(uname -s)" in
        Linux) host=linux-x86_64 ;;
        Darwin) host=darwin-x86_64 ;;
        *) printf '%s\n' "Set up the Android toolchain using a supported Linux/macOS shell." >&2; return 1 ;;
    esac
    if [[ -z "$java" && -x /usr/lib/jvm/java-21-openjdk-amd64/bin/java ]]; then
        java=/usr/lib/jvm/java-21-openjdk-amd64
    fi
    if [[ -z "$java" || ! -x "$java/bin/java" || ! -x "$java/bin/keytool" ]]; then
        printf '%s\n' "Set JAVA_HOME to a Java runtime/JDK with java and keytool (Java 21 recommended)." >&2
        return 1
    fi
    if [[ ! -x "$ndk/toolchains/llvm/prebuilt/$host/bin/aarch64-linux-android26-clang" ]]; then
        printf 'Android NDK r28c is missing: %s\n' "$ndk" >&2
        return 1
    fi
    if [[ ! -f "$sdk/platforms/android-35/android.jar" || ! -f "$sdk/platforms/android-36/android.jar" || ! -x "$sdk/build-tools/36.0.0/aapt" ]]; then
        printf 'Install platforms;android-35, platforms;android-36 and build-tools;36.0.0 under %s\n' "$sdk" >&2
        return 1
    fi
    export ANDROID_HOME="$sdk"
    export ANDROID_NDK_HOME="$ndk" ANDROID_NDK_ROOT="$ndk" JAVA_HOME="$java"
    export PATH="$java/bin:$sdk/platform-tools:$sdk/cmdline-tools/latest/bin:$HOME/.local/bin:$PATH"
    export CC_aarch64_linux_android="$ndk/toolchains/llvm/prebuilt/$host/bin/aarch64-linux-android26-clang"
    export AR_aarch64_linux_android="$ndk/toolchains/llvm/prebuilt/$host/bin/llvm-ar"
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="${CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS:--Clink-arg=-Wl,-z,max-page-size=16384 -Clink-arg=-Wl,-z,common-page-size=16384}"
}
rms_android_env
