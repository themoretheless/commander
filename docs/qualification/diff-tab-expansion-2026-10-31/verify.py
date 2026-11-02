from pathlib import Path
import hashlib,json,shutil
Q=Path(__file__).resolve().parent
root=Q.parents[2]
checks=json.loads((Q/'checks.json').read_text())
assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3105 passed; 0 failed; 3 ignored' in (Q/'tests.log').read_text()
for scene,key in [('diff_options','native-launch.json'),('diff_swap','diff_swap-launch.json'),('diff_options','native-first-launch.json')]:
 launch=json.loads((Q/key).read_text())
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene;target=Q/(scene+'-first' if key=='native-first-launch.json' else scene)
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 log=Path(launch['out']+'.log')
 if log.exists():shutil.copy2(log,target/'native.log')
 m=json.loads((target/'manifest.json').read_text())
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
counts=[5,4,5,4,3,2,3,4,3,2,2,2,2,1,2,1,1]
steps=['expand_tabs']*3+['space_changes','ignore_case','space_changes','ignore_case','space_changes','ignore_case']+['all_spaces']*3+['ignore_crlf']*3+['reload','swap']
options=dict(space_changes=False,ignore_case=False,all_spaces=False,ignore_crlf=False,expand_tabs=False)
for i,step in enumerate(steps):
 s=json.loads((Q/'diff_options'/f'options-{i*3}.json').read_text())
 assert s==json.loads((Q/'diff_options-first'/f'options-{i*3}.json').read_text())
 assert s['phase']==i*3 and s['counts']==[counts[i]]*2,(i,s['counts'])
 assert s['options']==options,(i,s['options'],options)
 if i in [1,3]:
  assert any(r['kind']=='Equal' and r['text']=='x       z' for r in s['rows'])
 if step in options:options[step]=not options[step]
r=json.loads((Q/'diff_options/options-final.json').read_text())
assert r['phase']==51 and r['counts']==[1,1] and r['options']==options
assert all(options.values())
assert r['source']==['alpha  beta\nSAME\nk e p t\r\nx\tz\nold\nend\n','alpha beta\nsame\nkept\nx       z\nnew\nend\n']
assert [x['text'] for x in r['rows']]==['alpha  beta','SAME','k e p t','x\tz','old','new','end']
assert r['hunks']==[dict(left_start=5,right_start=5)]
assert r['paths'][0].endswith('/left/diff.txt') and r['paths'][1].endswith('/right/diff.txt')
print('PASS: six checks, 3105 tests; native options 14/14 twice, swap 14/14; seventeen interactions, tab toggle reversal, original sources, Reload and Swap')
