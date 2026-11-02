from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/diff-swap-sides-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest();binary=h('target/debug/commander')
for key,dest,scene in [('native-launch.json','native','diff_swap'),('diff_options-launch.json','native-options','diff_options')]:
 launch=load(q/key);assert launch['binary_sha256']==launch['core_sha256']==binary
 out=Path(launch['out']);src=out/scene;m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
oldleft='HEAD  a\r\nsame\nleft one\nleft two\nend';oldright='head a\r\nsame\nright\nend\n'
newleft='HEAD  a\r\nsame\nleft UPDATED\nend';newright='head a\r\nsame\nright UPDATED\nend\n'
initial=load(q/'native/swap-0.json');right,left=map(Path,initial['paths'])
for phase in [0,3,6,9,12,15,17,20,21,23,26]:
 s=load(q/f'native/swap-{phase}.json');reversed=phase in [15,26]
 assert s['phase']==phase and s['paths']==list(map(str,[left,right] if reversed else [right,left]))
 assert s['source']==([oldleft,oldright] if reversed else [oldright,oldleft])
 assert s['names']==(['diff.txt (left)','diff.txt (right)'] if reversed else ['diff.txt (right)','diff.txt (left)'])
 assert s['options']==dict(space_changes=phase>=3,ignore_case=phase>=6)
 assert s['intraline']==(phase>=9) and s['numbers']==(phase>=12)
 assert s['current_left']==str(left.parent) and s['current_right']==str(right.parent)
 counts=tuple(sum(r['kind']==k for r in s['rows']) for k in ['Insert','Delete'])
 assert counts==((4,3) if phase<6 else (2,3) if reversed else (3,2))
 if phase>=6:
  assert s['rows'][0]==dict(kind='Equal',text='HEAD  a' if reversed else 'head a')
 # Independently count source positions from each kind, including EOF changes.
 a=b=1;expected=[]
 for row in s['rows']:
  k=row['kind'];expected.append([a if k!='Insert' else None,b if k!='Delete' else None]);a+=k!='Insert';b+=k!='Delete'
 assert s['source_numbers']==expected
 if phase in [20,21,23]:assert s['query_focused']
assert load(q/'native/swap-21.json')['find_query']=='left'
assert load(q/'native/swap-23.json')['find_query']=='' # Ctrl+U edited the field, no reversal.
final=load(q/'native/swap-final.json');assert final['phase']==29 and final['paths']==list(map(str,[left,right])) and final['source']==[newleft,newright]
assert final['options']==dict(space_changes=True,ignore_case=True) and final['intraline'] and final['numbers']
assert tuple(sum(r['kind']==k for r in final['rows']) for k in ['Insert','Delete'])==(2,2)
assert all(f'Diff swap QA phase {phase}\n' in (q/'native.log').read_text() for phase in range(29))
options=load(q/'native-options/options-final.json');assert options['counts']==[1,1] and options['options']==dict(space_changes=True,ignore_case=True)
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3072 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/app/diff_dialog.rs','src/app/keys.rs','src/diff_navigation.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',swap_native_checks=14,options_regression_checks=14,input_phases=29,exact_sources_and_operand_paths=True,source_numbers=True,text_field_ownership=True,retained_sources_until_explicit_reload=True,all_effect_calls=0,binary_matches_launch=True,full_tests_passed=3072,ignored=3,checks=6),indent=2)+'\n')
print('14 swap + 14 options native checks; source direction, field ownership, explicit Reload and six checks verified')
