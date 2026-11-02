from pathlib import Path
import hashlib,json,shutil
Q=Path(__file__).resolve().parent
root=Q.parents[2]
checks=json.loads((Q/'checks.json').read_text())
assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3102 passed; 0 failed; 3 ignored' in (Q/'tests.log').read_text()
for scene,key in [('diff_options','native-launch.json'),('diff_swap','diff_swap-launch.json')]:
 launch=json.loads((Q/key).read_text())
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene
 target=Q/scene
 if source.exists(): shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text())
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
counts=[4,3,2,3,4,3,2,2,2,2,1,2,1,1]
spaces=[False,True,True,False,False,True,True,True,True,True,True,True,True,True]
case=[False,False,True,True,False,False,True,True,True,True,True,True,True,True]
allspaces=[False]*7+[True,False,True,True,True,True,True]
endings=[False]*10+[True,False,True,True]
for i in range(14):
 s=json.loads((Q/'diff_options'/f'options-{i*3}.json').read_text())
 assert s['phase']==i*3 and s['counts']==[counts[i]]*2
 assert s['options']==dict(space_changes=spaces[i],ignore_case=case[i],all_spaces=allspaces[i],ignore_crlf=endings[i])
 assert all(r['text'].removesuffix('␍') in ['alpha beta','alpha  beta','same','SAME','kept','k e p t','new','old','end'] for r in s['rows'])
r=json.loads((Q/'diff_options/options-final.json').read_text())
assert r['phase']==42 and r['counts']==[1,1]
assert r['options']==dict(space_changes=True,ignore_case=True,all_spaces=True,ignore_crlf=True)
assert r['source']==['alpha  beta\nSAME\nk e p t\r\nold\nend\n','alpha beta\nsame\nkept\nnew\nend\n']
assert [x['text'] for x in r['rows']]==['alpha  beta','SAME','k e p t','old','new','end']
assert r['hunks']==[dict(left_start=4,right_start=4)]
assert r['paths'][0].endswith('/left/diff.txt') and r['paths'][1].endswith('/right/diff.txt')
print('PASS: six checks, 3102 tests; native options 14/14, swap 14/14; fourteen policy transitions, CRLF toggle reversal, original text, Reload and Swap')
