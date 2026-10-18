"""Read-only synthetic-only profile evidence (marker required). No SQL mutations."""
import json, pathlib, sqlite3, sys
profile = pathlib.Path(sys.argv[1]).resolve()
assert (profile / '.enterprise-synthetic').is_file()
db = sqlite3.connect(f'file:{profile}/headstate.db?mode=ro', uri=True)
result = {'integrity': db.execute('PRAGMA integrity_check').fetchone()[0]}
result['counts'] = {'queue_scan': db.execute('SELECT count(*) FROM queue_scan').fetchone()[0]}
# Fixed synthetic schema only. Unknown/missing tables are not zero.
corpus_tables = ['claude_session', 'claude_subagent', 'claude_session_usage', 'claude_session_model', 'claude_session_pr', 'claude_usage_ledger', 'claude_index_ledger', 'claude_index_state', 'claude_fts_identity', 'claude_transcript_fts', 'claude_link_checkpoint', 'claude_link_baseline', 'claude_population_state', 'claude_population_generation', 'claude_corpus_epoch', 'claude_corpus_inventory', 'claude_corpus_directory']
existing = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
result['corpusCounts'] = {table: {'status': 'measured', 'rows': db.execute(f'SELECT count(*) FROM "{table}"').fetchone()[0]} if table in existing else {'status': 'absent'} for table in corpus_tables}
result['sqliteConnectionSemantics'] = 'wal_autocheckpoint is this read-only inspector connection setting, not a persisted writer setting'
result['sqlite'] = {name: db.execute('PRAGMA ' + name).fetchone()[0] for name in ['page_count', 'freelist_count', 'page_size', 'wal_autocheckpoint']}
def size(name):
    try:
        return {'status': 'measured', 'bytes': (profile / name).stat().st_size}
    except FileNotFoundError:
        return {'status': 'absent'}
    except OSError:
        return {'status': 'unknown', 'reason': 'metadata_unavailable'}
result['files'] = {'db': size('headstate.db'), 'walRetained': size('headstate.db-wal')}
result['queues'] = []
for list_id, revision, payload in db.execute('SELECT list,revision,payload FROM queue_scan ORDER BY list'):
    state = json.loads(payload)
    result['queues'].append({'list': list_id, 'revision': revision,
        **{key: state.get(key) for key in ['coverage_valid', 'completed_total', 'done', 'pages', 'tainted', 'failures', 'no_work']},
        'candidates': len(state.get('candidates', [])), 'seen': len(state.get('seen', []))})
print(json.dumps(result))
