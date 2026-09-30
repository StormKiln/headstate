# GitLab slice 5: MR detail read contract

This slice adds an internal GitLab.com detail adapter. It is not registered as
a Tauri command or shown in the shared desktop/phone UI; slice 6 wires the
source selector and detail route. The adapter accepts the full
`(provider, host, project path, IID)` identity and refuses non-GitLab.com
hosts until a self-managed fixture exists.

The mandatory MR read validates the returned IID and exact GitLab.com URL
against the requested nested project path. It retains `detailed_merge_status`
as GitLab's raw value. It does not compute merge readiness from the coarse
`merge_status`, draft flag, approvals, CI, or discussion count. A null or stale
`head_pipeline` does not become green CI.

Pipelines, current-head jobs, approvals, approval rules, notes, and
discussions have separate available/unavailable receipts. All optional
requests run through `glab api --hostname gitlab.com -i` with credentials
owned by `glab`; response bodies and stderr never enter error strings or
diagnostics. Each request has a 15-second timeout and a 4 MiB output limit;
the detail operation has a 60-second budget. Paged sections request at most
five pages of 100 entries. A later page failure retains received entries as
partial; an absent terminal pagination header yields unknown coverage.
The current-head job request is issued only when the MR pipeline list is
complete, a pipeline SHA matches the MR head SHA, and that pipeline supplies
its project ID. Jobs are requested under that project ID so a fork pipeline
does not get looked up under the target project. The job list describes that
pipeline, not every possible commit check or child pipeline.

The approvals receipt keeps `approved_by`, requirement counts, and the
current user's approval capability separately. An empty `approved_by` with
`approvals_required: 0` is not a reviewer approval. Approval rules may be
unavailable on a tier or to a permission set while the rest of the detail
remains usable. A resolvable discussion count is present only after complete
pagination and known per-note resolution flags; system notes and resolved
diff notes do not count as open threads. Notes and discussions are retained
separately so the later UI can avoid showing the same comment twice.

Fixture provenance: the earlier [GitLab.com probe](gitlab-slice-0-probe.md)
observed draft MR !1 with `draft_status` despite coarse
`merge_status: can_be_merged`, zero required approvals with no approving
reviewer, one successful MR pipeline/job, and one resolved diff discussion
alongside system notes. This slice uses synthetic, redacted versions of those
shapes plus failure and pagination cases. No new live GitLab read was made
because the disposable project path is intentionally absent from the
repository. Self-managed hosts, required approval rules, live denied
permissions, unresolved live discussions, multiple current-head pipelines,
child pipelines, and merge-train capability remain unverified.

Endpoint contracts were checked against GitLab's [merge request API](https://docs.gitlab.com/api/merge_requests/), [approval API](https://docs.gitlab.com/api/merge_request_approvals/), [discussion API](https://docs.gitlab.com/api/discussions/), and [jobs API](https://docs.gitlab.com/api/jobs/).
