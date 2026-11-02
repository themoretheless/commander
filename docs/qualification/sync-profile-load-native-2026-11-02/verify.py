from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
hashfile=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3281 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in read(q/'source-sha256.json').items():assert hashfile(root/p)==h,p
scenes={}
for scene,receipt,clipboard in [('sync_review','native-launch.json',6),('sync_contents','sync_contents-launch.json',2)]:
 launch=read(q/receipt);assert launch['binary_sha256']==hashfile(root/'target/debug/commander')==launch['core_sha256']
 d=Path(launch['out'])/scene;m=read(d/'manifest.json');assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==15 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=clipboard,opener=0,trash=0,free_space=0)
 assert (d/'frame.png').stat().st_size>10000
 scenes[scene]=d
p=scenes['sync_review'];final=read(p/'review-state.json');base=final['baseline']
for phase,direction in [(63,'Skip'),(66,'ToRight'),(69,'Skip')]:
 snap=read(p/f'mask-phase-{phase}.json');assert snap['masks'] and snap['query']=='*.MD;!read*'
 assert len(snap['indices'])==1 and snap['rows'][snap['indices'][0]]==dict(name='notes.md',direction=direction)
 assert snap['hidden_planned']==8
 assert {r['name']:r['direction'] for r in snap['rows'] if r['name']!='notes.md'}=={r['name']:r['direction'] for r in base if r['name']!='notes.md'}
assert final['query']=='' and final['hidden_planned']==0 and len(final['indices'])==9
report=read(p/'plan-shown.json');assert report['exported_rows']==9 and report['full_planned']==9
p=scenes['sync_contents'];report=read(p/'final-report.json');assert report['exported_rows']==14 and report['comparison']=='shallow_name_size_mtime_and_bytes'
last=read(p/'contents-24.json');assert last['contents'] and last['stamp_contents']
assert sum(a['content']=='equal' for a in last['actions'])==2
assert all(a['direction']=='Skip' for a in last['actions'] if a['error'])
print('PASS: six gates, source/binary hashes, native masks and content control, exact targets and effects')

import csv
p=scenes['sync_review']
for phase in [63,66,69]:
 report=read(p/f'mask-report-{phase}.json')
 assert report['view_match_mode']=='whole_name_masks' and report['view_query']=='*.MD;!read*'
 assert report['exported_rows']==1 and report['rows'][0]['name']=='notes.md'
assert read(p/'plan-shown.json')['view_match_mode']=='contains_all_words'
with (p/'plan-full.csv').open(newline='') as file:
 reader=csv.DictReader(file);assert reader.fieldnames[-1]=='View match mode'
 assert all(row['View match mode']=='contains_all_words' for row in reader)
assert read(scenes['sync_contents']/'final-report.json')['view_match_mode']=='contains_all_words'
print('PASS: native mask/word metadata and appended CSV column')

p=scenes['sync_review']
saved=read(p/'profile-phase-91.json');renamed=read(p/'profile-phase-105.json');deleted=read(p/'profile-phase-108.json')
assert saved['selected']=='Work' and renamed['selected']=='Renamed' and deleted['selected'] is None
before=saved['saved']['profiles'];after=renamed['saved']['profiles']
assert len(before)==len(after)==1 and before[0]['name']=='Work' and after[0]['name']=='Renamed'
expected=dict(before[0]);expected['name']='Renamed';assert after[0]==expected
assert deleted['saved']['profiles']==[]
final=read(p/'review-state.json');assert before[0]['left']==final['left'] and before[0]['right']==final['right']
assert before[0]['policy']=='TwoWay' and not before[0]['compare_contents'] and not before[0]['dry_run']
print('PASS: native save, rename-only metadata change, selected deletion and unchanged comparison plan')

p=scenes['sync_review'];swapped=read(p/'comparison-phase-94.json');loaded=read(p/'comparison-phase-97.json')
original=read(p/'profile-phase-91.json')['saved']['profiles'][0]
assert swapped['left']==original['right'] and swapped['right']==original['left']
assert loaded['left']==original['left'] and loaded['right']==original['right']
for snapshot,direction in [(swapped,'ToLeft'),(loaded,'ToRight')]:
 assert snapshot['stamp']['left_root']==snapshot['left'] and snapshot['stamp']['right_root']==snapshot['right']
 assert not snapshot['allow_large_plan'] and snapshot['query']=='' and snapshot['sort']=='plan_order'
 assert next(r['direction'] for r in snapshot['directions'] if r['name']=='notes.md')==direction
final=read(p/'review-state.json');assert final['rows']==final['baseline']
assert final['sort']=='plan_order' and not final['descending']
print('PASS: native side swap, saved-root restoration, fresh stamped/default plan, unchanged file boundary')
