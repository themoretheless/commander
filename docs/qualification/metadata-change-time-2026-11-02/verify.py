from pathlib import Path
from datetime import datetime,timezone
import hashlib,json,shutil,re,calendar
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3147 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key in [('changed_sort','native-launch.json'),('file_identity_info','file_identity_info-launch.json')]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
d=q/'changed_sort';facts={r['name']:r for r in json.loads((d/'changed-facts.json').read_text())}
assert len(facts)==5 and facts['change-folder']['folder'] and facts['change-link.txt']['symlink']
assert (facts['change-link.txt']['changed_seconds'],facts['change-link.txt']['changed_nanos'])!=(facts['change-old.txt']['changed_seconds'],facts['change-old.txt']['changed_nanos'])
def timekey(t):return t['secs_since_epoch'],t['nanos_since_epoch']
def ctime(r):return r['changed_seconds'],r['changed_nanos']
def utc_parts(text):
 m=re.fullmatch(r'(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?Z',text);assert m,text
 return calendar.timegm(datetime.strptime(m[1],'%Y-%m-%dT%H:%M:%S').timetuple()),int((m[2] or '').ljust(9,'0'))
for stage,(column,order) in enumerate([('Changed','Asc'),('Changed','Desc'),('Modified','Asc'),('Modified','Desc'),('Changed','Asc'),('Changed','Desc')]):
 s=json.loads((d/f'sort-{stage}.json').read_text());assert s['cursor']=='change-middle.txt' and s['selected']==2
 assert s['config']['sort_col']==column and s['config']['sort_order']==order
 rows=s['entries'];assert len(rows)==5
 for r in rows:
  assert timekey(r['changed'])==ctime(facts[r['name']]);assert r['modified']==facts[r['name']]['modified']
 key=ctime if column=='Changed' else lambda r:timekey(r['modified'])
 files={n:r for n,r in facts.items() if not r['folder']};keys=sorted({key(r) for r in files.values()},reverse=order=='Desc')
 expected=['change-folder']+[name for k in keys for name in sorted(files) if key(files[name])==k]
 assert [r['name'] for r in rows]==expected,(stage,rows,expected)
 report=s['report'];assert report['view']['sort_col']==column and report['view']['sort_order']==order
 assert report['exported_rows']==1 and report['rows'][0]['name']=='change-old.txt'
 assert utc_parts(report['rows'][0]['changed_utc'])==ctime(facts['change-old.txt'])
print('PASS: six checks; 3147 tests; six native orders match independent lexical ctime and display mtime facts, preserve cursor/hidden selection and exact JSON time; two native scenarios 14/14 each')
