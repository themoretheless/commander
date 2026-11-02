from pathlib import Path
import json,hashlib,shutil,re
q=Path('docs/qualification/symlink-info-cache-2026-10-31');launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out']);src=out/'symlink_info';m=json.loads((src/'manifest.json').read_text());s=json.loads((src/'info-state.json').read_text())
assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks']) and all(v==0 for v in m['native_effect_calls'].values())
assert s['kind']=='Symlink to TXT file' and s['link_status']=='Target exists' and s['size']=='16 B' and s['path'].endswith('/left/link.txt -> ../right/portable.txt')
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
assert launch['binary_sha256']==launch['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
shutil.copytree(src,q/'native',dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/'native.log')
files={'src/panel/link_info.rs','src/panel.rs','src/panel/preview.rs','src/app/render.rs','src/visual_qa.rs','.github/workflows/ci.yml'}
files.update(str(p) for p in Path('src').rglob('*.rs') if re.search(r'\bsymlink: None',p.read_text()))
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in sorted(files)},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',checks=14,link_status=s['link_status'],kind=s['kind'],target_suffix='../right/portable.txt',size=s['size'],native_effect_calls=0,binary_matches_launch=True),indent=2)+'\n')
print('14 native checks, cached target/status and exact launch hashes verified')
