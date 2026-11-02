import { createContext, createElement, useContext, type ReactNode } from "react";
import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { getGitLabHost, setGitLabHost } from "./gitlabHost";
import { getGitLabAuthState, type GitLabAuthState } from "./tauri";
import { useSourceSelection } from "@/store/sourceSelection";

export type GitLabAuthCheck = UseQueryResult<GitLabAuthState>;

/// The configured host's GitLab sign-in check. `AuthGate` owns it and hands
/// it down through `GitLabAuthProvider`, so a retry from the banner or from
/// the queue's own line re-runs the one check and both show its result.
export function useGitLabAuth(): GitLabAuthCheck {
  const selection = useSourceSelection((s) => s.selection);
  const host = useQuery({ queryKey: ["gitlab-host"], queryFn: getGitLabHost, retry: false });
  return useQuery({
    queryKey: ["gitlab-auth", host.data ?? "invalid"],
    queryFn: getGitLabAuthState,
    staleTime: 60_000,
    refetchInterval: selection === "github" ? false : 60_000,
    retry: false,
    enabled: selection !== "github" && (host.isSuccess || host.isError),
  });
}

// null outside AuthGate: there is no check to show or re-run.
const GitLabAuthContext = createContext<GitLabAuthCheck | null>(null);
export function GitLabAuthProvider({ value, children }: { value: GitLabAuthCheck; children: ReactNode }) {
  return createElement(GitLabAuthContext.Provider, { value }, children);
}
export const useGitLabAuthCheck = () => useContext(GitLabAuthContext);

/// Save the GitLab host and drop every result that belonged to the old one.
export function useSaveGitLabHost(onSaved?: () => void) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: setGitLabHost,
    onSuccess: async (saved) => {
      onSaved?.();
      queryClient.setQueryData(["gitlab-host"], saved);
      // Reset active observers as well as cached receipts. removeQueries
      // alone leaves a mounted AuthGate displaying the previous host's
      // result indefinitely. A reset clears that result and refetches.
      const matchesGitLab = ({ queryKey }: { queryKey: readonly unknown[] }) =>
        queryKey[0] !== "gitlab-host" && queryKey.some((part) => typeof part === "string" && part.startsWith("gitlab"));
      await queryClient.cancelQueries({ predicate: matchesGitLab });
      await queryClient.resetQueries({ predicate: matchesGitLab });
      queryClient.removeQueries({ predicate: (query) => matchesGitLab(query) && !query.isActive() });
    },
  });
}
