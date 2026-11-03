from pathlib import Path
import json,hashlib,subprocess,sys
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3475 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==digest
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()
d=Path(launch['out'])/'checksum_selected';manifest=read(d/'manifest.json')
assert manifest['status']=='captured' and manifest['error'] is None
assert len(manifest['checks'])==14 and all(c['passed'] for c in manifest['checks'])
assert manifest['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
assert (d/'checksum.txt').read_bytes()==b'abc'
known=['SHA-256','SHA-384','SHA-512','SHA3-256','SHA3-512','BLAKE2s-256','BLAKE2b-512','BLAKE3','CRC32','MD5','SHA-1','Adler-32','XXH3-64','XXH3-128']
paths=None
for phase,expected in [(6,'XXH3-128'),(10,'XXH3-128'),(14,None),(21,'XXH3-64')]:
 state=read(d/f'selected-{phase}.json')
 assert state['busy'] is False and state['only']==expected
 assert state['summary']=='Calculated 1 file(s)'
 assert len(state['rows'])==1 and state['rows'][0]['error'] is None and state['rows'][0]['skipped'] is None
 assert state['rows'][0]['algorithms']==([expected] if expected else known)
 if paths is None:paths=state['paths']
 assert state['paths']==paths==[state['rows'][0]['path']]
 assert state['rows'][0]['path'].endswith('/left/checksum.txt')
 assert (state['rows'][0]['content'] is None)==(expected is not None)
 if phase==10:assert state['selected']=='SHA-256' and state['comparable'] is False
 else:assert state['comparable'] is True
subprocess.run([sys.executable,str(q/'verify_all.py')],check=True)
(q/'verification.json').write_text(json.dumps(dict(selected_native_checks=14,selected_clipboard_calls=0,all_native_checks=14,all_clipboard_calls=38,scopes=['XXH3-128','unrequested SHA-256','all 14','XXH3-64'],captured_paths_preserved=True,source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: selected-only/all native scenes 14/14 each; exact available digests, unavailable selection, captured paths and prior full exports verified')
