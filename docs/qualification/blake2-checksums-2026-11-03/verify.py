from pathlib import Path
import hashlib,json,subprocess,sys
q=Path(__file__).resolve().parent;root=q.parents[2]
subprocess.run([sys.executable,str(q/'generate_oracle.py')],check=True)
checks=json.loads((q/'checks.json').read_text())
assert [c['name'] for c in checks]==['audit','build','tests','clippy','check','fmt','diff']
assert all(c['exit_code']==0 for c in checks)
log=(q/'tests.log').read_text()
assert '3361 passed; 0 failed; 3 ignored' in log
for name in ['checksum::tests::blake2_streams_python_vectors_compares_encodings_and_exports_lists','checksum_list::tests::blake2_tags_hints_and_ambiguous_widths_are_distinct_from_sha2']:
 assert f'test {name} ... ok' in log
for p,digest in json.loads((q/'source-sha256.json').read_text()).items():
 assert hashlib.sha256((root/p).read_bytes()).hexdigest()==digest,p
print('PASS: seven gates, 3361 tests, independent Python BLAKE2 oracle and matching source hashes; native not claimed')
