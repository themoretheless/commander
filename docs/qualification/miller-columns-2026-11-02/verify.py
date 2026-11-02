from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for name,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/name)==digest,name
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3202 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['miller_browser','folder_tabs','minimum_window','zoom_200_accessible']:
 key='native-launch.json' if scene=='miller_browser' else scene+'-launch.json'
 launch=json.loads((q/key).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['scenario']==scene and m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 assert m['ui_scale']==(2.0 if scene=='zoom_200_accessible' else 1.0)
 if scene!='miller_browser':continue
 steps={i:json.loads((d/f'step-{i}.json').read_text()) for i in [6,9,12,15,18,21,24,26,29]}
 a=Path(steps[6]['path']);left=a.parent;inner=a/'inner';file=a/'a.txt';assert a.name=='column-A' and left.name=='left'
 for i,r in steps.items():
  target=inner if i==9 else a;assert Path(r['path'])==target and r['phase']==i
  assert r['mode']==('List' if i==18 else 'Miller')
  expected=[] if i==18 else list(reversed(list(target.parents)[:8]));assert [Path(c['path']) for c in r['columns']]==expected
  assert r['selected']==([str(file)] if i>=15 else [])
  if i>=12:assert r['cursor']==str(file)
  for c in r['columns']:
   assert c['mode']=='List' and c['status']=='Listed'
   for p in map(Path,c['entries']):assert p.parent==Path(c['path']) and p.is_relative_to(left)
 assert {str(file),str(inner)}<=set(steps[9]['columns'][-1]['entries'])
 assert str(a) in steps[29]['columns'][-1]['entries']
 assert (d/'preview.txt').read_bytes()==(d/'source-A.txt').read_bytes()==b'Miller A owner\n'
 assert (d/'source-B.txt').read_bytes()==b'Miller B owner\n' and (d/'preview.txt').read_bytes()!=(d/'source-B.txt').read_bytes()
print('PASS: six checks; 3202 tests; four native workflows each 14/14 and zero external effects; independent ancestor routes, native file focus, selected owner, List cache release, Columns restoration and F3 content matched preserved A/B sources; source/binary/tool hashes and PNG dimensions.')
