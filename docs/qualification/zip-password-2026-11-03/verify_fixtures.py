from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;count=0;expected=0
for manifest,prefix,extensions in [('oracle.json','',['.zip']),('custom-oracle.json','',['.zipx']),('aes-oracle.json','aes-',['.zipx','.zip'])]:
 for name,record in json.loads((q/manifest).read_text()).items():
  paths=[q/'fixtures'/f'{prefix}{name}{extension}' for extension in extensions]
  paths=[p for p in paths if p.exists()];assert len(paths)==1,(manifest,name,paths)
  assert hashlib.sha256(paths[0].read_bytes()).hexdigest()==record['sha256'],paths[0]
  count+=1
  if 'expected_sha256' in record:
   path=q/'fixtures'/f'{prefix}{name}.txt'
   assert hashlib.sha256(path.read_bytes()).hexdigest()==record['expected_sha256'],path
   expected+=1
empty=json.loads((q/'empty-password-oracle.json').read_text())
assert hashlib.sha256((q/'fixtures/aes-empty-password.zipx').read_bytes()).hexdigest()==empty['sha256']
assert hashlib.sha256((q/'fixtures/aes-empty-password.txt').read_bytes()).hexdigest()==empty['expected_sha256']
count+=1;expected+=1
results=dict(archives=count,expected_payload_hashes=expected)
(q/'fixture-verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
