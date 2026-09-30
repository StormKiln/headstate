import { createContext, createElement, useContext, type ReactNode } from "react";

// `AuthGate` always provides this in the shipped tree. The default keeps
// standalone App and StatusBar tests on their existing authenticated path.
export type GitHubAuthAvailability = boolean | null;

const GitHubAuthAvailabilityContext = createContext<GitHubAuthAvailability>(true);

export function GitHubAuthProvider({
  available,
  children,
}: {
  available: GitHubAuthAvailability;
  children: ReactNode;
}) {
  return createElement(GitHubAuthAvailabilityContext.Provider, { value: available }, children);
}

export function useGitHubAuthAvailable(): GitHubAuthAvailability {
  return useContext(GitHubAuthAvailabilityContext);
}

// null means the account has not been verified. undefined is only the
// standalone component default; AuthGate always supplies an explicit value.
const GitLabViewerContext = createContext<string | null | undefined>(undefined);
export function GitLabViewerProvider({ viewer, children }: { viewer: string | null; children: ReactNode }) {
  return createElement(GitLabViewerContext.Provider, { value: viewer }, children);
}
export const useGitLabViewer = () => useContext(GitLabViewerContext);
