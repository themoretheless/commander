from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/command-stop-race-2026-10-31');launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out']);src=out/'per_item_command';m=json.loads((src/'manifest.json').read_text());s=json.loads((src/'per-item-state.json').read_text());r=json.loads((src/'command-report.json').read_text())
assert m['status']=='captured' and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']['clipboard']==3 and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
assert s['batch']==dict(total=2,completed=2,succeeded=2,failed=0,cancelled=False,stopped_on_error=False)
assert s['exit_code']==0 and s['error'] is None and s['regex_ready'] and s['search_state']==[2,0] and s['text_percent']==125 and s['line_state']==[True,10,9,False,True]
assert (src/'copied-stdout.txt').read_text()==s['stdout']==r['stdout']
assert s['stderr']==r['stderr']=='' and s['context']==r['context'] and r['batch']==s['batch']
assert s['stdout'].count(s['expected_cwd']+'\n')==2 and '\nnotes.md\n' in s['stdout'] and '\nDesign\n' in s['stdout']
after=json.loads((q/'stress-after.json').read_text());before=json.loads((q/'stress-before.json').read_text());assert before['attempts']==64 and before['failed']==2 and after['attempts']==256 and after['failed']==0
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest();assert launch['binary_sha256']==launch['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
shutil.copytree(src,q/'native',dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/'native.log')
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/app/mod.rs','src/app/run_command_dialog.rs','src/command_batch.rs','src/visual_qa.rs']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',checks=16,normal_invocations=2,normal_output_reconstructed=True,recording_clipboard_writes=3,stress_before_failures=2,stress_after_failures=0,stress_after_attempts=256,binary_matches_launch=True),indent=2)+'\n')
print('16 native checks and normal output passed; 64-before / 256-after stress results verified')
