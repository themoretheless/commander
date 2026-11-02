from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text())
assert [c['name'] for c in checks]==['audit','build','tests','clippy','check','fmt','diff']
assert all(c['exit_code']==0 for c in checks)
log=(q/'tests.log').read_text()
assert '3371 passed; 0 failed; 3 ignored' in log
for name in ['panel::visit::tests::ordered_path_terms_match_documented_examples_unicode_and_boundaries','panel::visit::tests::ordered_terms_preserve_order_rank_identity_and_refuse_oversized_queries','app::omnibar_dialog::tests::ordered_term_toggle_pointer_and_keyboard_keep_form_ownership','session::tests::recent_match_modes_roundtrip_without_losing_order_paths_or_ranks']:
 assert f'test {name} ... ok' in log
for p,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==digest,p
print('PASS: seven gates, 3371 tests, ordered path examples/limits/ranking, pointer/keyboard and session regressions, matching source hashes; native not claimed')
