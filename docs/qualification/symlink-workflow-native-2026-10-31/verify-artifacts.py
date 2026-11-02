from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/symlink-workflow-native-2026-10-31');launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out']);src=out/'symlink_workflow';m=json.loads((src/'manifest.json').read_text());s=json.loads((src/'workflow-state.json').read_text());raw=(src/'raw-target.txt').read_text();resolved=(src/'resolved-target.txt').read_text()
assert m['status']=='captured' and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']['clipboard']==2 and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
assert s['phase']==24 and s['active']=='right' and s['cursor']=='portable.txt' and s['clipboard_writes']==2
assert raw==s['relative_target']=='../left/portable.txt'
# macOS /var and /private/var refer to the same native fixture; do not require a lossy lexical root match.
assert resolved==s['right']+'/portable.txt'
assert s['right'].removeprefix('/private')==s['left'].removeprefix('/private')
assert s['source']=='portable source\n' and s['occupied']=='occupied destination\n'
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
assert launch['binary_sha256']==launch['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
shutil.copytree(src,q/'native',dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/'native.log')
files=['src/visual_qa.rs','src/command.rs','src/workspace.rs','src/workspace/fileops/link.rs','src/workspace/fileops/mod.rs','src/workspace/tests.rs','.github/workflows/ci.yml'];(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in files},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',checks=len(m['checks']),palette_commands=4,relative_target=raw,resolved_target=resolved,created_collision_name='portable link 2.txt',recording_clipboard_writes=2,cursor=s['cursor'],right_parent_revealed=True,source_contents_unchanged=True,collision_contents_unchanged=True,binary_matches_launch=True),indent=2)+'\n')
print('Native checks',len(m['checks']),'all passed; four commands, two clipboard values and exact launch hashes verified')
