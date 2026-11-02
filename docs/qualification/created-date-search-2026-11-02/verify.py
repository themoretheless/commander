from pathlib import Path
import hashlib,json,re,datetime,calendar
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/relative)==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3184 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
def instant(text):
 match=re.fullmatch(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|\+00:00)',text);assert match,text
 return calendar.timegm(datetime.datetime.fromisoformat(match[1]).timetuple())*10**9+int((match[2] or '').ljust(9,'0'))
for scene,key in [('created_search','native-launch.json'),('accessed_search','accessed_search-launch.json')]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 if scene=='accessed_search':
  facts={r['name']:r for r in json.loads((d/'accessed-facts.json').read_text())};observed={n:f['seconds']*10**9+f['nanos'] for n,f in facts.items()};epoch=1767225600*10**9
  states=[('accessed-4.json',epoch+250000000,epoch+750000000),('accessed-7.json',None,0),('accessed-final.json',epoch+750000000,None)]
  metric='accessed_ns'
 else:
  raw=json.loads((d/'created-facts.json').read_text());facts={r['name']:r for r in raw['facts']};observed={n:f['seconds']*10**9+f['nanos'] for n,f in facts.items()}
  first=observed['born-first.txt'];middle=observed['born-middle.txt'];last=observed['born-last.txt'];assert first<middle<last
  assert all(f['modified_seconds']*10**9>last+86400*10**9 for f in facts.values())
  expressions=raw['expressions'];assert len(expressions)==3
  expected_bounds=[[first,last],[first],[middle]]
  for expression,bounds in zip(expressions,expected_bounds):assert [instant(token.split(':',1)[1]) for token in expression.split() if token.startswith('created-')]==bounds
  states=[('created-4.json',first,last),('created-7.json',None,first),('created-final.json',middle,None)];metric='created_ns'
 for index,(name,low,high) in enumerate(states):
  s=json.loads((d/name).read_text());assert s['skipped']==1 and s['root'].endswith('/left')
  expected=sorted(n for n,t in observed.items() if (low is None or t>low) and (high is None or t<high))
  assert s['matched']==len(expected) and sorted(r['name'] for r in s['rows'])==expected,(scene,name,s,expected)
  if scene=='created_search':assert s['expression']==expressions[index]
  for r in s['rows']:
   assert r[metric]==observed[r['name']]
   assert any(e.startswith('created-' if scene=='created_search' else 'accessed-') for e in r['explanations'])
print('PASS: six checks; 3184 tests; created/accessed Find native 16/16; raw APFS birth seconds/nanos and UTC boundaries, strict first/middle/last and empty membership, differing mtimes, archive unknown skips, explanations; source/binary/tool hashes.')
