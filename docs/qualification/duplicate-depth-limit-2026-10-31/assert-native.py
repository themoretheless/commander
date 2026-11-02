from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/duplicate-depth-limit-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
a=load(q/'native-launch.json');b=load(q/'names-launch.json')
assert a['binary_sha256']==a['core_sha256']==b['binary_sha256']==b['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
for launch,dest in [(a,'native'),(b,'native-names')]:
 out=Path(launch['out']);src=out/launch['scenario'];m=load(src/'manifest.json');assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks']) and all(v==0 for v in m['native_effect_calls'].values())
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
root={'a.dat','b.dat'};all_files=root|{'one/c.dat','one/two/d.dat'}
for phase,depth,draft,names in [(0,None,'',root),(3,None,'',all_files),(7,None,'0',all_files),(10,0,'0',root),(16,0,'1',root),(19,1,'1',root|{'one/c.dat'}),(25,1,'bad',root|{'one/c.dat'}),(28,None,'',all_files)]:
 s=load(q/f'native/depth-{phase}.json');assert s['phase']==phase and s['depth']==depth and s['draft']==draft and s['recursive']==(phase!=0) and s['issues']==0
 assert {p.split('/left/',1)[1] for p in s['files']}==names
s=load(q/'native/depth-final.json');assert s['phase']==31 and s['depth'] is None and s['draft']=='' and not s['recursive'] and s['issues']==0 and {p.split('/left/',1)[1] for p in s['files']}==root
log=(q/'native.log').read_text();assert all(f'Duplicate depth QA phase {phase},' in log for phase in range(31))
for phase,names in [(11,{'one.JPG','two.jpg'}),(14,{'one.JPG','two.jpg','nested/three.jpg'}),(20,{'two.jpg','nested/three.jpg'}),(26,{'two.jpg','nested/three.jpg'})]:
 s=load(q/f'native-names/names-{phase}.json');assert {p.split('/left/',1)[1] for p in s['files']}==names and s['issues']==0
s=load(q/'native-names/names-final.json');assert len(s['files'])==6 and s['applied']==s['draft']==dict(include='',exclude='',case=False)
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3059 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/dedup_scan.rs','src/app/mod.rs','src/app/duplicates_dialog.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',depth_native_checks=14,names_native_checks=14,input_phases=31,exact_depth_file_sets=True,drafts_leave_results_unchanged=True,invalid_draft_keeps_applied_limit=True,reset_restores_unlimited=True,recursion_off_restores_listing_scope=True,recording_effect_calls=0,binary_matches_launch=True,full_tests_passed=3059,ignored=3,checks=6),indent=2)+'\n')
print('14 depth + 14 name-filter checks, exact file sets, Apply/Reset behavior and final checks verified')
