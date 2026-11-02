from pathlib import Path
import hashlib,json,re,datetime,calendar
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/relative)==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3188 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
def instant(text):
 match=re.fullmatch(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|\+00:00)',text);assert match,text
 return calendar.timegm(datetime.datetime.fromisoformat(match[1]).timetuple())*10**9+int((match[2] or '').ljust(9,'0'))
for scene,key in [('changed_search','native-launch.json'),('created_search','created_search-launch.json')]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 metric='changed' if scene=='changed_search' else 'created';prefix='status' if metric=='changed' else 'born'
 raw=json.loads((d/f'{metric}-facts.json').read_text());facts={r['name']:r for r in raw['facts']};observed={n:f['seconds']*10**9+f['nanos'] for n,f in facts.items()}
 first=observed[f'{prefix}-first.txt'];middle=observed[f'{prefix}-middle.txt'];last=observed[f'{prefix}-last.txt'];assert first<middle<last
 assert all(f['modified_seconds']*10**9>last+86400*10**9 for f in facts.values())
 if metric=='changed':
  for n,f in facts.items():
   assert observed[n]>f['before_seconds']*10**9+f['before_nanos']
   assert f['before_mode']==0o644 and f['mode']==0o600
   assert (f['modified_seconds'],f['modified_nanos'])==(f['before_modified_seconds'],f['before_modified_nanos'])
   assert f['before_size']==f['size']==len(b'status fixture')
 expressions=raw['expressions'];assert len(expressions)==3
 expected_bounds=[[first,last],[first],[middle]]
 for expression,bounds in zip(expressions,expected_bounds):assert [instant(token.split(':',1)[1]) for token in expression.split() if token.startswith(metric+'-')]==bounds
 states=[(f'{metric}-4.json',first,last),(f'{metric}-7.json',None,first),(f'{metric}-final.json',middle,None)]
 for index,(name,low,high) in enumerate(states):
  s=json.loads((d/name).read_text());assert s['skipped']==1 and s['root'].endswith('/left')
  expected=sorted(n for n,t in observed.items() if (low is None or t>low) and (high is None or t<high))
  assert s['matched']==len(expected) and sorted(r['name'] for r in s['rows'])==expected,(scene,name,s,expected)
  assert s['expression']==expressions[index]
  for r in s['rows']:
   assert r[metric+'_ns']==observed[r['name']]
   assert any(e.startswith(metric+'-') for e in r['explanations'])
print('PASS: six checks; 3188 tests; changed/created Find native 16/16; raw ctime/birth nanos and exact UTC boundaries, chmod advances status time with identical mtime/size, interval/empty/after membership, archive unknowns, explanations; source/binary/tool hashes.')
