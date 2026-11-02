from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/duplicate-name-filters-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
a=load(q/'native-launch.json');b=load(q/'sizes-launch.json')
assert a['binary_sha256']==a['core_sha256']==b['binary_sha256']==b['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
for launch,dest,clips in [(a,'native',0),(b,'native-sizes',3)]:
 out=Path(launch['out']);src=out/launch['scenario'];m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
expected={0:{'one.JPG','two.jpg','skip.jpg','note.txt'},8:{'one.JPG','two.jpg','skip.jpg','note.txt'},11:{'one.JPG','two.jpg'},14:{'one.JPG','two.jpg','nested/three.jpg'},17:{'one.JPG','two.jpg','nested/three.jpg'},20:{'two.jpg','nested/three.jpg'},26:{'two.jpg','nested/three.jpg'}}
for step,names in expected.items():
 s=load(q/f'native/names-{step}.json');assert s['issues']==0 and s['step']==step
 observed={p.split('/left/',1)[1] for p in s['files']};assert observed==names,(step,observed)
 assert s['recursive']==(step>=14)
 if step in [11,14,17,20,26]:assert s['applied']==dict(include='*.jpg',exclude='skip*',case=step>=20)
s=load(q/'native/names-8.json');assert s['draft']==dict(include='*.jpg',exclude='skip*',case=False) and s['applied']==dict(include='',exclude='',case=False)
s=load(q/'native/names-26.json');assert s['draft']['include']=='[' and s['applied']['include']=='*.jpg'
s=load(q/'native/names-final.json');assert s['phase']==32 and s['recursive'] and s['issues']==0 and len(s['files'])==6 and s['applied']==s['draft']==dict(include='',exclude='',case=False)
assert {p.split('/left/',1)[1] for p in s['files']}=={'one.JPG','two.jpg','skip.jpg','note.txt','nested/three.jpg','nested/skip.jpg'}
log=(q/'native.log').read_text();assert all(f'Duplicate names QA phase {phase},' in log for phase in range(32))
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3057 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/main.rs','src/duplicate_name_filter.rs','src/dedup_scan.rs','src/app/mod.rs','src/app/duplicates_dialog.rs','src/workspace.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',names_native_checks=14,sizes_native_checks=14,input_phases=32,draft_does_not_apply=True,excluded_paths_absent=True,case_policy_and_recursive_retained=True,invalid_draft_keeps_applied_results=True,reset_restores_six_copies=True,names_recording_effect_calls=0,sizes_recording_clipboard_writes=3,binary_matches_launch=True,full_tests_passed=3057,ignored=3,checks=6),indent=2)+'\n')
print('14 name-filter + 14 size-regression native checks, exact file sets, draft/apply/reset and final checks verified')
