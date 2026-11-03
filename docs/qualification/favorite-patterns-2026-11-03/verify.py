from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3515 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==h,p
launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out'])/'favorite_picker'
assert launch['binary_sha256']==launch['core_sha256']==hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()
m=json.loads((out/'manifest.json').read_text());assert m['status']=='captured' and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
assert all(v==0 for v in m['native_effect_calls'].values())
for phase,mode,query,indices in [(41,'Match','@пркт',[]),(44,'Smart','@пркт',[150]),(49,'Smart','@ПРКТ',[]),(52,'Ignore','@ПРКТ',[150])]:
 r=json.loads((out/f'case-{phase}.json').read_text());assert r==dict(phase=phase,case=mode,query=query,total=len(indices),indices=indices)
for phase,mode,query,indices,error in [(60,'Literal','@пркт',[],False),(65,'Literal','@Проект',[150],False),(68,'Exact','@Проект',[],False),(73,'Exact','@Проект GTK',[150],False),(76,'Glob','@Проект GTK',[150],False),(81,'Glob','@Про* GTK',[150],False),(89,'Regex','@^Проект.*GTK$',[150],False),(94,'Regex','@[',[],True),(97,'Fuzzy','@[',[],False)]:
 r=json.loads((out/f'mode-{phase}.json').read_text());assert r['phase']==phase and r['mode']==mode and r['query']==query and r['indices']==indices and r['total']==len(indices) and bool(r['error'])==error
r=json.loads((out/'picker-state.json').read_text());assert len(r['bookmarks']['items'])==152 and r['query']=='@пркт' and r['indices']==[150] and r['matches']==1
(q/'verification.json').write_text(json.dumps(dict(checks=7,tests_passed=3515,tests_ignored=3,native_checks=16,case_checkpoints=4,mode_checkpoints=9,source_hashes_verified=True),indent=2)+'\n')
print('Verified seven gates, source hashes, native picker and thirteen matching checkpoints')
