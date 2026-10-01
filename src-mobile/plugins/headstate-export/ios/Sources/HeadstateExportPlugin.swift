import Foundation
import CryptoKit
import UIKit
import Tauri

public enum ExportFailure: Error, Equatable { case capacity, unavailable, tooLarge }

/// Immutable app-owned files: reuse identical bytes; retain for 24 hours so
/// asynchronous share targets may finish reading after the sheet closes.
public final class MarkdownExportStore {
  public let root: URL
  public init(root: URL) { self.root = root }
  public func prepare(_ markdown: String, now: Date = Date()) throws -> URL {
    let data = Data(markdown.utf8)
    guard data.count <= 8 * 1024 * 1024 else { throw ExportFailure.tooLarge }
    let fm = FileManager.default
    try fm.createDirectory(at: root, withIntermediateDirectories: true)
    let entries = try fm.contentsOfDirectory(at: root, includingPropertiesForKeys: [.contentModificationDateKey])
      .filter { $0.lastPathComponent.range(of: "^[a-f0-9]{64}$", options: .regularExpression) != nil }
    for entry in entries {
      let date = try entry.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate ?? now
      if now.timeIntervalSince(date) > 24 * 60 * 60 { try fm.removeItem(at: entry) }
    }
    let hash = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    let directory = root.appendingPathComponent(hash, isDirectory: true)
    let file = directory.appendingPathComponent("transcript.md")
    if fm.fileExists(atPath: file.path) {
      try fm.setAttributes([.modificationDate: now], ofItemAtPath: directory.path)
      return file
    }
    let live = try fm.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)
      .filter { $0.lastPathComponent.range(of: "^[a-f0-9]{64}$", options: .regularExpression) != nil }
    let bytes = live.reduce(0) { total, entry in
      total + ((try? entry.appendingPathComponent("transcript.md").resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
    }
    guard live.count < 8 && bytes + data.count <= 32 * 1024 * 1024 else { throw ExportFailure.capacity }
    try fm.createDirectory(at: directory, withIntermediateDirectories: false)
    do {
      try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
      try fm.setAttributes([.modificationDate: now], ofItemAtPath: directory.path)
      return file
    } catch { try? fm.removeItem(at: directory); throw error }
  }
}

/// UIKit is entered only on main. One owner resolves presentation, activity
/// completion and interactive dismissal through the same once-only terminal.
public final class MarkdownSharePresenter: NSObject, UIAdaptivePresentationControllerDelegate {
  private let store: MarkdownExportStore
  private var completion: ((String) -> Void)?
  private var controller: UIActivityViewController?
  public init(store: MarkdownExportStore) { self.store = store }
  public func present(_ markdown: String, from host: UIViewController?, completion: @escaping (String) -> Void) {
    dispatchPrecondition(condition: .onQueue(.main))
    guard self.completion == nil else { completion("busy"); return }
    guard var host = host, host.viewIfLoaded?.window != nil else { completion("failed"); return }
    while let presented = host.presentedViewController { host = presented }
    guard !host.isBeingDismissed && !host.isBeingPresented else { completion("failed"); return }
    self.completion = completion
    DispatchQueue.global(qos: .userInitiated).async {
      let result = Result { try self.store.prepare(markdown) }
      DispatchQueue.main.async {
        guard self.completion != nil else { return }
        switch result {
        case .failure(let error): self.finish(error as? ExportFailure == .capacity ? "capacity" : "failed")
        case .success(let file):
          guard host.viewIfLoaded?.window != nil, !host.isBeingDismissed, host.presentedViewController == nil else { self.finish("failed"); return }
          let sheet = UIActivityViewController(activityItems: [file], applicationActivities: nil)
          self.controller = sheet
          sheet.completionWithItemsHandler = { [weak self] _, completed, _, error in
            self?.finish(error != nil ? "failed" : completed ? "shared" : "cancelled")
          }
          if let popover = sheet.popoverPresentationController {
            popover.sourceView = host.view
            popover.sourceRect = CGRect(x: host.view.bounds.midX, y: host.view.bounds.midY, width: 1, height: 1)
            popover.permittedArrowDirections = []
          }
          sheet.presentationController?.delegate = self
          host.present(sheet, animated: true) { sheet.presentationController?.delegate = self }
        }
      }
    }
  }
  public func presentationControllerDidDismiss(_ presentationController: UIPresentationController) { finish("cancelled") }
  private func finish(_ outcome: String) {
    dispatchPrecondition(condition: .onQueue(.main))
    guard let callback = completion else { return }
    completion = nil
    controller = nil
    callback(outcome)
  }
}

private struct ShareArgs: Decodable { let markdown: String }
private struct ShareReply: Encodable { let outcome: String }
class HeadstateExportPlugin: Plugin {
  private lazy var presenter = MarkdownSharePresenter(store: MarkdownExportStore(root:
    FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("headstate-export", isDirectory: true)))
  @objc public func share(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ShareArgs.self)
    DispatchQueue.main.async {
      self.presenter.present(args.markdown, from: self.manager.viewController) { outcome in
        invoke.resolve(ShareReply(outcome: outcome))
      }
    }
  }
}
@_cdecl("init_plugin_headstate_export")
func initPlugin() -> Plugin { HeadstateExportPlugin() }
