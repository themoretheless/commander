from pathlib import Path
import hashlib,json,subprocess,urllib.parse
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text())
assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3512 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==h,p
launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out'])/'favorite_import'
assert launch['binary_sha256']==launch['core_sha256']==hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()
m=json.loads((out/'manifest.json').read_text());assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=1,opener=0,trash=0,free_space=0)
items=json.loads((out/'imported-bookmarks.json').read_text())['items'];assert [i['name'] for i in items]==['Existing Workspace','Проект GTK','Other imported']
assert [i['slot'] for i in items]==[3,None,None]
lines=(out/'gtk-export.txt').read_text().splitlines();assert len(lines)==3
for line,item in zip(lines,items):
 uri,label=line.split(' ',1);assert label==item['name'] and urllib.parse.unquote(urllib.parse.urlparse(uri).path)==item['path']
(q/'verification.json').write_text(json.dumps(dict(checks=7,tests_passed=3512,tests_ignored=3,native_gtk_control_checks=14,native_xbel_interaction='not exercised; model and workspace integration tested',source_hashes_verified=True),indent=2)+'\n')
print('Verified seven gates, source hashes and native GTK regression receipts')
