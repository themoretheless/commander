from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/relative)==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3177 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
for scene,key in [('hard_links_sort','native-launch.json'),('inode_sort','inode_sort-launch.json')]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 prefix='links' if scene=='hard_links_sort' else 'inode';metric='HardLinks' if prefix=='links' else 'Inode'
 facts={r['name']:r for r in json.loads((d/f'{prefix}-facts.json').read_text())}
 original=facts[f'{prefix}-old.txt'];alias=facts[f'{prefix}-new.txt'];link=facts[f'{prefix}-link.txt']
 assert (original['device'],original['inode'])==(alias['device'],alias['inode'])
 assert link['symlink'] and link['inode']!=original['inode']
 if prefix=='links':assert original['links']==alias['links']==3 and facts['links-middle.txt']['links']==1
 for stage,(column,order) in enumerate([(metric,'Asc'),(metric,'Desc'),('Name','Asc'),('Name','Desc'),(metric,'Asc'),(metric,'Desc')]):
  s=json.loads((d/f'sort-{stage}.json').read_text());assert s['cursor']==f'{prefix}-middle.txt' and s['selected']==2
  assert s['config']['sort_col']==column and s['config']['sort_order']==order
  assert s['config']['folders_first'] and not s['config']['folders_by_name']
  rows=s['entries'];assert len(rows)==len(facts)
  if prefix=='links':
   for r in rows:assert r['hard_links']==(facts[r['name']]['links'] if facts[r['name']]['regular'] else None)
  else:
   for r in rows:assert r['identity']==[facts[r['name']]['device'],facts[r['name']]['inode']]
  directories=sorted(name for name,f in facts.items() if f.get('directory',False))
  files=[name for name in facts if name not in directories]
  if column=='Name':expected=directories+sorted(files,reverse=order=='Desc')
  elif prefix=='links':
   known=[name for name in files if facts[name]['regular']];unknown=sorted(set(files)-set(known))
   keys=sorted({facts[name]['links'] for name in known},reverse=order=='Desc')
   expected=directories+[name for key in keys for name in sorted(known) if facts[name]['links']==key]+unknown
  else:
   keys=sorted({(r['inode'],r['device']) for r in facts.values()},reverse=order=='Desc')
   expected=[name for key in keys for name in sorted(facts) if (facts[name]['inode'],facts[name]['device'])==key]
  assert [r['name'] for r in rows]==expected,(scene,stage,rows,expected)
  report=s['report'];assert report['view']['sort_col']==column and report['view']['sort_order']==order
  assert report['exported_rows']==1 and report['rows'][0]['name']==f'{prefix}-old.txt'
  if prefix=='links':assert report['rows'][0]['hard_links']==3
print('PASS: six checks; 3177 tests; raw nlink/inode/type facts match six native count/name sort orders including external alias, unknown-last, folder grouping, cursor/hidden selection and JSON context; two native scenarios 14/14; source/binary/tool hashes.')
