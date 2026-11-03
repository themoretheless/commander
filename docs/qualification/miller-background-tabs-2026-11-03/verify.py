from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3519 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==h,p
launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out'])/'miller_browser'
assert launch['binary_sha256']==launch['core_sha256']==hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()
m=json.loads((out/'manifest.json').read_text());assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
assert all(v==0 for v in m['native_effect_calls'].values())
r=json.loads((out/'background-tab.json').read_text());before=json.loads((out/'background-before.json').read_text());assert r['before']==r['after']==before
owner=Path(before['path']);assert owner.name=='column-A' and before['cursor']==str(owner/'a.txt') and before['selected']==[str(owner/'a.txt')]
assert r['tabs']['active']==0 and [i['path'] for i in r['tabs']['entries']]==[str(owner),str(owner.parent/'column-B')]
assert r['right']==str(owner.parent.parent/'right')
assert all(i['view']['mode']=='Miller' for i in r['tabs']['entries'])
assert (out/'source-A.txt').read_bytes()==(out/'preview.txt').read_bytes()==b'Miller A owner\n'
assert (out/'source-B.txt').read_bytes()==b'Miller B owner\n'
for phase in [58,60,62,64]:
 k=json.loads((out/f'keyboard-{phase}.json').read_text());assert k['mode']=='Miller'
(q/'verification.json').write_text(json.dumps(dict(checks=7,tests_passed=3519,tests_ignored=3,native_checks=14,background_tab_owner_preserved=True,keyboard_checkpoints=4,source_hashes_verified=True),indent=2)+'\n')
print('Verified seven gates, hashes, native Miller background tab and preserved action owner')
