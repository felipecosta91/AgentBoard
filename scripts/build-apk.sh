#!/bin/bash
# Builds the Android APK -> dist/AgentBoard.apk (needs JDK 17+, Android SDK, Gradle via Android Studio's cache).
set -euo pipefail
cd "$(dirname "$0")/../android"
export JAVA_HOME="${JAVA_HOME:-$(/usr/libexec/java_home -v 17)}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
echo "sdk.dir=$ANDROID_HOME" > local.properties
GRADLE="$(command -v gradle || ls -d "$HOME"/.gradle/wrapper/dists/gradle-*/*/gradle-*/bin/gradle | sort | tail -1)"
"$GRADLE" --no-daemon assembleRelease
mkdir -p ../dist
cp app/build/outputs/apk/release/app-release.apk ../dist/AgentBoard.apk
echo "APK: $(cd .. && pwd)/dist/AgentBoard.apk"
