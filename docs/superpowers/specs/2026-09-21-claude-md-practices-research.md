# Research: what a CLAUDE.md should and should not hold — annotated bibliography

**Date:** 2026-09-21
**Status:** Research input to `2026-09-21-claude-md-advice-design.md`; feeds producer 7 (content shape)

Companion to the 7.1 "content shape" sub-issue. Researched 2026-09-21 from a sandbox whose
egress proxy blocks most of the open web. **Reachability governs how much of each
source is quoted**: `code.claude.com`, `platform.claude.com`, `github.com` (pages, not
the API), `raw.githubusercontent.com` were readable in full; `anthropic.com`,
`claude.com`, `humanlayer.dev`, `arxiv.org` (and every mirror tried: export.arxiv.org,
papers.cool, huggingface.co, bytez.com, pith.science, awesomepapers.io, r.jina.ai),
`medium.com`, `dev.to`, `cursor.com`, `agents.md`, `simonwillison.net`,
`news.ycombinator.com`, `claudelint.com`, `zenodo.org`, `theregister.com`,
`infoworld.com`, `researchgate.net` and every personal blog were blocked. For blocked
sources the quotes below come from **search-engine snippets** and are marked `[snippet]`;
they must be re-verified against the original before a threshold is shipped.

Each entry says **measured** (someone counted something) or **asserted** (advice without
a measurement), because the issue promises not to print a confident number that might
be wrong.

---

## 1. Anthropic, first-party

### 1.1 "How Claude remembers your project" — https://code.claude.com/docs/en/memory
Fetched in full (590 lines). **Asserted throughout**
(documentation of behaviour, plus advice); the behaviour statements are authoritative
for what Claude Code *does*, the advice statements are not measured.

**Load order and scope** (verbatim):
> "The table below lists them in load order, from broadest scope to most specific, so a
> project instruction appears in context after a user instruction."

Managed policy (`/etc/claude-code/CLAUDE.md`, `/Library/Application Support/ClaudeCode/CLAUDE.md`,
`C:\Program Files\ClaudeCode\CLAUDE.md`, or `claudeMd` key in managed settings) →
User (`~/.claude/CLAUDE.md`) → Project (`./CLAUDE.md` or `./.claude/CLAUDE.md`) →
Local (`./CLAUDE.local.md`, "add to `.gitignore`").

> "All discovered files are concatenated into context rather than overriding each other.
> Across the directory tree, content is ordered from the filesystem root down to your
> working directory. For the `foo/bar/` example, `foo/CLAUDE.md` appears in context
> before `foo/bar/CLAUDE.md`, so instructions closer to where you launched Claude are
> read last. Within each directory, `CLAUDE.local.md` is appended after `CLAUDE.md`."

**Subdirectory lazy loading** (verbatim):
> "Claude also discovers `CLAUDE.md` and `CLAUDE.local.md` files in subdirectories under
> your current working directory. Instead of loading them at launch, they are included
> when Claude reads files in those subdirectories."

And from https://code.claude.com/docs/en/debug-your-config (fetched in full):
> "They load when Claude reads a file in that directory with the Read tool, not at
> launch and not when writing or creating files there."

> "The built-in Explore and Plan agents skip `CLAUDE.md`."

**Delivery mechanism** (verbatim, Troubleshoot section):
> "CLAUDE.md content is delivered as a user message after the system prompt, not as part
> of the system prompt itself. Claude reads it and tries to follow it, but there's no
> guarantee of strict compliance, especially for vague or conflicting instructions."

**Size** (verbatim):
> "**Size**: target under 200 lines per CLAUDE.md file. Longer files consume more
> context and reduce adherence. If your instructions are growing large, use path-scoped
> rules so instructions load only when Claude works with matching files. You can also
> split content into imports for organization, though imported files still load and
> enter the context window at launch."

> "Claude Code loads a CLAUDE.md file of up to 4 MiB in full and skips a larger file.
> Shorter files produce better adherence."

> "Files over 200 lines consume more context and may reduce adherence."

Note the hedge shift: "reduce adherence" in one sentence, "may reduce adherence" in
another. No measurement is cited. See §2.2 for a study that could not detect the effect.

**Structure / specificity / consistency** (verbatim):
> "**Structure**: use markdown headers and bullets to group related instructions."
> "**Specificity**: write instructions that are concrete enough to verify."
> "**Consistency**: if two rules contradict each other, Claude may pick one
> arbitrarily. Review your CLAUDE.md files, nested CLAUDE.md files in subdirectories,
> and `.claude/rules/` periodically to remove outdated or conflicting instructions."

**Imports** (verbatim):
> "Imported files can recursively import other files, with a maximum depth of four hops."
> "Import parsing skips Markdown code spans and fenced code blocks."
> "An import in a project-level memory file is external when its path resolves outside
> your working directory ... it shows an approval dialog listing the files."

**HTML comments**:
> "Block-level HTML comments (`<!-- maintainer notes -->`) in CLAUDE.md files are
> stripped before the content is injected into Claude's context ... Comments inside
> code blocks are preserved."

**`.claude/rules/`**: all `*.md` discovered recursively; without `paths:` frontmatter
they "are loaded at launch with the same priority as `.claude/CLAUDE.md`"; with `paths:`
they "trigger when Claude reads files matching the pattern". `~/.claude/rules/` loads
before project rules. Brace expansion budget: 1,000 patterns and 4 MiB per rule.

**What belongs where** (verbatim):
> "Keep it to facts Claude should hold in every session: build commands, conventions,
> project layout, 'always do X' rules. If an entry is a multi-step procedure or only
> matters for one part of the codebase, move it to a skill or a path-scoped rule."

**`/doctor` trim check** (verbatim; changelog puts it at v2.1.206):
> "it cuts content Claude can derive from the codebase, such as directory layouts,
> dependency lists, and architecture overviews, and keeps pitfalls, rationale, and
> conventions that differ from tool defaults."

**`/init`**: "If a CLAUDE.md already exists, `/init` suggests improvements rather than
overwriting it." Reads `.cursor/rules/`, `.cursorrules`, `.github/copilot-instructions.md`;
with `CLAUDE_CODE_NEW_INIT=1` also `AGENTS.md`, `.devin/rules/`, `.windsurf/rules/`,
`.clinerules`.

**`#` shortcut**: no longer documented. Changelog **2.0.70**: "Removed # shortcut for
quick memory entry (tell Claude to edit your CLAUDE.md instead)".

**AGENTS.md** (v2.1.277+): read only when no `CLAUDE.md`/`.claude/CLAUDE.md`/
`CLAUDE.local.md` exists in cwd or above. Table verbatim: "An `AGENTS.md` and a
`CLAUDE.md` ... → Your `CLAUDE.md` files only". And:
> "A `CLAUDE.md` that tells Claude in words to read `AGENTS.md`: Claude sees
> `AGENTS.md` only if it decides to open the file. Delete the `CLAUDE.md` so Claude
> reads `AGENTS.md` directly, or replace the sentence with an `@AGENTS.md` import."

**Compaction**: "Project-root CLAUDE.md survives compaction ... Nested CLAUDE.md files
in subdirectories and rules with `paths:` frontmatter reload as Claude reads files they
apply to."

**Enforcement** (verbatim): "Claude treats them as context, not enforced configuration.
To block an action regardless of what Claude decides, use a PreToolUse hook instead."

### 1.2 "Best practices for Claude Code" — https://code.claude.com/docs/en/best-practices
Fetched in full. This is the current home of the April-2025 engineering post's CLAUDE.md
advice (anthropic.com itself was blocked). **Asserted.**

Include/exclude table (verbatim column headings and rows):
- Include: "Bash commands Claude can't guess", "Code style rules that differ from
  defaults", "Testing instructions and preferred test runners", "Repository etiquette
  (branch naming, PR conventions)", "Architectural decisions specific to your project",
  "Developer environment quirks (required env vars)", "Common gotchas or non-obvious
  behaviors".
- Exclude: "Anything Claude can figure out by reading code", "Standard language
  conventions Claude already knows", "Detailed API documentation (link to docs
  instead)", "Information that changes frequently", "Long explanations or tutorials",
  "File-by-file descriptions of the codebase", "Self-evident practices like 'write
  clean code'".

> "Keep it concise. For each line, ask: *'Would removing this cause Claude to make
> mistakes?'* If not, cut it. Bloated CLAUDE.md files cause Claude to ignore your actual
> instructions!"

> "If Claude keeps skipping one instruction, add emphasis such as 'IMPORTANT' to that
> line alone. If you emphasize many lines, none of them stands out."

> "**The over-specified CLAUDE.md.** If your CLAUDE.md is too long, Claude ignores half
> of it because important rules get lost in the noise. **Fix**: Ruthlessly prune. If
> Claude already does something correctly without the instruction, delete it or convert
> it to a hook."

"Ignores half of it" is rhetorical, not a measurement.

> "Treat CLAUDE.md like code: review it when things go wrong, prune it regularly, and
> test changes by observing whether Claude's behavior actually shifts."

> "Unlike CLAUDE.md instructions which are advisory, hooks are deterministic and
> guarantee the action happens."

### 1.3 "Extend Claude Code" — https://code.claude.com/docs/en/features-overview
Fetched in full. **Asserted.**
> "**Rule of thumb:** Keep CLAUDE.md under 200 lines. If it's growing, move reference
> content to skills or split into `.claude/rules/` files."
> "**Put guardrails in hooks.** An instruction like 'never edit `.env`' in CLAUDE.md or
> a skill is a request, not a guarantee."
Context-cost table: CLAUDE.md "Session start / Full content / Every request"; Skills
"Descriptions at start, full content when used"; Hooks "Zero, unless hook returns
additional context".
Trigger table: "Claude gets a convention or command wrong twice → Add it to CLAUDE.md";
"You paste the same playbook or multi-step procedure into chat for the third time →
Capture it as a skill".

### 1.4 "Explore the context window" — https://code.claude.com/docs/en/context-window
Fetched in full. An interactive simulation; the page
itself says **"Token counts are illustrative. Actual values vary with your CLAUDE.md
size, MCP servers, and file lengths."** So: **illustrative, not measured.**

Startup events and their illustrative token counts, `MAX = 200000`:
System prompt 4,200 · Auto memory (MEMORY.md) 680 · Environment info 280 · MCP tool
names (deferred) 120 · Skill descriptions 450 · `~/.claude/CLAUDE.md` 320 · Project
CLAUDE.md 1,800. Sum ≈ 7,850 tokens ≈ **3.9 % of a 200k window**, of which CLAUDE.md
files are 2,120 ≈ **1.1 %**. Later: path-scoped rule 380 and 290 tokens each when a
matching file is read. After `/compact`: "System prompt, CLAUDE.md, memory, and MCP tools
reload automatically"; "The skill listing does not reload."

Order in the simulation: system prompt → auto memory → environment → MCP names → skill
descriptions → user CLAUDE.md → project CLAUDE.md → user prompt. (The memory page says
the git block is "at the very end of the system prompt".)

### 1.5 "Set up Claude Code in a monorepo or large codebase" — https://code.claude.com/docs/en/large-codebases
Fetched in full. **Asserted.**
> "a single CLAUDE.md at the repository root tends to either grow to cover every
> subsystem's conventions, costing context on instructions unrelated to the current
> task, or stay too generic to be useful."
Two-level split: root = "instructions that apply everywhere"; per-subdirectory =
"conventions specific to that area's stack". Root example line: "Run package scripts
from the package directory, not the monorepo root."
Rot practices: "Review in pull requests"; "**Revisit after major model releases**:
instructions that worked around an older model's limitation may become overhead once a
newer model handles the case on its own"; "Add a Stop hook that proposes updates".
Skills: "when there are many, some skills lose their descriptions entirely"; "Keep
descriptions short and lead with words a request would contain".

### 1.6 Skills: authoring best practices — https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
Fetched in full. **Asserted** (with two ~token examples: "approximately 50 tokens" vs
"approximately 150 tokens" for concise vs verbose).
Hard rules: `name` ≤ 64 chars, lowercase/digits/hyphens, no "anthropic"/"claude";
`description` non-empty, ≤ 1,024 chars, no XML tags, **third person**, "should include
both what the Skill does and when to use it".
> "Keep SKILL.md body under 500 lines for optimal performance"
> "Keep references one level deep from SKILL.md" (Claude "might use commands like
> `head -100` to preview content rather than reading entire files")
> "For reference files longer than 100 lines, include a table of contents at the top."
> "Avoid time-sensitive information" (example: "If you're doing this before August
> 2025, use the old API" → move to an "Old patterns" section)
> "Use consistent terminology"; "Avoid offering too many options"; "Avoid Windows-style
> paths"; "always use fully qualified tool names" (`ServerName:tool_name`).
> "At startup, only the metadata (name and description) from all Skills is pre-loaded."

### 1.7 Skills in Claude Code — https://code.claude.com/docs/en/skills
Fetched. Frontmatter reference (fields: `name`, `description`, `when_to_use`,
`argument-hint`, `arguments`, `disable-model-invocation`, `user-invocable`,
`allowed-tools`, `disallowed-tools`, `model`, `effort`, `context`, `agent`,
`background`, `hooks`, `paths`, `shell`, `metadata`, `license`, `compatibility` ≤ 500
chars). "the combined `description` and `when_to_use` text is truncated at 1,536
characters in the skill listing". "Keep `SKILL.md` under 500 lines." After compaction
"keeping the first 5,000 tokens of each. Re-attached skills share a combined budget of
25,000 tokens." Changelog **2.1.32**: "Skill character budget now scales with context
window (2% of context)".

### 1.8 Claude Code changelog — https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md
Downloaded (7,158 lines) and grepped. **Behaviour facts.**
Version → entry:
- **2.0.43** "Fixed nested `CLAUDE.md` files not loading when @-mentioning files"
- **2.0.64** "Added support for `.claude/rules/`"
- **2.0.70** "Removed # shortcut for quick memory entry"
- **2.1.2** "Fixed binary files ... being accidentally included in memory when using `@include` directives"
- **2.1.6** "Improved the external CLAUDE.md imports approval dialog"
- **2.1.20** `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` for `--add-dir`
- **2.1.32** skill character budget = 2 % of context
- **2.1.69** "Added `InstructionsLoaded` hook event that fires when CLAUDE.md or `.claude/rules/*.md` files are loaded"; "Fixed project skills without a `description:` frontmatter field not appearing"
- **2.1.72** "Changed CLAUDE.md HTML comments (`<!-- ... -->`) to be hidden from Claude when auto-injected"
- **2.1.84** "Rules and skills `paths:` frontmatter now accepts a YAML list of globs"
- **2.1.89** "Fixed nested CLAUDE.md files being re-injected dozens of times in long sessions that read many files"
- **2.1.169** "The 'CLAUDE.md is too long' warning threshold now scales with the model's context window"; `--safe-mode` disables CLAUDE.md
- **2.1.205/206** `/doctor` becomes a full checkup; "Added a `/doctor` check that proposes trimming checked-in `CLAUDE.md` files by cutting content Claude could derive from the codebase"
- **2.1.211** nested rules honour `--setting-sources`
- **2.1.217** brace-expansion budget for `paths:`
- **2.1.271** `omitClaudeMd` agent frontmatter
- **2.1.277** AGENTS.md read when no CLAUDE.md
- Also (version 2.1.1xx, line 1817): the feedback-survey share uploads "the system prompt (which includes your `CLAUDE.md` instructions) ... Secrets are redacted as before" — evidence that CLAUDE.md content leaves the machine on some paths, which is why a secrets check is worth having.

The "CLAUDE.md is too long" warning value is **not** in the changelog or docs. A search
snippet for issue anthropics/claude-code#2766 (June 2025, reporter's file 44.7k chars)
and a later snippet say the warning fires "over 40.0k characters" `[snippet, unverified]`.
Since 2.1.169 it scales with the model's window, so **Headstate cannot reproduce this
warning** — it does not know the model a session will use.

### 1.9 `/doctor`, `/context`, `/memory` — https://code.claude.com/docs/en/commands
Fetched. `/doctor`: "Finds unused skills, MCP servers, and plugins versus their context
cost ... Deduplicates local `CLAUDE.md` files against checked-in ones, trims checked-in
`CLAUDE.md` files by cutting content Claude could derive from the codebase, and migrates
the always-loaded guidance that remains into skills and nested `CLAUDE.md` files that
load on demand." `/context`: "Shows optimization suggestions for context-heavy tools,
memory bloat, and capacity warnings."

### 1.10 "Effective context engineering for AI agents" — anthropic.com/engineering (Sep 2025)
**Blocked.** `[snippet]`: "finding the right altitude — between brittle, over-specified
logic and vague, underspecified guidance"; "strive for the minimal set of information
that fully outlines expected behavior, starting with minimal prompts and then adding
instructions based on observed failure modes". **Asserted.**

### 1.11 Anthropic staff, public
- Boris Cherny, X, 16 Apr 2026 (six Opus 4.7 tips; read via
  a community "claude-code-best-practice" collection on GitHub `tips/claude-boris-6-tips-16-apr-26.md`):
  none is about CLAUDE.md content; #6 "Give Claude a way to verify its work".
- "Anytime Claude does something wrong, we add it to the CLAUDE.md" — attributed to
  Cherny in a community "boris-cherny-claude-code-playbook" on GitHub README (theme #03,
  13 tips; the per-tip source URLs are in that repo's `TIPS.md`, not fetched). **Asserted.**
- Thariq Shihipar quote on simonwillison.net (18 Sep 2026): **blocked**, content unknown.

---

## 2. Measured studies (the load-bearing part)

### 2.1 Gloaguen, Mündler et al. (ETH Zurich SRI Lab), "Evaluating AGENTS.md: Are Repository-Level Context Files Helpful for Coding Agents?", arXiv:2602.11988, Feb 2026
**Blocked** at arxiv.org, sri.inf.ethz.ch and every mirror; four independent snippets agree. **Measured.**
- "providing context files does not generally improve task success rates, while
  increasing inference cost by over 20% on average"
- "Developer-provided files improved performance by about 4% on average; LLM-generated
  files reduced performance by about 3%"
- Conditions: developer-provided / none / LLM-generated; models Sonnet 4.5, GPT-4.1,
  o4-mini, Qwen 3; SWE-bench tasks plus a new set of issues from repos with committed
  context files.
- Conclusion `[snippet]`: "unnecessary requirements from context files make tasks
  harder, and human-written context files should describe only minimal requirements."
Relevance: the cost claim is solid and directional; the "+4 %" for human files is small.
This is the evidence for "every line costs; the case for brevity is the bill".

### 2.2 Damon McMillan, "Instruction Adherence in Coding Agent Configuration Files: A Factorial Study of Four File-Structure Variables", arXiv:2605.10039, May 2026
**Blocked**; three snippets agree. **Measured.**
- 1,650 Claude Code CLI sessions, 16,050 function-level observations, two TypeScript
  codebases, five tasks, "primarily Sonnet 4.6, with Opus 4.6 as a CLI-matched
  cross-model check and Opus 4.7 reported descriptively".
- Manipulated: file size **25 to 500 lines**, instruction position, file architecture
  (single vs split), presence of a contradiction in an adjacent file.
- "None of the four structural variables or three two-way interactions produces a
  detectable contrast after multiple-testing correction."
- What did move adherence: task type, and **sequence position within a session** —
  compliance "progressively degrades as it continuously generates more code".
Caveats: a single trivial target instruction (an annotation), TypeScript only, ≤ 500
lines. **It directly contradicts the docs' "longer files reduce adherence" within the
tested range**, so a Headstate line-count finding must be worded as cost and Anthropic's
stated target, never as "adherence will drop".

### 2.3 dos Santos, Costa, Montandon, Silva, Valente (UFMG), "Configuration Smells in AGENTS.md Files: Common Mistakes in Configuring Coding Agents", arXiv:2606.15828, June 2026
**Blocked** (arxiv, zenodo, register, infoworld, dev.ua); four snippets agree. **Measured prevalence; thresholds borrowed.**
- 100 popular open-source repos each holding an AGENTS.md or CLAUDE.md; **91 %** carry
  at least one smell.
- Six smells and prevalence: **Lint Leakage 62 %** ("restate rules already enforced by
  automated tools such as linters and formatters"), **Context Bloat 42 %** (detected at
  **≥ 200 lines**, threshold taken from Anthropic), **Skill Leakage 35 %** ("rarely used
  tools or practices get added to the AGENTS.md file, which gets loaded in every agent
  session"), **Conflicting Instructions 28 %**, **Init Fossilization 24 %** ("files are
  generated once but never reviewed or edited again, so they include stale or
  irrelevant rules"), **Blind References 16 %** ("reference external documents (e.g. via
  URLs) without explaining when that resource becomes relevant").
- "Two smells (Skill Leakage and Conflicting Instructions) increase the likelihood of
  Context Bloat by 83%."
- Detection: "automated heuristics"; context bloat and init fossilization are
  "based on pre-established thresholds" (the fossilization threshold was not in any
  snippet).
Dataset: zenodo.org/records/20600328 (blocked). This is the closest thing to a
peer-reviewed rule catalogue and maps almost one-to-one onto a content-shape check.

### 2.4 "From Anatomy to Smells: An Empirical Study of SKILL.md in Agent Skills", arXiv:2607.01456, July 2026
**Blocked**; snippet. **Measured.** 238 real skills; 13 higher-level / 44 lower-level
components; 26 authoring practices extracted from 29 sources; smell catalogue as their
inverses; "over 99% of real-world Agent Skill instruction files violate authoring best
practices, and those violations almost never disappear as skills evolve" (1,199
commits). The 26 practices were not retrievable; Anthropic's checklist (§1.6) is the
likely superset.

### 2.5 "Rule Taxonomy and Evolution in AI IDEs: A Mining and Survey Study", arXiv:2606.12231, June 2026
**Blocked**; snippet. **Measured.** 83 projects, 7,310 rules, taxonomy of 5 primary /
25 secondary categories; 1,540 evolution events; 99 survey responses. "repositories
favor low-level workflow and formatting rules while practitioners prioritize
architectural constraints"; rules "change often, mainly through expansions and
enrichments". Supports: files grow by accretion, and formatting rules (the lint-leakage
class) dominate in practice.

### 2.6 "Context Engineering for AI Agents in Open-Source Software", arXiv:2510.21413, Oct 2025
**Blocked**, no snippet with numbers obtained. Listed for completeness; not load-bearing.

### 2.7 Jaroslawicz et al., "How Many Instructions Can LLMs Follow at Once?" (IFScale), July 2025
Not fetched directly; cited via HumanLayer `[snippet]`: "frontier thinking LLMs can
follow approximately 150-200 instructions with reasonable consistency"; degradation is
uniform across instructions rather than dropping the newest; degradation shape differs
by model (threshold / linear / exponential). **Measured, but on a synthetic benchmark,
not on CLAUDE.md.** It is the only source behind the community's "instruction count"
framing.

### 2.8 Practitioner measurements (self-reported, method partly unknown)
- Cem Karaca, Medium, Feb 2026, "My CLAUDE.md Was Eating 42,000 Tokens Per
  Conversation": 1,200-line monolith → modular skills; "cut ... costs by 83%".
  **Blocked**; snippet. Measured by the author, method unstated.
- dhondooo, dev.to, Sep 2026, "My CLAUDE.md was 48,000 tokens. I cut it to 4,400 — and
  then measured whether that broke anything": 194,492 chars ≈ 48,600 tokens → 4,400;
  pre-cut "hit rate" 93 %; "the one real defect was a badly written sentence — not the
  architecture"; takeaway "measure your own hit rate before building anything to fix
  it". **Blocked**; snippet. Post-cut number not in any snippet.
- anthropics/claude-code#2766 (30 Jun 2025): 44.7k-char CLAUDE.md, saw the too-long
  warning; closed not planned, no maintainer answer. anthropics/claude-code#46724
  (11 Apr 2026, fetched): "200+ lines" project + user files; "Claude follows 'do X'
  rules ... inconsistently, and almost never follows 'stop and check before doing Y'
  process rules"; reliably followed: "formatting, technical constraints, safety rules";
  ignored: "gating/process rules, post-action checklists, proactive enforcement,
  document references". Closed not planned. **Anecdotal but consistent with §2.2's
  sequence-decay finding**: the rules that fail are the ones that must fire *later*.

---

## 3. Community guidance (asserted unless noted)

### 3.1 HumanLayer, "Writing a good CLAUDE.md", Nov 2025 — https://www.humanlayer.dev/blog/writing-a-good-claude-md
**Blocked** (also blocked: understandingdata, aiengineerguide, groff.dev,
garden.diego.codes, rajrajhans mirrors). Reconstructed from five snippets that agree:
- "keep CLAUDE.md files **under 300 lines**, with shorter being even better. At
  HumanLayer, their root CLAUDE.md file is **less than sixty lines**."
- "Claude Code's system prompt already contains **~50 instructions**" (they counted;
  method unstated) and IFScale's 150–200 → "as few instructions as possible".
- WHY / WHAT / HOW structure; "Invest deliberate effort in crafting each line".
- "Don't use `/init` output" as the file (it produces derivable content).
- "Use auto-fixing linters like Biome instead of instruction-based style guides".
- "Include file:line references to point Claude to the authoritative context instead of
  code snippets".
- Progressive disclosure: an `agent_docs/` directory, pointed to from CLAUDE.md, "letting
  Claude decide what to read".
Author attribution differs between mirrors (Dex Horthy vs Kyle Mistele); verify before
quoting a name. So: **the number asked about is 300, with 60 as their own root.**

### 3.2 Linters (rule lists = what the community already thinks is checkable)
- **felixgeelhaar/cclint** (github, fetched): file-size defaults **"10,000 characters
  and 200 lines"**; import syntax + resolution with **"max depth (5 hops)"** (note: the
  docs say **four**); duplicate imports; circular imports; "Import Context Cost" warns at
  **5 unique imports** ("still load at session start"); **Secret Detection** (`sk-*`,
  `sk-ant-*`, GitHub tokens, PEM blocks, high-entropy assignments; masks; ignores
  placeholders); **Command Safety** (`rm -rf /`, `curl | bash`); **Monorepo Hierarchy**
  ("Parent/child conflicts, duplicate content across hierarchy"); **Enforcement Hint**
  ("YOU MUST" → hook recommendation); **Content Appropriateness** (generic instructions
  such as "follow best practices"); **Karpathy Recommendations** ("Hedging language
  ('try to'), filler ('please'), show-don't-tell violations, prose length", Info only);
  **AGENTS.md Rule** (fallback/bridge guidance); **Structure** requires "Project
  Overview", "Development Commands", "Architecture" sections (contradicts Anthropic:
  "no required format", and `/doctor` *removes* architecture overviews).
- **jnschilling/claudelint** (README fetched; rule pages on claudelint.com blocked):
  CLAUDE.md "Size limits, import syntax, circular references, frontmatter"; commands
  `validate-cc-md`, `optimize-cc-md`; rule docs generated from source.
- **retif/claudecode-linter** (README fetched): "90 rules across 8 artifact types";
  10 `claude-md/*` rules (seen: `claude-md/no-todos`, "Blank line before headings");
  `skill-md/body-word-count` "recommended: 500-5000" words; seven `*/schema-valid`
  rules "auto-extracted from Claude Code's cli.js bundle — the same Zod schemas the
  runtime calls".
- **carlrannaberg/cclint** (README fetched): "Required sections presence", "Template
  compliance checking" — same template-shape idea, same contradiction with Anthropic.

### 3.3 AGENTS.md convention — agents.md (blocked); the agents.md convention repository on GitHub (fetched: 24.5k stars)
"a simple, open format for guiding coding agents"; sample sections "Dev environment
tips / Testing instructions / PR instructions"; no format or length rule. Secondary
`[snippet]` (morphllm): "stewarded by the Linux Foundation with adoption across 60,000+
open-source projects"; recommends CLAUDE.md = `@AGENTS.md` + Claude-only additions.
Simon Willison `[snippet]` on 2.1.277: he can "stop dropping CLAUDE.md files which just
contain '@AGENTS.md'".

### 3.4 Cursor rules — cursor.com/docs/rules (blocked; snippet)
"Keep rules under 500 lines"; "split large rules into multiple, composable rules";
"reference files instead of copying their contents"; "Start simple and add rules only
when you notice Agent making the same mistake repeatedly". Rule types: Always /
Auto-attached by glob / Agent-requested / Manual — the same three loading modes as
CLAUDE.md / `paths:` rules / skills.

### 3.5 Karpathy (Jan 26 2026 X thread) `[snippet]`
Four failure modes: silent assumptions, over-complication, scope creep, weak success
criteria. A third party turned it into a CLAUDE.md; "41 % → 11 %" error-rate claims
circulate without a stated method. **Not measured in any verifiable way**; cclint's
"Karpathy" rule is the only checkable descendant.

---

## 4. Disagreements and ranges (do not print one number)

| Quantity | Values seen | Sources |
|---|---|---|
| Line ceiling per CLAUDE.md | **200** (target) · 300 (soft cap) · 60 (HumanLayer's own root) · 500 (Cursor rules; Anthropic SKILL.md body) | §1.1, §1.3, §3.1, §3.4, §1.6 |
| Hard skip | 4 MiB per file | §1.1 |
| "Too long" warning | ~40.0k chars `[snippet]`, scales with model window since 2.1.169 | §1.8 |
| Import depth | 4 hops (docs) vs 5 (cclint) | §1.1, §3.2 |
| CLAUDE.md share of a 200k window | ~1.1 % illustrative (2,120 tokens); practitioners report 8k–48k tokens (4–24 %) before trimming | §1.4, §2.8 |
| Instruction budget | ~150–200 followed reliably (IFScale), ~50 already in the system prompt (HumanLayer) | §2.7, §3.1 |
| Does length cut adherence? | Docs: yes / "may" · McMillan: **no detectable effect 25–500 lines** · ETH: cost +20 %, success ±4 % | §1.1, §2.2, §2.1 |

---

## 5. What this means for the check design (summary; the issue carries the rules)

1. Line/size findings are **cost + Anthropic's target**, never an adherence prediction
   (§2.2). Token figures stay labelled estimates (`claudemd/tokens.rs` header).
2. The **smell catalogue** (§2.3) gives six deterministic-ish classes with prevalence:
   lint leakage, context bloat, skill leakage, conflicting instructions, init
   fossilization, blind references. Four have a mechanical detector; two (skill leakage,
   conflicts beyond simple token clashes) are judgement.
3. **Load order and lazy loading** (§1.1) turn "wrong file" into a mechanism: an
   `ALWAYS/NEVER` rule in a subdirectory CLAUDE.md is invisible until a Read in that
   directory, and invisible to Explore/Plan subagents entirely.
4. The linters converge on: size, imports (depth, unresolved, circular, outside-cwd),
   secrets, dangerous commands, duplicate content across the hierarchy, enforcement
   language → hooks, and a "required sections" template that Anthropic's own `/doctor`
   contradicts. Headstate should adopt the first six and reject the template.
5. Everything about *voice* (imperative, hedging, "please"), *altitude*, WHY/WHAT/HOW,
   "would removing this cause a mistake", and revisit-after-model-release is guidance
   text, not a finding.
