from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/diff-intraline-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
a=load(q/'native-launch.json');b=load(q/'navigation-launch.json')
assert a['binary_sha256']==a['core_sha256']==b['binary_sha256']==b['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
for launch,n,dest in [(a,14,'native'),(b,15,'native-navigation')]:
 out=Path(launch['out']);src=out/launch['scenario'];m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==n and all(c['passed'] for c in m['checks']) and all(v==0 for v in m['native_effect_calls'].values())
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
s=load(q/'native/intraline-final.json');assert s['enabled'] and s['phase']==9 and s['counts']==[2,1] and s['simplified_pairs']==0
assert s['ranges']=={'1':[dict(start=12,end=15),dict(start=30,end=31)],'2':[dict(start=12,end=13),dict(start=14,end=15),dict(start=29,end=30)]}
assert s['source_numbers']==[[1,1],[2,None],[None,2],[3,3],[None,4],[4,5]]
assert ''.join(row['text']+'\n' for row in s['rows'] if row['kind']!='Insert')==s['source'][0]
assert ''.join(row['text']+'\n' for row in s['rows'] if row['kind']!='Delete')==s['source'][1]
for phase,enabled in [(0,False),(3,True),(6,False)]:
 t=load(q/f'native/intraline-{phase}.json');assert t['enabled']==enabled and t['counts']==[2,1]
 if not enabled:assert t['ranges']=={}
log=(q/'native.log').read_text();assert all(f'Diff intraline QA phase {phase},' in log for phase in range(9))
r=load(q/'native-navigation/reload-state.json');assert r['display_rows']==37 and r['hidden']==152 and r['find_rows']==[52] and r['show_numbers'] and len(r['hunks'])==4 and 10 in r['visible']
for excluded,source in [('Insert','right_text'),('Delete','left_text')]:assert ''.join(row['text']+'\n' for row in r['rows'] if row['kind']!=excluded)==r[source]
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3054 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/main.rs','src/diff_intraline.rs','src/app/mod.rs','src/app/diff_dialog.rs','src/app/ui_state.rs','src/app/update.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',intraline_native_checks=14,navigation_native_checks=15,control_pointer_phases=9,exact_original_text_reconstructed=True,exact_character_spans=True,source_coordinates_verified=True,recording_effect_port_calls=0,binary_matches_launch=True,full_tests_passed=3054,ignored=3,checks=6),indent=2)+'\n')
print('14 intraline + 15 navigation checks; text, exact spans, coordinates and final checks verified')
