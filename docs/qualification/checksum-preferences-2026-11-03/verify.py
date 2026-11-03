from pathlib import Path
import json,hashlib,subprocess,sys
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3478 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
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
for phase,requested,checked in [(34,['SHA-256','XXH3-64'],['SHA-256','XXH3-64']),(38,['SHA-256','XXH3-64'],['XXH3-64']),(42,['XXH3-64'],['XXH3-64']),(46,['XXH3-64'],[]),(50,['XXH3-64'],['XXH3-64'])]:
 assert read(d/f'set-enabled-{phase}.json') is (phase!=46)
 state=read(d/f'set-{phase}.json')
 assert state['busy'] is False and state['summary']=='Calculated 1 file(s)'
 assert state['requested']==requested and state['checked']==checked
 assert state['paths']==paths and len(state['rows'])==1
 assert state['rows'][0]['algorithms']==requested and state['rows'][0]['error'] is None and state['rows'][0]['skipped'] is None
 assert state['rows'][0]['content'] is None
 assert state['comparable'] is True
choices=read(d/'preferences-reopen.json')
expected_choices=dict(version=1,mode='checked',selected='XXH3-64',checked=['XXH3-64'])
assert choices['saved']==choices['restored']==expected_choices
fresh=read(d/'preferences-result.json')
assert fresh['paths']==paths and fresh['busy'] is False
assert fresh['summary']=='Calculated 1 file(s)' and fresh['selected']=='XXH3-64'
assert fresh['requested']==fresh['checked']==['XXH3-64']
assert len(fresh['rows'])==1 and fresh['rows'][0]['algorithms']==['XXH3-64']
assert fresh['rows'][0]['error'] is None and fresh['rows'][0]['skipped'] is None
assert fresh['rows'][0]['content'] is None and fresh['comparable'] is True
assert not (d/'preferences-error.txt').exists()
subprocess.run([sys.executable,str(q/'verify_all.py')],check=True)
(q/'verification.json').write_text(json.dumps(dict(selected_native_checks=14,selected_clipboard_calls=0,all_native_checks=14,all_clipboard_calls=38,scopes=['XXH3-128','unrequested SHA-256','all 14','XXH3-64','checked SHA-256 + XXH3-64','edited pending set','empty pending set'],captured_paths_preserved=True,source_and_binary_hashes_match=True,session_settings_roundtrip=True,fresh_scope_after_restore=['XXH3-64']),indent=2)+'\n')
print('PASS: both native scenes 14/14; saved/restored preferences, fresh requested scope, captured paths and prior exports verified')
