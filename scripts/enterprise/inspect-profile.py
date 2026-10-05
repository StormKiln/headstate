"""Read-only, identity-free synthetic profile evidence. No SQL mutations."""
import json, pathlib, sqlite3, sys, datetime
profile = pathlib.Path(sys.argv[1]).resolve()
assert (profile / '.enterprise-synthetic').is_file()
db = sqlite3.connect(f'file:{profile}/headstate.db?mode=ro', uri=True)
result = {'integrity': db.execute('PRAGMA integrity_check').fetchone()[0]}
result['counts'] = {table: db.execute(f'SELECT count(*) FROM {table}').fetchone()[0]
                    for table in ['stats_cache', 'pr_history', 'pr_slice', 'pr_backfill_scope', 'pr_backfill_page', 'pr_scope_evidence', 'queue_scan']}
settings = dict(db.execute("SELECT key,value FROM settings WHERE key IN ('stats_viewer','stats_generation')"))
result['ownerGeneration'] = json.loads(settings.get('stats_generation','null'))
result['ownerSlot'] = {'synthetic-viewer':'original','synthetic-replacement':'replacement',None:None}.get(json.loads(settings.get('stats_viewer','null')),'unexpected')
result['slices'] = dict(db.execute('SELECT state,count(*) FROM pr_slice GROUP BY state'))
result['queues'] = []
for list_id, revision, payload in db.execute('SELECT list,revision,payload FROM queue_scan ORDER BY list'):
    state = json.loads(payload)
    result['queues'].append({'list': list_id, 'revision': revision,
        **{key: state.get(key) for key in ['coverage_valid', 'completed_total', 'done', 'pages', 'tainted', 'failures', 'no_work']},
        'candidates': len(state.get('candidates', [])), 'seen': len(state.get('seen', []))})
result['historyCoverage'] = []
today = datetime.datetime.now(datetime.timezone.utc).date()
for key, days, measure in db.execute('SELECT scope_key,horizon_days,measure FROM pr_backfill_scope'):
    required = {str(today - datetime.timedelta(days=i)) for i in range(1, days + 1)}
    covered = set()
    for start, end in db.execute("SELECT slice_from,slice_to FROM pr_slice WHERE scope_key=? AND state='complete'", (key,)):
        first, last = datetime.date.fromisoformat(start[:10]), datetime.date.fromisoformat(end[:10])
        covered.update(str(first + datetime.timedelta(days=i)) for i in range((last-first).days+1))
    result['historyCoverage'].append({'horizonDays': days, 'measure': measure, 'coveredRequiredDays': len(required & covered), 'missingRequiredDays': len(required-covered)})
print(json.dumps(result))
