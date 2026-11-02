from pathlib import Path
import hashlib,json,plistlib,shlex,shutil,subprocess,sys,time
q=Path(__file__).resolve().parent;root=q.parents[2]
scene=sys.argv[1] if len(sys.argv)>1 else 'copy_details'
assert scene == 'copy_details'
app=root/'target/CommanderFolderQA.app';core=app/'Contents/MacOS/CommanderFolderQA-core';wrapper=core.with_name('CommanderFolderQA')
assert subprocess.run(['pgrep','-f',str(core)],stdout=subprocess.DEVNULL).returncode!=0,'Owned native QA process is still running'
core.parent.mkdir(parents=True,exist_ok=True)
plist=app/'Contents/Info.plist';plist.write_bytes(plistlib.dumps(dict(CFBundleExecutable='CommanderFolderQA',CFBundleIdentifier='dev.commander.folder-qa',CFBundleName='CommanderFolderQA',CFBundlePackageType='APPL',CFBundleVersion='1',NSHighResolutionCapable=True)))
binary=root/'target/debug/commander';out=q/'native-captures'/str(time.time_ns());out.mkdir(parents=True)
staged=core.with_name(core.name+'.'+str(time.time_ns())+'.new');shutil.copy2(binary,staged);staged.replace(core)
wrapper.write_text('#!/bin/sh\nexport RUST_BACKTRACE=1\nexec '+shlex.quote(str(core))+' --visual-qa '+shlex.quote(scene)+' --output '+shlex.quote(str(out))+' >'+shlex.quote(str(out/'native.log'))+' 2>&1\n');wrapper.chmod(0o755)
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
key='native-launch.json'
record=q/key
if record.exists():shutil.copy2(record,out/'previous-launch.json')
record.write_text(json.dumps(dict(scenario=scene,out=str(out),binary_sha256=h(binary),core_sha256=h(core),wrapper_sha256=h(wrapper),plist_sha256=h(plist)),indent=2)+'\n')
print(out)
