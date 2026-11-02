from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3253 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(root/'target/debug/commander');assert h(root/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['tab_selection_session','folder_tabs']:
 launch=json.loads((q/('native-launch.json' if scene=='tab_selection_session' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='tab_selection_session':
  stages=[json.loads((d/f'selection-{i}.json').read_text()) for i in range(4)]
  for i,v in enumerate(stages):
   assert v['stage']==i and set(v['selected'])==(set() if i==1 else {'README.txt','notes.md'})
   assert v['root']==stages[0]['root']
   if i<3:
    snapshot=v['snapshot'];assert snapshot==stages[0]['snapshot']
    assert snapshot['version']==1 and snapshot['platform']=='unix'
    assert bytes(snapshot['root']).decode()==v['root']
    assert {bytes(n) for n in snapshot['names']}=={b'README.txt',b'notes.md'}
   else:assert v['snapshot'] is None
 else:
  records=[json.loads((d/f'history-{p}.json').read_text()) for p in [87,93,99]]
  assert [len(v['left'].get('closed',[])) for v in records]==[4,3,0]
  assert [v['phase'] for v in records]==[87,93,99]
  assert all(len(v['left']['entries'])==2 and len(v['right']['closed'])==2 for v in records)
  assert records[-1]['left']['entries'][0]['title']=='Work' and records[-1]['left']['entries'][0]['close_protected']
old=json.loads((root/'docs/qualification/image-metadata-json-2026-11-02/source-sha256.json').read_text())
assert all(old[p]==h(root/p) for p in ['Cargo.toml','Cargo.lock'])
comparison=json.loads((root/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(root/'deny.toml')
print('PASS: six gates; 3253 passed / 3 ignored; selection and folder-tabs native workflows each 14/14; exact selection transitions and native snapshot names; source/binary/tool hashes. Process restart UI capture and previous dependency audit failures remain open.')
