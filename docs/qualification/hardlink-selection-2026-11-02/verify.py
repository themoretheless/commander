from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3150 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clips in [('hardlink_selection','native-launch.json',1),('inode_sort','inode_sort-launch.json',0)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
d=q/'hardlink_selection';facts={r['name']:r for r in json.loads((d/'hardlink-facts.json').read_text())};key=lambda r:(r['device'],r['inode'])
assert key(facts['hard-original.txt'])==key(facts['hard-alias.txt'])
assert facts['hard-original.txt']['links']==facts['hard-alias.txt']['links']==2
assert key(facts['hard-copy.txt'])!=key(facts['hard-original.txt']) and facts['hard-link.txt']['symlink']
for stage in range(2):
 s=json.loads((d/f'sort-{stage}.json').read_text());assert s['cursor']=='hard-original.txt' and s['selected']==3
 assert len(s['entries'])==4
 assert sorted(r['name'] for r in s['entries'] if r['selected'])==['hard-alias.txt','hard-original.txt']
 for r in s['entries']:
  assert r['identity']==list(key(facts[r['name']]))
  assert r['regular_identity']==(None if r['symlink'] else r['identity'])
report=json.loads((d/'hardlink-selection.json').read_text());assert report['exported_rows']==2 and report['selected_rows']==3 and report['hidden_selected_rows']==1
assert sorted(r['name'] for r in report['rows'])==['hard-alias.txt','hard-original.txt']
for r in report['rows']:assert (r['device_id'],r['file_id'])==key(facts[r['name']]) and r['hard_links']==2
d=q/'inode_sort';facts={r['name']:r for r in json.loads((d/'inode-facts.json').read_text())}
for stage,(column,order) in enumerate([('Inode','Asc'),('Inode','Desc'),('Name','Asc'),('Name','Desc'),('Inode','Asc'),('Inode','Desc')]):
 s=json.loads((d/f'sort-{stage}.json').read_text());assert s['cursor']=='inode-middle.txt' and s['selected']==2 and s['config']['sort_col']==column and s['config']['sort_order']==order
 for r in s['entries']:assert r['identity']==[facts[r['name']]['device'],facts[r['name']]['inode']]
 if column=='Name':expected=sorted(facts,reverse=order=='Desc')
 else:
  keys=sorted({(r['inode'],r['device']) for r in facts.values()},reverse=order=='Desc')
  expected=[name for key in keys for name in sorted(facts) if (facts[name]['inode'],facts[name]['device'])==key]
 assert [r['name'] for r in s['entries']]==expected
print('PASS: six checks; 3150 tests; native hard-link selection and JSON agree with independent raw identity/link facts, preserving hidden picks and cursor; neighboring six-order inode sort agrees; two native scenarios 14/14 each')
