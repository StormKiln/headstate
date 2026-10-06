# Native plugin compilation

`make check-native-android` and `make check-native-ios` stage complete in-repo
plugin packages outside the checkout, resolve the native Tauri API through
`src-mobile/Cargo.lock`, and compile every declared native package. Keep the
separate bridge-name check: compilers cannot compare Rust dispatch strings.

Android uses the committed Gradle 8.14.3 wrapper (JAR SHA256
`e996d452d2645e70c01c11143ca2d3742734a28da2bf61f25c82bdc288c9e637`), distribution
SHA256 in wrapper properties, AGP 8.11.0 and Kotlin 1.9.25. CI downloads
Temurin 21.0.12.1+1 from the official release with a literal SHA256, and installs
Android platform 36/build-tools 35.0.0. Gradle dependency locks include buildscript
and each module's transitive resolved configurations. To deliberately refresh:
`python3 scripts/check-native.py android --update-locks`; review all changed locks.
Normal CI uses strict locks and never that option. No uploaded native build cache
is introduced. The wrapper's upstream license is in `android/LICENSE`.

The Android host fixture assembles an APK containing both Tauri's ordinary
FileProvider shape and the actual export plugin. The merged manifest must retain
both distinct components, authorities and path resources. This catches collisions
that library compilation alone misses. It is a compile fixture, not a runtime
bridge test or the shipped companion app.

Swift uses the actual Package.swift and the resolved Tauri API with a locked
SwiftRs 1.0.7 revision. The script supplies and verifies Package.resolved for each
package and uses `-onlyUsePackageVersionsFromResolvedFile`. CI explicitly selects
Xcode 26.3 on `macos-15`; local release verification used Xcode 26.6. Existing
MLDSA symbols require an iOS26 SDK even though deployment/runtime availability
checks target older systems. All builds explicitly target iOS Simulator.

The script prints package inventory, tool versions, compiler commands, elapsed
time and preserved output directory. A fresh stage prevents previous build products
from hiding missing native source. Dependency-download caches may still be warm;
report that distinction when quoting elapsed time. `--work-dir` requires an empty
staging location and is useful for inspection and isolated fault injection.

These gates do not prove native presentation, URI permissions, final Rust bridge
registration, signing or physical-device behavior. Runtime evidence remains
separate. Plugin exports use closed kinds with neutral `transcript.md` or
`headstate-measurements.jsonl` filenames, bounded 8 MiB UTF-8 input, and private immutable
files. JSONL uses `application/x-ndjson` on Android and an exported text-conforming
UTI on iOS. Kind-qualified byte-identical exports reuse one file and renew its
24-hour lease; both kinds share eight files / 32 MiB of cache capacity. New distinct
exports are refused at capacity rather than deleting a URI a recipient might
still be reading. Cleanup touches only expired app-owned hash directories on a
later export. Chooser completion does not claim that any destination saved data.
