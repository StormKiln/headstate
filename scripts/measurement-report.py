#!/usr/bin/env python3
"""Bounded offline schema-1 report inspection (#1738).

An export can be structurally valid while omitting the evidence needed to explain
an incident. Never turn missing producers, sampled comparisons or lost intervals
into a healthy/complete verdict. No network, private input echo or inferred clocks.
"""
import json
import math
from pathlib import Path
import re
import statistics
import sys

MAX_FILE = 129 * 1024 * 1024
MAX_HEADER = 65536
MAX_RECORD = 1024
MAX_RECORDS = 500000
MAX_CAPTURES = 64
MAX_SCOPES = 256
MAX_CLIENT_POPULATIONS = 256
MAX_SERIES = 256
MAX_SAMPLES = 256
DOMAINS = ['queue', 'stats', 'stop_failure', 'transcript', 'client', 'read_transport']
ROLE = 'desktop phone'
PLATFORM = 'macos windows linux ios android unknown'
OUTCOME = 'success failed refused canceled coalesced cache_reuse not_issued unknown unsupported unmeasured'
STATS = 'registered registration_failed committed commit_failed accepted retained rejected cache_reuse no_work unknown unsupported'
PHASE = 'read follow page render raf_proxy evict hidden idle'
AGGREGATE = 'admitted refused canceled completed active max_concurrency tick idle declined continuation no_work candidates hidden read growth eviction cache_reuse coalesced not_issued upsert'
WORK = 'foreground background unknown'
FAILURE = 'hook_observed boundary_observed matched unpaired out_of_window duplicate clock_anomaly censored ambiguous'
PROVENANCE = 'ingested_hook sampled_idle candidate_pair retrospective_pair'
LOSS = 'dropped invalid cardinality stale_handle budget coalesced clock_anomaly overflow writer malformed unclean_capture durable_gap_records durable_gap_bytes durable_gap_segments deferred_aggregate_gaps rotated_out rotated_bytes'.split()
CAPS = 'keys key_bytes data control operations aggregates routine_per_minute transitions_per_minute failures_per_minute segments segment_bytes'.split()
TOKEN = {'epoch': 'epoch', 'capture': 'u64', 'id': 'u64'}
# A trailing ? denotes an optional/nullable reading. It never implies zero.
EVENTS = {
 'queue_receipt': {'owner':TOKEN,'receipt?':TOKEN,'list':'authored reviewing unknown','operation?':TOKEN,'revision':'u64','receipt_revision?':'u64','phase':'not_requested fetching ready partial unknown retrying failed not_asked','coverage':'complete partial unknown','rows':'u32','receipt_age_ms?':'u64','outcome':'accepted rejected_older rejected_ownership rejected_cas no_work unknown'},
 'operation': {'operation_class?':'detail action','operation':TOKEN,'parent?':TOKEN,'domain':' '.join(DOMAINS),'stage':'started submitted acknowledged published completed','outcome':OUTCOME,'elapsed_ms?':'u64','affected_fields?':'u16'},
 'stats_progress': {'scope':TOKEN, **{f'{k}?':'u16' for k in ['days','covered_days','partial_days','unknown_days']}, **{f'{k}?':'u32' for k in ['retrieved','accumulated','total']}, 'outcome':STATS,'elapsed_ms?':'u64'},
 'stop_failure_match': {'session':TOKEN,'observed_boundary?':'u32','observation':PROVENANCE,'delta_ms?':'i64','observed_lag_ms?':'i64','hook_age_ms?':'u64','outcome':FAILURE},
 'transcript': {'operation':TOKEN,'phase':PHASE,'elapsed_ms?':'u64','bytes?':'u32','rows?':'u32','resident_rows?':'u32','capability':'measured unsupported unmeasured'},
 'aggregate': {'domain':' '.join(DOMAINS),'metric':AGGREGATE,'work':WORK,'count':'u64','interval_ms':'u64','coalesced':'bool'},
}
CLIENTS = {
 'mounted_review': {'source':'github gitlab unknown','list':'authored reviewing unknown','surface':'ready_panel review_list','selection':'all_repositories selected_scope unknown','receipt?':TOKEN,'scope?':TOKEN, **{f'{k}?':'u32' for k in ['inventory_count','eligible_count','visible_count','visible_retained_count','visible_retained_readiness_count','visible_readiness_unknown_count','visible_last_known_count','visible_advisory_unavailable_count']}, 'footer_location':'desktop_footer phone_banner hidden unmeasured','footer':'checked needs_checking partly_checked coverage_unknown not_checked checking retrying load_failed refresh_failed background_stopped auth_unavailable auth_unknown legacy_up_to_date hidden unavailable'},
 'stats_view': {'observation?':'readback mounted','scope?':TOKEN,'outcome':STATS,'elapsed_ms?':'u64','rows?':'u32'},
 'transcript_view': {'operation?':TOKEN,'phase':PHASE,'elapsed_ms?':'u64','rows?':'u32','resident_rows?':'u32','capability':'measured unsupported unmeasured'},
}
RANGE = {'records':'u64','oldest_wall_ms?':'u64','newest_wall_ms?':'u64'}
HEADER = {'kind':'header','schema':'schema','build':'build','platform':PLATFORM,'role':ROLE,'cutoff_seq':'u64','cutoff_epoch':'epoch',**RANGE,'loss':{**{k+'?':'u64' for k in LOSS},'by_domain':'domains'},'caps':{k:'u64' for k in CAPS},'epochs':'u32','captures':'u32','incomplete':'bool','deferred':'deferred','lifecycle':{'captures_opened':'u64','captures_closed':'u64','active_capture?':'u64'},'omitted_prefix?':RANGE}
TRAILER = {'kind':'trailer','schema':'schema','records':'u64','cutoff_seq':'u64','complete':'bool','incomplete':'bool'}
ENVELOPE = {'schema':'schema','epoch':'epoch','capture':'u64','seq':'u64','monotonic_ms':'u64','wall_time_ms':'u64','role':ROLE,'event':'event'}

class Invalid(ValueError):
    pass

def closed(obj, spec):
    if not isinstance(obj, dict) or obj.keys() - {k.rstrip('?') for k in spec}:
        raise Invalid()
    for key, rule in spec.items():
        name = key.rstrip('?')
        if key.endswith('?') and obj.get(name) is None:
            continue
        if name not in obj:
            raise Invalid()
        validate(obj[name], rule)

def tagged(value, specs):
    if not isinstance(value, dict) or value.get('kind') not in specs:
        raise Invalid()
    closed({k:v for k,v in value.items() if k!='kind'}, specs[value['kind']])

def validate(value, rule):
    if isinstance(rule, dict):
        closed(value, rule)
    elif rule in ('u64','u32','u16','i64'):
        bits = int(rule[1:]); low = -(2**63) if rule=='i64' else 0; high = 2**63-1 if rule=='i64' else 2**bits-1
        if type(value) is not int or not low <= value <= high: raise Invalid()
    elif rule == 'bool':
        if type(value) is not bool: raise Invalid()
    elif rule == 'schema':
        if type(value) is not int or value != 1: raise Invalid()
    elif rule == 'epoch':
        if not isinstance(value,str) or not re.fullmatch('[0-9a-f]{32}',value): raise Invalid()
    elif rule == 'build':
        if not isinstance(value,str) or len(value.encode('utf8')) > 128: raise Invalid()
    elif rule == 'domains':
        if not isinstance(value,list) or len(value) not in (5,6): raise Invalid()
        for n in value: validate(n,'u64')
    elif rule == 'deferred':
        if not isinstance(value,list) or len(value)>128: raise Invalid()
        for item in value: closed(item,{'domain':' '.join(DOMAINS),'metric':AGGREGATE,'work':WORK,'count':'u64','elapsed_ms':'u64'})
    elif rule == 'event':
        if isinstance(value,dict) and value.get('kind')=='client':
            closed({k:v for k,v in value.items() if k!='kind'},{'observation':'client'})
        else: tagged(value,EVENTS)
    elif rule == 'client':
        tagged(value,CLIENTS)
    elif not isinstance(value,str) or value not in rule.split():
        raise Invalid()

def unique(pairs):
    obj={}
    for k,v in pairs:
        if k in obj: raise Invalid()
        obj[k]=v
    return obj

def decode(line):
    try:
        return json.loads(line.decode('utf8'),object_pairs_hook=unique,parse_constant=lambda _: (_ for _ in ()).throw(Invalid()))
    except (ValueError,UnicodeError,RecursionError,OverflowError):
        raise Invalid() from None

def tokens(value):
    if isinstance(value,dict):
        if set(value)==set(TOKEN): yield value
        else:
            for v in value.values(): yield from tokens(v)

def distribution(values, missing, seen):
    out={'measured':seen,'missing':missing,'retained_samples':len(values)}
    if values:
        values=sorted(values);out.update(min=values[0],median=statistics.median(values),max=values[-1])
        if len(values)>=20: out['p95']=values[math.ceil(.95*len(values))-1]
        if len(values)>=100: out['p99']=values[math.ceil(.99*len(values))-1]
    out['prefix_samples_only']=seen>len(values)
    return out

class Analysis:
    def __init__(self):
        self.issues=set();self.invalid=False;self.groups={};self.series_count=0;self.scope_count=0;self.client_population_count=0;self.domains=set();self.validated=0;self.body=0;self.skipped=0
    def issue(self, code, invalid=False):
        self.issues.add(code);self.invalid |= invalid
    def client_population(self, group, event):
        kind = event['kind']
        dimensions = ('observation', 'outcome') if kind == 'stats_view' else (
            'source', 'list', 'surface', 'selection', 'footer_location', 'footer')
        scope = event.get('scope')
        # Equality is used only within this already validated role/epoch/capture.
        # Receipt identity is not a substitute for unavailable selected-scope identity.
        scope_key = (kind, scope['id']) if scope else None
        key = (kind, scope_key, tuple(event.get(field) for field in dimensions))
        populations = group['client_populations']
        if key in populations:
            return populations[key]['ordinal']
        if self.client_population_count >= MAX_CLIENT_POPULATIONS:
            self.issue('client_population_capacity'); self.skipped += 1
            return None
        scope_ordinal = None
        if scope_key is not None:
            if scope_key not in group['client_scope_ordinals']:
                if self.scope_count >= MAX_SCOPES:
                    self.issue('client_scope_capacity'); self.skipped += 1
                    return None
                self.scope_count += 1
                group['client_scope_ordinals'][scope_key] = self.scope_count
            scope_ordinal = group['client_scope_ordinals'][scope_key]
        else:
            self.issue('client_identity_unavailable')
        self.client_population_count += 1
        populations[key] = dict(kind=kind, ordinal=self.client_population_count,
            scope_ordinal=scope_ordinal, identity='linked_scope' if scope else 'unlinked_mixed',
            dimensions={field:event.get(field) for field in dimensions})
        return self.client_population_count

    def observe(self, r):
        self.validated+=1
        if any(t['epoch']!=r['epoch'] or t['capture']!=r['capture'] for t in tokens(r['event'])):
            self.issue('foreign_token',True);return
        key=(r['role'],r['epoch'],r['capture'])
        if key not in self.groups:
            if len(self.groups)>=MAX_CAPTURES: self.skipped+=1;self.issue('capture_capacity');return
            self.groups[key]={'role':r['role'],'capture_ordinal':len(self.groups)+1,'events':{},'series':{},'scopes':{},'client_populations':{},'client_scope_ordinals':{},'aggregates':{},'last_seq':0,'last_mono':None,'records':0}
        g=self.groups[key]
        if r['seq']<=g['last_seq']: self.issue('duplicate_or_out_of_order');return
        if g['last_seq'] and r['seq']>g['last_seq']+1:self.issue('sequence_gap')
        g['last_seq']=r['seq']
        if g['last_mono'] is not None and r['monotonic_ms']<g['last_mono']:self.issue('monotonic_regression')
        g['last_mono']=r['monotonic_ms'];g['records']+=1
        e=r['event'];kind=e['kind'];domain={'queue_receipt':'queue','stats_progress':'stats','stop_failure_match':'stop_failure','transcript':'transcript','client':'client'}.get(kind,e.get('domain'))
        self.domains.add(domain)
        if kind=='client':e=e['observation'];kind=e['kind']
        label=kind+':'+str(e.get('observation') or e.get('operation_class') or e.get('phase') or e.get('metric') or 'unspecified')+':'+str(e.get('outcome') or e.get('footer') or e.get('capability') or 'observed')
        if kind=='operation':label += ':'+e['domain']+':'+e['stage']
        if kind=='queue_receipt':label += ':'+e['list']
        if kind in ('stats_view','mounted_review'):
            population = self.client_population(g, e)
            if population is None:return
            label += ':population_'+str(population)
        g['events'][label]=g['events'].get(label,0)+1
        for field in ('elapsed_ms','delta_ms','observed_lag_ms','hook_age_ms','rows','bytes','resident_rows','inventory_count','eligible_count','visible_count','visible_retained_count','visible_retained_readiness_count','visible_readiness_unknown_count','visible_last_known_count','visible_advisory_unavailable_count','affected_fields'):
            spec=CLIENTS.get(kind,EVENTS.get(kind,{}))
            if field not in spec and field+'?' not in spec:continue
            metric=label+':'+field
            if metric not in g['series']:
                if self.series_count>=MAX_SERIES:self.issue('series_capacity');continue
                self.series_count+=1;g['series'][metric]=[[],0,0]
            series=g['series'][metric];v=e.get(field)
            if v is None:series[1]+=1
            else:
                series[2]+=1
                if len(series[0])<MAX_SAMPLES:series[0].append(v)
                else:self.issue('sample_capacity')
        if kind in ('stats_progress','queue_receipt'):
            token=e['scope'] if kind=='stats_progress' else e['owner']
            skey=(kind,token['id'],e.get('list'))
            if skey not in g['scopes']:
                if self.scope_count>=MAX_SCOPES:self.issue('scope_capacity');return
                self.scope_count+=1;g['scopes'][skey]={'kind':kind,'ordinal':self.scope_count,'observations':0,'first':None,'last':None}
            scope=g['scopes'][skey];scope['observations']+=1
            # Rejected attempts do not advance accepted inventory evidence.
            if kind=='queue_receipt' and e['outcome']!='accepted':return
            fields=('days','covered_days','partial_days','unknown_days','retrieved','accumulated','total','outcome') if kind=='stats_progress' else ('revision','receipt_revision','rows','phase','coverage')
            snapshot={f:e.get(f) for f in fields}
            if scope['first'] is None:scope['first']=snapshot
            scope['last']=snapshot
        if kind=='aggregate':
            name=e['domain']+':'+e['metric']+':'+e['work'];a=g['aggregates'].setdefault(name,{'summaries':0,'reported_count_sum':0,'max_reported_count':0,'coalesced_summaries':0,'last_end':None,'overlap':False})
            start=r['monotonic_ms']-e['interval_ms']
            if start<0 or (a['last_end'] is not None and start<a['last_end']):a['overlap']=True;self.issue('aggregate_interval_overlap')
            a['last_end']=r['monotonic_ms'];a['summaries']+=1;a['reported_count_sum']=(a['reported_count_sum']+e['count']) if a['reported_count_sum'] is not None else None;a['max_reported_count']=max(a['max_reported_count'],e['count']);a['coalesced_summaries']+=int(e['coalesced'])
            if e['metric'] in ('active','max_concurrency') or a['overlap']:a['reported_count_sum']=None

    def output(self,h=None):
        captures=[]
        for (_,epoch,_),g in self.groups.items():
            captures.append({k:v for k,v in g.items() if k not in ('series','scopes','client_populations','client_scope_ordinals','last_seq','last_mono')})
            captures[-1].update(series={k:distribution(*v) for k,v in g['series'].items()},scopes=list(g['scopes'].values()),client_populations=list(g['client_populations'].values()),build_attribution='exporter_header_only' if h and epoch==h['cutoff_epoch'] else 'unknown')
            for a in captures[-1]['aggregates'].values():a.pop('last_end',None)
        return {'integrity':'invalid' if self.invalid else 'valid','quality':'qualified' if self.issues else 'unqualified','field_acceptance':False,'issues':sorted(self.issues),'records_validated':self.validated,'records_omitted_by_analysis_capacity':self.skipped,'captures':captures,'unmeasured_domains':[d for d in DOMAINS if d not in self.domains], 'metadata_loss':h.get('loss') if h else None,'metadata_lifecycle':h.get('lifecycle') if h else None,'deferred_untimed_summaries':h.get('deferred',[]) if h else [],'omitted_prefix':h.get('omitted_prefix') if h else None,'export_range':{k:h[k] for k in ('oldest_wall_ms','newest_wall_ms','records')} if h else None,'method':'median is midpoint of central values; p95/p99 nearest-rank only at n>=20/100; bounded first256 samples per series; no cross-clock subtraction or inferred lineage','limits':{'file_bytes':MAX_FILE,'records':MAX_RECORDS,'captures':MAX_CAPTURES,'series':MAX_SERIES,'scopes':MAX_SCOPES,'client_populations':MAX_CLIENT_POPULATIONS,'samples_per_series':MAX_SAMPLES},'cautions':['Missing domains are unmeasured, not healthy.','Counters in export metadata are one cumulative snapshot, not per-capture loss.','Overlapping Ready qualifiers are not a partition.','StopFailure is sampled/censored; eviction units are retained hook/boundary entries, not lost records.','RAF is a scheduling proxy; native read and client await are separate populations.','No exact lineage or physical field acceptance is inferred.']}

def analyze(path):
    a=Analysis();h=None;trailer=None;total=0;oldest=None;newest=None;previous=None;epochs=0;captures=0
    try:
        with Path(path).open('rb') as source:
            line=source.readline(MAX_HEADER+1);total+=len(line)
            if len(line)>MAX_HEADER or not line.endswith(b'\n'):raise Invalid()
            h=decode(line);closed(h,HEADER)
            omitted=h.get('omitted_prefix')
            if omitted is not None:
                low,high=omitted.get('oldest_wall_ms'),omitted.get('newest_wall_ms')
                if (omitted['records']==0 and (low is not None or high is not None)) or (omitted['records']>0 and (low is None or high is None or low>high or not h['incomplete'])):a.issue('omission_metadata_mismatch',True)
            if h['incomplete'] or any(h['loss'].get(k,0) for k in LOSS if k!='coalesced') or any(h['loss']['by_domain']):a.issue('export_loss_or_incomplete')
            if h.get('omitted_prefix',{}).get('records',0):a.issue('omitted_prefix')
            if h['loss'].get('coalesced',0):a.issue('coalesced_observations')
            if h['deferred']:a.issue('deferred_untimed_summaries')
            while True:
                line=source.readline(MAX_RECORD+1)
                if not line:break
                total+=len(line)
                if total>MAX_FILE or len(line)>MAX_RECORD or not line.endswith(b'\n'):a.issue('input_bound_or_truncated_line',True);break
                try:
                    r=decode(line)
                    if trailer is not None:raise Invalid()
                    if isinstance(r,dict) and r.get('kind')=='trailer':closed(r,TRAILER);trailer=r;continue
                    a.body+=1
                    if a.body>MAX_RECORDS:a.issue('record_capacity',True);break
                    closed(r,ENVELOPE)
                    if r['epoch']==h['cutoff_epoch'] and r['seq']>h['cutoff_seq']:a.issue('beyond_cutoff',True)
                    pair=(r['epoch'],r['capture'])
                    if previous is None or previous[0]!=pair[0]:epochs+=1
                    if previous!=pair:captures+=1
                    previous=pair
                    oldest=r['wall_time_ms'] if oldest is None else min(oldest,r['wall_time_ms'])
                    newest=r['wall_time_ms'] if newest is None else max(newest,r['wall_time_ms'])
                    a.observe(r)
                except (Invalid,TypeError,RecursionError):a.issue('invalid_record',True)
            if trailer is None:a.issue('missing_trailer',True)
            elif not trailer['complete'] or any(trailer[k]!=h[k] for k in ('records','cutoff_seq','incomplete')):a.issue('trailer_mismatch',True)
            if a.body!=h['records'] or (oldest,newest)!=(h.get('oldest_wall_ms'),h.get('newest_wall_ms')) or (epochs,captures)!=(h['epochs'],h['captures']):a.issue('header_population_mismatch',True)
    except (OSError,Invalid,TypeError,RecursionError,UnicodeError):a.issue('unreadable_or_invalid_header',True);h=None
    return a.output(h)

def main(argv):
    if len(argv) not in (1,2) or (len(argv)==2 and argv[1]!='--json'):
        print('Usage: measurement-report.py INPUT [--json]',file=sys.stderr);return 2
    result=analyze(argv[0])
    if len(argv)==2:print(json.dumps(result,indent=2,sort_keys=True))
    else:
        print('Measurement report: '+result['integrity']+', '+result['quality']+'; not field acceptance')
        print('Validated records:',result['records_validated'])
        print('Unmeasured domains:',', '.join(result['unmeasured_domains']) or 'none among recorded categories')
        print('Qualifications:',', '.join(result['issues']) or 'none detected in exported structure')
        for group in result['captures']:
            print('\nCapture',group['capture_ordinal'],group['role'],'records',group['records'],'build',group['build_attribution'])
            for population in group['client_populations']:print(' Client population',json.dumps(population,sort_keys=True))
            for category,n in sorted(group['events'].items()):print(' ',category,n)
            for name,v in sorted(group['series'].items()):print(' ',name,json.dumps(v,sort_keys=True))
            for scope in group['scopes']:print(' ',scope['kind'],'scope',scope['ordinal'],json.dumps(scope,sort_keys=True))
            for name,v in sorted(group['aggregates'].items()):print(' ',name,json.dumps(v,sort_keys=True))
        print('\nMetadata loss (whole exported population):',json.dumps(result['metadata_loss'],sort_keys=True))
        print('Omitted prefix:',json.dumps(result['omitted_prefix'],sort_keys=True))
        print('Deferred untimed summaries:',json.dumps(result['deferred_untimed_summaries'],sort_keys=True))
        print(result['method'])
        for warning in result['cautions']:print(warning)
    return 2 if result['integrity']=='invalid' else (1 if result['quality']=='qualified' else 0)
if __name__=='__main__':sys.exit(main(sys.argv[1:]))
