from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3267 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(root/'target/debug/commander');assert h(root/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['profile_tabs','workspace_profiles']:
 launch=json.loads((q/('native-launch.json' if scene=='profile_tabs' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='profile_tabs':
  envelope=json.loads((d/'tab-profile-store.json').read_text());assert envelope['generation']==1 and envelope['store']=='commander.workspace_profiles'
  profiles=envelope['payload']['profiles'];assert len(profiles)==1 and profiles[0]['name']=='Full'
  layout=profiles[0]['tab_layout'];assert layout['version']==1 and layout['active']==1
  actual=json.loads((d/'tab-profile-applied.json').read_text());assert actual['phase']==13 and actual['active']=='Right'
  for side in ['left','right']:
   assert len(layout[side]['entries'])==2 and layout[side]['active']==1
   assert actual[side]['entries']==layout[side]['entries'] and actual[side]['active']==1
   assert len(actual[side]['closed'])==1
   assert not any('selection_snapshot' in row for row in layout[side]['entries'])
  assert layout['left']['entries'][0]['view']['mode']=='Icons' and layout['left']['entries'][1]['title']=='Project'
  assert Path(profiles[0]['left_root']).name=='Design' and Path(profiles[0]['right_root']).name=='Archive'
 else:
  first=json.loads((d/'profile-store-9.json').read_text());second=json.loads((d/'profile-store-13.json').read_text());applied=json.loads((d/'profile-applied.json').read_text())
  assert first['generation']==1 and second['generation']==2
  initial=first['payload']['profiles'][0];final=second['payload']['profiles'][0]
  assert initial['name']==final['name']=='Work'
  assert Path(final['left_root'])==Path(initial['left_root'])/'Design' and Path(final['right_root'])==Path(initial['right_root'])/'Archive'
  assert applied['phase']==17 and applied['left']==final['left_root'] and applied['right']==final['right_root']
  assert len(applied['tabs']['entries'])==2 and applied['tabs']['entries'][1]['path']==initial['right_root']
  assert final['tab_layout']['left']['entries']==applied['tabs']['entries']
old=json.loads((root/'docs/qualification/image-metadata-json-2026-11-02/source-sha256.json').read_text())
assert all(old[p]==h(root/p) for p in ['Cargo.toml','Cargo.lock'])
comparison=json.loads((root/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(root/'deny.toml')
print('PASS: six gates; 3267 passed / 3 ignored; full tab-set and prior named-profile native workflows each 14/14; disk layout exactly matches restored runtime entries, active indices/side, background Icons/title and retained closed history; source/binary/tool hashes. Existing dependency audit failures remain open.')
