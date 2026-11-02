from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/command-stop-native-2026-10-31'); launch=json.loads((q/'native-launch.json').read_text()); out=Path(launch['out']); src=out/'per_item_stop'
m=json.loads((src/'manifest.json').read_text());s=json.loads((src/'per-item-stop-state.json').read_text());r=json.loads((src/'command-report.json').read_text())
assert m['status']=='captured' and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
assert all(v==0 for v in m['native_effect_calls'].values())
assert s['phase']==6 and s['run_present'] is False
assert s['batch']==dict(total=2,completed=1,succeeded=0,failed=1,cancelled=True,stopped_on_error=False)
assert s['exit_code'] is None and s['error']=='Per-item command cancelled'
assert s['context']['execution']=='per_item_continue' and s['context']['attempted_invocations']==1 and len(s['context']['inputs'])==2
assert len(s['files'])==2 and all(f['contents']=='initial\n' for f in s['files'])
assert s['files'][0]['started']=='started' and s['files'][1]['started'] is None
assert s['context']['inputs']==[f['path'] for f in s['files']]
assert '\nready\n' in s['stdout'] and 'item 2 / 2' not in s['stdout'] and 'Invocation terminated by signal' in s['stderr']
for key in ['context','batch','stdout','stderr','exit_code','error']: assert r[key]==s[key]
log=Path(str(out)+'.log').read_text();assert all(f'Per-item Stop QA phase {phase},' in log for phase in range(6))
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
assert launch['binary_sha256']==launch['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
shutil.copytree(src,q/'native',dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/'native.log')
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/app/mod.rs','src/app/run_command_dialog.rs','src/command_batch.rs','src/command_report.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',checks=16,button_phases=6,attempted_invocations=1,unstarted_invocations=1,original_files_preserved=2,report_matches_result=True,recording_effect_port_calls=0,binary_matches_launch=True),indent=2)+'\n')
print('16 native checks, Run/Stop phases, cancellation, untouched files and report verified')
