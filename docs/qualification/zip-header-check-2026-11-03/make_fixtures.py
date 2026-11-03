from pathlib import Path
import zipfile,io,struct,json,hashlib,subprocess,os
q=Path(__file__).resolve().parent;manifest={};plain=b'Header consistency needle\n';b=io.BytesIO()
with zipfile.ZipFile(b,'w',compression=zipfile.ZIP_STORED) as z:z.writestr(zipfile.ZipInfo('payload.txt',(2026,11,3,13,27,58)),plain)
normal=b.getvalue()
for name,offset,mask in [('normal',None,0),('name',30,1),('method',8,8),('encrypted',6,1),('descriptor',6,8),('timestamp',10,32),('crc',14,1),('packed-size',18,1),('size',22,1)]:
 data=bytearray(normal)
 if offset is not None:data[offset]^=mask
 path=q/'fixtures'/f'{name}.zip';path.write_bytes(data)
 # The central directory and complete payload stay identical in every mutant.
 with zipfile.ZipFile(io.BytesIO(data)) as z:assert z.infolist()[0].CRC==zipfile.ZipFile(io.BytesIO(normal)).infolist()[0].CRC
 result=subprocess.run([os.environ['SEVEN_ZIP'],'t',str(path)],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 manifest[name]=dict(sha256=hashlib.sha256(data).hexdigest(),external_exit=result.returncode)
duplicate=io.BytesIO()
with zipfile.ZipFile(duplicate,'w') as z:
 for content in [b'old payload',b'new payload']:
  z.writestr(zipfile.ZipInfo('same.txt',(2026,11,3,13,27,58)),content)
data=duplicate.getvalue();path=q/'fixtures/duplicates.zip';path.write_bytes(data)
result=subprocess.run([os.environ['SEVEN_ZIP'],'t',str(path)],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
manifest['duplicates']=dict(sha256=hashlib.sha256(data).hexdigest(),external_exit=result.returncode)
(q/'oracle.json').write_text(json.dumps(manifest,indent=2)+'\n');(q/'fixtures/expected.txt').write_bytes(plain);print(json.dumps(manifest,indent=2))
