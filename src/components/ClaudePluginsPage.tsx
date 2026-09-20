import { Card } from "@/components/ui/card";
import { useClaudeDefinitions, useClaudeMcpServers, useClaudePlugins } from "../api/hooks";
import type {
  ClaudeDefinition,
  ClaudeDefinitionSource,
  ClaudeMcpTransport,
  ClaudeSettingsOrigin,
} from "../api/tauri";
import { definitionSourceLabel } from "../lib/definitionSource";
import { relativeTime } from "../lib/time";
import type {
  InstalledPlugin,
  PluginContribution,
  PluginFootprint,
  PluginUsage,
  PluginsReport,
} from "../types/pr";
import { PartialScanNotice } from "./PartialScanNotice";
import { QueryError, errorMessage } from "./QueryError";
import { PluginCallsChart } from "./stats/PluginCallsChart";

/// How many days the plugin activity chart covers.
///
/// Mirrored from `claude/plugins.rs`'s `ACTIVITY_DAYS`;
/// `mirroredConstants.test.ts` checks the two agree, so a change here
/// without a change there fails rather than silently drawing a window
/// the backend did not fill.
export const PLUGIN_ACTIVITY_DAYS = 30;

/// A plugin's total counted calls, across all four contribution shapes.
///
/// One helper rather than the sum written out at each of the four places
/// that need it: they must agree, and a forgotten `command_calls` in one
/// of them would rank a plugin below its real usage in one panel and not
/// in another.
function callsOf(u: PluginUsage): number {
  return u.mcp_calls + u.skill_calls + u.agent_calls + u.command_calls;
}

/// Whether a plugin shows ANY evidence of doing work -- calls or
/// engagement.
///
/// Deliberately not a sum (#1082). The two figures stay separate
/// everywhere they are shown; this is only the question "is there
/// anything here at all", which decides whether a plugin appears in the
/// "used" tally rather than what number is printed for it.
///
/// `remember` is why it exists: 0 calls and a real footprint, which a
/// calls-only test counts as unused and would then argue for
/// uninstalling.
function showedActivity(u: PluginUsage): boolean {
  return callsOf(u) > 0 || u.footprint.calls > 0 || u.footprint.install_reads > 0;
}

/// Whether a plugin ships nothing that could ever produce a counted call.
///
/// When this is true, a zero is a property of the plugin's SHAPE rather
/// than a measurement of the user's habits -- an LSP works by being
/// loaded, not by being called -- and the page says so instead of
/// printing "no calls".
///
/// `read === true` is required, and is the whole subtlety: an install
/// path we could not read tells us nothing about what the plugin ships,
/// and treating that silence as "ships nothing" would turn a failed read
/// into a confident claim. Absent is not zero, applied to a feature list.
function shipsNothingCountable(c: PluginContribution | undefined): boolean {
  if (c === undefined || !c.read) return false;
  return !c.mcp && !c.skills && !c.agents && !c.commands;
}

/// What was counted, said once, at the top.
///
/// # Why the page says this at all
///
/// Because the number it is about is not the obvious one. A plugin's
/// tool names appear in every session's availability list, so the
/// intuitive reading of "how often was this plugin used" -- how often
/// does its name show up -- is wrong by three orders of magnitude and
/// wrong in RANK: it reports `chrome-devtools-mcp`, never called once on
/// the development machine, as the busiest plugin there by a distance.
///
/// A user comparing these figures against a `grep` of their own will get
/// a different answer, and the honest thing is to say which question
/// this page answered rather than let them conclude it is broken.
const COUNTING_RULE =
  "Counted from calls that were actually made. A plugin's tools are offered to every session, " +
  "so merely appearing in a session does not count here.";

/// The limit on what "calls" measures, said on the page (#1082).
///
/// # Why this has to be here and not in a doc comment
///
/// Because the number invites a conclusion it cannot support. A call
/// count is exact about invocations and says nothing about value, and
/// the two come apart completely for a plugin whose contribution is
/// instructions rather than tools: `remember` was called 0 times and
/// wrote 406 memory files. A reader who takes the calls column as
/// "value" uninstalls it and loses them.
///
/// Stated as a fact about the measurement, not an apology for it
/// (#1088): what is counted, what is not, and what the other column is.
const ENGAGEMENT_RULE =
  "Calls are not the same as value. A plugin can contribute without ever being called — by " +
  "adding instructions, context or background behaviour — so engagement counts a second thing: " +
  "tool calls that worked on files the plugin owns.";

/// Installed plugins, what they were used for, and what that is worth
/// (#1075).
///
/// # The argument this page is making
///
/// Its output is the input to "should I uninstall this?". That makes
/// every absence dangerous in a specific direction: a plugin shown as
/// unused that is not unused loses the user something they rely on, and
/// they will not find out until it is gone. So this page is more careful
/// about the difference between zero and unknown than a page whose
/// numbers only inform.
///
/// Three states, never collapsed:
///
/// - **not measured** -- we have no reading. Rendered in words, never a
///   `0`.
/// - **measured zero** -- the scan read every transcript and this plugin
///   was in none of them. A real fact, and the page's most common one:
///   7 of 22 installed plugins show any usage at all.
/// - **not countable** -- the plugin ships nothing that can produce a
///   tool call. `clangd-lsp`'s install path holds a LICENSE and a
///   README; it contributes background behaviour and always will. Its
///   zero measures nothing about the user's habits, so the page declines
///   to present it as disuse.
export function ClaudePluginsPage() {
  const { data, isLoading, isError, error, refetch } = useClaudePlugins();

  const header = (
    <div>
      <h2 className="text-base font-semibold text-[#e6edf3]">Plugins</h2>
      <p className="mt-1 text-xs text-[#8b949e]">{COUNTING_RULE}</p>
      <p className="mt-1 text-xs text-[#8b949e]">{ENGAGEMENT_RULE}</p>
    </div>
  );

  // Error first: measured and failed. A struct of zeros here would draw
  // a chart and a table of "0 calls" that look exactly like a quiet
  // month, which on this page argues for uninstalling everything.
  if (isError) {
    return (
      <div className="p-4">
        {header}
        <div className="mt-4">
          <QueryError
            title="Plugins could not be read"
            message={errorMessage(error)}
            onRetry={() => void refetch()}
          >
            <p className="mt-2">
              No figures are shown rather than zeroes: a plugin listed with no calls would look
              unused, and nothing here was measured.
            </p>
          </QueryError>
        </div>
      </div>
    );
  }

  // Not measured yet. A reserved frame rather than a skeleton of fake
  // rows, following `ClaudeOverviewPage`.
  if (isLoading || !data) {
    return (
      <div className="p-4">
        {header}
        <div className="mt-4 min-h-40" aria-busy="true" />
      </div>
    );
  }

  return <Loaded report={data} header={header} onRetry={() => void refetch()} />;
}

function Loaded({
  report,
  header,
  onRetry,
}: {
  report: PluginsReport;
  header: React.ReactNode;
  onRetry: () => void;
}) {
  const { installed, usage, activity, unreadable, inventory_failure, inventory_absent } = report;
  const partial = unreadable.length > 0;
  // Two different questions, deliberately two lists.
  //
  // `called` ranks plugins by invocations and is what the bar chart
  // draws -- a bar has to be a bar of ONE quantity, and mixing
  // engagement into it would be the blended number #1082 forbids.
  //
  // `active` is "showed any evidence of doing work", which is what the
  // headline tally should count: `remember` has 0 calls and a real
  // footprint, and counting it as unused is the defect being fixed.
  const called = usage.filter((u) => callsOf(u) > 0);
  const active = usage.filter(showedActivity);
  const byName = new Map(installed.map((p) => [p.name, p] as const));

  return (
    <div className="p-4">
      {header}

      {/* Above the figures it qualifies, never instead of them: the
          transcripts that DID read are real. */}
      {partial && (
        <div className="mt-3">
          <PartialScanNotice
            unreadable={unreadable}
            consequence="every count below is a floor — the true figures can only be higher."
          />
        </div>
      )}

      {/* The inventory and the usage scan fail independently. An
          unreadable inventory does not discard the counts, so this is a
          banner beside them rather than the error page. */}
      {inventory_failure !== null && (
        <div
          role="alert"
          className="mt-3 rounded border border-[#30363d] bg-[#161b22] px-4 py-2 text-xs text-[#8b949e]"
        >
          <p>
            The installed-plugin list could not be read, so this page can show what was called but
            not what you have installed. A plugin you own may be missing from the table below.
          </p>
          <p className="mt-1 break-all font-mono text-[#6e7681]">{inventory_failure}</p>
        </div>
      )}

      <Summary report={report} used={active.length} partial={partial} />

      <div className="mt-4">
        <PluginCallsChart points={activity} days={PLUGIN_ACTIVITY_DAYS} />
      </div>

      <div className="mt-4">
        <Ranked usage={called} partial={partial} byName={byName} />
      </div>

      <div className="mt-4">
        <Table usage={usage} byName={byName} partial={partial} />
      </div>

      <DefinitionsSection />

      <McpSection />

      {inventory_absent && installed.length === 0 && (
        // Measured, and the answer is none. Not a failure, and not
        // dressed as one -- `live.rs` draws this same line for a missing
        // session registry.
        <p className="mt-4 text-sm text-[#8b949e]">
          No plugins are installed. Claude Code keeps its inventory in{" "}
          <code className="font-mono text-xs">~/.claude/plugins</code>, and there is no such file
          yet.
        </p>
      )}

      <button
        type="button"
        onClick={onRetry}
        className="mt-4 rounded border border-[#30363d] px-3 py-1.5 text-xs text-[#e6edf3] hover:bg-[#161b22]"
      >
        Rescan transcripts
      </button>
      <p className="mt-1 text-xs text-[#6e7681]">
        {report.scanned === 0
          ? "Nothing changed since the last scan, so nothing was re-read."
          : `Read ${report.scanned.toLocaleString()} ${
              report.scanned === 1 ? "transcript" : "transcripts"
            } that had changed.`}
      </p>
    </div>
  );
}

/// The three headline figures.
function Summary({
  report,
  used,
  partial,
}: {
  report: PluginsReport;
  used: number;
  partial: boolean;
}) {
  const total = report.usage.reduce((n, u) => n + callsOf(u), 0);
  const installed = report.installed.length;
  return (
    <div className="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-3">
      <Card className="px-4 py-3">
        <div className="text-xs text-[#8b949e]">Installed</div>
        <div className="mt-1 text-2xl font-semibold text-[#e6edf3]">
          {report.inventory_failure !== null ? "—" : installed.toLocaleString()}
        </div>
        <div className="mt-1 text-xs text-[#6e7681]">
          {report.inventory_failure !== null
            ? "the inventory could not be read"
            : `from ${new Set(report.installed.map((p) => p.marketplace)).size} marketplace${
                new Set(report.installed.map((p) => p.marketplace)).size === 1 ? "" : "s"
              }`}
        </div>
      </Card>
      <Card className="px-4 py-3">
        {/* "Showed activity", not "used at least once": the tally now
            includes plugins with engagement and no calls, and calling
            that "used" would overstate what was measured about them. */}
        <div className="text-xs text-[#8b949e]">Showed activity</div>
        <div data-testid="activity-tally" className="mt-1 text-2xl font-semibold text-[#e6edf3]">
          {used.toLocaleString()}
        </div>
        <div className="mt-1 text-xs text-[#6e7681]">
          {/* The denominator is the finding. "7" alone is not an
              argument; "7 of 22" is. */}
          {report.inventory_failure !== null
            ? "of an unknown number installed"
            : `of ${installed.toLocaleString()} installed`}
        </div>
      </Card>
      <Card className="px-4 py-3">
        <div className="text-xs text-[#8b949e]">Calls counted</div>
        <div className="mt-1 text-2xl font-semibold text-[#e6edf3]">
          {partial ? `at least ${total.toLocaleString()}` : total.toLocaleString()}
        </div>
        <div className="mt-1 text-xs text-[#6e7681]">across every transcript on this machine</div>
      </Card>
    </div>
  );
}

/// The ranked bar, plugins by usage.
///
/// Drawn as proportional bars rather than a second recharts chart: this
/// is a ranking of at most a couple of dozen labelled rows, which is the
/// `Leaderboard` case -- it declines recharts entirely for exactly this
/// shape, because a bar chart of named categories is a table with one
/// visual column and reads better as one.
function Ranked({
  usage,
  partial,
  byName,
}: {
  usage: PluginUsage[];
  partial: boolean;
  byName: Map<string, InstalledPlugin>;
}) {
  if (usage.length === 0) {
    // Measured, and it was zero. Said as a fact about the reading, with
    // the reason it is not necessarily a disappointment.
    return (
      <Card className="px-4 py-3">
        <div className="text-sm font-semibold">Most used</div>
        <p className="mt-2 text-sm text-[#8b949e]">
          No plugin call was recorded in any transcript on this machine. That is a measurement, not
          an error — plugins that contribute skills, agents or MCP tools leave a trace only when
          one is actually called, and a plugin can still be doing its job without one. The
          engagement column below is the other half of the picture.
        </p>
      </Card>
    );
  }
  const max = callsOf(usage[0]);
  return (
    <Card className="px-4 py-3">
      <div className="text-sm font-semibold">Most used</div>
      <div className="text-xs text-[#8b949e]">
        {partial ? "at least this many calls each — the scan was short" : "calls per plugin"}
      </div>
      <ul className="mt-3 space-y-2">
        {usage.map((u) => {
          const n = callsOf(u);
          const installed = byName.has(u.name);
          return (
            <li key={u.name}>
              <div className="flex items-baseline justify-between gap-2 text-xs">
                <span className="truncate font-mono text-[#e6edf3]">
                  {u.name}
                  {!installed && (
                    // A plugin with recorded calls that is not in the
                    // inventory was used and then removed. Worth saying:
                    // it explains a name the user cannot find in their
                    // plugin list, and it is evidence of past value.
                    <span className="ml-2 font-sans text-[#6e7681]">no longer installed</span>
                  )}
                </span>
                <span className="shrink-0 tabular-nums text-[#8b949e]">
                  {partial ? `≥ ${n.toLocaleString()}` : n.toLocaleString()}
                </span>
              </div>
              <div className="mt-1 h-2 w-full overflow-hidden rounded bg-[#21262d]">
                <div
                  className="h-full rounded bg-[#1f6feb]"
                  style={{ width: `${max === 0 ? 0 : Math.max(2, (n / max) * 100)}%` }}
                />
              </div>
            </li>
          );
        })}
      </ul>
    </Card>
  );
}

/// Every installed plugin, and what is known about each.
function Table({
  usage,
  byName,
  partial,
}: {
  usage: PluginUsage[];
  byName: Map<string, InstalledPlugin>;
  partial: boolean;
}) {
  if (usage.length === 0) return null;
  return (
    <Card className="px-0 py-0">
      <div className="px-4 py-3">
        <div className="text-sm font-semibold">All plugins</div>
        <div className="text-xs text-[#8b949e]">
          what each contributes, and what was counted for it
        </div>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-xs">
          <thead className="border-y border-[#30363d] text-[#8b949e]">
            <tr>
              <th scope="col" className="px-4 py-2 font-medium">
                Plugin
              </th>
              <th scope="col" className="px-4 py-2 font-medium">
                Source
              </th>
              {/* What it ships, so a zero in the next column reads
                  correctly: a skills-only plugin and an MCP server with
                  31 tools cannot be judged by the same number. */}
              <th scope="col" className="px-4 py-2 font-medium">
                Contributes
              </th>
              <th scope="col" className="px-4 py-2 font-medium">
                Calls
              </th>
              {/* Deliberately its own column. Merging it into "Calls"
                  would produce a blended figure that answers neither
                  question -- see ENGAGEMENT_RULE. */}
              <th scope="col" className="px-4 py-2 font-medium">
                Engagement
              </th>
              <th scope="col" className="px-4 py-2 font-medium">
                Failures
              </th>
              <th scope="col" className="px-4 py-2 font-medium">
                Last used
              </th>
            </tr>
          </thead>
          <tbody>
            {usage.map((u) => (
              <Row key={u.name} usage={u} plugin={byName.get(u.name)} partial={partial} />
            ))}
          </tbody>
        </table>
      </div>
    </Card>
  );
}

function Row({
  usage,
  plugin,
  partial,
}: {
  usage: PluginUsage;
  plugin: InstalledPlugin | undefined;
  partial: boolean;
}) {
  const n = callsOf(usage);
  const notCountable = shipsNothingCountable(plugin?.contribution);

  return (
    <tr className="border-b border-[#21262d] last:border-0">
      <th scope="row" className="px-4 py-2 text-left font-normal">
        <div className="font-mono text-[#e6edf3]">{usage.name}</div>
        {plugin?.version !== undefined && plugin.version !== null && (
          <div className="text-[#6e7681]">v{plugin.version}</div>
        )}
      </th>
      <td className="px-4 py-2 align-top text-[#8b949e]">
        {plugin === undefined ? (
          <span className="text-[#6e7681]">not installed</span>
        ) : (
          <>
            <div>{plugin.marketplace === "" ? "unknown marketplace" : plugin.marketplace}</div>
            {plugin.scope !== null && <div className="text-[#6e7681]">{plugin.scope}</div>}
          </>
        )}
      </td>
      <td className="px-4 py-2 align-top text-[#8b949e]">
        <Contributes contribution={plugin?.contribution} />
      </td>
      <td className="px-4 py-2 align-top tabular-nums">
        <CallCount usage={usage} n={n} partial={partial} notCountable={notCountable} />
      </td>
      <td className="px-4 py-2 align-top tabular-nums">
        <Engagement footprint={usage.footprint} partial={partial} />
      </td>
      <td className="px-4 py-2 align-top tabular-nums text-[#8b949e]">
        {/* A failure count is only meaningful against calls that
            happened. With no calls there is nothing to have failed, and
            a "0" in this column would read as a clean record rather than
            as an empty one. */}
        {n === 0 ? <span className="text-[#6e7681]">—</span> : usage.failures.toLocaleString()}
      </td>
      <td className="px-4 py-2 align-top text-[#8b949e]">
        {usage.last_called_at === null ? (
          <span className="text-[#6e7681]">—</span>
        ) : (
          relativeTime(usage.last_called_at, new Date())
        )}
      </td>
    </tr>
  );
}

/// What a plugin ships, so a zero in the Calls column reads correctly.
///
/// #1082's second requirement. A plugin shipping only `skills/` cannot
/// be measured like one exposing 31 MCP tools: the first is reached by
/// the model choosing to follow instructions, the second by an explicit
/// call. Naming the shape lets the reader judge whether a low count is
/// surprising at all.
///
/// An install path we could not read says so rather than guessing --
/// the same absent-is-not-zero rule, applied to a feature list.
function Contributes({ contribution }: { contribution: PluginContribution | undefined }) {
  if (contribution === undefined || !contribution.read) {
    return <span className="text-[#6e7681]">not known</span>;
  }
  const parts = [
    contribution.mcp && "an MCP server",
    contribution.skills && "skills",
    contribution.agents && "agents",
    contribution.commands && "commands",
  ].filter((p): p is string => typeof p === "string");

  if (parts.length === 0) {
    // Real, and the reason `rust-analyzer-lsp` must never read as
    // "unused": it ships a README and contributes background behaviour
    // that leaves no tool call by construction.
    return <span className="text-[#6e7681]">background behaviour only</span>;
  }
  return <span>{parts.join(", ")}</span>;
}

/// Engagement: work done on what the plugin owns (#1082).
///
/// # The three states, which must not collapse
///
/// - **untraceable** (`owned_known === false`) — this plugin has no
///   owned directory we can follow, so we have no reading. Words, never
///   a `0`: rendering it as zero would say "this plugin did nothing"
///   when what we mean is "we cannot see what it did".
/// - **traceable, nothing found** — a real measured zero.
/// - **a figure** — with the tool that did the work, because the shape
///   is the argument: 408 `Read`s is the model consulting the plugin's
///   material; 185 `Write`s is the plugin's output being produced.
function Engagement({ footprint, partial }: { footprint: PluginFootprint; partial: boolean }) {
  if (!footprint.owned_known) {
    // Absent is not zero. We cannot trace this one, and must say so.
    return <span className="text-[#6e7681]">not traced</span>;
  }
  if (footprint.calls === 0 && footprint.install_reads === 0) {
    return <span className="text-[#8b949e]">none recorded</span>;
  }

  // The busiest tool, named. One is enough to convey the shape without
  // turning a table cell into a second table.
  const top = Object.entries(footprint.by_tool).sort((a, b) => b[1] - a[1])[0];
  return (
    <div>
      <div className="text-[#e6edf3]">
        {partial ? `at least ${footprint.calls.toLocaleString()}` : footprint.calls.toLocaleString()}
      </div>
      {top !== undefined && (
        <div className="text-[#6e7681]">
          mostly {top[0]} ({top[1].toLocaleString()})
        </div>
      )}
      {footprint.install_reads > 0 && (
        <div className="text-[#6e7681]">
          {footprint.install_reads.toLocaleString()} of its own files read
        </div>
      )}
    </div>
  );
}

/// The one cell where absent, zero and not-countable must not collapse.
function CallCount({
  usage,
  n,
  partial,
  notCountable,
}: {
  usage: PluginUsage;
  n: number;
  partial: boolean;
  notCountable: boolean;
}) {
  // No reading at all. NEVER a bare zero: this plugin may predate the
  // scan, and a zero here argues for uninstalling it on the strength of
  // nothing.
  if (!usage.measured) {
    return <span className="text-[#6e7681]">no calls recorded</span>;
  }
  if (n > 0) {
    return (
      <span className="text-[#e6edf3]">
        {partial ? `at least ${n.toLocaleString()}` : n.toLocaleString()}
      </span>
    );
  }
  // Measured zero, but the plugin ships nothing that could ever be
  // counted. Saying "0 calls" here would be true and misleading: an LSP
  // works by being loaded, not by being called.
  if (notCountable) {
    return (
      <span className="text-[#6e7681]">
        nothing here is counted — this plugin makes no tool calls
      </span>
    );
  }
  // Measured, and genuinely zero. Qualified while the scan is short,
  // because a floor of zero is not a zero.
  return (
    <span className="text-[#8b949e]">{partial ? "none recorded so far" : "no calls"}</span>
  );
}

/// A scope badge, so every row says where it came from.
///
/// The scope is on the ROW rather than only in a grouping header
/// because the list is sorted by name -- which is what puts a colliding
/// pair adjacent, and a collision is unreadable if you cannot see which
/// row is which.
function SourceBadge({ source }: { source: ClaudeDefinitionSource }) {
  const tone =
    source.scope === "user"
      ? "text-[#58a6ff]"
      : source.scope === "project"
        ? "text-[#3fb950]"
        : "text-[#d2a8ff]";
  return (
    <span
      className={`ml-2 text-[10px] ${tone}`}
      title={source.scope === "user" ? "~/.claude" : source.path}
    >
      {definitionSourceLabel(source)}
    </span>
  );
}

/// Every skill, subagent and slash command on this machine (#1129,
/// #1215).
///
/// The plugins table above reports which plugins SHIP a `skills/`
/// directory and nothing about what is inside it, so a user could not
/// answer "what subagents do I have". Hand-written definitions -- the
/// ones belonging to no plugin -- were invisible entirely, and before
/// #1215 so was everything outside `~/.claude`.
///
/// # Why a collision is shown rather than resolved
///
/// Two definitions of one kind with one name shadow each other, and
/// which one Claude Code loads is a rule this app has not measured.
/// `claude/definitions.rs`'s header makes the argument at length: a
/// precedence rule invented here would be a second source of truth for
/// someone else's behaviour, and unlike a wrong merged VALUE a wrong
/// precedence here deletes a definition from the page entirely. So both
/// are listed, marked, and their sources named -- the user resolves it
/// against the tool that actually decides.
///
/// Exported for its own test file: the plugins report this page also
/// renders is a large fixture, and reconstructing it to exercise a
/// sibling section would test the fixture.
export function DefinitionsSection() {
  const { data, isLoading, isError, error, refetch } = useClaudeDefinitions();

  if (isError) {
    // NOT an empty list. "You have no skills" and "we could not look"
    // are different answers, and this page's whole argument is that the
    // second must never render as the first.
    return (
      <div className="mt-6">
        <h3 className="text-sm font-semibold text-[#e6edf3]">Skills, agents and commands</h3>
        <div className="mt-2">
          <QueryError
            title="Definitions could not be read"
            message={errorMessage(error)}
            onRetry={() => void refetch()}
          />
        </div>
      </div>
    );
  }

  if (isLoading || !data) {
    return (
      <div className="mt-6">
        <h3 className="text-sm font-semibold text-[#e6edf3]">Skills, agents and commands</h3>
        <div className="mt-2 min-h-20" aria-busy="true" />
      </div>
    );
  }

  // Indices, not definitions: a collision names rows in THIS list, and
  // matching by name again on the frontend would be a second
  // implementation of the grouping that could disagree with the first.
  const colliding = new Set<number>();
  for (const c of data.collisions) for (const m of c.members) colliding.add(m);

  const indexed = data.definitions.map((d, i) => ({ d, i }));
  const byKind = (k: ClaudeDefinition["kind"]) => indexed.filter((x) => x.d.kind === k);

  return (
    <div className="mt-6">
      <h3 className="text-sm font-semibold text-[#e6edf3]">Skills, agents and commands</h3>
      <p className="mt-1 text-xs text-[#8b949e]">
        Everything in <code>~/.claude</code>, in each scanned project&rsquo;s{" "}
        <code>.claude</code>, and in every installed plugin.
      </p>

      {/* What could not be read, ABOVE the list rather than instead of
          it: the definitions that did read are real and worth showing,
          which is the trade `PartialScanNotice` states. Each refusal
          names its SCOPE, because a project behind a permission wall
          hides an unknown number of definitions and a successful user
          scan beside it must not paper over that. */}
      {data.unreadable.length > 0 && (
        <div role="status" className="mt-2 text-xs text-[#d29922]">
          <p>
            {data.unreadable.length} scope{data.unreadable.length === 1 ? "" : "s"} could not
            be read, so the list below may not be all of them.
          </p>
          <ul className="mt-1 space-y-0.5">
            {data.unreadable.map((r) => (
              <li key={r.detail} className="text-[10px] text-[#8b949e]">
                {r.detail}
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* Collisions, named rather than resolved. See this component's
          doc comment and `claude/definitions.rs`'s header. */}
      {data.collisions.length > 0 && (
        <div role="status" className="mt-2 text-xs text-[#d29922]">
          <p>
            {data.collisions.length} name{data.collisions.length === 1 ? " is" : "s are"}{" "}
            claimed by more than one scope. Which one Claude Code loads is its rule, not
            Headstate&rsquo;s, so both are listed below.
          </p>
          <ul className="mt-1 space-y-0.5">
            {data.collisions.map((c) => (
              <li key={`${c.kind}:${c.name}`} className="text-[10px] text-[#8b949e]">
                {c.kind} <span className="text-[#e6edf3]">{c.name}</span>:{" "}
                {c.members
                  .map((m) => definitionSourceLabel(data.definitions[m].source))
                  .join(" and ")}
              </li>
            ))}
          </ul>
        </div>
      )}

      {data.definitions.length === 0 ? (
        <p className="mt-2 text-sm text-[#8b949e]">
          No skills, agents or commands are defined here.
        </p>
      ) : (
        <div className="mt-2 space-y-3">
          {(["skill", "agent", "command"] as const).map((kind) => {
            const items = byKind(kind);
            if (items.length === 0) return null;
            return (
              <div key={kind}>
                <p className="text-xs uppercase tracking-wide text-[#8b949e]">
                  {kind}s ({items.length})
                </p>
                <ul className="mt-1 space-y-0.5">
                  {items.map(({ d, i }) => (
                    // Keyed on the INDEX as well as the path: overlapping
                    // scan roots can reach one file twice, and a
                    // duplicate key would drop a row React should keep.
                    <li key={`${i}:${d.path}`} className="text-xs">
                      <span className="text-[#e6edf3]">{d.name}</span>
                      <SourceBadge source={d.source} />
                      {/* Marked on the ROW as well as in the summary
                          above: a user scrolling the list has to be able
                          to see that this entry is not the only one
                          answering to this name. */}
                      {colliding.has(i) && (
                        <span className="ml-1 text-[10px] text-[#d29922]">(name collision)</span>
                      )}
                      {/* A name taken from the filename is marked, so a
                          reader can tell it from one the author wrote. */}
                      {!d.namedInFrontmatter && (
                        <span className="ml-1 text-[10px] text-[#6e7681]">(from filename)</span>
                      )}
                      {d.description && (
                        <span className="ml-2 text-[#8b949e]">{d.description}</span>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/// Which scope defines a server, in words a reader can act on.
///
/// The PATH shape for the settings scopes, for the reason
/// `EffectiveSettingsPanel`'s own table gives: a reader needs to know
/// which file to open, and "project" does not say where. `plugin` is
/// the scope `Origin` grew for #1216 and names a directory rather than
/// a file, because a plugin's manifest may be `.mcp.json` or
/// `mcp.json`.
const MCP_ORIGIN_LABEL: Record<ClaudeSettingsOrigin, string> = {
  user: "~/.claude.json",
  project: "~/.claude.json (this project only)",
  local: ".claude/settings.local.json",
  plugin: "a plugin's .mcp.json",
};

/// One server's transport, for display.
function transportText(t: ClaudeMcpTransport): string {
  if (t.kind === "stdio") return t.command;
  if (t.kind === "url") return t.url;
  // Neither a command nor a url. Named rather than blank: the entry
  // exists and we could not describe it, which is a different fact from
  // a server with no transport.
  return "transport not recognised";
}

/// Every MCP server configured on this machine, and which scope defines
/// it (#1216).
///
/// Before this the app could not name a single MCP tool: `plugins.rs`
/// matched a filename to set one boolean, so a plugin contributing 31
/// tools and one contributing zero were one boolean apart.
///
/// Scope is the load-bearing column, not the list. A server added for
/// one project is easy to forget and then confusing everywhere else, so
/// every row says which scope defines it and a project-scoped row says
/// which project.
///
/// Exported for its own test file, for the reason `DefinitionsSection`
/// gives: the plugins report this page also renders is a large fixture.
export function McpSection() {
  const { data, isLoading, isError, error, refetch } = useClaudeMcpServers();

  if (isError) {
    // NOT an empty list, and this is the whole point of the ticket.
    // "You have no MCP servers" and "we could not look" are different
    // answers.
    return (
      <div className="mt-6">
        <h3 className="text-sm font-semibold text-[#e6edf3]">MCP servers</h3>
        <div className="mt-2">
          <QueryError
            title="MCP servers could not be read"
            message={errorMessage(error)}
            onRetry={() => void refetch()}
          />
        </div>
      </div>
    );
  }

  if (isLoading || !data) {
    return (
      <div className="mt-6">
        <h3 className="text-sm font-semibold text-[#e6edf3]">MCP servers</h3>
        <div className="mt-2 min-h-20" aria-busy="true" />
      </div>
    );
  }

  // A scope that could not be read means the list below is a floor, not
  // a total -- so the empty case must not say "none are configured"
  // while a refusal is standing.
  const refused = data.unreadable.length > 0;

  return (
    <div className="mt-6">
      <h3 className="text-sm font-semibold text-[#e6edf3]">MCP servers</h3>
      <p className="mt-1 text-xs text-[#8b949e]">
        Configured in <code>~/.claude.json</code> and in installed plugins. Read only —
        Headstate never writes to that file, which Claude Code rewrites while it runs.
      </p>

      {/* Refusals FIRST and in their own colour, above the list rather
          than instead of it: the servers that DID read are real. */}
      {data.unreadable.map((r) => (
        <p key={r.path} role="alert" className="mt-2 text-xs text-[#f85149]">
          {r.detail}
        </p>
      ))}

      {data.servers.length === 0 ? (
        refused ? (
          // The sentence the module exists to keep separate from the one
          // below it.
          <p className="mt-2 text-sm text-[#d29922]">
            No servers could be listed, because the configuration above could not be read.
            This is not the same as having none configured.
          </p>
        ) : (
          <p className="mt-2 text-sm text-[#8b949e]">No MCP servers are configured.</p>
        )
      ) : (
        <ul className="mt-2 space-y-1">
          {data.servers.map((s) => (
            <li key={`${s.origin}:${s.scopeDetail ?? ""}:${s.name}`} className="text-xs">
              <span className="text-[#e6edf3]">{s.name}</span>
              {/* The scope, in TEXT rather than by position or colour:
                  it is the column the page exists for. */}
              <span className="ml-2 text-[#8b949e]">{MCP_ORIGIN_LABEL[s.origin]}</span>
              {s.scopeDetail !== null && s.origin !== "user" && (
                <span className="ml-1 text-[10px] text-[#6e7681]">{s.scopeDetail}</span>
              )}
              <code className="ml-2 break-all text-[#6e7681]">{transportText(s.transport)}</code>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
