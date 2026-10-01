import type { BranchDeleteFrame, BranchScanFrame, PullRequest, StatsBackfillFrame, Worktree, WorktreeRemovalFrame } from "../types/pr";
import type { MergeRequest } from "../types/gitlab";
import type { PrIdentity } from "../types/identity";
import type { TranscriptActivity, SessionActivity } from "../types/transcript";
import type { SourceStatus } from "./sourceRefresh";
import type { SourcePollUpdate, UpdateRunDone } from "./tauri";

/// The event contract is explicit because subscribers intentionally consume
/// different projections. Optional fields preserve independently shipped older
/// desktops; present fields still have to be valid. Unknown additions survive.
export interface RemoteEvents {
  "prs-updated": PullRequest[];
  "reviewing-updated": PullRequest[];
  "poll-state": string;
  "poll-error": string;
  "store-error": string;
  "source-poll-status":
    // Only GitHub had the legacy uncorrelated shape. A broad SourceStatus
    // arm would also accept incomplete GitLab frames and could retire the
    // queue's live session or publish undefined rows (#711 review).
    | (SourceStatus & {
        source: { provider: "github"; host: string };
        mrs?: MergeRequest[] | null;
        last_received_at?: string | null;
      })
    | (SourcePollUpdate & { source: { provider: "gitlab"; host: string } });
  "gitlab-data-changed": PrIdentity;
  "prs-truncated": number | null;
  "reviewing-short": number | null;
  "prs-incomplete": number;
  "worktree-removal-progress": WorktreeRemovalFrame;
  "update-run-progress": [number, number];
  "update-run-done": UpdateRunDone;
  "branch-scan-progress": BranchScanFrame;
  "branch-delete-progress": BranchDeleteFrame;
  "worktree-size": [string, number | null];
  "worktree-safety": Worktree;
  "stats-backfill-progress": StatsBackfillFrame;
  "claude-session-activity": SessionActivity;
  "claude-transcript-activity": TranscriptActivity;
}

/// Existing remote command without a frontend wrapper. Mirrors ClaudeLiveState
/// in commands.rs; keeping an explicit entry makes an uncovered surface fail
/// generation instead of silently skipping it.
export interface RemoteOnlyReplies {
  claude_poll_live: {
    handoff: {
      runs: number; sessions: number; unparseable: string[]; unknown_version: number;
      without_session_id: number; without_pid: number; unvalidated: number;
      write_failures: string[]; offset: number; rotated: boolean;
    };
    sweep: {
      running: number; crashed: number; crashed_already_known: number;
      crashed_sessions: { session_id: string; name: string | null; cwd: string | null }[];
      unknown: number; without_session_id: number; write_failures: string[]; unreadable: string[];
    };
    running: LiveRecord[];
    unconfirmed: LiveRecord[];
    indexed: {
      indexed: number; unchanged: number; unreadable: string[]; scan_unreadable: string[];
      write_failures: string[]; truncated: number; remaining: number; elapsed_ms: number;
    } | null;
  };
}
interface LiveRecord {
  pid: number; session_id: string | null; proc_start_raw: string | null;
  pid_start_epoch: number | null; cwd: string | null; name: string | null;
  claude_version: string | null; status: string | null; path: string;
}
