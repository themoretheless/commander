from pathlib import Path
import csv,hashlib,json
q=Path(__file__).resolve().parent; root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=json.loads((q/'checks.json').read_text())
assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3173 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/p)==digest,p
launch=json.loads((q/'native-launch.json').read_text())
assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
n=Path(launch['out'])/'disk_usage_export'
m=json.loads((n/'manifest.json').read_text())
assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']['clipboard']==6
for name in ['scan-before.json','scan-filtered.json','scan-after.json']:
 r=json.loads((n/name).read_text());assert r['metric']=='logical'
 assert r['summary']['measurement_complete']
 assert r['summary']['known_bytes']==r['summary']['logical_bytes']
 assert r['summary']['file_allocation_bytes'] is None
before=json.loads((n/'scan-before.json').read_text());after=json.loads((n/'scan-after.json').read_text())
assert before['summary']['logical_bytes']==before['summary']['known_bytes']==246
assert after['summary']['logical_bytes']==306
for name,count in [('visible-before.csv',4),('visible-after.csv',5),('visible-collapsed.csv',7)]:
 rows=list(csv.DictReader((n/name).open()));assert len(rows)==count,(name,len(rows))
 for row in rows:
  assert len(row)==21 and row['metric']=='logical'
  assert row['known_bytes']==row['logical_bytes']
  assert row['unknown_measurements']=='0' and row['measurement_overflowed']=='false'
  assert row['measurement_complete']=='true' and row['file_allocation_bytes']==''
print('PASS: six checks, 3173 tests, source/binary hashes, native legacy 14/14 and JSON/CSV measurements. Allocation-switch native qualification remains open.')
