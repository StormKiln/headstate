# GitLab slice 4: authored and review queue receipts

This slice adds bounded GitLab.com list reads behind the existing source cache
and poll status helpers. It does not register the source commands or change the
desktop/phone UI. Slice 6 supplies the persisted selector and starts background
GitLab polling only when GitLab is selected. Until then, no GitLab network read
runs at application startup.

`glab api --hostname gitlab.com -i` reads `state=opened` merge requests in
`created_by_me` or `reviews_for_me` scope. Each scope has its own source/list
receipt. Each request has a 15-second ceiling; a list has a 30-second and
five-page ceiling, with at most 100 rows requested per page. A later timeout,
failed page, malformed row, repeated identity, or page ceiling retains valid
earlier rows as `partial`. A measured total is kept only when page headers
agree and it is at least the number of received rows. A response without
conclusive pagination headers has `unknown`
coverage. `X-Total: 0` with a terminal page is a measured empty list.
Credential material remains inside `glab`; subprocess stderr and response
bodies are never used in error messages or logs.

GitLab rows use a separate `MergeRequest` summary. Identity includes provider,
host, full nested project path, and IID. The mapper checks that `web_url` is on
the requested host and matches the IID. List fields are stored only when
present and parseable. CI, review verdict, and unresolved discussions stay
`null`: list responses do not measure them. `detailed_merge_status` is retained
as GitLab's raw value, not translated into GitHub merge readiness.
`reviews_for_me` names GitLab's scope and makes no claim that approval is still
outstanding. Slice 5 can enrich these fields after detail reads; slice 6 must
render them according to their optional status.

GitHub keeps its existing `prs` receipt and snapshot. GitLab emits `mrs` in
`source-poll-status` and stores `GitLabAvailable { mrs, coverage, ... }`; it does
not emit legacy GitHub row or truncation events. The provider-specific cache
decoder rejects a GitHub payload under a GitLab key, and vice versa. A partial
GitLab result has its own status and cannot replace another source/list.

The GitLab.com probe in `gitlab-slice-0-probe.md` observed two authored draft
MRs and an empty reviewer scope. The reviewer mapping here is tested with a
synthetic assigned-reviewer fixture; it is not a claim of live review behavior.
No self-managed host is enabled or live-tested. The slice 5 detail adapter and
slice 6 source selector/background activation remain separate work.
