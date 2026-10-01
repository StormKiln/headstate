# Contrast checks

Run `make check-contrast` after `yarn playwright install chromium`. CI runs
this target in the frontend job. It discovers production source, compiles the
real Tailwind utilities and dark theme, checks their computed colors in
Chromium, then builds and measures actual mounted components using axe.

The source inventory includes new TS/TSX files, shared UI primitives and the
production CSS entry. Only tests and synthetic harness/fixture files are
excluded. New CSS files require adding their compiled entry to the guard.
There is no production file exemption list or parallel test palette.

Literal text/background pairs, explicit hover/selected states and literal
ancestor backgrounds are checked at 4.5:1. Meaningful imported Lucide icons
use 3:1. Unknown inherited surfaces use the raised `#21262d` contract; this
is a conservative default for the current dark surfaces, not a proof of
arbitrary React inheritance. Tailwind resolves named, semantic, hex and
alpha-background utilities. Opaque text is required: text alpha and active
ancestor opacity fail discovery. Genuine static disabled controls,
`disabled:` states, `aria-hidden` decoration and explicitly hidden transition
states are excluded; `disabled={busy}` does not exempt the enabled state.

Dynamic colors require named contracts: transcript palette and label tests,
or the actual finite literals returned by health color functions. New
unclassified dynamic inline colors fail. Conditional branches and component
boundaries cannot be reconstructed completely from source; add a mounted
representative when introducing a new surface or state family.

The rendered matrix mounts the actual Button variants, Tabs, selected stats
row/count, ToolVersions path, ReportDialog action, BulkBar skipped reasons,
TranscriptHeader phone variant and the session Resume action through the
app harness. It checks resting, hovered and keyboard interaction states.
The narrow 390px harness also enables touch and checks the full masked Asked
text at default and doubled root font size without horizontal overflow or
an unmasked title. Withheld prompts stay absent. The fixtures are synthetic;
the components and compiled styles are production code.

These are contrast regressions and a finite interaction/layout matrix, not
complete WCAG certification. They do not establish every route/state,
light-theme support, native WebView rendering, physical-device or assistive
technology behavior, focus-ring contrast, or every chart/third-party widget.
Axe incomplete results are printed for review, never described as passes.
Actual root-font doubling is distinct from browser/OS zoom certification.
