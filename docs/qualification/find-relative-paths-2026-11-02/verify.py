from pathlib import Path
import hashlib,json,re,datetime,calendar,zipfile
q=Path(__file__).resolve().parent;root=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(root/relative)==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3196 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
def instant(text):
 match=re.fullmatch(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d{1,9}))?(Z|\+00:00)',text);assert match,text
 return calendar.timegm(datetime.datetime.fromisoformat(match[1]).timetuple())*10**9+int((match[2] or '').ljust(9,'0'))
for scene,key,effects in [('search_results','native-launch.json',14),('find_sort','find_sort-launch.json',6)]:
 launch=json.loads((q/key).read_text());assert h(root/'target/debug/commander')==launch['binary_sha256']==launch['core_sha256']
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 if scene=='search_results':
  initial=json.loads((d/'relative-initial.json').read_text());final=json.loads((d/'relative-final.json').read_text())
  base=Path(final['applied_root']);assert base.is_absolute() and str(base)==initial['root']==final['root']
  assert initial['generation']==final['generation']==1 and final['error'] is None
  expected=[str(Path(p).relative_to(base)) for p in final['paths']];selected=[str(Path(p).relative_to(base)) for p in final['selected_paths']]
  assert expected==final['names']==['README.txt','notes.md','Design'] and selected==['README.txt']
  assert (d/'relative-all.txt').read_text()=='\n'.join(expected)
  assert (d/'relative-selected.txt').read_text()=='\n'.join(selected)
  assert (d/'parents-all.txt').read_text()==str(base)
  old=json.loads((d/'parent-copy-proof.json').read_text());assert old['parent_selected'] and old['clipboard_calls']==11 and old['text']==str(base)
  continue
 if scene=='find_sort':
  raw=json.loads((d/'sort-facts.json').read_text());facts={f['name']:f for f in raw['facts']}
  initial=json.loads((d/'sort-initial.json').read_text());final=json.loads((d/'displayed-final.json').read_text());rank=initial['original_names'];target=initial['highlighted_name']
  assert len(set(rank))==6 and final['names']==rank and final['highlighted_name']==target and final['generation']==initial['generation']
  assert facts['sort-file2.bin']['size']>facts['sort-file10.bin']['size'] and facts['sort-file2.bin']['blocks']==0
  assert facts['sort-file10.bin']['links']==facts['sort-alias.bin']['links']==2
  with zipfile.ZipFile(d/'source.zip') as z:
   member=z.infolist()[0];assert len(z.infolist())==1 and member.filename=='sort-member.bin' and member.file_size==5 and member.date_time==(1980,1,1,0,0,0)
  zone=datetime.timezone(datetime.timedelta(seconds=raw['utc_offset_seconds']))
  def bucket(t,now):
   if t is None:return 6
   if t>now:return 0
   days=(datetime.datetime.fromtimestamp(now//10**9,zone).date()-datetime.datetime.fromtimestamp(t//10**9,zone).date()).days
   return 0 if days<=0 else 1 if days==1 else 2 if days<=6 else 3 if days<=30 else 4 if days<=365 else 5
  def natural(n):return tuple((1,int(t)) if t.isdigit() else (0,t.lower()) for t in re.split(r'(\d+)',n))
  observed_orders=[]
  for number,(field,desc,pivot) in enumerate([('name',False,'None'),('name',True,'None'),('size',True,'None'),('allocated',True,'None'),('allocated',True,'Modified'),('rank',False,'None')],1):
   r=json.loads((d/f'sort-{number}.json').read_text());c=r['context'];assert c['result_sort']==dict(field=field,descending=desc) and c['pivot']==pivot and c['generation']==initial['generation']
   assert c['completed'] and not c['running'] and not c['pending_rerun'] and not r['partial'] and c['skipped']==0 and c['matched']==c['visible_rows']==r['exported_rows']==6
   values={};times={}
   for row in r['rows']:
    n=row['display_name'];native=row['location']['kind']=='native';f=facts[n] if native else facts['other.zip']
    size=f['size'] if native else 5;allocated=f['blocks']*512 if native and f['regular'] else None;links=f['links'] if native and f['regular'] else None
    assert row['listed_size_bytes']==size and row['allocated_bytes']==allocated and row['hard_links']==links,(n,row)
    t=f['modified_seconds']*10**9+f['modified_nanos'];assert instant(row['modified_utc'])==t
    if not native:assert row['created_utc'] is None and row['changed_utc'] is None
    values[n]=natural(n) if field=='name' else size if field=='size' else allocated if field=='allocated' else rank.index(n);times[n]=t
   expected=sorted(rank,key=lambda n:values[n],reverse=desc) if field!='allocated' else sorted(rank,key=lambda n:(values[n] is None,-values[n] if values[n] is not None else 0,rank.index(n)))
   if pivot=='Modified':expected=sorted(expected,key=lambda n:bucket(times[n],instant(c['ordered_at_utc'])))
   assert [row['display_name'] for row in r['rows']]==expected,(number,expected,r['rows'])
   assert [row['row'] for row in r['rows']]==list(range(1,7))
   hl=json.loads((d/f'highlight-{number}.json').read_text());assert hl['highlighted_name']==target and hl['highlighted_row']==expected.index(target)
   observed_orders.append(expected)
  assert observed_orders[0].index(target)!=observed_orders[1].index(target) and observed_orders[-1]==rank
  continue
print('PASS: six checks; 3196 tests; native relative-copy and neighboring sorting each 16/16; selected/all text independently matched applied-root paths, stable generation and visible order; prior parent workflow retained; six neighboring raw-metadata sort reports; source/binary/tool hashes.')
