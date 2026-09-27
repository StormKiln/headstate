// The iOS side of tauri-plugin-headstate-notify.
//
// Three commands, all `UNUserNotificationCenter`: read the authorization
// state, request it, and post one local notification. The Rust side
// (src/lib.rs) owns the ask-once policy and documents the JSON; this
// file matches it and decides nothing.
//
// # Local notifications, not push
//
// No APNs, no entitlement, no certificate, no device token. A
// `UNNotificationRequest` with a nil trigger is delivered by the system
// immediately, from inside whatever execution the app already has -- and
// what the app has is the background refresh window
// tauri-plugin-headstate-refresh is granted. That is the whole of the
// delivery path, and its cost is that iOS decides when those windows
// open: a new pull request surfaces within the hour rather than
// instantly. #789 accepts that explicitly.
//
// # Threading: why a semaphore
//
// `getNotificationSettings` and `requestAuthorization` are
// completion-handler APIs, and the bridge into Rust is synchronous --
// `run_mobile_plugin` blocks until `invoke.resolve`. So each command
// waits on a `DispatchSemaphore` for its completion handler.
//
// That is only safe because Tauri calls plugin commands on its own
// serial background queue, never the main queue: the completion handlers
// are delivered on an internal queue of UNUserNotificationCenter's
// choosing, so nothing here waits on the queue it is blocking. Blocking
// the main thread on `requestAuthorization` would deadlock the moment
// the system needed the main thread to present the permission sheet.
//
// `requestAuthorization` has no timeout, by decision: it is waiting for
// a HUMAN, and the only correct answer to "the user has not decided yet"
// is to keep waiting. A timeout would resolve as "not granted" while the
// sheet was still on screen, cache that as the answer (Rust caches
// anything that is not `prompt`), and permanently silence an app the
// user was in the middle of allowing. The window iOS grants is short and
// it may expire first -- the refresh plugin's expiration handler ends
// the task either way -- and a notification lost to that is one missing
// notification, which is the right failure.
//
// # Trying it
//
// The simulator delivers local notifications and shows the permission
// sheet, so this is testable without a device: drive a refresh window
// with `_simulateLaunchForTaskWithIdentifier:` (see
// HeadstateRefreshPlugin.swift) and the notification appears if the app
// is backgrounded. In the FOREGROUND nothing is shown, because no
// `UNUserNotificationCenterDelegate` is installed -- see below.
//
// # The delegate: no foreground banners, and a tap is PARKED (#1486)
//
// #789 installed no `UNUserNotificationCenterDelegate`, so iOS
// suppressed banners in the foreground and a tap simply opened the app.
// A session notification needs the tap to land on that session, so the
// plugin is now the delegate, installed in `initPlugin` -- which Rust
// calls while the app is being set up, before launch finishes, which is
// when iOS requires a delegate for a tap that launched the app to be
// delivered.
//
// - `willPresent` answers NO options: the foreground behaviour is what
//   it was. The app shows its own in-app toasts while it is open.
// - `didReceive` stores the notification's `session` (from `userInfo`)
//   in `tappedSession` and calls nothing. Rust's `takeTapped` reads and
//   clears it when the webview asks, which it does whenever it becomes
//   visible. On a cold launch the webview does not exist yet when the
//   response arrives; a parked value waits for it, and an event pushed
//   at nobody would be lost.
//
// A pull-request notification carries no `session`, so tapping one
// parks nothing and just opens the app, as before.

import Foundation
import Tauri
import UserNotifications

struct PostArgs: Decodable {
  let title: String
  let body: String
  /// The Claude Code session a tap should open (#1486). An id, never
  /// text; absent for a notification a tap should just open the app for.
  let session: String?
}

/// `{"session": "<id>" | null}`: what `takeTapped` answers.
struct TappedResponse: Encodable {
  let session: String?
}

/// `{"permission": "granted" | "prompt" | "denied"}`, the three states
/// Rust's `Permission` decodes. Lowercase because Rust's enum is
/// `rename_all = "lowercase"`.
struct PermissionResponse: Encodable {
  let permission: String
}

class HeadstateNotifyPlugin: Plugin, UNUserNotificationCenterDelegate {
  /// The session of the notification last tapped, until Rust takes it.
  /// Guarded by `lock`: the delegate runs on the main queue and
  /// `takeTapped` on Tauri's plugin queue.
  private var tappedSession: String? = nil
  private let lock = NSLock()

  // MARK: UNUserNotificationCenterDelegate

  /// No banner, sound or list entry while the app is in the foreground --
  /// what iOS did before this delegate existed. The app shows in-app
  /// toasts instead.
  func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    willPresent notification: UNNotification,
    withCompletionHandler completionHandler:
      @escaping (UNNotificationPresentationOptions) -> Void
  ) {
    completionHandler([])
  }

  /// Park the tapped notification's session for `takeTapped`.
  func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    if response.actionIdentifier == UNNotificationDefaultActionIdentifier,
      let session = response.notification.request.content.userInfo["session"] as? String
    {
      lock.lock()
      tappedSession = session
      lock.unlock()
    }
    completionHandler()
  }

  /// `UNAuthorizationStatus` collapsed to the three states Rust acts on.
  ///
  /// `.provisional` counts as GRANTED: it is what a quiet-delivery
  /// authorization gives, notifications really are delivered under it,
  /// and reporting it as "prompt" would make Rust ask again for
  /// permission it already has. `.ephemeral` (App Clips) is granted for
  /// the same reason. Anything unknown from a future iOS is `prompt`,
  /// which is the state that asks -- the conservative direction, since
  /// the alternative is silently never notifying.
  private static func state(_ status: UNAuthorizationStatus) -> String {
    switch status {
    case .authorized, .provisional, .ephemeral:
      return "granted"
    case .denied:
      return "denied"
    case .notDetermined:
      return "prompt"
    @unknown default:
      return "prompt"
    }
  }

  // MARK: Commands (called from Rust)

  /// The current authorization state. Never prompts.
  @objc public func permission(_ invoke: Invoke) throws {
    let semaphore = DispatchSemaphore(value: 0)
    var status: UNAuthorizationStatus = .notDetermined
    UNUserNotificationCenter.current().getNotificationSettings { settings in
      status = settings.authorizationStatus
      semaphore.signal()
    }
    semaphore.wait()
    invoke.resolve(PermissionResponse(permission: Self.state(status)))
  }

  /// Show the permission sheet and report what the user chose.
  ///
  /// `[.alert, .sound]` and deliberately not `.badge`: the app keeps no
  /// badge count, and asking for a permission that is never used is a
  /// permission the user was asked for under false pretences.
  ///
  /// Rust calls this only when `permission` answered `prompt`, which is
  /// what makes the prompt appear ONCE. iOS enforces the same thing
  /// independently -- a second request after a denial resolves
  /// immediately without a sheet -- so the policy living in Rust costs
  /// nothing even if it were wrong.
  @objc public func requestPermission(_ invoke: Invoke) throws {
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false
    var failure: Error? = nil
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) {
      ok, error in
      granted = ok
      failure = error
      semaphore.signal()
    }
    semaphore.wait()
    if let failure = failure {
      // Not a rejection: a failed request is not a denial, and reporting
      // it as one would have Rust cache "denied" for a user who was
      // never actually asked. `prompt` means "still undetermined", which
      // is the truth and leaves the next window free to ask again.
      Logger.info("headstate-notify: the permission request failed: \(failure)")
      invoke.resolve(PermissionResponse(permission: "prompt"))
      return
    }
    // A refusal is reported as `denied` rather than `prompt`, which is
    // what lets Rust stop asking: iOS will not show the sheet again in
    // this app's lifetime anyway, so treating a refusal as undetermined
    // would mean a native round trip per notification forever.
    invoke.resolve(PermissionResponse(permission: granted ? "granted" : "denied"))
  }

  /// Post one notification now.
  ///
  /// A nil trigger means "deliver immediately". A UUID identifier rather
  /// than a stable one on purpose: a repeated identifier REPLACES the
  /// pending notification with that id, so two pull requests appearing
  /// in one window would collapse into one banner. Deduplication is
  /// Rust's job and it has already happened by the time this is called
  /// -- see `newly_appeared` and `Fired` on that side.
  @objc public func post(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(PostArgs.self)
    let content = UNMutableNotificationContent()
    content.title = args.title
    content.body = args.body
    content.sound = .default
    if let session = args.session {
      content.userInfo = ["session": session]
    }
    let request = UNNotificationRequest(
      identifier: UUID().uuidString, content: content, trigger: nil)

    let semaphore = DispatchSemaphore(value: 0)
    var failure: Error? = nil
    UNUserNotificationCenter.current().add(request) { error in
      failure = error
      semaphore.signal()
    }
    semaphore.wait()
    if let failure = failure {
      invoke.reject("could not post the notification: \(failure)")
      return
    }
    invoke.resolve()
  }

  /// Read and clear the session of the notification last tapped.
  @objc public func takeTapped(_ invoke: Invoke) throws {
    lock.lock()
    let session = tappedSession
    tappedSession = nil
    lock.unlock()
    invoke.resolve(TappedResponse(session: session))
  }
}

@_cdecl("init_plugin_headstate_notify")
func initPlugin() -> Plugin {
  let plugin = HeadstateNotifyPlugin()
  // Here rather than in `load(webview:)`: this runs while the app is
  // being set up, and iOS delivers the tap that LAUNCHED the app only to
  // a delegate installed before launch finishes. `delegate` is weak;
  // Tauri holds the plugin for the life of the app.
  UNUserNotificationCenter.current().delegate = plugin
  return plugin
}
