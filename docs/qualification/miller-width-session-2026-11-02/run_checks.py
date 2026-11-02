from pathlib import Path
import hashlib,json,os,subprocess
q=Path(__file__).resolve().parent;root=q.parents[2]
env=os.environ.copy();records=[]
for key,relative in [('COMMANDER_RHASH_VERIFY','target/qualification-tools/RHash-1.4.6/rhash'),('COMMANDER_B3SUM_VERIFY','target/qualification-tools/b3sum/bin/b3sum')]:
 tool=root/relative
 assert tool.is_file(),tool
 env[key]=str(tool)
 records.append(dict(environment=key,path=str(tool),version=subprocess.check_output([str(tool),'--version'],text=True).strip(),sha256=hashlib.sha256(tool.read_bytes()).hexdigest()))
(q/'validators.json').write_text(json.dumps(records,indent=2)+'\n')
checks=[]
for name,cmd in [('build',['cargo','build','--locked','--features','visual-qa']),('tests',['cargo','test','--locked','--features','visual-qa','--','--test-threads=4']),('clippy',['cargo','clippy','--locked','--all-targets','--features','visual-qa','--','-D','warnings']),('check',['cargo','check','--locked']),('fmt',['cargo','fmt','--check']),('diff',['git','diff','--check'])]:
 with (q/(name+'.log')).open('w') as f:code=subprocess.run(cmd,cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT).returncode
 checks.append(dict(name=name,command=cmd,exit_code=code));(q/'checks.json').write_text(json.dumps(checks,indent=2)+'\n');print(name,code,flush=True)
 if code:break
