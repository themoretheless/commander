from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3140 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clips in [('allocation_search','native-launch.json',0),('search_json','search_json-launch.json',5)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
d=q/'allocation_search';facts={r['name']:r for r in json.loads((d/'allocation-facts.json').read_text())}
assert facts['disk-empty']['allocated']==facts['disk-sparse']['allocated']==0
assert facts['disk-dense']['allocated']>0 and facts['disk-sparse']['logical']==8*1024*1024
for name,expression,expected in [('allocated-4.json','allocated:=0 name:disk-',['disk-empty','disk-sparse']),('allocated-7.json','allocated:>0 name:disk-',['disk-dense']),('allocated-final.json','allocated:=0 size:>1MB name:disk-',['disk-sparse'])]:
 s=json.loads((d/name).read_text());assert s['expression']==expression and s['source']=='Live + archives' and s['matched']==len(expected) and s['skipped']==1
 assert sorted(r['name'] for r in s['rows'])==expected
 for r in s['rows']:
  assert r['allocated']==facts[r['name']]['allocated'] and r['size']==facts[r['name']]['logical']
  assert any(f"observed {r['allocated']} allocated bytes" in detail for detail in r['explanations'])
d=q/'search_json';reports={name:json.loads((d/f'report-{name}.json').read_text()) for name in ['all','selected','visible','draft','empty']}
assert [reports[n]['exported_rows'] for n in ['all','selected','visible','draft','empty']]==[5,2,4,4,0]
assert reports['draft']['context']['draft_differs'] and reports['draft']['context']['expression']==reports['all']['context']['expression']=='report-'
assert reports['empty']['context']['completed'] and reports['empty']['context']['expression']=='does-not-exist'
for report in reports.values():
 assert not report['partial']
 for row in report['rows']:
  if row['location']['kind']=='archive_member':assert row['allocated_bytes'] is None
  else:assert type(row['allocated_bytes']) is int and row['allocated_bytes']>0
print('PASS: six checks, 3140 tests; physical/logical allocation queries match raw block counts; allocation Find 16/16 and search JSON 16/16')
