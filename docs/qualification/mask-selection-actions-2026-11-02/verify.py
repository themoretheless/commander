from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3257 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(root/'target/debug/commander');assert h(root/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['mask_actions','name_list_actions']:
 launch=json.loads((q/('native-launch.json' if scene=='mask_actions' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 stages=[json.loads((d/f'names-action-{i}.json').read_text()) for i in range(4)]
 for i,v in enumerate(stages):
  assert v['stage']==i and v['action']==['Remove','Invert','Invert','Add'][i]
  expected=({'photo.png'} if i in (0,2) else {'photo.png','README.txt','notes.md'})
  if scene=='mask_actions' and i==3:expected={'notes.md','README.txt'}
  assert set(v['selected'])==expected
  assert v['root']==stages[0]['root']
 progress=json.loads((d/'action-progress.json').read_text());assert progress['phase']==27 and progress['preview']==(3 if scene=='mask_actions' else 2) and progress['modal'] is None
old=json.loads((root/'docs/qualification/image-metadata-json-2026-11-02/source-sha256.json').read_text())
assert all(old[p]==h(root/p) for p in ['Cargo.toml','Cargo.lock'])
comparison=json.loads((root/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(root/'deny.toml')
print('PASS: six gates; 3257 passed / 3 ignored; mask and literal-name workflows each 14/14; exact Remove/Invert/Invert/Add sets and legacy Add exclusion subtraction; source/binary/tool hashes. Previous dependency audit failures remain open.')
