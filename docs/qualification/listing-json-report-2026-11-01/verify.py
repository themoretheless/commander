from pathlib import Path
import datetime,hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3116 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
validators=json.loads((q/'validators.json').read_text());assert [v['version'] for v in validators]==['RHash v1.4.6','b3sum 1.8.7']
for scene,key,clips in [('listing_json','native-launch.json',2),('allocated_sort','allocated_sort-launch.json',0)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured'
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 effects=m['native_effect_calls'];assert effects['clipboard']==clips and all(v==0 for k,v in effects.items() if k!='clipboard')
full=json.loads((q/'listing_json/listing-full.json').read_text());selected=json.loads((q/'listing_json/listing-selected.json').read_text())
for report,scope,count in [(full,'visible_rows',9),(selected,'visible_selected_rows',2)]:
 assert report['format']=='commander.listing-report' and report['version']==1 and report['snapshot']=='retained_listing_metadata'
 assert report['scope']==scope and report['exported_rows']==len(report['rows'])==count and report['listing_rows']==9
 assert report['view']['sort_col']=='Allocated' and report['view']['sort_order']=='Asc'
 assert report['facets']==dict(kind=None,min_size=None,max_age_days=None,min_age_days=None)
 assert [r['row'] for r in report['rows']]==list(range(1,count+1))
 for row in report['rows']:
  assert row['path']==report['folder']+'/'+row['name']
  if row['modified_utc'] is not None:datetime.datetime.fromisoformat(row['modified_utc'].replace('Z','+00:00'))
 rows={r['name']:r for r in report['rows']}
 assert rows['sparse.bin']['file_size_bytes']==8388608 and rows['sparse.bin']['allocated_bytes']==0
 assert rows['dense.bin']['file_size_bytes']==8192 and rows['dense.bin']['allocated_bytes']>0
assert full['visible_rows']==9 and full['selected_rows']==full['hidden_selected_rows']==0 and full['query']==''
assert full['rows'][0]['name']=='Design' and full['rows'][-1]['name']=='zz-link.bin'
for name,kind in [('Design','Directory'),('zz-link.bin','Symlink')]:
 row=next(r for r in full['rows'] if r['name']==name);assert row['kind']==kind and row['file_size_bytes'] is None and row['allocated_bytes'] is None
assert selected['visible_rows']==4 and selected['selected_rows']==3 and selected['hidden_selected_rows']==1 and selected['query']=='.bin'
assert [r['name'] for r in selected['rows']]==['sparse.bin','dense.bin']
assert all(r['name']!='notes.md' for r in selected['rows'])
for stage in range(4):
 state=json.loads((q/'allocated_sort'/f'sort-{stage}.json').read_text());assert state['cursor']=='sparse.bin'
 assert state['config']['sort_col']==('Size' if stage==2 else 'Allocated') and state['config']['sort_order']==('Desc' if stage==1 else 'Asc')
print('PASS: six checks, 3116 tests and external checksum oracles; JSON export 14/14 (two exact reports), allocation sort 14/14')
