import XCTest
@testable import tauri_plugin_headstate_export
final class MarkdownExportStoreTests: XCTestCase {
  func testExactBytesReuseCapacityAndExpiry() throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: root) }
    let store = MarkdownExportStore(root: root)
    let text = "# café 😀\nEarlier messages were not loaded.\n[masked]\n"
    let file = try store.prepare(text, now: Date(timeIntervalSince1970: 1000))
    XCTAssertEqual(file.lastPathComponent, "transcript.md")
    XCTAssertEqual(try Data(contentsOf: file), Data(text.utf8))
    for _ in 0..<20 { XCTAssertEqual(try store.prepare(text, now: Date(timeIntervalSince1970: 2000)), file) }
    XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: root.path).count, 1)
    for i in 0..<7 { _ = try store.prepare("other \(i)", now: Date(timeIntervalSince1970: 2000)) }
    XCTAssertThrowsError(try store.prepare("ninth", now: Date(timeIntervalSince1970: 3000)))
    XCTAssertTrue(FileManager.default.fileExists(atPath: file.path))
    _ = try store.prepare("after expiry", now: Date(timeIntervalSince1970: 90000))
    XCTAssertFalse(FileManager.default.fileExists(atPath: file.path))
    XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: root.path).count, 1)
  }
  func testUtf8LimitDoesNotCreateFile() throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    defer { try? FileManager.default.removeItem(at: root) }
    XCTAssertThrowsError(try MarkdownExportStore(root: root).prepare(String(repeating: "é", count: 4*1024*1024+1)))
    XCTAssertFalse(FileManager.default.fileExists(atPath: root.path))
  }
}
