from pathlib import Path
import hashlib,json,subprocess
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3309 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'checksums';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=16,opener=0,trash=0,free_space=0)
assert (d/'checksum.txt').read_bytes()==b'abc'
expected=hashlib.sha1((d/'checksum.txt').read_bytes()).hexdigest()
assert expected=='a9993e364706816aba3e25717850c26c9cd0d89d'
assert (d/'sha1-digest.txt').read_text()==expected
absolute=(d/'sha1-absolute.sha1').read_text();digest,path=absolute.rstrip('\n').split(' *',1)
assert digest==expected and Path(path).is_absolute() and path.endswith('/left/checksum.txt')
assert (d/'sha1-relative.sha1').read_text()==f'{expected} *./checksum.txt\n'
output=subprocess.run(['/usr/bin/shasum','-a','1','-c','sha1-relative.sha1'],cwd=d,capture_output=True,text=True)
assert output.returncode==0 and ': OK' in output.stdout,(output.stdout,output.stderr)
assert (d/'frame.png').stat().st_size>10000
print('PASS: seven gates, 3309 tests, source/binary hashes, native 14/14 and 16 isolated clipboard writes; actual SHA-1 digest/lists agree with Python and shasum')
