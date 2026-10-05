"""Hold only the explicitly named synthetic SQLite writer reservation until stdin closes."""
import pathlib, sqlite3, sys
profile = pathlib.Path(sys.argv[1]).resolve()
assert (profile / '.enterprise-synthetic').is_file()
db = sqlite3.connect(profile / 'headstate.db', timeout=0)
db.execute('BEGIN IMMEDIATE')
print('LOCKED', flush=True)
try:
    sys.stdin.readline()
finally:
    db.rollback()
    db.close()
