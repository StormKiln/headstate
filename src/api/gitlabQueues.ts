import { useCallback, useEffect, useMemo, useState } from "react";
import { listen, type UnlistenFn } from "./transport";
import { getSourceSnapshot, refreshSelectedSource, type SourceList, type SourcePollUpdate } from "./tauri";
import { GitLabQueueState } from "./gitlabQueueState";
import { safeUnlisten } from "./unlisten";
import { useGitLabViewer } from "./authAvailability";
import { IS_MOBILE_BUILD } from "../lib/target";

export function useGitLabQueue(list: SourceList, selected: boolean, host = "gitlab.com") {
  const viewer = useGitLabViewer();
  const enabled = selected && viewer !== null;
  const source = useMemo(() => ({ provider: "gitlab" as const, host }), [host]);
  const model = useMemo(() => new GitLabQueueState(host, list, viewer), [host, list, viewer]);
  const [state, setState] = useState(() => ({ source, list, model, snapshot: model.snapshot() }));
  const publish = useCallback(() => setState({ source, list, model, snapshot: model.snapshot() }), [model, source, list]);

  const refresh = useCallback(async () => {
    if (!enabled) return;
    const request = model.start(crypto.randomUUID());
    publish();
    try {
      const reply = await refreshSelectedSource(source, list, request.id);
      model.resolve(request, reply);
    } catch (error) {
      model.reject(request, error);
    }
    publish();
  }, [enabled, list, model, publish, source]);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    const unlisteners: UnlistenFn[] = [];
    const register = (pending: Promise<UnlistenFn>) => {
      pending.then((fn) => { if (active) unlisteners.push(fn); else safeUnlisten(fn); }, () => {});
    };
    register(listen<SourcePollUpdate>("source-poll-status", ({ payload }) => {
      if (!active || payload.source.provider !== "gitlab" || payload.source.host !== host || payload.list !== list) return;
      model.accept(payload);
      publish();
    }));
    register(listen("refresh-requested", () => { if (active) void refresh(); }));

    let needsRefresh = true;
    getSourceSnapshot(source, list).then((snapshot) => {
      if (!active) return;
      model.seed(snapshot.data);
      needsRefresh = snapshot.data.state !== "git_lab_available" || snapshot.data.stale_secs !== null || model.snapshot().rows === undefined;
      publish();
    }).catch(() => {
      // The live request supplies the actionable failure. A disconnected
      // phone must not turn a cache read failure into a measured empty list.
    }).finally(() => { if (active && needsRefresh) void refresh(); });

    const timer = window.setInterval(() => {
      if (!active) return;
      model.tick();
      publish();
      if (IS_MOBILE_BUILD) void refresh();
    }, 60_000);
    return () => {
      active = false;
      window.clearInterval(timer);
      for (const unlisten of unlisteners) safeUnlisten(unlisten);
    };
  }, [enabled, host, list, model, publish, refresh, source]);

  if (viewer === null) return { rows: undefined, coverage: null, staleSecs: null, loading: false, refreshing: false, error: "GitLab account has not been verified.", refresh };
  return { ...(state.source === source && state.list === list && state.model === model ? state.snapshot : model.snapshot()), refresh };
}
