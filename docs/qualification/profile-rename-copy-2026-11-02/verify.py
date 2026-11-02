from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3269 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(root/'target/debug/commander');assert h(root/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['profile_names','profile_tabs']:
 launch=json.loads((q/('native-launch.json' if scene=='profile_names' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==(16 if scene=='profile_names' else 14) and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='profile_names':
  seed=json.loads((d/'profile-before.json').read_text());assert seed['generation']==1 and len(seed['payload']['profiles'])==1
  original=seed['payload']['profiles'][0];assert original['name']=='Work' and original['left_filter']=='*.rs'
  data={k:v for k,v in original.items() if k!='name'}
  snapshots=[]
  for stage in range(3):
   store=json.loads((d/f'profile-names-{stage}.json').read_text());state=json.loads((d/f'profile-names-state-{stage}.json').read_text());snapshots.append(store)
   assert store['generation']==[2,3,3][stage] and state['stage']==stage
   assert store['payload']==state['book'] and state['selected']==('Project' if stage==0 else 'Copy')
   assert (state['error'] is None)==(stage!=2)
   assert [p['name'] for p in store['payload']['profiles']]==(['Project'] if stage==0 else ['Project','Copy'])
   for p in store['payload']['profiles']:assert {k:v for k,v in p.items() if k!='name'}==data
   assert Path(state['left'])==Path(original['left_root'])/'Design' and Path(state['right'])==Path(original['right_root'])/'Archive'
  assert (d/'profile-names-1.json').read_bytes()==(d/'profile-names-2.json').read_bytes()
 else:
  envelope=json.loads((d/'tab-profile-store.json').read_text());layout=envelope['payload']['profiles'][0]['tab_layout'];actual=json.loads((d/'tab-profile-applied.json').read_text())
  assert envelope['generation']==1 and layout['version']==1 and layout['active']==1 and actual['active']=='Right' and actual['phase']==13
  for side in ['left','right']:
   assert len(layout[side]['entries'])==2 and layout[side]['active']==actual[side]['active']==1
   assert actual[side]['entries']==layout[side]['entries'] and len(actual[side]['closed'])==1
  assert layout['left']['entries'][0]['view']['mode']=='Icons' and layout['left']['entries'][1]['title']=='Project'
old=json.loads((root/'docs/qualification/image-metadata-json-2026-11-02/source-sha256.json').read_text())
assert all(old[p]==h(root/p) for p in ['Cargo.toml','Cargo.lock'])
comparison=json.loads((root/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(root/'deny.toml')
print('PASS: six gates; 3269 passed / 3 ignored; profile rename/copy/conflict native 16/16 and tab-set restore control 14/14; exact saved-data preservation, generations 1/2/3/3, byte-identical store on collision and unchanged current roots; source/binary/tool hashes. Existing dependency audit failures remain open.')
