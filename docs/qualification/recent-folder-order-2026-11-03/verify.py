from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text())
assert [c['name'] for c in checks]==['audit','build','tests','clippy','check','fmt','diff']
assert all(c['exit_code']==0 for c in checks)
log=(q/'tests.log').read_text()
assert '3367 passed; 0 failed; 3 ignored' in log
for name in ['app::omnibar_dialog::tests::recent_order_pointer_and_focused_enter_preserve_interaction_ownership','panel::visit::tests::recent_order_changes_projection_without_mutating_visits_or_scores','session::tests::recent_order_roundtrips_both_modes_with_paths_and_ranks']:
 assert f'test {name} ... ok' in log
for p,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==digest,p
print('PASS: seven gates, 3367 tests, recent-order pointer/Enter/projection/session regressions and source hashes; native not claimed')
