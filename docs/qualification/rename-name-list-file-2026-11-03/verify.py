from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
checks=json.loads((q/'checks.json').read_text())
assert [c['name'] for c in checks]==['audit','build','tests','clippy','check','fmt','diff']
assert all(c['exit_code']==0 for c in checks)
log=(q/'tests.log').read_text()
assert '3357 passed; 0 failed; 3 ignored' in log
for name in ['app::batch_rename_dialog::tests::imported_name_list_requires_explicit_click_and_current_count','name_list_file::tests::accepts_exact_byte_limit_and_refuses_replaced_source','name_list_file::tests::import_preserves_bom_crlf_unicode_and_source_bytes','name_list_file::tests::refuses_invalid_oversized_nonregular_missing_and_cancelled_sources']:
 assert f'test {name} ... ok' in log
for p,digest in json.loads((q/'source-sha256.json').read_text()).items():
 assert hashlib.sha256((root/p).read_bytes()).hexdigest()==digest,p
print('PASS: seven gates, 3357 tests, four import regressions, matching source hashes; native capture not claimed')
