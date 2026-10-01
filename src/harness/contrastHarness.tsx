// Synthetic data, real components and compiled production CSS. No remote IO.
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { mockIPC } from '@tauri-apps/api/mocks';
import { Button } from '../components/ui/button';
import { Tabs, TabsList, TabsTrigger } from '../components/ui/tabs';
import { ToolVersions } from '../components/ToolVersions';
import { StatsSidebar } from '../components/StatsSidebar';
import { BulkBar } from '../components/BulkBar';
import { ReportDialog } from '../components/ReportDialog';
import { TranscriptHeader } from '../components/transcript/TranscriptHeader';
import { useFilters } from '../store/filters';
import { PR_FIXTURES } from '../fixtures/prs';
import { prKey } from '../lib/prIdentity';
import type { ClaudeSession, ClaudeSessionDetail } from '../types/pr';
import '../index.css';
const mode = new URLSearchParams(location.search).get('case') ?? 'controls';
const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
qc.setQueryData(['stats-tree'], { viewer: 'fixture', orgs: [], orgsTotal: 0, personal: [], personalTotal: 0, refusedFields: 0 });
mockIPC((cmd) => {
    if (cmd === 'tool_versions')
        return [{ name: 'git', version: { state: 'tooOld', found: '2.30', required: '2.41' }, path: '/fixture/git', matters: 'worktrees' }];
    if (cmd === 'get_ui_prefs')
        return { terminal_command: '', hidden_views: [] };
    if (cmd === 'list_worktrees')
        return { repos: [], unreadable: [] };
    if (cmd === 'plugin:app|version')
        return 'fixture';
    if (cmd === 'plugin:event|listen')
        return 1;
    if (cmd === 'plugin:event|unlisten')
        return;
    throw new Error(`unavailable synthetic command: ${cmd}`);
}, { shouldMockEvents: true });
const prs = PR_FIXTURES.slice(0, 2).map(p => ({ ...p, is_draft: false }));
useFilters.setState({ view: 'pr-stats', checked: prs.map(prKey) });
useFilters.getState().setStatsScope('all', 'fixture', undefined);
const session: ClaudeSession = { session_id: 'fixture', name: 'Fixture', opening_prompt: `First line\n${'long-unbroken-'.repeat(30)} ⟦hidden:token⟧ last line`, cwd: '/fixture', git_branch: 'main', last_activity_at: '2026-10-01T10:00:00Z', liveness: { state: 'dead', why: 'fixture' }, cwd_state: { state: 'exists' }, kind: { kind: 'own' }, subagents: 0, waiting: { state: 'no', reason: 'never-observed' }, context_pressure: null };
const detail: ClaudeSessionDetail = { session_id: 'fixture', claude_version: null, transcript_path: null, first_seen_at: '2026-10-01T09:00:00Z', liveness: session.liveness, transcript_state: { state: 'not-recorded' }, resume: { command: 'claude --resume fixture', anchored: true, caveat: null }, runs: 1, registry_failure: null, kind: { kind: 'own' }, subagents: [], parent: null, unattributed: null, compactions: null, agent_types: null, waiting: session.waiting };
createRoot(document.getElementById('root')!).render(<QueryClientProvider client={qc}><main className="h-full overflow-auto bg-[#161b22] p-4 text-[#e6edf3]">
  {mode === 'report' ? <ReportDialog context={{ error: 'Synthetic failure' }} onClose={() => { }}/> : mode === 'bulk' ? <BulkBar prs={prs}/> : <>
    <div data-contract="buttons" className="flex flex-wrap gap-3">{(['default', 'secondary', 'outline', 'ghost', 'destructive', 'link'] as const).map(variant => <Button key={variant} variant={variant}>{variant}</Button>)}<Button disabled>Disabled</Button></div>
    <Tabs defaultValue="one"><TabsList><TabsTrigger value="one">Selected tab</TabsTrigger><TabsTrigger value="two">Other tab</TabsTrigger></TabsList></Tabs>
    <ToolVersions /><StatsSidebar viewCounts={{ 'my-prs': 12 }}/>
    <TranscriptHeader session={session} detail={detail} now={Date.parse('2026-10-01T11:00:00Z')} variant="phone"/>
    <TranscriptHeader session={{ ...session, opening_prompt: 'WITHHELD_SENTINEL' }} detail={detail} now={0} variant="phone" withheld/>
  </>}
</main></QueryClientProvider>);
