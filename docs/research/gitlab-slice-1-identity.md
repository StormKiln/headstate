# GitLab slice 1: provider identity and wire model

The PR wire model adds `source: {provider, host}`. `provider` is `github`
or `gitlab`; `host` is the host authority without a scheme or path.
The existing `repo` field remains the full project path, including any
GitLab subgroups. `number` means the GitHub PR number or GitLab
project-local IID. The existing `id` remains a provider-local opaque API
identifier and is not a cross-provider row key.

Rust exposes `Provider`, `Host`, `ProjectPath`, `PrNumber`, `Source`, and
`PrIdentity`; TypeScript mirrors this contract. The host/path/number types
are semantic aliases, not validated input constructors. Adapter work must
supply canonical host authorities and project paths from its chosen source.

An old payload with no `source` reads as GitHub on `github.com`. Rust
serializes that source explicitly on the next response/write. A source
object missing provider or host fails deserialization; it cannot become a
GitHub identity through a partial default. Existing snapshot tables and
migration versions are unchanged. A SQLite regression test inserts the
old JSON shape directly and exercises the real snapshot loader.

Frontend keys encode `[provider, host, repo, number]` as a JSON tuple.
The same key is used for rows, checked rows, range selection, keyboard
selection, current-row matching, summary-strip keys, and court deduplication.
Opening a row retains its source in selection. Legacy payloads produce
the same key as explicit GitHub.com rows.

Bulk execution remains GitHub.com-only. A selection containing another
source is refused before any command is sent, and remains selected. Failed
GitHub outcomes resolve back to the submitted targets before rebuilding
selection. The command and outcome wire shapes are unchanged.

This slice provides identity, not a GitLab adapter. Authentication,
polling, cache partitioning/migration, notifications, detail/query routing,
provider-specific row actions, and source UI remain later slices. Current
live fetches still only produce GitHub.com rows. Synthetic alternate-host
tests prove identity separation; they do not establish self-managed GitLab
support. GitLab.com remains the first implementation target.
