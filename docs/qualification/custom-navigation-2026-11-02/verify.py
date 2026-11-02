from pathlib import Path
import hashlib,json,math,struct,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3242 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['custom_navigation','custom_shortcuts']:
 launch=json.loads((q/('native-launch.json' if scene=='custom_navigation' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='custom_shortcuts':
  shown=json.loads((d/'shortcut-state.json').read_text());assert shown['phase']==17 and shown['left_hidden'] and not shown['right_hidden'] and shown['left_sort']=='descending'
  expected=[{'command':'Toggle hidden files','keys':'Alt+G H'},{'command':'Reverse sort order','keys':'Alt+G S'}];assert shown['bindings']==expected
  names=['README.txt','quarterly-design-review-with-a-deliberately-long-filename-for-layout.txt','photo.png','notes.md','.shortcut-hidden.txt'];assert shown['names']==['Design']+sorted(names,key=str.casefold,reverse=True)
  assert Path(shown['left_path']).name=='left' and Path(shown['right_path'])==Path(shown['left_path']).parent/'right'
 else:
  before=json.loads((d/'list-navigation.json').read_text());after=json.loads((d/'icon-navigation.json').read_text())
  expected=['Design']+[f'nav{n:02}.txt' for n in range(10)]+['notes.md','photo.png','quarterly-design-review-with-a-deliberately-long-filename-for-layout.txt','README.txt']
  assert before['names']==after['names']==expected
  assert before['cursor']==5 and set(before['selected'])==set(expected[2:5])
  # The inspected native frame contains four columns; row motion must use four items.
  assert after['columns']==4 and after['mode']=='icons' and after['phase']==20
  assert after['cursor']==9 and set(after['selected'])==set(before['selected'])|set(expected[4:9])
  assert after['bindings']==[{'command':'Move cursor down','keys':'N'},{'command':'Extend selection down','keys':'Shift+N'},{'command':'Move to first item','keys':'B'}]
old=repo/'docs/qualification/image-metadata-json-2026-11-02';oldhash=json.loads((old/'source-sha256.json').read_text())
assert oldhash['Cargo.toml']==h(repo/'Cargo.toml') and oldhash['Cargo.lock']==h(repo/'Cargo.lock')
comparison=json.loads((repo/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(repo/'deny.toml')
print('PASS: six gates; 3242 tests; native list/Icons custom navigation and previous sequence/modal-isolation control each 14/14; independently expected cursor, selection, binding payloads and names; source/binary/tool hashes. Dependency audit remains open with unchanged graph/policy.')
