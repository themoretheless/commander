from pathlib import Path
import hashlib,json,shutil
Q=Path(__file__).resolve().parent
root=Q.parents[2]
checks=json.loads((Q/'checks.json').read_text())
assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3108 passed; 0 failed; 3 ignored' in (Q/'tests.log').read_text()
for scene,key in [('allocated_info','native-launch.json'),('symlink_info','symlink_info-launch.json')]:
 launch=json.loads((Q/key).read_text())
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']
 source=Path(launch['out'])/scene;target=Q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text())
 assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
s=json.loads((Q/'allocated_info/info-state.json').read_text())
assert s['kind']=='BIN file' and s['size']=='8.0 MB'
assert s['logical_bytes']==8388608 and s['allocated_bytes']==0 and s['allocated_size']=='0 B (0 bytes)'
assert s['path'].endswith('/left/sparse.bin')
s=json.loads((Q/'symlink_info/info-state.json').read_text())
assert s['kind']=='Symlink to TXT file' and s['link_status']=='Target exists'
assert s['path'].endswith('-> ../right/portable.txt')
print('PASS: six checks, 3108 tests; sparse-file Info 14/14, symlink Info 14/14; logical and allocated sizes remain distinct')
