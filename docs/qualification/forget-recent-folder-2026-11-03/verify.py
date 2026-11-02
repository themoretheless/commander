from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text())
assert [c['name'] for c in checks]==['audit','build','tests','clippy','check','fmt','diff']
assert all(c['exit_code']==0 for c in checks)
log=(q/'tests.log').read_text()
assert '3364 passed; 0 failed; 3 ignored' in log
for name in ['panel::visit::tests::forgetting_recent_path_removes_rank_preserves_peers_and_survives_roundtrip','panel::visit::tests::forgetting_recent_path_uses_exact_non_utf8_identity','app::omnibar_dialog::tests::forget_recent_control_requires_recent_row_and_reports_focus_ownership']:
 assert f'test {name} ... ok' in log
for p,digest in json.loads((q/'source-sha256.json').read_text()).items():
 assert hashlib.sha256((root/p).read_bytes()).hexdigest()==digest,p
print('PASS: seven gates, 3364 tests, recent-folder removal/ranking/snapshot and pointer/Enter regressions, matching source hashes; native not claimed')
