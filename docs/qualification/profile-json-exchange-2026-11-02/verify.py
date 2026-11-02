from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3272 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(root/'target/debug/commander');assert h(root/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['profile_exchange','profile_tabs']:
 launch=json.loads((q/('native-launch.json' if scene=='profile_exchange' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==(16 if scene=='profile_exchange' else 14) and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=(2 if scene=='profile_exchange' else 0),opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='profile_exchange':
  before=json.loads((d/'profile-export-before.json').read_text());imported=json.loads((d/'profile-import.json').read_text());after=json.loads((d/'profile-export-after.json').read_text())
  assert before['format']==imported['format']=='commander.workspace-profile' and before['version']==imported['version']==1 and before['platform']==imported['platform']=='unix'
  assert before['profile']['name']=='Work' and imported['profile']['name']=='Imported' and imported['profile']['left_filter']=='*.rs' and after==imported
  for phase in [3,10,14,18,22,29,33]:
   store=json.loads((d/f'exchange-store-{phase}.json').read_text());state=json.loads((d/f'exchange-state-{phase}.json').read_text());assert store['payload']==state['book'] and state['phase']==phase
   assert store['generation']==(1 if phase<18 else 2)
   assert store['payload']['profiles']==([before['profile']] if phase<18 else [before['profile'],imported['profile']])
   assert state['preview']==(imported['profile'] if phase==14 else None)
   assert state['selected']==('Work' if phase<18 else 'Imported') and (state['error'] is None)==(phase!=33)
   assert Path(state['left'])==Path(before['profile']['left_root'])/'Design' and Path(state['right'])==Path(before['profile']['right_root'])/'Archive'
  assert (d/'exchange-store-3.json').read_bytes()==(d/'exchange-store-14.json').read_bytes()
  assert (d/'exchange-store-18.json').read_bytes()==(d/'exchange-store-33.json').read_bytes()
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
print('PASS: six gates; 3272 passed / 3 ignored; reviewed profile JSON exchange native 16/16 and full-tab restore 14/14; actual clipboard documents, non-mutating review, explicit add generation, malformed-review preservation and unchanged current roots; source/binary/tool hashes. Existing dependency audit failures remain open.')
