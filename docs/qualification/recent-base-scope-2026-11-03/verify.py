from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text())
assert [c['name'] for c in checks]==['audit','build','tests','clippy','check','fmt','diff']
assert all(c['exit_code']==0 for c in checks)
log=(q/'tests.log').read_text()
assert '3374 passed; 0 failed; 3 ignored' in log
for name in ['panel::visit::tests::recent_base_scope_uses_components_and_composes_with_matching_and_ordering','panel::visit::tests::recent_base_scope_retains_exact_non_utf8_paths','app::omnibar_dialog::tests::recent_base_scope_pointer_and_keyboard_keep_form_ownership','panel::visit::tests::ordered_path_terms_match_documented_examples_unicode_and_boundaries']:
 assert f'test {name} ... ok' in log
for p,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==digest,p
print('PASS: seven gates, 3374 tests, scoped paths/matching/order/identity, real egui toggle and corrected final-keyword cases; native not claimed')
