from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/diff-comparison-options-2026-10-31')
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
load=lambda p:json.loads(p.read_text())
launch=load(q/'native-launch.json');navlaunch=load(q/'navigation-launch.json')
assert launch['binary_sha256']==launch['core_sha256']==navlaunch['binary_sha256']==navlaunch['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
for info,count,dest in [(launch,14,'native'),(navlaunch,15,'native-navigation')]:
 out=Path(info['out']);src=out/info['scenario'];m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==count and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
s=load(q/'native/options-final.json')
assert s['phase']==18 and s['options']==dict(space_changes=True,ignore_case=True) and s['counts']==[1,1]
assert s['source']==['alpha beta\nsame\nkept\nnew\nend\n','alpha  beta\nSAME\nkept\nold\nend\n']
assert s['rows']==[dict(kind=k,text=t) for k,t in [('Equal','alpha beta'),('Equal','same'),('Equal','kept'),('Delete','new'),('Insert','old'),('Equal','end')]]
assert s['source_numbers']==[[1,1],[2,2],[3,3],[4,None],[None,4],[5,5]] and s['hunks']==[dict(left_start=4,right_start=4)]
for phase,n,spaces,case in [(0,3,False,False),(3,2,True,False),(6,1,True,True),(9,2,False,True),(12,3,False,False),(15,2,True,False)]:
 state=load(q/f'native/options-{phase}.json');assert state['phase']==phase and state['counts']==[n,n] and state['options']==dict(space_changes=spaces,ignore_case=case)
log=(q/'native.log').read_text();assert all(f'Diff options QA phase {phase},' in log for phase in range(18))
r=load(q/'native-navigation/reload-state.json')
assert r['compact'] and r['show_numbers'] and r['display_rows']==37 and r['hidden']==152 and r['find_rows']==[52] and r['goto_selected'] is None and len(r['hunks'])==4
assert r['current']==0 and 10 in r['visible']
assert ''.join(row['text']+'\n' for row in r['rows'] if row['kind']!='Insert')==r['right_text']
assert ''.join(row['text']+'\n' for row in r['rows'] if row['kind']!='Delete')==r['left_text']
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3050 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/textdiff.rs','src/app/mod.rs','src/app/diff_dialog.rs','src/app/ui_state.rs','src/app/update.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',options_native_checks=14,navigation_native_checks=15,option_pointer_phases=18,changed_counts=[3,2,1,2,3,2,1],retained_sources_exact=True,display_original_minus_text=True,source_coordinates_verified=True,existing_navigation_operands_reconstructed=True,recording_effect_port_calls=0,matching_binary_launch_hashes=True,full_suite_passed=3050,ignored=3,checks=6),indent=2)+'\n')
print('14 options + 15 navigation native checks, policy round trips, original text/coordinates and final checks verified')
