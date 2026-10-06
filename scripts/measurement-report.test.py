#!/usr/bin/env python3
"""Offline report honesty/privacy contracts; fixtures are synthetic, not producer proof."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('measurement_report', HERE / 'measurement-report.py')
mod = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mod)
EPOCH = 'a' * 32

def header(records=1, **extra):
    return dict(kind='header', schema=1, build='synthetic', platform='macos', role='desktop', cutoff_seq=records,
                cutoff_epoch=EPOCH, oldest_wall_ms=1000 if records else None, newest_wall_ms=1000 if records else None,
                records=records, loss=dict(by_domain=[0]*6), caps=dict(keys=4096,key_bytes=1048576,data=1024,control=8,operations=1024,aggregates=128,routine_per_minute=12,transitions_per_minute=120,failures_per_minute=60,segments=8,segment_bytes=16777216),
                epochs=1 if records else 0,captures=1 if records else 0,incomplete=False,deferred=[],lifecycle=dict(captures_opened=1,captures_closed=0,active_capture=1), **extra)
def event(kind='transcript', **extra):
    return dict(kind=kind,operation=dict(epoch=EPOCH,capture=1,id=1),phase='read',elapsed_ms=5,bytes=20,rows=1,resident_rows=None,capability='measured',**extra)
def envelope(ev=None, **extra):
    return dict(schema=1,epoch=EPOCH,capture=1,seq=1,monotonic_ms=10,wall_time_ms=1000,role='desktop',event=ev or event(),**extra)
def report(records=None, h=None, trailer=True):
    records = [envelope()] if records is None else records
    h = h or header(len(records))
    result = [h,*records]
    if trailer: result.append(dict(kind='trailer',schema=1,records=len(records),cutoff_seq=h['cutoff_seq'],complete=True,incomplete=h['incomplete']))
    return result
class Contracts(unittest.TestCase):
    def run_report(self, values, raw=None):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'PRIVATE_SENTINEL_INPUT.jsonl'
            path.write_bytes(raw if raw is not None else (''.join(json.dumps(v)+'\n' for v in values)).encode())
            return mod.analyze(path)
    def test_valid_is_structural_not_field_acceptance(self):
        r=self.run_report(report());self.assertEqual(r['integrity'],'valid');self.assertIn('queue',r['unmeasured_domains']);self.assertFalse(r['field_acceptance'])
    def test_missing_and_mismatched_trailer(self):
        self.assertEqual(self.run_report(report(trailer=False))['integrity'],'invalid')
        r=report();r[-1]['records']=2;self.assertEqual(self.run_report(r)['integrity'],'invalid')
    def test_private_unknown_field_enum_and_filename_never_echo(self):
        for mutate in [lambda r:r[1]['event'].update(private='PRIVATE_SENTINEL'),lambda r:r[1]['event'].update(phase='PRIVATE_SENTINEL')]:
            r=report();mutate(r);out=self.run_report(r);self.assertEqual(out['integrity'],'invalid');self.assertNotIn('PRIVATE_SENTINEL',json.dumps(out))
        p=subprocess.run([sys.executable,str(HERE/'measurement-report.py'),'PRIVATE_SENTINEL_MISSING'],capture_output=True,text=True)
        self.assertNotEqual(p.returncode,0);self.assertNotIn('PRIVATE_SENTINEL',p.stdout+p.stderr)
    def test_recent_suffix_cutoff_and_omission(self):
        h=header();h['cutoff_seq']=99;h['incomplete']=True;h['omitted_prefix']=dict(records=98,oldest_wall_ms=1,newest_wall_ms=999)
        r=envelope();r['seq']=99
        out=self.run_report(report([r],h));self.assertEqual(out['integrity'],'valid');self.assertEqual(out['quality'],'qualified');self.assertEqual(out['omitted_prefix']['records'],98)
    def test_epochs_roles_and_historical_build_separate(self):
        a=envelope();b=envelope();b.update(epoch='b'*32,role='phone');b['event']['operation']['epoch']='b'*32
        h=header(2);h.update(epochs=2,captures=2,cutoff_seq=1)
        out=self.run_report(report([a,b],h));self.assertEqual(len(out['captures']),2);self.assertEqual(out['captures'][1]['build_attribution'],'unknown')
    def test_duplicate_out_of_order_and_cutoff(self):
        for seq in [1,0,3]:
            a=envelope();b=envelope();b['seq']=seq
            out=self.run_report(report([a,b]));self.assertNotEqual(out['quality'],'unqualified')
    def test_internal_sequence_gap_stays_qualified_even_without_loss_metadata(self):
        a=envelope();b=envelope();b['seq']=3
        h=header(2);h['cutoff_seq']=3
        out=self.run_report(report([a,b],h));self.assertIn('sequence_gap',out['issues'])
    def test_unknown_counts_and_small_sample_no_tail_claim(self):
        r=envelope();r['event']['elapsed_ms']=None
        out=self.run_report(report([r]));series=out['captures'][0]['series'];self.assertTrue(any(v['missing']==1 for v in series.values()))
        out=self.run_report(report());self.assertTrue(all('p95' not in v for v in out['captures'][0]['series'].values()))
    def test_actual_client_wrapper_accepts_closed_nested_observation(self):
        ev={'kind':'client','observation':{'kind':'stats_view','observation':'mounted','scope':None,'outcome':'accepted','elapsed_ms':None,'rows':1}}
        self.assertEqual(self.run_report(report([envelope(ev)]))['integrity'],'valid')
    def test_missing_domains_are_not_healthy(self):
        r=self.run_report(report([],header(0)));self.assertEqual(r['unmeasured_domains'],mod.DOMAINS);self.assertEqual(r['integrity'],'valid')
    def test_bounds_and_duplicate_json_keys(self):
        self.assertEqual(self.run_report([],raw=b'{"schema":1,"schema":1}\n')['integrity'],'invalid')
        self.assertEqual(self.run_report([],raw=b'x'* (mod.MAX_HEADER+1))['integrity'],'invalid')
    def test_loss_coalescing_are_not_fake_timestamps(self):
        h=header();h['loss'].update(dropped=2,coalesced=9);h['incomplete']=True
        out=self.run_report(report(h=h));self.assertEqual(out['quality'],'qualified');self.assertEqual(out['metadata_loss']['coalesced'],9);self.assertEqual(out['records_validated'],1)
    def test_aggregate_intervals_and_gauges_are_not_cumulative_snapshots(self):
        for metric in ['admitted','max_concurrency']:
            records=[]
            for i in range(3):
                r=envelope(dict(kind='aggregate',domain='read_transport',metric=metric,work='background',count=4,interval_ms=10,coalesced=True));r.update(seq=i+1,monotonic_ms=i+10);records.append(r)
            out=self.run_report(report(records));a=next(iter(out['captures'][0]['aggregates'].values()))
            self.assertIsNone(a['reported_count_sum']);self.assertEqual(a['max_reported_count'],4);self.assertIn('aggregate_interval_overlap',out['issues'])
    def test_signed_sample_and_provenance_stay_separate(self):
        records=[]
        for i,provenance in enumerate(['candidate_pair','retrospective_pair']):
            ev=dict(kind='stop_failure_match',session=dict(epoch=EPOCH,capture=1,id=1),observed_boundary=None,observation=provenance,delta_ms=-30000 if i==0 else None,observed_lag_ms=None,hook_age_ms=None,outcome='censored')
            r=envelope(ev);r['seq']=i+1;records.append(r)
        out=self.run_report(report(records));series=out['captures'][0]['series']
        self.assertEqual(series['stop_failure_match:candidate_pair:censored:delta_ms']['min'],-30000)
        self.assertEqual(series['stop_failure_match:retrospective_pair:censored:delta_ms']['missing'],1)
    def test_scope_capture_series_and_sample_capacity_are_explicit(self):
        from unittest.mock import patch
        records=[]
        for i in range(3):
            r=envelope();r.update(seq=i+1,capture=i+1);r['event']['operation']['capture']=i+1;records.append(r)
        h=header(3);h['captures']=3
        with patch.object(mod,'MAX_CAPTURES',1):
            out=self.run_report(report(records,h));self.assertIn('capture_capacity',out['issues']);self.assertEqual(out['records_omitted_by_analysis_capacity'],2)
        records=[]
        for i in range(3):
            r=envelope();r['seq']=i+1;records.append(r)
        with patch.object(mod,'MAX_SAMPLES',1):
            out=self.run_report(report(records));self.assertIn('sample_capacity',out['issues']);self.assertTrue(next(iter(out['captures'][0]['series'].values()))['prefix_samples_only'])
        with patch.object(mod,'MAX_SERIES',1):self.assertIn('series_capacity',self.run_report(report())['issues'])
    def test_rejected_queue_receipt_does_not_advance_accepted_population(self):
        ev=dict(kind='queue_receipt',owner=dict(epoch=EPOCH,capture=1,id=1),list='reviewing',revision=2,phase='ready',coverage='complete',rows=130,outcome='accepted')
        first=envelope(ev);second=envelope(dict(ev,revision=1,rows=999,outcome='rejected_older'));second['seq']=2
        out=self.run_report(report([first,second]));scope=out['captures'][0]['scopes'][0]
        self.assertEqual(scope['observations'],2);self.assertEqual(scope['last']['rows'],130);self.assertIsNone(scope['last']['receipt_revision'])
    def test_stats_scope_capacity_and_unknown_coverage_are_honest(self):
        from unittest.mock import patch
        records=[]
        for i in range(2):
            ev={'kind':'stats_progress','scope':dict(epoch=EPOCH,capture=1,id=i+1),'outcome':'cache_reuse','covered_days':0,'accumulated':236}
            r=envelope(ev);r['seq']=i+1;records.append(r)
        with patch.object(mod,'MAX_SCOPES',1):
            out=self.run_report(report(records));self.assertIn('scope_capacity',out['issues'])
            first=out['captures'][0]['scopes'][0]['first'];self.assertEqual(first['covered_days'],0);self.assertIsNone(first['unknown_days']);self.assertIsNone(first['total'])
    def test_metadata_only_coalescing_and_private_build_are_qualified_without_echo(self):
        h=header();h['loss']['coalesced']=1;h['build']='PRIVATE_SENTINEL'
        out=self.run_report(report(h=h));self.assertEqual(out['quality'],'qualified');self.assertNotIn('PRIVATE_SENTINEL',json.dumps(out))
    def test_cli_usage_private_filename_and_raw_errors_are_not_echoed(self):
        p=subprocess.run([sys.executable,str(HERE/'measurement-report.py'),'PRIVATE_SENTINEL','--PRIVATE_SENTINEL'],capture_output=True,text=True)
        self.assertEqual(p.returncode,2);self.assertNotIn('PRIVATE_SENTINEL',p.stdout+p.stderr)
    def test_unknown_schema_foreign_handle_and_omission_lie_are_invalid(self):
        r=report();r[1]['schema']=2;self.assertEqual(self.run_report(r)['integrity'],'invalid')
        r=report();r[1]['event']['operation']['capture']=2;self.assertEqual(self.run_report(r)['integrity'],'invalid')
        h=header();h['omitted_prefix']=dict(records=1,oldest_wall_ms=1,newest_wall_ms=2)
        self.assertEqual(self.run_report(report(h=h))['integrity'],'invalid')
    def test_unencodable_known_metadata_is_invalid_without_an_exception(self):
        h=header();h['build']='\ud800'
        self.assertEqual(self.run_report(report(h=h))['integrity'],'invalid')
    def test_client_stats_populations_preserve_scope_and_unlinked_identity(self):
        records=[]
        for i,scope in enumerate([1,2,None]):
            ev={'kind':'client','observation':{'kind':'stats_view','observation':'mounted','scope':dict(epoch=EPOCH,capture=1,id=scope) if scope else None,'outcome':'accepted','rows':[2,200,300][i]}}
            r=envelope(ev);r['seq']=i+1;records.append(r)
        out=self.run_report(report(records));g=out['captures'][0]
        self.assertEqual(sorted(v['median'] for k,v in g['series'].items() if k.endswith(':rows')),[2,200,300])
        self.assertEqual(len(g['client_populations']),3)
        self.assertEqual([p['identity'] for p in g['client_populations']],['linked_scope','linked_scope','unlinked_mixed'])
        self.assertNotEqual(g['client_populations'][0]['scope_ordinal'],g['client_populations'][1]['scope_ordinal'])
        self.assertIn('client_identity_unavailable',out['issues']);self.assertNotIn(EPOCH,json.dumps(out))
    def test_ready_closed_dimensions_never_silently_merge(self):
        base={'kind':'mounted_review','source':'github','list':'reviewing','surface':'ready_panel','selection':'all_repositories','footer_location':'desktop_footer','footer':'checked','visible_count':130}
        variants=[base,dict(base,selection='selected_scope'),dict(base,footer_location='phone_banner'),dict(base,source='gitlab'),dict(base,list='authored'),dict(base,surface='review_list')]
        records=[]
        for i,ev in enumerate(variants):
            r=envelope({'kind':'client','observation':ev});r['seq']=i+1;records.append(r)
        g=self.run_report(report(records))['captures'][0]
        self.assertEqual(len(g['client_populations']),6)
        self.assertTrue(all(p['identity']=='unlinked_mixed' for p in g['client_populations']))
        self.assertEqual(len([k for k in g['series'] if k.endswith(':visible_count')]),6)
    def test_client_scope_ordinals_are_capture_qualified_and_receipts_do_not_invent_scope(self):
        records=[]
        for i,(capture,provenance) in enumerate([(1,'readback'),(1,'mounted'),(2,'mounted')]):
            r=envelope({'kind':'client','observation':{'kind':'stats_view','observation':provenance,'scope':dict(epoch=EPOCH,capture=capture,id=77),'outcome':'accepted','rows':2}});r.update(seq=i+1,capture=capture);records.append(r)
        h=header(3);h['captures']=2
        out=self.run_report(report(records,h));a,b=out['captures'];self.assertEqual(a['client_populations'][0]['scope_ordinal'],a['client_populations'][1]['scope_ordinal']);self.assertNotEqual(a['client_populations'][0]['scope_ordinal'],b['client_populations'][0]['scope_ordinal'])
        records=[]
        for i in range(2):
            ev={'kind':'mounted_review','source':'github','list':'reviewing','surface':'ready_panel','selection':'selected_scope','footer_location':'desktop_footer','footer':'checked','receipt':dict(epoch=EPOCH,capture=1,id=i+1),'visible_count':i+1}
            r=envelope({'kind':'client','observation':ev});r['seq']=i+1;records.append(r)
        out=self.run_report(report(records));p=out['captures'][0]['client_populations'][0]
        self.assertEqual(p['identity'],'unlinked_mixed');self.assertIsNone(p['scope_ordinal']);self.assertIn('client_identity_unavailable',out['issues'])
    def test_client_population_and_scope_caps_qualify_omissions(self):
        from unittest.mock import patch
        records=[]
        for i in range(2):
            r=envelope({'kind':'client','observation':{'kind':'stats_view','scope':dict(epoch=EPOCH,capture=1,id=i+1),'outcome':'accepted','rows':i+1}});r['seq']=i+1;records.append(r)
        for limit,issue in [('MAX_CLIENT_POPULATIONS','client_population_capacity'),('MAX_SCOPES','client_scope_capacity')]:
            with patch.object(mod,limit,1):
                out=self.run_report(report(records));self.assertIn(issue,out['issues']);self.assertEqual(out['records_omitted_by_analysis_capacity'],1)
                self.assertEqual(len(out['captures'][0]['client_populations']),1)
    def test_large_file_and_record_budget_cannot_look_clean(self):
        from unittest.mock import patch
        with patch.object(mod,'MAX_FILE',1):self.assertEqual(self.run_report(report())['integrity'],'invalid')
        with patch.object(mod,'MAX_RECORDS',0):self.assertEqual(self.run_report(report())['integrity'],'invalid')
if __name__=='__main__': unittest.main()
