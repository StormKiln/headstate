import { createContext, useContext } from "react";

/// What every Jira surface needs to know, decided once by `JiraProvider`
/// (epic #2031).
///
/// `null` whenever keys cannot be matched yet -- Jira off, `acli` not
/// signed in, or the project list not (or not successfully) read. Every
/// consumer renders NOTHING for `null`: without the project list a
/// candidate key cannot be told from a look-alike, so showing one would be
/// a guess.
export interface JiraState {
  /// Project keys to match, with the user's exclusions already removed.
  projects: ReadonlySet<string>;
  /// The signed-in site, for building a browse link when Jira's reply
  /// carried none.
  site: string | null;
}

export const JiraContext = createContext<JiraState | null>(null);

/// The Jira state, or `null` when nothing Jira-related should render --
/// including when no provider is mounted at all.
export function useJira(): JiraState | null {
  return useContext(JiraContext);
}
