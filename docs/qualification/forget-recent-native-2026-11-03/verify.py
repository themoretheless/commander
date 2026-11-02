from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3364 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'recent_forget';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
snapshots=[read(d/f'recent-phase-{phase}.json') for phase in range(16)]
a,b,c=snapshots[7],snapshots[10],snapshots[15]
folder=Path(a['left']).parent;forgot=str(folder/'ForgetMe');kept=str(folder/'KeepMe')
assert a['query']=='ForgetMe' and a['visible']==[forgot,kept]
assert forgot in a['paths'] and a['stats']['uses'][forgot]['count']==2
assert b['query']=='ForgetMe' and b['visible']==[kept]
assert forgot not in b['paths'] and forgot not in b['stats']['uses']
assert c['query']=='KeepMe' and c['visible']==[kept]
assert c['paths']==b['paths'] and c['stats']==b['stats']
for s in [b,c]:
 assert s['stats']['uses'][kept]==a['stats']['uses'][kept]==dict(count=2,last=3)
 assert s['saved'] is not None
 assert forgot not in s['saved']['paths'] and forgot not in s['saved']['stats']['uses']
 assert s['saved']['stats']['uses'][kept]==dict(count=2,last=3)
assert all(s['left']==a['left'] and s['right']==a['right'] for s in snapshots)
assert (d/'keep.txt').read_bytes()==b'kept payload'
(q/'verification.json').write_text(json.dumps(dict(native_checks=16,observations=16,native_effects=0,removed_from_saved_paths_and_ranking=True,peer_preserved=True,panel_navigation_unchanged=True,source_bytes_unchanged=True,source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 16/16, sixteen snapshots, removed recent/rank in saved session, peer/source preservation, zero navigation/effects')
