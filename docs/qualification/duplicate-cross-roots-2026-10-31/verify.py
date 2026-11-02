from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/duplicate-cross-roots-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
binary=h('target/debug/commander')
for key,dest,scene in [('native-launch.json','native','duplicate_cross'),('duplicate_roots-launch.json','native-roots','duplicate_roots')]:
 launch=load(q/key);assert launch['binary_sha256']==launch['core_sha256']==binary
 out=Path(launch['out']);src=out/scene;m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==1 and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
base=load(q/'native/cross-0.json');left=Path(base['root']);right=Path(base['other_root'])
def member(p):
 p=Path(p)
 if p.is_relative_to(left):return ('1',str(p.relative_to(left)))
 assert p.is_relative_to(right);return ('2',str(p.relative_to(right)))
local=frozenset([('1','own-1.bin'),('1','own-2.bin')]);cross=frozenset([('1','cross.bin'),('2','cross.bin')]);other=frozenset([('2','invoice.pdf'),('2','release.zip')]);deep=frozenset([('1','nested/deep.bin'),('2','nested/deep.bin')])
for phase,want in [(0,{local}),(3,{local,cross,other}),(6,{cross}),(9,{cross,deep}),(12,{local,cross,other,deep}),(15,{cross,deep}),(18,{local}),(21,{cross,deep})]:
 s=load(q/f'native/cross-{phase}.json')
 assert s['root']==str(left) and s['other_root']==str(right) and s['issues']==0
 assert s['both']==(phase not in [0,18]) and s['recursive']==(phase>=9) and s['cross']==(phase>=6 and phase!=12)
 assert {frozenset(member(p) for p in g['files']) for g in s['groups']}==want
r=load(q/'native/cross-report.json');assert r['context']['only_cross_root_groups'] is True
assert r['context']['root']==str(left) and r['context']['additional_root']==str(right) and r['context']['scope']=='two_trees'
assert r['summary']==dict(groups=2,files=4,proposed_trash_files=2,proposed_trash_bytes=23,scan_issues=0,partial=False)
assert {frozenset(member(f['path']) for f in g['files']) for g in r['groups']}=={cross,deep}
assert all(sorted(f['action'] for f in g['files'])==['keep','trash'] for g in r['groups'])
assert all(f'Duplicate cross QA phase {phase},' in (q/'native.log').read_text() for phase in range(24))
r=load(q/'native-roots/two-roots-report.json');assert r['context']['only_cross_root_groups'] is False and r['summary']['groups']==4 and r['summary']['proposed_trash_bytes']==42
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3069 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/dedup_scan.rs','src/app/mod.rs','src/app/duplicates_dialog.rs','src/duplicate_report.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',cross_native_checks=14,roots_regression_checks=14,input_phases=24,exact_file_sets=True,retained_preference_when_disabled=True,report_context=True,clipboard_recording_writes_each=1,other_effect_calls=0,binary_matches_launch=True,full_tests_passed=3069,ignored=3,checks=6),indent=2)+'\n')
print('Cross-root exact file sets, report, retained preference, 14+14 native checks and six checks verified')
