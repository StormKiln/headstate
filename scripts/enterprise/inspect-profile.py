"""Read-only synthetic-only profile evidence (marker required). No SQL mutations."""
import json, pathlib, sqlite3, sys
profile = pathlib.Path(sys.argv[1]).resolve()
assert (profile / '.enterprise-synthetic').is_file()
db = sqlite3.connect(f'file:{profile}/headstate.db?mode=ro', uri=True)
result = {'integrity': db.execute('PRAGMA integrity_check').fetchone()[0]}
result['counts'] = {'queue_scan': db.execute('SELECT count(*) FROM queue_scan').fetchone()[0]}
result['queues'] = []
for list_id, revision, payload in db.execute('SELECT list,revision,payload FROM queue_scan ORDER BY list'):
    state = json.loads(payload)
    result['queues'].append({'list': list_id, 'revision': revision,
        **{key: state.get(key) for key in ['coverage_valid', 'completed_total', 'done', 'pages', 'tainted', 'failures', 'no_work']},
        'candidates': len(state.get('candidates', [])), 'seen': len(state.get('seen', []))})
print(json.dumps(result))
