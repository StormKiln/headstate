/// What "Report this" can learn about the desktop (#1575).
///
/// Every nullable field is `null` when it could not be gathered, and the
/// report prints that as "unknown" -- never as zero, never as a blank.

/// Mirrors `report::PollTickRecord` in `src-tauri/src/report.rs`.
export interface PollTickRecord {
  atUnixMs: number;
  ok: boolean;
  fetchMs: number;
  error: string | null;
  timedOutAfterSecs: number | null;
  attempt: number;
  reviewing: string;
}

/// Mirrors `report::PollReport` in `src-tauri/src/report.rs`.
export interface PollReport {
  operation: string;
  fetchTimeoutSecs: number;
  tickTimeoutSecs: number;
  focusedIntervalSecs: number | null;
  lastWaitSecs: number | null;
  ticksRecorded: number;
  recent: PollTickRecord[];
  failuresInRecent: number;
  lastSuccessSecsAgo: number | null;
}

/// Mirrors `report::InstallGuess` in `src-tauri/src/report.rs`.
export interface InstallGuess {
  /// One of the bug form's dropdown options exactly, or `null`.
  method: string | null;
  basis: string;
}

/// Mirrors `report::DiagnosticBundle` in `src-tauri/src/report.rs`.
export interface DiagnosticBundle {
  appVersion: string;
  os: string;
  arch: string;
  osVersion: string | null;
  install: InstallGuess;
  ghVersion: string | null;
  ghNote: string | null;
  poll: PollReport;
  graphqlRemaining: number | null;
  restRemaining: number | null;
  diagnosticsOn: boolean;
  logTail: string | null;
  logNote: string | null;
}
