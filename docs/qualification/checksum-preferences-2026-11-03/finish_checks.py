from pathlib import Path
import json,os,subprocess
q=Path(__file__).resolve().parent;root=q.parents[2]
env=os.environ.copy();env.update(CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0')
checks=json.loads((q/'initial/checks.json').read_text())[:3]
# Behavioural tests/audit already passed; the only follow-up source change is doc markup.
commands=[('build',['cargo','build','--locked','--features','visual-qa']),('clippy',['cargo','clippy','--locked','--all-targets','--features','visual-qa','--','-D','warnings']),('check',['cargo','check','--locked']),('fmt',['cargo','fmt','--check']),('diff',['git','diff','--check'])]
for name,cmd in commands:
 with (q/(name+'.log')).open('w') as f:code=subprocess.run(cmd,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT).returncode
 record=dict(name=name,command=cmd,exit_code=code)
 if name=='build':checks[1]=record
 else:checks.append(record)
 (q/'checks.json').write_text(json.dumps(checks,indent=2)+'\n');print(name,code,flush=True)
 if code:break
