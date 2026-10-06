import { captureReference } from "./measurementCapture";
import { useEffect, useSyncExternalStore } from "react";
import type { QueryClient } from "@tanstack/react-query";
import type { StatsBackfillFrame, StatsBoard, StatsOwner, StatsWindow } from "@/types/pr";
import { statsBoardCached } from "./tauri";
import { listen, type UnlistenFn } from "./transport";
import { observeStats } from "./statsMeasurement";
import { safeUnlisten } from "./unlisten";
import { commandError } from "@/lib/errorKind";

type Question = { scopeKind: string; scopeValue?: string; measure: "merged" | "opened"; days: number };
type Status = { refreshing: boolean; error?: string; unsupported?: boolean; retained?: boolean };
const owners = new WeakMap<QueryClient, { generation: number; unsupported: boolean; listeners: Set<() => void>; entries: Map<string, Controller> }>();
function registry(qc: QueryClient) {
  let value = owners.get(qc);
  if (!value) {
    value = { generation: 0, unsupported: false, listeners: new Set(), entries: new Map() };
    owners.set(qc, value);
    const state = value;
    // Query GC also releases inactive controllers. The one capability refusal
    // stays with the backend generation, not with any individual question.
    qc.getQueryCache().subscribe(event => {
      if (event.type !== "removed") return;
      const id = JSON.stringify(event.query.queryKey);
      state.entries.get(id)?.retire();
      state.entries.delete(id);
    });
  }
  return value;
}
export const statsOwnership = (qc: QueryClient) => registry(qc).generation;
export function retireStatsOwnership(qc: QueryClient) {
  const value = registry(qc); value.generation++; value.unsupported = false;
  for (const entry of value.entries.values()) entry.retire();
  value.entries.clear();
  for (const listener of value.listeners) listener();
}
const sameOwner = (a: StatsOwner | undefined, b: StatsOwner | undefined) => !!a && !!b && a.viewer === b.viewer && a.generation === b.generation;
const sameWindow = (a: StatsWindow | undefined, b: StatsWindow | undefined) => !!a && !!b && a.from === b.from && a.to === b.to;
const population = (b: StatsBoard) => b.accumulating ? b.accumulated : b.retrieved;
function entry(qc: QueryClient, key: readonly unknown[]) {
  const value = registry(qc), id = JSON.stringify(key);
  let result = value.entries.get(id);
  if (!result) { result = new Controller(qc, key); value.entries.set(id, result); }
  return result;
}
/** Normal loads and cache reads cannot publish over a newer normal request.
 * Cache reads wait for TanStack's normal load to settle before starting. */
export async function readStatsBoard(qc: QueryClient, key: readonly unknown[], work: () => Promise<StatsBoard>) {
  const value = entry(qc, key), version = ++value.normalVersion;
  const result = await work();
  // Ordinary observer cleanup (including StrictMode replay) does not retire
  // TanStack's normal request. Ownership retirement does.
  if (value.retired || version !== value.normalVersion) throw new DOMException("Stats owner retired", "AbortError");
  return {...result, measurementScope:captureReference(qc, result.measurementScope)};
}
class Controller {
  epoch = 0; normalVersion = 0; retired = false;
  question?: Question;
  private users = 0;
  private status: Status = { refreshing: false };
  private listeners = new Set<() => void>();
  private stop?: () => void;
  private inFlight = false;
  private scheduled = false;
  private dirty = 0;
  private applied = 0;
  private attempts = 0;
  private retryTimer?: ReturnType<typeof setTimeout>;
  private midnight?: ReturnType<typeof setTimeout>;
  private stream?: string;
  private owner?: StatsOwner;
  private highWater = 0;
  private cacheChange = -1;
  private foreignHandshake = false;
  private initialHint?: StatsBackfillFrame;
  private retiredStreams = new Set<string>();
  constructor(private qc: QueryClient, private key: readonly unknown[]) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  snapshot = () => this.status;
  private publish(value: Status) { this.status = value; for (const listener of this.listeners) listener(); }
  private query() { return this.qc.getQueryCache().get<StatsBoard>(this.qc.defaultQueryOptions({queryKey:this.key}).queryHash); }
  private board() { return this.query()?.state.data; }
  private adopt(board: StatsBoard) {
    if (!sameOwner(this.owner, board.owner)) {
      this.owner = board.owner; this.highWater = 0; this.cacheChange = -1; this.foreignHandshake = false; this.retiredStreams.clear();
    }
    if (board.stream && board.stream !== this.stream) {
      if (this.stream) this.retiredStreams.add(this.stream);
      if (this.retiredStreams.size > 8) this.retiredStreams.delete(this.retiredStreams.values().next().value!);
      this.stream = board.stream; this.highWater = 0; this.cacheChange = -1; this.foreignHandshake = false;
    }
  }
  private frame = (frame: StatsBackfillFrame) => {
    const board = this.board(), observation = frame.observation;
    if (!observation || this.retired) return;
    if (!board) {
      // A final event can precede the initial normal reply. Keep one bounded
      // hint, not its numeric authority; the eventual exact-question read is
      // owner-checked by the native command. Dirty work waits for that reply.
      if (!this.initialHint) this.request();
      const previous = this.initialHint?.observation;
      if (!previous || previous.stream !== observation.stream || observation.sequence > previous.sequence) this.initialHint = frame;
      return;
    }
    if (!sameOwner(board.owner, frame.owner) || frame.scopeKey !== board.scopeKey || this.retired) return;
    this.adopt(board);
    if (board.window && (observation.to < board.window.from || observation.from > board.window.to)) return;
    if (observation.stream !== this.stream) {
      if (!this.retiredStreams.has(observation.stream) && !this.foreignHandshake) { this.foreignHandshake = true; this.request(); }
      return;
    }
    if (observation.sequence <= this.highWater) return;
    this.highWater = observation.sequence;
    if (observation.cacheChange <= this.cacheChange) return;
    this.cacheChange = observation.cacheChange;
    this.request();
  };
  retry = () => { if (!this.status.unsupported) this.request(); };
  private request() {
    if (this.retired || registry(this.qc).unsupported) return;
    this.dirty++; this.attempts = 0;
    clearTimeout(this.retryTimer); this.retryTimer = undefined;
    this.schedule();
  }
  private schedule() {
    if (this.scheduled) return;
    this.scheduled = true;
    queueMicrotask(() => { this.scheduled = false; void this.pump(); });
  }
  private async pump() {
    const query = this.query(), board = query?.state.data, question = this.question;
    if (this.retired || !this.users || this.inFlight || this.retryTimer || registry(this.qc).unsupported || document.visibilityState === "hidden" || this.dirty <= this.applied || !board?.owner || !question || query?.state.fetchStatus !== "idle") return;
    this.adopt(board);
    const epoch = this.epoch, normalVersion = this.normalVersion, dirty = this.dirty;
    this.inFlight = true; this.publish({...this.status,refreshing:true});
    let succeeded = false;
    const measurementStarted = performance.now();
    const rejected = () => observeStats(this.qc, "readback", "rejected", undefined, performance.now()-measurementStarted);
    try {
      const answer = await statsBoardCached(question.scopeKind, question.scopeValue, question.measure, question.days, board.owner);
      if (this.retired || epoch !== this.epoch || this.query() !== query || normalVersion !== this.normalVersion || query.state.fetchStatus !== "idle") { rejected(); return; }
      const current = this.board();
      if (!current || !sameOwner(current.owner, answer.owner) || answer.scopeKey !== current.scopeKey) { rejected(); return; }
      if (current.window && (answer.window.to < current.window.to || (answer.window.to === current.window.to && answer.window.from < current.window.from))) { rejected(); return; }
      const measured = {...current,...answer.measurement,measurementScope:captureReference(this.qc, answer.measurementScope),owner:answer.owner,viewer:answer.viewer,scopeKey:answer.scopeKey,window:answer.window,stream:answer.stream};
      // Backfill registration belongs to the normal load, not this readback.
      // Weaker same-window evidence cannot erase useful foreground-only rows.
      const retain = sameWindow(current.window, answer.window) && population(current) > 0
        && ((!current.accumulating && (!measured.complete || population(measured) < population(current)))
          || (!measured.complete && population(measured) < population(current)));
      const next = retain ? {...current,stream:answer.stream,window:answer.window} : measured;
      this.qc.setQueryData(this.key, next);
      observeStats(this.qc, "readback", retain ? "retained" : "accepted", next, performance.now()-measurementStarted);
      this.adopt(next); this.applied = dirty; this.attempts = 0; succeeded = true;
      this.publish({refreshing:true,retained:retain});
    } catch (error) {
      if (this.retired || epoch !== this.epoch) { rejected(); return; }
      const unsupported = /not a Headstate command|unknown command|command.*not found/i.test(commandError(error).message);
      if (unsupported) {
        const state = registry(this.qc); state.unsupported = true;
        for (const listener of state.listeners) listener();
      }
      observeStats(this.qc,"readback",unsupported?"unsupported":"rejected",undefined,performance.now()-measurementStarted);
      this.publish({refreshing:true,error:unsupported?"This desktop does not support local Stats refresh.":"Stored Stats refresh failed; previous measurements are retained.",unsupported});
      if (!unsupported && this.dirty === dirty && this.attempts < 2) {
        const delay = [5_000,30_000][this.attempts++];
        this.retryTimer = setTimeout(() => { this.retryTimer=undefined; this.schedule(); },delay);
      }
    } finally {
      this.inFlight=false;
      if (!this.retired) {
        // The old observer generation owns the pending command until settlement.
        // Its answer is fenced above; a remounted observer gets one trailing read.
        this.publish({...this.status,refreshing:false});
        if (epoch !== this.epoch || (succeeded && this.dirty > this.applied) || this.dirty > dirty || this.normalVersion !== normalVersion) this.schedule();
      }
    }
  }
  private armMidnight() {
    clearTimeout(this.midnight);
    const now=new Date(), next=Date.UTC(now.getUTCFullYear(),now.getUTCMonth(),now.getUTCDate()+1);
    this.midnight=setTimeout(()=>{this.request();this.armMidnight();},Math.max(1,next-Date.now()+10));
  }
  connect() {
    if (this.users++ > 0) return () => this.disconnect();
    const epoch=this.epoch, unlisteners:UnlistenFn[]=[];
    let closed=false, previousConnection:string|undefined;
    const register=(promise:Promise<UnlistenFn>)=>{void promise.then(fn=>{if(closed)safeUnlisten(fn);else unlisteners.push(fn);},()=>{});};
    register(listen<StatsBackfillFrame>("stats-backfill-progress",event=>{if(!closed&&epoch===this.epoch)this.frame(event.payload);}));
    register(listen<{state?:string}>("connection-state",event=>{
      if(closed||epoch!==this.epoch)return;
      const state=event.payload.state;
      if(state==='connected'&&previousConnection!=='connected')this.request();
      previousConnection=state;
    }));
    const visible=()=>{if(document.visibilityState==='hidden'){clearTimeout(this.retryTimer);this.retryTimer=undefined;}else this.request();};
    document.addEventListener('visibilitychange',visible);
    const unsubscribe=this.qc.getQueryCache().subscribe(event=>{
      if(event.query.queryHash!==this.qc.defaultQueryOptions({queryKey:this.key}).queryHash)return;
      if(event.type==='removed'){this.retire();return;}
      const board=this.board();
      if(board){
        this.adopt(board);
        const hint=this.initialHint;this.initialHint=undefined;
        if(hint)this.frame(hint);
        if(board.backfill.state==='registered'&&board.backfill.lastFrame)this.frame(board.backfill.lastFrame);}
      this.schedule();
    });
    const existing=this.board();
    if(existing){this.adopt(existing);this.request();}
    this.armMidnight();
    this.stop=()=>{closed=true;for(const fn of unlisteners)safeUnlisten(fn);unsubscribe();document.removeEventListener('visibilitychange',visible);clearTimeout(this.retryTimer);clearTimeout(this.midnight);};
    return ()=>this.disconnect();
  }
  private disconnect(){if(--this.users===0){this.stop?.();this.stop=undefined;this.epoch++;}}
  retire(){this.retired=true;this.epoch++;this.stop?.();this.stop=undefined;clearTimeout(this.retryTimer);clearTimeout(this.midnight);}
}
export function useStatsBoardRefresh(qc:QueryClient,key:readonly unknown[],question:Question,enabled:boolean){
  const owner=registry(qc);
  useSyncExternalStore(listener=>{owner.listeners.add(listener);return()=>{owner.listeners.delete(listener);};},()=>`${owner.generation}:${owner.unsupported}`);
  let value=entry(qc,key);
  if(value.retired){owner.entries.delete(JSON.stringify(key));value=entry(qc,key);}
  value.question=question;
  const status=useSyncExternalStore(value.subscribe,value.snapshot);
  useEffect(()=>enabled?value.connect():undefined,[value,enabled]);
  return {...status,unsupported:owner.unsupported,error:status.error??(owner.unsupported?"This desktop does not support local Stats refresh.":undefined),retry:value.retry};
}
