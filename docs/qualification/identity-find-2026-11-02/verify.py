from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
reference=json.loads((q/'samefile-reference.json').read_text());assert reference['results_relative_to_fixture']==['nested/alias.txt','original.txt']
assert hashlib.sha256(Path(reference['path']).read_bytes()).hexdigest()==reference['sha256']
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3153 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clips,count in [('identity_find','native-launch.json',0,16),('hardlink_selection','hardlink_selection-launch.json',1,14)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==count and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
d=q/'identity_find';facts={r['path']:r for r in json.loads((d/'identity-facts.json').read_text())};key=lambda r:(r['device'],r['inode'])
source=facts['identity-original.txt'];device,inode=key(source)
assert key(source)==key(facts['nested/identity-alias.txt']) and key(source)!=key(facts['identity-copy.txt']) and facts['identity-link.txt']['symlink'] and key(source)!=key(facts['identity-link.txt'])
for filename,archives,skipped in [('identity-command.json','off',0),('identity-final.json','on',1)]:
 s=json.loads((d/filename).read_text());base=Path(s['root']);assert s['source']==('Live (archives off)' if archives=='off' else 'Live + archives') and s['matched']==2 and s['skipped']==skipped
 assert s['expression']==f'device:{device} inode:{inode} type:regular archives:{archives}'
 assert sorted(str(Path(r['path']).relative_to(base)) for r in s['rows'])==['identity-original.txt','nested/identity-alias.txt']
 for row in s['rows']:
  assert f'device:{device} (observed device {device})' in row['explanations']
  assert f'inode:{inode} (observed inode {inode})' in row['explanations']
d=q/'hardlink_selection';facts={r['name']:r for r in json.loads((d/'hardlink-facts.json').read_text())}
assert key(facts['hard-original.txt'])==key(facts['hard-alias.txt']) and facts['hard-original.txt']['links']==2
for stage in range(2):
 s=json.loads((d/f'sort-{stage}.json').read_text());assert s['cursor']=='hard-original.txt' and s['selected']==3
 assert sorted(r['name'] for r in s['entries'] if r['selected'])==['hard-alias.txt','hard-original.txt']
 for row in s['entries']:assert row['identity']==list(key(facts[row['name']]))
r=json.loads((d/'hardlink-selection.json').read_text());assert r['exported_rows']==2 and r['selected_rows']==3 and r['hidden_selected_rows']==1
assert sorted(row['name'] for row in r['rows'])==['hard-alias.txt','hard-original.txt']
print('PASS: six checks; 3153 tests; recursive identity Find 16/16 matches raw source/alias facts, excludes copy/link and exposes unknown archive metadata; neighboring visible-group selection 14/14 preserves cursor/hidden picks and JSON')
