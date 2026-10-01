# Native transcript measurement probe

This is a development-only, bounded macOS AppKit/WKWebView runner. It loads
`dist-harness/harness/transcript.html` with a preserved production end window,
using the actual transcript renderer and its synthetic follow backend. It
uses a nonpersistent website data store and its own loopback server, window,
process and output directory. Nothing here is part of a shipping app target.

After `make bench-transcript-browser BENCH_TRANSCRIPT_OUT=/absolute/fixtures`:

```sh
make bench-transcript-native \
  BENCH_TRANSCRIPT_OUT=/absolute/fixtures \
  BENCH_NATIVE_OUT=/absolute/new-native-run
```

The output directory must be fresh. Fixtures, build log, runner log and JSON
results remain on disk. The native host stops within 20 seconds, with an
outer 25-second timeout killing only its own launched host. Do not run this
alongside a build or another timed benchmark. Keep its own window in front
on an unlocked Mac; hidden/occluded/inactive state returns nonzero and an
`unavailable` result. Visibility is checked every poll and loss is latched
by native/window and document visibility events after baseline.

The fixed phases are empty-renderer baseline, transcript open, one synthetic
append and a two-second quiet interval. Each reports separate host and
WebContent RSS and physical footprint using `proc_pid_rusage`, plus row/read
counts. The verified renderer PID comes only from this WKWebView's guarded
`_webProcessIdentifier` development SPI; an unavailable selector, changed PID
or missing memory sample refuses qualification. There is no process-name
search, global process trace or attachment to other apps. Private SPI makes
this unsuitable for app distribution; future WebKit may refuse it.

Do **not** sum the processes or call their memory transcript allocations:
shared pages, code and caches are included. No exact JS heap/CPU, native
allocation breakdown or compositor presentation is measured. DOM readiness
plus double-rAF is explicitly a paint-opportunity proxy; a successful probe
is named `measured-proxies`, never native budget acceptance. Older navigation
and native eviction are also unmeasured; Chromium has separate growth and
retention checks. macOS results cannot satisfy physical iPhone budgets.

Exit 0 means only the four visible proxy/memory phases completed with stable
own-PID attribution. Exit 3 means unavailable/incomplete, not a passing zero.
`python3 scripts/transcript-native-bench.test.py` tests that qualification
rejects hidden, incomplete, misattributed and missing-memory evidence.
