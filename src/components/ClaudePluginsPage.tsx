import { Card } from "@/components/ui/card";
import { useClaudePlugins } from "../api/hooks";
import { relativeTime } from "../lib/time";
import type {
  InstalledPlugin,
  PluginContribution,
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
  const used = usage.filter((u) => callsOf(u) > 0);
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

      <Summary report={report} used={used.length} partial={partial} />

      <div className="mt-4">
        <PluginCallsChart points={activity} days={PLUGIN_ACTIVITY_DAYS} />
      </div>

      <div className="mt-4">
        <Ranked usage={used} partial={partial} byName={byName} />
      </div>

      <div className="mt-4">
        <Table usage={usage} byName={byName} partial={partial} />
      </div>

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
        <div className="text-xs text-[#8b949e]">Used at least once</div>
        <div className="mt-1 text-2xl font-semibold text-[#e6edf3]">{used.toLocaleString()}</div>
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
          one is actually called.
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
              <th scope="col" className="px-4 py-2 font-medium">
                Calls
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
      <td className="px-4 py-2 align-top tabular-nums">
        <CallCount usage={usage} n={n} partial={partial} notCountable={notCountable} />
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
