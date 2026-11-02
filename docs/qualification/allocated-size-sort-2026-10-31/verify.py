from pathlib import Path
import hashlib,json,shutil
Q=Path(__file__).resolve().parent;root=Q.parents[2]
checks=json.loads((Q/'checks.json').read_text())
assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3110 passed; 0 failed; 3 ignored' in (Q/'tests.log').read_text()
for scene,key in [('allocated_sort','native-launch.json'),('allocated_info','allocated_info-launch.json')]:
 launch=json.loads((Q/key).read_text())
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene;target=Q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text())
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
for stage in range(4):
 s=json.loads((Q/'allocated_sort'/f'sort-{stage}.json').read_text())
 assert s['phase']==stage*6+5 and s['cursor']=='sparse.bin'
 assert s['config']['sort_col']==('Size' if stage==2 else 'Allocated')
 assert s['config']['sort_order']==('Desc' if stage==1 else 'Asc')
 entries=s['entries'];assert len(entries)==9 and entries[0]['name']=='Design'
 rows={e['name']:e for e in entries};names=[e['name'] for e in entries]
 assert rows['sparse.bin']['logical']==8388608 and rows['sparse.bin']['allocated']==0
 assert rows['dense.bin']['logical']==8192 and rows['dense.bin']['allocated']>0
 assert rows['zz-link.bin']['symlink'] and rows['zz-link.bin']['allocated'] is None
 assert names.index('sparse.bin')<names.index('dense.bin') if stage!=1 and stage!=2 else names.index('dense.bin')<names.index('sparse.bin')
 if stage!=2:
  values=[e['allocated'] for e in entries[1:] if e['allocated'] is not None]
  assert values==sorted(values,reverse=stage==1)
  assert names[-1]=='zz-link.bin'
  assert names.index('empty.bin')<names.index('sparse.bin')
 else:
  logical=[e['logical'] for e in entries[1:]]
  assert logical==sorted(logical)
a=json.loads((Q/'allocated_sort/sort-0.json').read_text());b=json.loads((Q/'allocated_sort/sort-3.json').read_text());assert a['entries']==b['entries']
s=json.loads((Q/'allocated_info/info-state.json').read_text());assert s['logical_bytes']==8388608 and s['allocated_bytes']==0 and s['allocated_size']=='0 B (0 bytes)'
print('PASS: six checks, 3110 tests; native allocation sort 14/14 and Info 14/14; four metric/direction transitions, unknown-last and preserved cursor')
