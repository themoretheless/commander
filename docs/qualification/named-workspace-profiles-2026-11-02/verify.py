from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3264 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(root/'target/debug/commander');assert h(root/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['workspace_profiles','folder_tabs']:
 launch=json.loads((q/('native-launch.json' if scene=='workspace_profiles' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='workspace_profiles':
  first=json.loads((d/'profile-store-9.json').read_text());replaced=json.loads((d/'profile-store-13.json').read_text());applied=json.loads((d/'profile-applied.json').read_text())
  for generation,v in [(1,first),(2,replaced)]:
   assert v['format']=='commander.persist' and v['store']=='commander.workspace_profiles' and v['schema']==1 and v['generation']==generation
   assert len(v['payload']['profiles'])==1 and v['payload']['profiles'][0]['name']=='Work'
  initial=first['payload']['profiles'][0];final=replaced['payload']['profiles'][0]
  assert Path(final['left_root'])==Path(initial['left_root'])/'Design' and Path(final['right_root'])==Path(initial['right_root'])/'Archive'
  assert {k:v for k,v in initial.items() if k not in ('left_root','right_root')}=={k:v for k,v in final.items() if k not in ('left_root','right_root')}
  assert applied['phase']==17 and applied['left']==final['left_root'] and applied['right']==final['right_root']
  assert len(applied['tabs']['entries'])==2 and applied['tabs']['active']==0 and applied['tabs']['entries'][0]['path']==applied['left'] and applied['tabs']['entries'][1]['path']==initial['right_root']
 else:
  records=[json.loads((d/f'history-{p}.json').read_text()) for p in [87,93,99]]
  assert [len(v['left'].get('closed',[])) for v in records]==[4,3,0]
  assert [v['phase'] for v in records]==[87,93,99]
  assert all(len(v['left']['entries'])==2 and len(v['right']['closed'])==2 for v in records)
old=json.loads((root/'docs/qualification/image-metadata-json-2026-11-02/source-sha256.json').read_text())
assert all(old[p]==h(root/p) for p in ['Cargo.toml','Cargo.lock'])
comparison=json.loads((root/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(root/'deny.toml')
print('PASS: six gates; 3264 passed / 3 ignored; named profiles and folder-tabs control each native 14/14; actual atomic store generations and replaced roots match applied workspace, background tab retained; source/binary/tool hashes. Existing dependency audit failures remain open.')
