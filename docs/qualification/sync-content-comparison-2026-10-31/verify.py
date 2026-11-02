from pathlib import Path
import hashlib,json,shutil
Q=Path(__file__).resolve().parent
root=Q.parents[2]
checks=json.loads((Q/'checks.json').read_text())
assert len(checks)==6 and all(x['exit_code']==0 for x in checks)
assert '3096 passed; 0 failed; 3 ignored' in (Q/'tests.log').read_text()
for scene,key,clips in [('sync_contents','native-launch.json',2),('sync_review','sync_review-launch.json',4)]:
 launch=json.loads((Q/key).read_text())
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene
 target=Q/scene
 if source.exists():
  shutil.copytree(source,target,dirs_exist_ok=True)
 manifest=json.loads((target/'manifest.json').read_text())
 assert manifest['status']=='captured' and len(manifest['checks'])==15
 assert all(x['passed'] for x in manifest['checks'])
 effects=manifest['native_effect_calls']
 assert effects['clipboard']==clips and all(v==0 for k,v in effects.items() if k!='clipboard')
p=Q/'sync_contents'
for phase in range(0,27,3):
 s=json.loads((p/f'contents-{phase}.json').read_text())
 assert s['contents']==s['stamp_contents']==(phase not in [0,12])
 counts={k:sum(a['content']==k for a in s['actions']) for k in ['equal','different','unavailable','unchecked']}
 expected=([0,0,0,14] if phase in [0,12] else [1,2,1,10] if phase<12 else [2,1,1,10])
 assert list(counts.values())==expected,(phase,counts)
 assert all(a['direction']=='Skip' for a in s['actions'] if a['content'] in ['equal','unavailable'])
 if phase==21: assert s['shown']==['unverified.lnk']
 if phase in [0,3,12,15,24]: assert len(s['shown'])==14
for filename,eq,diff in [('initial-report.json',1,2),('final-report.json',2,1)]:
 r=json.loads((p/filename).read_text());rows=r['rows']
 assert r['comparison']=='shallow_name_size_mtime_and_bytes'
 assert r['full_rows']==r['exported_rows']==len(rows)==14
 assert sum(x['content_comparison']=='equal' for x in rows)==eq
 assert sum(x['content_comparison']=='different' for x in rows)==diff
 unknown=[x for x in rows if x['content_comparison']=='unavailable']
 assert len(unknown)==1 and unknown[0]['direction']=='skip' and unknown[0]['content_error']
 rby={x['name']:x for x in rows}
 assert rby['same-bytes.txt']['direction']=='skip'
 assert rby['same-stat.bin']['content_comparison']==('different' if diff==2 else 'equal')
print('PASS: six checks, 3096 tests, native 15/15 + 15/15; content transitions, fresh bytes, safe skipping and reports')
