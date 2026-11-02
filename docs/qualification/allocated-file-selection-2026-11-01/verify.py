from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3119 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for scene,key in [('allocated_selection','native-launch.json'),('listing_json','listing_json-launch.json')]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured'
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==2 and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
state=json.loads((q/'allocated_selection/sort-0.json').read_text());entries=state['entries']
assert len(entries)==21 and state['cursor']=='sparse.bin'
visible=[e for e in entries if '.bin' in e['name']]
assert len(visible)==16
candidates=[e for e in visible if e['allocated'] is not None and not e['folder'] and not e['symlink']]
physical={e['name'] for e in sorted(candidates,key=lambda e:(e['allocated'],e['name']),reverse=True)[:10]}
assert physical=={f'disk-{i:02}.bin' for i in range(2,12)}
logical={e['name'] for e in sorted([e for e in visible if not e['folder']],key=lambda e:(e['logical'],e['name']),reverse=True)[:10]}
assert 'sparse.bin' in logical and 'zz-link.bin' in logical
for filename,expected,total in [('allocated-selection.json',physical,11),('logical-selection.json',physical|logical,13)]:
 report=json.loads((q/'allocated_selection'/filename).read_text())
 assert report['scope']=='visible_selected_rows' and report['query']=='.bin'
 assert report['listing_rows']==21 and report['visible_rows']==16 and report['selected_rows']==total and report['hidden_selected_rows']==1
 assert report['exported_rows']==len(expected)==len(report['rows'])
 assert {r['name'] for r in report['rows']}==expected
 assert all(r['path']==report['folder']+'/'+r['name'] for r in report['rows'])
 assert 'notes.md' not in expected
 assert report['view']['sort_col']=='Name' and report['view']['sort_order']=='Asc'
for stage,total in [(0,11),(1,11),(2,13),(3,13)]:
 s=json.loads((q/'allocated_selection'/f'sort-{stage}.json').read_text());assert s['cursor']=='sparse.bin'
 selected={e['name'] for e in s['entries'] if e['selected']}
 assert len(selected)==total and 'notes.md' in selected
 assert physical<=selected
 assert ('sparse.bin' in selected)==(stage>=2)
full=json.loads((q/'listing_json/listing-full.json').read_text());selected=json.loads((q/'listing_json/listing-selected.json').read_text())
assert full['exported_rows']==9 and selected['exported_rows']==2 and selected['hidden_selected_rows']==1
assert [r['name'] for r in selected['rows']]==['sparse.bin','dense.bin']
print('PASS: six checks, 3119 tests; allocation selection 14/14 and listing JSON 14/14; independent top-ten ranking, retained hidden picks and logical-size contrast')
