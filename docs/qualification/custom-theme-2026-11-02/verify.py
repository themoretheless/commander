from pathlib import Path
import hashlib,json,math,struct,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3261 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['custom_theme','shortcut_exchange']:
 launch=json.loads((q/('native-launch.json' if scene=='custom_theme' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==(16 if scene=='custom_theme' else 14) and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=int(scene=='shortcut_exchange'),opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='custom_theme':
  draft=json.loads((d/'theme-draft.json').read_text());applied=json.loads((d/'theme-applied.json').read_text());assert draft==applied
  assert applied['format']=='commander.theme' and applied['version']==1 and len(applied['colors'])==15
  assert applied['colors']['bg_panel']=='#182838' and applied['colors']['accent']=='#389dcc'
  assert all(len(c)==7 and c[0]=='#' and int(c[1:],16)>=0 for c in applied['colors'].values())
 else:
  old_bindings=[{'command':'Reverse sort order','keys':'F5'}];new_bindings=[{'command':'Move cursor down','keys':'N'}]
  assert json.loads((d/'shortcuts-export.json').read_text())=={'format':'commander.shortcuts','version':1,'bindings':old_bindings}
  before=json.loads((d/'before-apply.json').read_text());assert before['applied']==old_bindings and before['draft']==new_bindings
  assert json.loads((d/'after-apply.json').read_text())=={'applied':new_bindings,'cursor':1,'cursor_name':'Design','phase':16}
old=repo/'docs/qualification/image-metadata-json-2026-11-02';oldhash=json.loads((old/'source-sha256.json').read_text())
assert oldhash['Cargo.toml']==h(repo/'Cargo.toml') and oldhash['Cargo.lock']==h(repo/'Cargo.lock')
comparison=json.loads((repo/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(repo/'deny.toml')
print('PASS: six gates; 3261 passed / 3 ignored; custom-theme Apply native 16/16, shortcut exchange control 14/14; exact draft/applied theme JSON; source/binary/tool hashes. Existing dependency audit failures remain open.')
