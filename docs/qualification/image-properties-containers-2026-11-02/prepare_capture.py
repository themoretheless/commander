"""Prepare the owned QA wrapper; launch CommanderFolderQA through CUA afterwards."""
from pathlib import Path
import hashlib,json,shlex,shutil,sys,time
q=Path(__file__).resolve().parent
root=q.parents[2]
scene=sys.argv[1] if len(sys.argv)>1 else 'exif_png_info'
assert scene in ('exif_info','exif_png_info','exif_tiff_info','exif_webp_info','image_header_info')
binary=root/'target/debug/commander'
core=root/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core'
wrapper=core.with_name('CommanderFolderQA')
out=q/'native-captures'/str(time.time_ns())
out.mkdir(parents=True)
# Fresh inode avoids in-place modification of a previously executed Mach-O.
staged=core.with_name(core.name+'.'+str(time.time_ns())+'.new')
shutil.copy2(binary,staged)
staged.replace(core)
wrapper.write_text('#!/bin/sh\nexport RUST_BACKTRACE=1\nexec '+shlex.quote(str(core))+' --visual-qa '+shlex.quote(scene)+' --output '+shlex.quote(str(out))+' >'+shlex.quote(str(out/'native.log'))+' 2>&1\n')
wrapper.chmod(0o755)
h=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
key='native-launch.json' if scene=='exif_png_info' else scene+'-launch.json'
record=q/key
if record.exists():shutil.copy2(record,out/'previous-launch.json')
record.write_text(json.dumps(dict(scenario=scene,out=str(out),binary_sha256=h(binary),core_sha256=h(core)),indent=2)+'\n')
print(out)
