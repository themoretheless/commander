from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3145 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key in [('inode_sort','native-launch.json'),('file_identity_info','file_identity_info-launch.json')]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
d=q/'inode_sort';facts={r['name']:r for r in json.loads((d/'inode-facts.json').read_text())}
assert (facts['inode-old.txt']['device'],facts['inode-old.txt']['inode'])==(facts['inode-new.txt']['device'],facts['inode-new.txt']['inode'])
assert facts['inode-link.txt']['symlink'] and facts['inode-link.txt']['inode']!=facts['inode-old.txt']['inode']
for stage,(column,order) in enumerate([('Inode','Asc'),('Inode','Desc'),('Name','Asc'),('Name','Desc'),('Inode','Asc'),('Inode','Desc')]):
 s=json.loads((d/f'sort-{stage}.json').read_text());assert s['cursor']=='inode-middle.txt' and s['selected']==2
 assert s['config']['sort_col']==column and s['config']['sort_order']==order
 rows=s['entries'];assert len(rows)==4
 for r in rows:assert r['identity']==[facts[r['name']]['device'],facts[r['name']]['inode']]
 if column=='Name':expected=sorted(facts,reverse=order=='Desc')
 else:
  keys=sorted({(r['inode'],r['device']) for r in facts.values()},reverse=order=='Desc')
  expected=[name for key in keys for name in sorted(facts) if (facts[name]['inode'],facts[name]['device'])==key]
 assert [r['name'] for r in rows]==expected,(stage,rows,expected)
 report=s['report'];assert report['view']['sort_col']==column and report['view']['sort_order']==order
 assert report['exported_rows']==1 and report['rows'][0]['name']=='inode-old.txt'
print('PASS: six checks; 3145 tests; six native sort orders match raw lexical inode/device facts, preserve cursor/hidden selection and JSON context; two native scenarios 14/14 each')
