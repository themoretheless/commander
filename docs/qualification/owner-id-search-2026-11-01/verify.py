from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text())
assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3125 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for record in json.loads((q/'validators.json').read_text()):
 assert hashlib.sha256(Path(record['path']).read_bytes()).hexdigest()==record['sha256']
for scene,key in [('owner_search','native-launch.json'),('broken_link_search','broken_link_search-launch.json')]:
 launch=json.loads((q/key).read_text())
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text())
 assert m['status']=='captured' and m['error'] is None and m['modal_owner']=='Find'
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
d=q/'owner_search'
for phase,expression,names in [(4,'uid:424242 gid:606060 name:owned-',['owned-alpha.txt','owned-link']),(7,'uid:!424242 gid:606061 name:owned-',['owned-beta.txt']),(10,'uid:424242 gid:!606060 name:owned-',[])]:
 state=json.loads((d/f'owner-{phase}.json').read_text())
 assert state['phase']==phase and state['expression']==expression and state['source']=='Live + archives'
 assert state['matched']==len(names) and state['skipped']==1
 assert sorted(p.split('owners.tar!/')[-1] for p in state['paths'])==names
 assert all('/owners.tar!/' in p for p in state['paths'])
 assert len(state['explanations'])==len(names)
 for explanation in state['explanations']:assert all(term in explanation for term in expression.split()[:2])
facts=json.loads((d/'owner-facts.json').read_text());state=json.loads((d/'owner-final.json').read_text())
assert type(facts['uid']) is int and type(facts['gid']) is int
assert state['expression']==f"uid:{facts['uid']} gid:{facts['gid']} archives:off name:owned-"
assert state['source']=='Live (archives off)' and state['matched']==1 and state['skipped']==0
assert len(state['paths'])==1 and state['paths'][0].endswith('/left/owned-native.txt')
assert f"uid:{facts['uid']}" in state['explanations'][0] and f"gid:{facts['gid']}" in state['explanations'][0]
link=json.loads((q/'broken_link_search/links-final.json').read_text())
assert link['expression']=='type:broken-link archives:off' and link['matched']==1 and link['skipped']==1 and not link['truncated']
assert len(link['paths'])==1 and link['paths'][0].endswith('/notdir.lnk')
print('PASS: six checks, 3125 tests; owner search 16/16 and broken-link search 16/16; four query states and raw native UID/GID agree')
