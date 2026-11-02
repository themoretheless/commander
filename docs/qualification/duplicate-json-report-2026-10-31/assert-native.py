from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/duplicate-json-report-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
a=load(q/'native-launch.json');b=load(q/'duplicate_sizes-launch.json');binary=h('target/debug/commander');assert binary==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
for launch,dest in [(a,'native'),(b,'native-sizes')]:
 assert launch['binary_sha256']==launch['core_sha256']==binary
 out=Path(launch['out']);src=out/launch['scenario'];m=load(src/'manifest.json');assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==3 and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
before=load(q/'native/proposal-before.json');skipped=load(q/'native/proposal-skipped.json');restored=load(q/'native/proposal-restored.json')
assert before==restored and (q/'native/proposal-before.json').read_bytes()==(q/'native/proposal-restored.json').read_bytes()
for report,all_skipped in [(before,False),(skipped,True),(restored,False)]:
 assert report['format']=='commander_duplicate_report' and report['version']==1 and report['snapshot']=='retained_duplicate_scan_proposal'
 c=report['context'];assert c==dict(root=c['root'],scope='listing',metadata_source='retained_pane_listing',max_subfolder_levels=None,configured_subfolder_levels=None,min_bytes=0,max_bytes=None,names=dict(include='',exclude='',case=False),minimum_copies=2,keep_policy='shortest_path',group_sort='savings')
 assert Path(c['root']).is_absolute()
 assert report['summary']==dict(groups=2,files=5,proposed_trash_files=0 if all_skipped else 3,proposed_trash_bytes=0 if all_skipped else 84,scan_issues=0,partial=False)
 assert [(g['group'],g['original_group'],g['size_bytes']) for g in report['groups']]==[(1,2,64),(2,1,10)]
 expected=[{'z-long-copy.bin':'trash','z.bin':'keep'},{'a1.bin':'keep','a2.bin':'trash','a3.bin':'trash'}]
 for group,want in zip(report['groups'],expected):
  assert [f['member'] for f in group['files']]==list(range(1,len(group['files'])+1))
  assert {Path(f['path']).name:f['action'] for f in group['files']}==({name:'skip' for name in want} if all_skipped else want)
  assert all(str(Path(f['path']).parent)==c['root'] for f in group['files'])
log=(q/'native.log').read_text();assert all(f'Duplicate report QA phase {phase},' in log for phase in range(18))
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3064 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
repeat=load(q/'listing-repeat.json');assert repeat['attempts']==64 and repeat['failed']==0 and all(r['exit_code']==0 for r in repeat['results'])
assert repeat['test_binary_sha256']==h(repeat['test_binary'])
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/main.rs','src/duplicate_report.rs','src/dedup.rs','src/app/duplicates_dialog.rs','src/panel.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',report_native_checks=14,csv_native_checks=14,input_phases=18,recording_clipboard_writes_each=3,other_recording_effect_calls=0,exact_sorted_groups_and_original_ids=True,retained_applied_context=True,skip_all_trashes_nothing=True,restored_report_byte_identical=True,binary_matches_launch=True,full_tests_passed=3064,ignored=3,listing_test_repeats=64,checks=6),indent=2)+'\n')
print('14 JSON + 14 CSV checks, exact reports/actions/context, 64 listing repeats and final checks verified')
