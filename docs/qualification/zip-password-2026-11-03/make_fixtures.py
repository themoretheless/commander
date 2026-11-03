from pathlib import Path
import os,subprocess,tempfile,json,hashlib,zipfile
q=Path(__file__).resolve().parent
seven=os.environ['SEVEN_ZIP']
payload=('Password extraction: ёж / café\n'*128).encode()
results={}
with tempfile.TemporaryDirectory(prefix='commander-password-fixtures-') as tmp:
 base=Path(tmp); (base/'payload.txt').write_bytes(payload)
 for name,method in [('stored','Copy'),('deflate','Deflate'),('bzip2','BZip2'),('deflate64','Deflate64')]:
  path=q/'fixtures'/f'{name}.zip'
  path.unlink(missing_ok=True)
  subprocess.run([seven,'a','-tzip','-mem=ZipCrypto','-pfixture-password',f'-mm={method}',str(path),'payload.txt'],cwd=base,check=True,stdout=subprocess.DEVNULL)
  out=subprocess.run([seven,'x','-so','-pfixture-password',str(path),'payload.txt'],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE).stdout
  assert out==payload
  with zipfile.ZipFile(path) as z:
   info=z.infolist()[0]; assert info.flag_bits&1
   results[name]=dict(method=info.compress_type,sha256=hashlib.sha256(path.read_bytes()).hexdigest())
(q/'fixtures'/'expected.txt').write_bytes(payload)
(q/'oracle.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
