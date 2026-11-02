from pathlib import Path
import datetime,hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3133 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clips in [('time_sort','native-launch.json',0),('listing_json','listing_json-launch.json',2),('symlink_info','symlink_info-launch.json',0)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
def ns(t):return None if t is None else t['secs_since_epoch']*10**9+t['nanos_since_epoch']
def utc(t):
 if t is None:return None
 seconds,nanos=divmod(ns(t),10**9)
 base=datetime.datetime.fromtimestamp(seconds,datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%S')
 suffix='' if nanos==0 else '.'+f'{nanos:09d}'[:(3 if nanos%10**6==0 else 6 if nanos%1000==0 else 9)]
 return base+suffix+'Z'
d=q/'time_sort';facts={r['name']:r for r in json.loads((d/'time-facts.json').read_text())}
assert set(facts)=={'clock-old.txt','clock-middle.txt','clock-new.txt'}
assert ns(facts['clock-old.txt']['accessed'])<ns(facts['clock-middle.txt']['accessed'])<ns(facts['clock-new.txt']['accessed'])
assert ns(facts['clock-old.txt']['modified'])>ns(facts['clock-middle.txt']['modified'])>ns(facts['clock-new.txt']['modified'])
for stage,(column,field,order) in enumerate([('Accessed','accessed','Asc'),('Accessed','accessed','Desc'),('Created','created','Asc'),('Created','created','Desc'),('Modified','modified','Asc'),('Accessed','accessed','Asc')]):
 s=json.loads((d/f'sort-{stage}.json').read_text());assert s['phase']==stage*6+5
 assert s['config']['sort_col']==column and s['config']['sort_order']==order
 assert s['cursor']=='clock-middle.txt' and s['selected']==2
 known=[r for r in facts.values() if r[field] is not None];unknown=[r for r in facts.values() if r[field] is None]
 expected=[r['name'] for r in sorted(known,key=lambda r:((1 if order=='Asc' else -1)*ns(r[field]),r['name']))+sorted(unknown,key=lambda r:r['name'])]
 assert [r['name'] for r in s['entries']]==expected
 for row in s['entries']:
  for date in ['accessed','created','modified']:assert row[date]==facts[row['name']][date]
  assert row['selected']==(row['name']=='clock-old.txt')
 report=s['report'];assert report['scope']=='visible_selected_rows' and report['exported_rows']==1 and report['hidden_selected_rows']==1
 assert report['view']['sort_col']==column and report['view']['sort_order']==order
 row=report['rows'][0];assert row['name']=='clock-old.txt'
 for date in ['accessed','created','modified']:assert row[date+'_utc']==utc(facts[row['name']][date])
full=json.loads((q/'listing_json/listing-full.json').read_text());selected=json.loads((q/'listing_json/listing-selected.json').read_text());rows={r['name']:r for r in full['rows']}
assert full['exported_rows']==9 and selected['exported_rows']==2 and selected['hidden_selected_rows']==1
for row in rows.values():
 assert row['accessed_utc'] is not None
 assert row['created_utc'] is None or row['created_utc'].endswith('Z')
for row in selected['rows']:
 for date in ['modified_utc','accessed_utc','created_utc']:assert row[date]==rows[row['name']][date]
info=json.loads((q/'symlink_info/info-state.json').read_text());assert info['kind']=='Symlink to TXT file' and info['link_status']=='Target exists'
assert info['accessed'].startswith('Link object: ')
assert info['created']=='Unavailable' or info['created'].startswith('Link object: ')
print('PASS: six checks, 3133 tests; six native date orders match raw timestamps; time sort / listing JSON / link Info each 14/14')
