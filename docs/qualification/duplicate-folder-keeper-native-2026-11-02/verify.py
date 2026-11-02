from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
hashfile=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3282 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in read(q/'source-sha256.json').items():assert hashfile(root/p)==h,p
scenes={}
for scene,receipt,clipboard in [('duplicate_cross','native-launch.json',2),('duplicate_roots','duplicate_roots-launch.json',1)]:
 launch=read(q/receipt);assert launch['binary_sha256']==launch['core_sha256']==hashfile(root/'target/debug/commander')
 d=Path(launch['out'])/scene;m=read(d/'manifest.json');assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=clipboard,opener=0,trash=0,free_space=0)
 assert (d/'frame.png').stat().st_size>10000;scenes[scene]=d
p=scenes['duplicate_cross'];before=read(p/'keeper-24.json');primary=read(p/'keeper-27.json');other=read(p/'keeper-30.json')
assert before['context']==primary['context']==other['context']
for report,key in [(primary,'root'),(other,'additional_root')]:
 assert report['summary']==dict(groups=2,files=4,proposed_trash_files=2,proposed_trash_bytes=23,scan_issues=0,partial=False)
 for group in report['groups']:
  kept=[f for f in group['files'] if f['action']=='keep'];trash=[f for f in group['files'] if f['action']=='trash']
  assert len(kept)==len(trash)==1
  assert Path(kept[0]['path']).is_relative_to(report['context'][key])
  assert not Path(trash[0]['path']).is_relative_to(report['context'][key])
assert read(p/'cross-report.json')==other
projection=lambda j:[(g['original_group'],g['size_bytes'],[(f['member'],f['path']) for f in g['files']]) for g in j['groups']]
assert projection(before)==projection(primary)==projection(other)
control=read(scenes['duplicate_roots']/'two-roots-report.json');assert control['summary']['groups']==4 and control['summary']['scan_issues']==0
print('PASS: six gates, 3282 tests, source/binary identities, native root preference, exact reports and effects')
