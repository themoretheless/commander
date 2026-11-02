from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
reference=json.loads((q/'fd-reference.json').read_text());assert reference['results_relative_to_fixture']==['scope [literal]/child.txt','scope [literal]/nested']
assert hashlib.sha256(Path(reference['path']).read_bytes()).hexdigest()==reference['sha256']
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3142 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key in [('parent_search','native-launch.json'),('allocation_search','allocation_search-launch.json')]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
d=q/'parent_search';final=json.loads((d/'parent-final.json').read_text());base=Path(final['root'])
for name,expected,parent in [('parent-4.json',[base/'scope [literal]/child.txt',base/'scope [literal]/nested'],base/'scope [literal]'),('parent-7.json',[base/'scope [literal]/child.txt'],base/'scope [literal]'),('parent-final.json',[base/'bundle.zip!/nested/child.txt'],base/'bundle.zip!/nested')]:
 s=json.loads((d/name).read_text());assert s['source']=='Live + archives' and s['matched']==len(expected) and s['skipped']==0
 assert sorted(r['path'] for r in s['rows'])==sorted(str(p) for p in expected)
 assert str(parent) in s['expression']
 for row in s['rows']:
  assert Path(row['path']).parent==parent
  assert any(detail=='parent:'+str(parent) for detail in row['explanations'])
d=q/'allocation_search';facts={r['name']:r for r in json.loads((d/'allocation-facts.json').read_text())}
for filename,expected in [('allocated-4.json',['disk-empty','disk-sparse']),('allocated-7.json',['disk-dense']),('allocated-final.json',['disk-sparse'])]:
 s=json.loads((d/filename).read_text());assert sorted(r['name'] for r in s['rows'])==expected and s['skipped']==1
 for row in s['rows']:assert row['allocated']==facts[row['name']]['allocated'] and row['size']==facts[row['name']]['logical']
print('PASS: six checks; 3142 tests; literal immediate parents agree across native/index/archive queries; parent Find and neighboring allocation Find 16/16 each')
