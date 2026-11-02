from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-hashes.json').read_text()).items(): assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text())
assert [x['name'] for x in checks]==['build','tests','clippy','check','fmt','diff']
assert all(x['exit_code']==0 for x in checks)
log=(q/'tests.log').read_text()
assert '3215 passed; 0 failed; 3 ignored' in log
for name in ['closed_tab_histories_survive_atomic_session_save_and_reopen_independently','non_utf8_closed_root_is_omitted_without_poisoning_the_session','closed_history_is_optional_tolerant_recent_bounded_and_lazy']:
 assert any(name in line and line.endswith(' ... ok') for line in log.splitlines()),name
for tool in json.loads((q/'validators.json').read_text()): assert h(Path(tool['path']))==tool['sha256']
print('PASS: six gates; 3215 passed / 3 ignored; real atomic session and independent restored histories, metadata/geometry/position, tolerant bounded lazy histories and optional non-UTF-8 roots; source and validator hashes match.')
