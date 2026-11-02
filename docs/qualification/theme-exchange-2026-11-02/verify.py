from pathlib import Path
import hashlib,json,math,struct,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3261 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['theme_exchange','custom_theme']:
 launch=json.loads((q/('native-launch.json' if scene=='theme_exchange' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=int(scene=='theme_exchange'),opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='theme_exchange':
  imported=json.loads((d/'theme-import.json').read_text());exported=json.loads((d/'theme-export.json').read_text());assert exported==imported
  assert imported['colors']['bg_panel']=='#182838' and imported['format']=='commander.theme' and imported['version']==1 and len(imported['colors'])==15
  for file,phase in [('applied',10),('rejected',21),('copied',25),('reset',29)]:
   v=json.loads((d/(file+'.json')).read_text());assert v['phase']==phase
   if file=='reset':assert v['saved'] is None and v['panel']==[27,24,21]
   else:assert v['saved']==imported and v['panel']==[24,40,56]
   assert v['draft']==(json.dumps(imported,ensure_ascii=False,indent=2) if file=='applied' else 'invalid theme draft')
   assert (v['error'] is None)==(file=='applied')
 else:
  draft=json.loads((d/'theme-draft.json').read_text());assert json.loads((d/'theme-applied.json').read_text())==draft
  assert draft['colors']['bg_panel']=='#182838' and draft['colors']['accent']=='#389dcc'
old=repo/'docs/qualification/image-metadata-json-2026-11-02';oldhash=json.loads((old/'source-sha256.json').read_text())
assert oldhash['Cargo.toml']==h(repo/'Cargo.toml') and oldhash['Cargo.lock']==h(repo/'Cargo.lock')
comparison=json.loads((repo/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(repo/'deny.toml')
print('PASS: six gates; 3261 passed / 3 ignored; theme exchange and prior Apply each native 16/16; actual pasted JSON, rejection preservation, applied-only clipboard export and reset RGB/data ownership; source/binary/tool hashes. Existing dependency audit failures remain open.')
