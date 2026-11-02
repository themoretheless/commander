from pathlib import Path
import hashlib,json,math,struct,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3247 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['shortcut_exchange','custom_shortcuts']:
 launch=json.loads((q/('native-launch.json' if scene=='shortcut_exchange' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=int(scene=='shortcut_exchange'),opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='custom_shortcuts':
  shown=json.loads((d/'shortcut-state.json').read_text());assert shown['phase']==17 and shown['left_hidden'] and not shown['right_hidden'] and shown['left_sort']=='descending'
  expected=[{'command':'Toggle hidden files','keys':'Alt+G H'},{'command':'Reverse sort order','keys':'Alt+G S'}];assert shown['bindings']==expected
  names=['README.txt','quarterly-design-review-with-a-deliberately-long-filename-for-layout.txt','photo.png','notes.md','.shortcut-hidden.txt'];assert shown['names']==['Design']+sorted(names,key=str.casefold,reverse=True)
  assert Path(shown['left_path']).name=='left' and Path(shown['right_path'])==Path(shown['left_path']).parent/'right'
 else:
  raw=(d/'shortcuts-export.json').read_bytes();assert len(raw)<=65536
  exported=json.loads(raw);old_bindings=[{'command':'Reverse sort order','keys':'F5'}];new_bindings=[{'command':'Move cursor down','keys':'N'}]
  assert exported=={'format':'commander.shortcuts','version':1,'bindings':old_bindings}
  imported=json.loads((d/'shortcut-import.json').read_text());assert imported=={'format':'commander.shortcuts','version':1,'bindings':new_bindings}
  before=json.loads((d/'before-apply.json').read_text());assert before['applied']==old_bindings and before['draft']==new_bindings
  after=json.loads((d/'after-apply.json').read_text());assert after=={'applied':new_bindings,'cursor':1,'cursor_name':'Design','phase':16}
old=repo/'docs/qualification/image-metadata-json-2026-11-02';oldhash=json.loads((old/'source-sha256.json').read_text())
assert oldhash['Cargo.toml']==h(repo/'Cargo.toml') and oldhash['Cargo.lock']==h(repo/'Cargo.lock')
comparison=json.loads((repo/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(repo/'deny.toml')
print('PASS: six gates; 3247 tests; native export/import/Apply/motion and previous sequence/modal-isolation control each 14/14; actual recorded clipboard JSON and pre/post-Apply ownership independently matched; source/binary/tool hashes. Dependency audit remains open with unchanged graph/policy.')
