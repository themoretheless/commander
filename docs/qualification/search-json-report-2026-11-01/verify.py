from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3137 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
assert any(c['name']=='tests' and '--test-threads=4' in c['command'] for c in checks)
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clips in [('search_json','native-launch.json',5),('search_results','search_results-launch.json',12)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
def path(p):
 if p['encoding']=='utf8':return p['text'].encode('utf8')
 assert p['encoding']=='unix_bytes_hex';return bytes.fromhex(p['hex'])
def location(r):
 l=r['location']
 if l['kind']=='native':return ('native',path(l['path']))
 assert l['kind']=='archive_member';return ('archive_member',path(l['archive']),path(l['member']),l['index'])
d=q/'search_json';reports={name:json.loads((d/f'report-{name}.json').read_text()) for name in ['all','selected','visible','draft','empty']}
full=reports['all'];applied_root=full['root'];native_root=path(applied_root);assert native_root.endswith(b'/left')
assert full['format']=='commander.search-report' and full['version']==1 and full['exported_rows']==5 and full['scope']=='visible_results'
for name,report in reports.items():
 assert report['format']==full['format'] and report['root']==applied_root and report['partial']==False
 c=report['context'];assert c['completed']==True and c['running']==False and c['pending_rerun']==False
 assert c['generation']>0 and c['source']=='Live + archives' and c['skipped']==0 and c['truncated']==False and c['archive_error'] is None
 assert report['exported_rows']==len(report['rows'])
 assert [r['row'] for r in report['rows']]==list(range(1,len(report['rows'])+1))
 assert c['mode']=='Exact' and c['pivot']=='None'
 if name=='empty':
  assert c['expression']=='does-not-exist' and c['matched']==c['received_rows']==c['visible_rows']==c['visible_selected_rows']==0 and not c['draft_differs'];continue
 assert c['expression']=='report-' and c['matched']==c['received_rows']==5
 assert c['draft_differs']==(name=='draft')
 assert all(r['score']>0 and any(e['field']=='name' and 'report-' in e['detail'] for e in r['explanation']) for r in report['rows'])
 for r in report['rows']:
  for e in r['explanation']:assert all(a<=b for a,b in e['matched_character_ranges'])
all_rows={location(r):r for r in full['rows']};assert len(all_rows)==5
archive=[r for r in full['rows'] if r['location']['kind']=='archive_member'];assert len(archive)==1
assert location(archive[0])[1:]==(native_root+b'/search.zip',b'report-archive.txt',0) and archive[0]['native_id'] is None
capability=json.loads((d/'path-capability.json').read_text())
raw=[r for r in full['rows'] if r['location']['kind']=='native' and r['location']['path']['encoding']=='unix_bytes_hex']
if capability['raw_path_supported']:
 assert len(raw)==1 and path(raw[0]['location']['path'])==native_root+b'/report-\xff.txt' and capability['raw_path_error'] is None
else:
 assert not raw and capability['raw_path_error']
 assert ('native',native_root+b'/report-\n.txt') in all_rows
for r in full['rows']:
 if r['location']['kind']=='native':assert r['native_id']['device_id']>0 and r['native_id']['file_id']>0
selected=reports['selected'];assert selected['scope']=='visible_selected_results' and selected['exported_rows']==2
expected_selected={('native',native_root+b'/report-a.txt'),location(archive[0])}
assert {location(r) for r in selected['rows']}==expected_selected
for name in ['selected','visible','draft']:
 report=reports[name];assert report['context']['visible_rows']==4 and report['context']['visible_selected_rows']==2
 for row in report['rows']:
  before=all_rows[location(row)]
  for key in row:
   if key!='row':assert row[key]==before[key]
visible=reports['visible'];assert visible['exported_rows']==4 and reports['draft']['rows']==visible['rows']
assert {location(r) for r in visible['rows']}==set(all_rows)-{('native',native_root+b'/report-hidden.txt')}
assert [location(r) for r in selected['rows']]==[location(r) for r in visible['rows'] if location(r) in expected_selected]
proof=json.loads((q/'search_results/parent-copy-proof.json').read_text())
assert proof['parent_selected'] and proof['clipboard_calls']==11 and proof['text'].endswith('/left')
a=proof['choice_rect'];b=proof['visible_rect'];assert a and b and a['max_y']>a['min_y']
for key in ['min_x','min_y','max_x','max_y']:assert abs(a[key]-b[key])<0.01
print('PASS: six checks, 3137 tests; five real JSON clipboard reports, applied/draft and empty context, lossless native/archive locations; search JSON 16/16 and legacy search results 16/16')
