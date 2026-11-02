from pathlib import Path
import csv,hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3175 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,d in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/p)==d,p
for key,scene,effects in [('native-launch.json','disk_count',3),('disk_allocation-launch.json','disk_allocation',3),('disk_usage_export-launch.json','disk_usage_export',6)]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 n=Path(launch['out'])/scene;m=json.loads((n/'manifest.json').read_text())
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 if scene=='disk_usage_export':
  for name in ['visible-before.csv','visible-after.csv','visible-collapsed.csv']:
   for row in csv.DictReader((n/name).open()):
    assert len(row)==24 and row['measurement_unit']=='bytes' and row['metric']=='logical'
    assert row['known_value']==row['known_bytes']==row['logical_bytes'] and row['file_count']==''
  continue
 facts=json.loads((n/'allocation-facts.json').read_text())
 assert any(f['relative_path']=='sparse/inner/large.bin' and f['length']==32*1024*1024 and f['blocks']==0 for f in facts)
 assert any(f['links']==2 and f['regular'] for f in facts)
 def total(prefix,metric):
  seen=set();size=0
  for f in facts:
   if prefix and not f['relative_path'].startswith(prefix+'/'):continue
   if f['directory'] or (metric=='file_allocation' and not f['regular']):continue
   if metric=='file_count':size+=1;continue
   identity=(f['dev'],f['ino'])
   if identity in seen:continue
   seen.add(identity);size+=f['blocks']*512 if metric=='file_allocation' else f['length']
  return size
 primary='file_count' if scene=='disk_count' else 'file_allocation'
 reports={name:json.loads((n/name).read_text()) for name in ['logical-initial.json','allocated-map.json','logical-restored.json','allocated-final.json']}
 origin=reports['logical-initial.json']['root']
 for name,r in reports.items():
  metric=primary if name.startswith('allocated') else 'logical';count=metric=='file_count';assert r['root']==origin and r['metric']==metric
  assert r['measurement_unit']==('entries' if count else 'bytes')
  if count:assert r['measurement_scope']=='non_directory_entries_including_hard_link_aliases'
  summary=r['summary'];assert summary['known_value']==total('',metric) and summary['measurement_complete']
  assert summary['known_bytes']==(None if count else total('',metric))
  assert summary['logical_bytes']==(total('',metric) if metric=='logical' else None)
  assert summary['file_allocation_bytes']==(total('',metric) if metric=='file_allocation' else None)
  assert summary['file_count']==(total('',metric) if count else None)
  for d in r['directories']:
   assert d['known_value']==total(d['relative_path'],metric),(name,d)
   assert d['measurement_complete'] and d['measurement']==dict(unknown=0,overflowed=False)
   assert d['known_bytes']==(None if count else d['known_value'])
   assert d['file_count']==(d['known_value'] if count else None)
  for item in r['map_items']:
   path=item['relative_path']
   if item['is_directory']:expected=total(path,metric)
   else:
    f=next(f for f in facts if f['relative_path']==path)
    expected=1 if count else (f['blocks']*512 if metric=='file_allocation' and f['regular'] else (0 if metric=='file_allocation' else f['length']))
   assert item['known_value']==expected,(name,item,expected)
   assert item['known_bytes']==(None if count else expected)
   assert item['file_count']==(expected if count else None)
   if count:assert item['logical_bytes'] is None and item['file_allocation_bytes'] is None
  largest=max(r['map_items'],key=lambda d:d['known_value'])['relative_path']
  assert largest==('many' if count else ('dense' if metric=='file_allocation' else 'sparse')),(name,largest)
 rows=list(csv.DictReader((n/'allocated-tree.csv').open()))
 assert rows
 for row in rows:
  assert len(row)==24 and row['metric']==primary and row['logical_bytes']==''
  assert int(row['known_value'])==total(row['relative_path'],primary)
  if primary=='file_count':
   assert row['known_value']==row['file_count'] and row['known_bytes']==row['file_allocation_bytes']=='' and row['measurement_unit']=='entries'
  else:assert row['known_value']==row['known_bytes']==row['file_allocation_bytes'] and row['file_count']==''
  assert row['unknown_measurements']=='0' and row['measurement_complete']=='true' and row['measurement_overflowed']=='false'
print('PASS: six checks; 3175 tests; source/binary hashes; three native scenarios 14/14; raw entry/inode/block oracle; zero files and hard-link aliases; distinct count/byte units; root/subtree/map/CSV totals; metric ranking; captured-root rescan.')
