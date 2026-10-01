// swift-tools-version:5.3
import PackageDescription

let package = Package(
  name: "tauri-plugin-headstate-export",
  platforms: [
    // The app's deployment target (gen/apple/project.yml).
    // BGTaskScheduler needs iOS 13.
    .iOS(.v14)
  ],
  products: [
    .library(
      name: "tauri-plugin-headstate-export",
      type: .static,
      targets: ["tauri-plugin-headstate-export"])
  ],
  dependencies: [
    // Copied here by build.rs from the tauri crate; see ios/.gitignore.
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-headstate-export",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources"),
    .testTarget(name: "ExportTests", dependencies: [.byName(name: "tauri-plugin-headstate-export")], path: "Tests")
  ]
)
