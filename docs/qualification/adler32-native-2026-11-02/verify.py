from pathlib import Path
import hashlib,json,zlib
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3338 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'checksums';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=19,opener=0,trash=0,free_space=0)
data=(d/'checksum.txt').read_bytes();assert data==b'abc'
expected=f'{zlib.adler32(data):08x}';assert expected=='024d0127'
assert (d/'adler32-digest.txt').read_text()==expected
assert (d/'adler32-relative.adler32').read_text()==f'{expected} *./checksum.txt\n'
absolute=(d/'adler32-absolute.adler32').read_text();digest,path=absolute.rstrip('\n').split(' *',1)
assert digest==expected and Path(path).is_absolute() and path.endswith('/left/checksum.txt')
sha1=hashlib.sha1(data).hexdigest();assert (d/'sha1-digest.txt').read_text()==sha1
assert (d/'sha1-relative.sha1').read_text()==f'{sha1} *./checksum.txt\n'
snapshots=[read(d/f'checksum-phase-{phase}.json') for phase in range(144)]
assert snapshots[131]['adler_match'] and snapshots[131]['relative']
assert snapshots[134]['clipboard_calls']==17 and snapshots[134]['clipboard']==expected
assert snapshots[137]['clipboard_calls']==18 and snapshots[137]['clipboard']==f'{expected} *./checksum.txt\n'
assert snapshots[143]['clipboard_calls']==19 and snapshots[143]['clipboard']==absolute and snapshots[143]['adler_match'] and not snapshots[143]['relative']
assert all(a['clipboard_calls']<=b['clipboard_calls'] for a,b in zip(snapshots,snapshots[1:]))
(q/'verification.json').write_text(json.dumps(dict(native_checks=14,observations=144,clipboard_calls=19,adler32=expected,oracle='Python zlib.adler32',source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 14/14, 144 observations, 19 isolated clipboard writes; actual Adler-32 digest and relative/absolute lists match Python zlib')
