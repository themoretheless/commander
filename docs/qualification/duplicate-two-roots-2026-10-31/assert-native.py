from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/duplicate-two-roots-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
a=load(q/'native-launch.json');b=load(q/'duplicate_report-launch.json');binary=h('target/debug/commander');assert binary==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
for launch,dest,clips in [(a,'native',1),(b,'native-report',3)]:
 assert launch['binary_sha256']==launch['core_sha256']==binary
 out=Path(launch['out']);src=out/launch['scenario'];m=load(src/'manifest.json');assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
base=load(q/'native/roots-0.json');left=Path(base['root']);right=Path(base['other_root']);assert left!=right
local=frozenset([('1','own-1.bin'),('1','own-2.bin')]);cross=frozenset([('1','cross.bin'),('2','cross.bin')]);other=frozenset([('2','invoice.pdf'),('2','release.zip')]);deep=frozenset([('1','nested/deep.bin'),('2','nested/deep.bin')])
def member(p):
 p=Path(p)
 if p.is_relative_to(left):return ('1',str(p.relative_to(left)))
 assert p.is_relative_to(right);return ('2',str(p.relative_to(right)))
def groups(rows):return {frozenset(member(p) for p in g['files']) for g in rows}
for phase,both,recursive,want in [(0,False,False,{local}),(3,True,False,{local,cross,other}),(6,True,True,{local,cross,other,deep}),(9,False,True,{local}),(12,False,False,{local}),(15,True,False,{local,cross,other}),(18,True,True,{local,cross,other,deep})]:
 s=load(q/f'native/roots-{phase}.json');assert s['phase']==phase and s['root']==str(left) and s['other_root']==str(right) and s['both']==both and s['recursive']==recursive and s['issues']==0 and groups(s['groups'])==want
 assert s['fresh']==(phase!=0)
 if phase>=9:assert s['current_left']==str(left/'nested') and s['current_right']==str(right/'nested')
r=load(q/'native/two-roots-report.json');assert r['format']=='commander_duplicate_report' and r['version']==1
c=r['context'];assert c['root']==str(left) and c['additional_root']==str(right) and c['scope']=='two_trees' and c['metadata_source']=='fresh_tree_enumeration' and c['max_subfolder_levels'] is None and c['minimum_copies']==2
assert r['summary']==dict(groups=4,files=8,proposed_trash_files=4,proposed_trash_bytes=42,scan_issues=0,partial=False)
assert {frozenset(member(f['path']) for f in g['files']) for g in r['groups']}=={local,cross,other,deep}
assert all(len(g['files'])==2 and sorted(f['action'] for f in g['files'])==['keep','trash'] for g in r['groups'])
log=(q/'native.log').read_text();assert all(f'Duplicate roots QA phase {phase},' in log for phase in range(21))
before=load(q/'native-report/proposal-before.json');skipped=load(q/'native-report/proposal-skipped.json');restored=load(q/'native-report/proposal-restored.json')
assert before==restored and 'additional_root' not in before['context'] and before['context']['scope']=='listing' and before['context']['metadata_source']=='retained_pane_listing'
assert before['summary']['proposed_trash_files']==3 and before['summary']['proposed_trash_bytes']==84 and skipped['summary']['proposed_trash_files']==0
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3067 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/dedup_scan.rs','src/app/mod.rs','src/app/duplicates_dialog.rs','src/duplicate_report.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',roots_native_checks=14,single_report_native_checks=14,input_phases=21,exact_root_group_sets=True,retained_roots_after_panel_context_change=True,fresh_single_root_fallback=True,combined_report_context=True,one_keeper_per_group=True,recording_clipboard_writes_roots=1,recording_clipboard_writes_single_report=3,other_recording_effect_calls=0,binary_matches_launch=True,full_tests_passed=3067,ignored=3,checks=6),indent=2)+'\n')
print('14 roots + 14 single-report checks, exact file sets, frozen roots/fresh fallback and final checks verified')
