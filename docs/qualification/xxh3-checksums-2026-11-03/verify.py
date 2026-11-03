from pathlib import Path
import base64,hashlib,json,zlib
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3473 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'checksums';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=38,opener=0,trash=0,free_space=0)
data=(d/'checksum.txt').read_bytes();assert data==b'abc'
expected=f'{zlib.adler32(data):08x}';assert expected=='024d0127'
assert (d/'adler32-digest.txt').read_text()==expected
assert (d/'adler32-relative.adler32').read_text()==f'{expected} *./checksum.txt\n'
absolute=(d/'adler32-absolute.adler32').read_text();digest,path=absolute.rstrip('\n').split(' *',1)
assert digest==expected and Path(path).is_absolute() and path.endswith('/left/checksum.txt')
sha1=hashlib.sha1(data).hexdigest();assert (d/'sha1-digest.txt').read_text()==sha1
assert (d/'sha1-relative.sha1').read_text()==f'{sha1} *./checksum.txt\n'
snapshots=[read(d/f'checksum-phase-{phase}.json') for phase in range(393)]
assert snapshots[131]['adler_match'] and snapshots[131]['relative']
assert snapshots[134]['clipboard_calls']==17 and snapshots[134]['clipboard']==expected
assert snapshots[137]['clipboard_calls']==18 and snapshots[137]['clipboard']==f'{expected} *./checksum.txt\n'
assert snapshots[143]['clipboard_calls']==19 and snapshots[143]['clipboard']==absolute
encoded=base64.b64encode(bytes.fromhex(expected)).decode('ascii')
assert encoded=='Ak0BJw==' and (d/'adler32-base64.txt').read_text()==encoded
assert snapshots[146]['clipboard_calls']==20 and snapshots[146]['clipboard']==encoded and snapshots[146]['adler_match'] and not snapshots[146]['relative']
assert not snapshots[149]['adler_match']
assert snapshots[154]['adler_match'] and not snapshots[159]['adler_match'] and snapshots[164]['adler_match']
assert snapshots[164]['clipboard_calls']==20 and snapshots[164]['clipboard']==encoded
assert all(a['clipboard_calls']<=b['clipboard_calls'] for a,b in zip(snapshots,snapshots[1:]))
for bits,base in [(256,164),(512,202)]:
 stem=f'sha3-{bits}'; a=getattr(hashlib,f'sha3_{bits}')(data); expected=a.hexdigest(); encoded=base64.b64encode(a.digest()).decode()
 assert (d/f'{stem}-digest.txt').read_text()==expected
 assert (d/f'{stem}-base64.txt').read_text()==encoded
 digest,path=(d/f'{stem}-absolute.sums').read_text().rstrip('\n').split(' *',1)
 assert digest==expected and Path(path).is_absolute() and path.endswith('/left/checksum.txt')
 key=f'sha3_{bits}'
 assert snapshots[base+11][key+'_match']
 assert snapshots[base+14]['clipboard']==expected
 assert snapshots[base+17]['clipboard']==f'{expected} *{path}\n'
 assert snapshots[base+20]['clipboard']==encoded
 assert snapshots[base+28][key+'_match']
 assert snapshots[base+33][key+'_invalid']
 assert snapshots[base+38][key+'_match']
assert snapshots[240]['clipboard_calls']==26
for stem,base in [('blake2s',240),('blake2b',278)]:
 a=getattr(hashlib,stem)(data);expected=a.hexdigest();encoded=base64.b64encode(a.digest()).decode()
 assert (d/f'{stem}-digest.txt').read_text()==expected
 assert (d/f'{stem}-base64.txt').read_text()==encoded
 digest,path=(d/f'{stem}-absolute.sums').read_text().rstrip('\n').split(' *',1)
 assert digest==expected and Path(path).is_absolute() and path.endswith('/left/checksum.txt')
 assert snapshots[base+11][stem+'_match']
 assert snapshots[base+14]['clipboard']==expected
 assert snapshots[base+17]['clipboard']==f'{expected} *{path}\n'
 assert snapshots[base+20]['clipboard']==encoded
 assert snapshots[base+28][stem+'_match']
 assert snapshots[base+33][stem+'_invalid']
 assert snapshots[base+38][stem+'_match']
assert snapshots[316]['clipboard_calls']==32
vector=next(row for row in read(q/'vectors.json') if row['size']==3)
for bits,base in [(64,316),(128,354)]:
 stem=f'xxh3-{bits}';key=f'xxh3_{bits}';expected=vector[key];encoded=vector[f'base64_{bits}']
 assert hashlib.sha256(data).hexdigest()==vector['sha256']
 assert (d/f'{stem}-digest.txt').read_text()==expected
 assert (d/f'{stem}-base64.txt').read_text()==encoded
 digest,path=(d/f'{stem}-absolute.sums').read_text().rstrip('\n').split(' *',1)
 assert digest==expected and Path(path).is_absolute() and path.endswith('/left/checksum.txt')
 assert snapshots[base+11][key+'_match']
 assert snapshots[base+14]['clipboard']==expected
 assert snapshots[base+17]['clipboard']==f'{expected} *{path}\n'
 assert snapshots[base+20]['clipboard']==encoded
 assert snapshots[base+28][key+'_match']
 assert snapshots[base+33][key+'_invalid']
 assert snapshots[base+38][key+'_match']
assert snapshots[392]['clipboard_calls']==38
(q/'verification.json').write_text(json.dumps(dict(native_checks=14,observations=393,clipboard_calls=38,oracle='Official xxHash C v0.8.3 plus Python Base64',source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 14/14, 393 observations, 38 isolated clipboard writes; XXH3-64/128 Hex/Base64, absolute lists, invalid comparison and recovery match official C reference')
