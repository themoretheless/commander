from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3122 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for scene,key,clips in [('file_identity_info','native-launch.json',0),('listing_json','listing_json-launch.json',2),('symlink_info','symlink_info-launch.json',0)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured'
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clips and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
facts={f['name']:f for f in json.loads((q/'file_identity_info/identity-facts.json').read_text())}
a=facts['original.bin'];b=facts['alias.bin'];link=facts['link.bin']
assert (a['device_id'],a['file_id'])==(b['device_id'],b['file_id'])
assert a['hard_links']==b['hard_links']==2
assert link['device_id']==a['device_id'] and link['file_id']!=a['file_id'] and link['hard_links']==1
state=json.loads((q/'file_identity_info/info-state.json').read_text())
assert state['name']=='alias.bin' and state['kind']=='BIN file' and state['hard_links']=='2'
assert state['filesystem_id']==f"Device {b['device_id']}, inode {b['file_id']}"
assert state['path'].endswith('/left/alias.bin')
full=json.loads((q/'listing_json/listing-full.json').read_text());selected=json.loads((q/'listing_json/listing-selected.json').read_text())
rows={r['name']:r for r in full['rows']}
assert full['exported_rows']==9 and selected['exported_rows']==2 and selected['hidden_selected_rows']==1
assert [r['name'] for r in selected['rows']]==['sparse.bin','dense.bin']
for row in full['rows']:
 assert type(row['device_id']) is int and type(row['file_id']) is int
 if row['kind']=='File':assert row['hard_links']==1
 else:assert row['hard_links'] is None
assert rows['zz-link.bin']['file_id']!=rows['sparse.bin']['file_id']
assert rows['zz-link.bin']['device_id']==rows['sparse.bin']['device_id']
for r in selected['rows']:
 before=rows[r['name']]
 for key in ['device_id','file_id','hard_links']:assert r[key]==before[key]
s=json.loads((q/'symlink_info/info-state.json').read_text());assert s['kind']=='Symlink to TXT file' and s['link_status']=='Target exists'
print('PASS: six checks, 3122 tests; hard-link Info 14/14, listing JSON 14/14, symlink Info 14/14; raw device/inode/count reference agrees')
