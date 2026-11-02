from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/duplicate-minimum-copies-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
launches=[load(q/name) for name in ['native-launch.json','duplicate_names-launch.json','duplicate_sizes-launch.json']]
binary=h('target/debug/commander');assert binary==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
for launch,dest,clips in zip(launches,['native','native-names','native-sizes'],[0,0,3]):
 assert launch['binary_sha256']==launch['core_sha256']==binary
 out=Path(launch['out']);src=out/launch['scenario'];m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
a={'a1.bin','a2.bin','a3.bin'};b={'z.bin','z-long-copy.bin'};br=b|{'nested/z-extra.bin'}
for phase,minimum,draft,recursive,expected in [(0,2,'',False,{10:a,64:b}),(3,2,'',False,{10:a,64:b}),(7,2,'3',False,{10:a,64:b}),(10,3,'3',False,{10:a}),(13,3,'3',True,{10:a,64:br}),(19,3,'1',True,{10:a,64:br}),(22,2,'',True,{10:a,64:br}),(25,2,'',False,{10:a,64:b}),(29,2,'3',False,{10:a,64:b})]:
 s=load(q/f'native/copies-{phase}.json');assert s['phase']==phase and s['minimum']==minimum and s['draft']==draft and s['recursive']==recursive and s['issues']==0
 observed={g['size']:{p.split('/left/',1)[1] for p in g['files']} for g in s['groups']};assert observed==expected,(phase,observed)
s=load(q/'native/copies-final.json');assert s['phase']==32 and s['minimum']==3 and s['draft']=='3' and not s['recursive'] and s['issues']==0 and s['keep']==[0]
assert len(s['groups'])==1 and s['groups'][0]['size']==10 and {p.split('/left/',1)[1] for p in s['groups'][0]['files']}==a
log=(q/'native.log').read_text();assert all(f'Duplicate copies QA phase {phase},' in log for phase in range(32))
for phase,expected in [(11,{'one.JPG','two.jpg'}),(14,{'one.JPG','two.jpg','nested/three.jpg'}),(20,{'two.jpg','nested/three.jpg'}),(26,{'two.jpg','nested/three.jpg'})]:
 s=load(q/f'native-names/names-{phase}.json');assert {p.split('/left/',1)[1] for p in s['files']}==expected and s['issues']==0
s=load(q/'native-names/names-final.json');assert len(s['files'])==6 and s['applied']==s['draft']==dict(include='',exclude='',case=False)
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3062 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/dedup_scan.rs','src/app/mod.rs','src/app/duplicates_dialog.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',copies_native_checks=14,names_native_checks=14,sizes_native_checks=14,input_phases=32,exact_threshold_groups=True,draft_does_not_apply=True,invalid_one_preserves_results=True,reset_restores_two=True,recursion_retains_threshold=True,keeper_count_remains_one=True,copies_and_names_effect_calls=0,sizes_recording_clipboard_writes=3,binary_matches_launch=True,full_tests_passed=3062,ignored=3,checks=6),indent=2)+'\n')
print('14 copies + 14 names + 14 sizes checks; exact groups, draft/apply/reset, keeper count and final checks verified')
