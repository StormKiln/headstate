import { createContext, useContext } from "react";
import type { ExpectationMap } from "@/lib/jira/drift";
import type { LinkMaps } from "@/lib/jira/links";

/// What every Jira surface needs to know, decided once by `JiraProvider`
/// (epic #2031).
///
/// `null` whenever keys cannot be matched yet -- Jira off, `acli` not
/// signed in, or the link index not (or not successfully) read. Every
/// consumer renders NOTHING for `null`: without the project list a
/// candidate key cannot be told from a look-alike, so showing one would be
/// a guess.
export interface JiraState {
  /// Project keys matched, with the user's exclusions already removed.
  projects: ReadonlySet<string>;
  /// The signed-in site, for building a browse link when Jira's reply
  /// carried none.
  site: string | null;
  /// Which keys each row names, and what names each key, from the
  /// desktop's index.
  links: LinkMaps;
  /// Settings' per-project status expectations, for the drift rules.
  expectations: ExpectationMap;
  /// Whether pull-request rows show ticket chips (Settings › Jira).
  chipsOnPrRows: boolean;
  /// Settings › Jira's branch-name template for Start work (#2045);
  /// empty is the default.
  branchTemplate: string;
}

export const JiraContext = createContext<JiraState | null>(null);

/// The Jira state, or `null` when nothing Jira-related should render --
/// including when no provider is mounted at all.
export function useJira(): JiraState | null {
  return useContext(JiraContext);
}
