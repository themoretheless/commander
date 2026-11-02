from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
hashfile=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3280 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in read(q/'source-sha256.json').items():assert hashfile(root/p)==h,p
scenes={}
for scene,receipt,clipboard in [('sync_review','native-launch.json',5),('sync_contents','sync_contents-launch.json',2)]:
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
report=read(p/'plan-shown.json');assert report['exported_rows']==9 and report['full_planned']==8
p=scenes['sync_contents'];report=read(p/'final-report.json');assert report['exported_rows']==14 and report['comparison']=='shallow_name_size_mtime_and_bytes'
last=read(p/'contents-24.json');assert last['contents'] and last['stamp_contents']
assert sum(a['content']=='equal' for a in last['actions'])==2
assert all(a['direction']=='Skip' for a in last['actions'] if a['error'])
print('PASS: six gates, source/binary hashes, native masks and content control, exact targets and effects')
