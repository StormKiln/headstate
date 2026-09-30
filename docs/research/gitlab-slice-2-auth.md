# GitLab slice 2: independent authentication and diagnostics

Headstate checks GitLab.com through `glab auth status --hostname gitlab.com`.
The binary is discovered on the desktop's PATH, common install locations,
or `HEADSTATE_GLAB`. The check has a 15-second wall-clock limit and kills
the subprocess on timeout. Standard output and error are discarded; the
response contains only a fixed host, status kind, and static message. No
GitLab credential is read by Headstate, written to SQLite or logs, or sent
to the paired phone. The phone's read command runs on the desktop and
receives only that status.

GitHub retains its existing startup token/client path. Its auth failure
does not suppress the application shell or local views. GitLab auth failure
does not change the GitHub client or poll. A missing `gh` leaves the GitLab
status check available; a missing or rejected `glab` leaves GitHub
available. The tool diagnostics show `glab` separately from `gh` and state
which provider each tool affects. `glab` 1.119.0 was exercised by the
slice 0 probe; no earliest supported version has been established, so
diagnostics report its version without an age verdict.

When GitHub auth is absent, the PR status bar says GitHub is not refreshing
and withholds the React Query cache-read timestamp. A warm SQLite snapshot
can make that timestamp look like a fresh poll even though no GitHub poll
was started. Cached rows remain visible under the auth warning. CLI repair
messages name the desktop running Headstate, so the paired phone does not
instruct its user to install a CLI on the phone.
On a connected phone, its connection banner applies the same rule: a
reachable desktop with unavailable GitHub auth is amber and does not claim
the cached PR list was just updated. An unanswered auth IPC call remains
unknown, with neutral wording instead of a sign-in verdict. The offline
phone keeps the cached list and treats desktop auth as unknown.

An unsuccessful CLI check is **unverified**, not automatically "expired":
the same exit status can represent a rejected credential, a disconnected
network, or a host error. The UI points to `glab auth status --hostname
gitlab.com` for the concrete reason. A timeout is distinct, and a missing
binary is distinct. The CLI's own text is never exposed because it could
contain sensitive information. Pending status remains a pending query,
not an unauthenticated verdict.

This slice supports **GitLab.com only**. Self-managed host enumeration,
custom API paths, version/tier checks, and multiple active GitLab hosts
need a fixture and come later. This slice does not fetch merge requests or
add the source selector. GitHub lists therefore remain GitHub data even
when GitLab.com authentication is verified. Provider-aware polling and
error channels are slice 3.
