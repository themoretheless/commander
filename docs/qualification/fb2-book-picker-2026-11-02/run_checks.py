from pathlib import Path
import json,os,subprocess
q=Path(__file__).resolve().parent;root=q.parents[2]
env=os.environ.copy();env.update(CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0')
(q/'environment.json').write_text(json.dumps({k:env[k] for k in ['CARGO_INCREMENTAL','CARGO_PROFILE_DEV_DEBUG','CARGO_PROFILE_TEST_DEBUG']},indent=2)+'\n')
checks=[]
commands=[('audit',['cargo','deny','--all-features','--locked','check','advisories','bans','licenses','sources']),('build',['cargo','build','--locked','--features','visual-qa']),('tests',['cargo','test','--locked','--features','visual-qa','--','--test-threads=4']),('clippy',['cargo','clippy','--locked','--all-targets','--features','visual-qa','--','-D','warnings']),('check',['cargo','check','--locked']),('fmt',['cargo','fmt','--check']),('diff',['git','diff','--check'])]
commands += [('build-final',['cargo','build','--locked','--features','visual-qa']),('menu-final',['cargo','test','--locked','--features','visual-qa','book_picker_switches','--','--test-threads=1'])]
for name,cmd in commands:
 with (q/(name+'.log')).open('w') as f:code=subprocess.run(cmd,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT).returncode
 checks.append(dict(name=name,command=cmd,exit_code=code));(q/'checks.json').write_text(json.dumps(checks,indent=2)+'\n');print(name,code,flush=True)
 if code:break
