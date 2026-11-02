from pathlib import Path
import csv,hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3173 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,d in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/p)==d,p
for key,scene,effects in [('native-launch.json','disk_allocation',3),('disk_usage_export-launch.json','disk_usage_export',6)]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 n=Path(launch['out'])/scene;m=json.loads((n/'manifest.json').read_text())
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 if scene=='disk_usage_export':continue
 facts=json.loads((n/'allocation-facts.json').read_text())
 assert any(f['relative_path']=='sparse/inner/large.bin' and f['length']==32*1024*1024 and f['blocks']==0 for f in facts)
 assert any(f['links']==2 and f['regular'] for f in facts)
 def total(prefix,allocation):
  seen=set();size=0
  for f in facts:
   if prefix and not f['relative_path'].startswith(prefix+'/'):continue
   if f['directory'] or (allocation and not f['regular']):continue
   identity=(f['dev'],f['ino'])
   if identity in seen:continue
   seen.add(identity);size+=f['blocks']*512 if allocation else f['length']
  return size
 reports={name:json.loads((n/name).read_text()) for name in ['logical-initial.json','allocated-map.json','logical-restored.json','allocated-final.json']}
 origin=reports['logical-initial.json']['root']
 for name,r in reports.items():
  allocation=name.startswith('allocated');assert r['root']==origin
  assert r['metric']==('file_allocation' if allocation else 'logical')
  summary=r['summary'];assert summary['known_bytes']==total('',allocation) and summary['measurement_complete']
  assert summary['logical_bytes']==(None if allocation else total('',False))
  assert summary['file_allocation_bytes']==(total('',True) if allocation else None)
  for d in r['directories']:
   assert d['known_bytes']==total(d['relative_path'],allocation),(name,d)
   assert d['measurement_complete'] and d['measurement']==dict(unknown=0,overflowed=False)
  for item in r['map_items']:
   path=item['relative_path']
   if item['is_directory']:expected=total(path,allocation)
   else:
    f=next(f for f in facts if f['relative_path']==path);expected=f['blocks']*512 if allocation and f['regular'] else (0 if allocation else f['length'])
   assert item['known_bytes']==expected,(name,item,expected)
  largest=max(r['map_items'],key=lambda d:d['known_bytes'])['relative_path']
  assert largest==('dense' if allocation else 'sparse'),(name,largest)
 rows=list(csv.DictReader((n/'allocated-tree.csv').open()))
 assert rows
 for row in rows:
  assert len(row)==21 and row['metric']=='file_allocation' and row['logical_bytes']==''
  assert int(row['known_bytes'])==int(row['file_allocation_bytes'])==total(row['relative_path'],True)
  assert row['unknown_measurements']=='0' and row['measurement_complete']=='true' and row['measurement_overflowed']=='false'
print('PASS: six checks; 3173 tests; final source/binary hashes; allocation and legacy native 14/14; 3/6 clipboard effects; raw dev/ino/blocks oracle; sparse/hardlink accounting; tree/map/CSV; metric ranking; captured-root rescan after pane navigation.')
