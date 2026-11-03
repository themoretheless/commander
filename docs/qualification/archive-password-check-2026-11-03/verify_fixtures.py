from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
for name,record in json.loads((q/'oracle.json').read_text()).items():
 assert hashlib.sha256((q/'fixtures'/f'{name}.zipx').read_bytes()).hexdigest()==record['sha256'],name
for path,digest in json.loads((q/'fixture-dependencies.json').read_text()).items():
 assert hashlib.sha256((root/path).read_bytes()).hexdigest()==digest,path
print('Two mixed archives and every inherited matrix fixture match recorded hashes.')
