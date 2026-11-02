from pathlib import Path
import hashlib,json,re,datetime,calendar,zipfile
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/relative)==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3190 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
def instant(text):
 match=re.fullmatch(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|\+00:00)',text);assert match,text
 return calendar.timegm(datetime.datetime.fromisoformat(match[1]).timetuple())*10**9+int((match[2] or '').ljust(9,'0'))
for scene,key,effects in [('time_groups','native-launch.json',3),('changed_search','changed_search-launch.json',0)]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 if scene=='changed_search':
  raw=json.loads((d/'changed-facts.json').read_text());facts={r['name']:r for r in raw['facts']};observed={n:f['seconds']*10**9+f['nanos'] for n,f in facts.items()};a=observed['status-first.txt'];b=observed['status-middle.txt'];c=observed['status-last.txt'];assert a<b<c
  for i,(name,low,high) in enumerate([('changed-4.json',a,c),('changed-7.json',None,a),('changed-final.json',b,None)]):
   s=json.loads((d/name).read_text());assert s['expression']==raw['expressions'][i] and s['skipped']==1
   expected=sorted(n for n,t in observed.items() if (low is None or t>low) and (high is None or t<high));assert sorted(r['name'] for r in s['rows'])==expected
   for r in s['rows']:assert r['changed_ns']==observed[r['name']]
  continue
 raw=json.loads((d/'group-facts.json').read_text());facts={r['name']:r for r in raw['facts']};initial=json.loads((d/'group-initial.json').read_text());final=json.loads((d/'displayed-final.json').read_text())
 assert initial['highlighted_name']==final['highlighted_name']=='group-old.txt'
 assert initial['generation']==final['generation']
 with zipfile.ZipFile(d/'source.zip') as z:
  members=z.infolist();assert len(members)==1 and members[0].filename=='group-member.txt'
  assert members[0].date_time==(1980,1,1,0,0,0) # placeholder -> existing container-mtime fallback
 archive_modified=raw['archive_modified_seconds']*10**9+raw['archive_modified_nanos']
 offset=raw['utc_offset_seconds'];zone=datetime.timezone(datetime.timedelta(seconds=offset))
 def bucket(value,now):
  if value is None:return 6
  if value>now:return 0
  current=datetime.datetime.fromtimestamp(now//10**9,zone).date();date=datetime.datetime.fromtimestamp(value//10**9,zone).date();days=(current-date).days
  return 0 if days<=0 else 1 if days==1 else 2 if days<=6 else 3 if days<=30 else 4 if days<=365 else 5
 orders={}
 for name,pivot,field,phase in [('created.json','Created','created',14),('changed.json','Changed','changed',20),('modified.json','Modified','modified',26)]:
  r=json.loads((d/name).read_text());context=r['context'];assert context['pivot']==pivot and context['generation']==initial['generation']
  assert context['completed'] and not context['running'] and not context['pending_rerun'] and not r['partial']
  assert context['matched']==context['visible_rows']==r['exported_rows']==3 and context['skipped']==0
  now=instant(context['ordered_at_utc']);values={}
  for row in r['rows']:
   n=row['display_name']
   if row['location']['kind']=='native':
    f=facts[n]
    for key in ['created','changed','modified']:assert instant(row[key+'_utc'])==f[key+'_seconds']*10**9+f[key+'_nanos'],(n,key)
    values[n]=f[field+'_seconds']*10**9+f[field+'_nanos']
   else:
    assert row['created_utc'] is None and row['changed_utc'] is None
    assert instant(row['modified_utc'])==archive_modified
    values[n]=archive_modified if field=='modified' else None
  expected=sorted(initial['original_names'],key=lambda n:(bucket(values[n],now),initial['original_names'].index(n)))
  orders[pivot]=expected;assert [row['display_name'] for row in r['rows']]==expected
  hl=json.loads((d/f'highlight-{phase}.json').read_text());assert hl['pivot']==pivot and hl['highlighted_name']=='group-old.txt' and hl['highlighted_row']==expected.index('group-old.txt')
 assert orders['Created']!=orders['Modified']
 assert orders['Created']==final['names']
 assert json.loads((d/'highlight-14.json').read_text())['highlighted_row']!=json.loads((d/'highlight-26.json').read_text())['highlighted_row']
print('PASS: six checks; 3190 tests; two native Find workflows 16/16; raw metadata/ZIP placeholder and container facts, three copied pivot reports, stable rank/buckets, archive Unknown, unchanged generation, actual rendered order and moving highlight identity; source/binary/tool hashes.')
