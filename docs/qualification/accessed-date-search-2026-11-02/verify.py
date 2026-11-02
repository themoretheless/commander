from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/relative)==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3180 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
for scene,key in [('accessed_search','native-launch.json'),('allocation_search','allocation_search-launch.json')]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 if scene=='allocation_search':
  facts={r['name']:r for r in json.loads((d/'allocation-facts.json').read_text())}
  for file,expected in [('allocated-4.json',['disk-empty','disk-sparse']),('allocated-7.json',['disk-dense']),('allocated-final.json',['disk-sparse'])]:
   s=json.loads((d/file).read_text());assert sorted(r['name'] for r in s['rows'])==sorted(expected)
   assert s['skipped']==1
   for r in s['rows']:assert r['allocated']==facts[r['name']]['allocated'] and r['size']==facts[r['name']]['logical']
  continue
 facts={r['name']:r for r in json.loads((d/'accessed-facts.json').read_text())}
 observed={name:f['seconds']*10**9+f['nanos'] for name,f in facts.items()}
 epoch2026=1767225600*10**9
 assert observed=={'access-early.txt':epoch2026+100000000,'access-middle.txt':epoch2026+500000000,'access-late.txt':epoch2026+900000000,'access-pre.txt':-500000000}
 assert len({f['modified_seconds'] for f in facts.values()})==1 and facts['access-middle.txt']['modified_seconds']==1577836800
 for name,low,high in [('accessed-4.json',epoch2026+250000000,epoch2026+750000000),('accessed-7.json',None,0),('accessed-final.json',epoch2026+750000000,None)]:
  s=json.loads((d/name).read_text());assert s['skipped']==1 and s['matched']==1
  assert s['root'].endswith('/left')
  expected=sorted(n for n,t in observed.items() if (low is None or t>low) and (high is None or t<high))
  assert sorted(r['name'] for r in s['rows'])==expected
  for r in s['rows']:
   assert r['accessed_ns']==observed[r['name']]
   assert any(e.startswith('accessed-') for e in r['explanations'])
print('PASS: six checks; 3180 tests; native accessed/allocation Find 16/16; independent raw atime seconds/nanos, strict fractional/pre-epoch membership, mtime distinction, archive unknown skips, explanations and source/binary/tool hashes.')
