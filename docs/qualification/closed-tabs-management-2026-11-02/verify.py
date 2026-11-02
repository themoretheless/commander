from pathlib import Path
import hashlib,json,struct
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for name,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/name)==digest,name
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3217 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['miller_browser','folder_tabs']:
 key='native-launch.json' if scene=='miller_browser' else scene+'-launch.json'
 launch=json.loads((q/key).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['scenario']==scene and m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 assert m['ui_scale']==(2.0 if scene=='zoom_200_accessible' else 1.0)
 if scene=='folder_tabs':
  states={i:json.loads((d/f'history-{i}.json').read_text()) for i in [87,93,99]}
  baseline=states[87]
  assert len(baseline['left']['closed'])==4 and len(baseline['right']['closed'])==2
  assert states[93]['left']['closed']==baseline['left']['closed'][:-1]
  assert states[99]['left'].get('closed',[])==[]
  for r in states.values():
   assert r['right']==baseline['right']
   assert r['left']['entries']==baseline['left']['entries'] and r['left']['active']==baseline['left']['active']
   assert r['revision']==baseline['revision'] and r['active']==baseline['active']=='Left'
   assert r['left_path']==baseline['left_path'] and r['right_path']==baseline['right_path']
  continue
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
 changes={i:json.loads((d/f'resize-{i}.json').read_text()) for i in [30,34,39,43,46,52,57]}
 revision=changes[30]['revision'];pane=m['left_pane']['rect'] if 'rect' in m['left_pane'] else m['left_pane']
 for i,r in changes.items():
  expected=(220,360) if i==30 else (280,360) if i in [34,52] else (280,300)
  assert (r['parent_width'],r['current_width'])==expected and r['revision']==revision and Path(r['path'])==a and r['selected']==[str(file)]
  assert r['mode']==('List' if i==43 else 'Miller')
  if i==43:assert r['columns']==[] and r['rects'] is None;continue
  assert list(map(Path,r['columns']))==list(reversed(list(a.parents)[:8]))
  parent,current=r['rects']
  for rect,width in zip(r['rects'],expected):
   assert all(isinstance(v,(int,float)) and abs(v)<100000 for v in rect.values())
   assert abs(rect['max_x']-rect['min_x']-width)<=0.51 and rect['max_y']>rect['min_y']
  assert abs(parent['max_x']+8-current['min_x'])<=0.51
  assert current['min_x']>=pane['min_x']-0.51 and current['max_x']<=pane['max_x']+0.51
 assert changes[46]['current_width']==changes[43]['current_width']==300 and changes[52]['current_width']==360 and changes[57]['current_width']==300
 keys={i:json.loads((d/f'keyboard-{i}.json').read_text()) for i in [58,60,62,64]}
 for i,r in keys.items():
  target=left if i==62 else a
  assert r['phase']==i and Path(r['path'])==target and r['mode']=='Miller'
  assert Path(r['cursor'])==(a if i==62 else file)
  assert Path(r['right_path'])==left.parent/'right'
  assert list(map(Path,r['columns']))==list(reversed(list(target.parents)[:8]))
  assert (r['parent_width'],r['current_width'])==(280,300)
  assert r['selected']==([] if i==62 else [str(file)])
 assert keys[58]['revision']==keys[60]['revision']==changes[57]['revision']
 assert keys[62]['revision']>keys[60]['revision'] and keys[64]['revision']>keys[62]['revision']
print('PASS: six gates; 3217 tests; both native workflows 14/14; exact history 4 -> 3 -> 0, independent right history and unchanged live owners; neighboring Miller/F3/resize/keyboard routes; source/binary/validator hashes and frame dimensions.')
