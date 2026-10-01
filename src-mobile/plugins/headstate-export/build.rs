fn main() {
    tauri_plugin::Builder::new(&["share"])
        .android_path("android")
        .ios_path("ios")
        .build();
}
