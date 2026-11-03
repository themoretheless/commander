from pathlib import Path
import os,zipfile,struct,subprocess,json,hashlib
q=Path(__file__).resolve().parent;source=q.parent/'zip-password-2026-11-03'/'fixtures';(q/'fixtures').mkdir(exist_ok=True);manifest={}
for name,second in [('mixed','aes-ppmd-3-v2.zipx'),('late-failure','aes-ppmd-tail-bad-tag.zipx')]:
 local=bytearray();central=bytearray()
 for filename,fixture in [('first.txt','stored.zip'),('second.txt',second)]:
  data=(source/fixture).read_bytes()
  with zipfile.ZipFile(source/fixture) as z:info=z.infolist()[0];at=z.start_dir
  n,e=struct.unpack_from('<HH',data,26);member=filename.encode();head=bytearray(data[:30]);struct.pack_into('<H',head,26,len(member));offset=len(local)
  local+=head+member+data[30+n:at]
  n,e,c=struct.unpack_from('<HHH',data,at+28);head=bytearray(data[at:at+46]);struct.pack_into('<H',head,28,len(member));struct.pack_into('<I',head,42,offset)
  central+=head+member+data[at+46+n:at+46+n+e+c]
 output=local+central+struct.pack('<IHHHHIIH',0x06054b50,0,0,2,2,len(central),len(local),0);path=q/'fixtures'/f'{name}.zipx';path.write_bytes(output)
 result=subprocess.run([os.environ['SEVEN_ZIP'],'t','-pfixture-password',str(path)],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 assert (result.returncode==0)==(name=='mixed'),(name,result.stdout,result.stderr)
 if name=='mixed':
  for filename,expected in [('first.txt','expected.txt'),('second.txt','aes-ppmd-3-v2.txt')]:
   decoded=subprocess.check_output([os.environ['SEVEN_ZIP'],'x','-so','-pfixture-password',str(path),filename],stderr=subprocess.DEVNULL)
   assert decoded==(source/expected).read_bytes()
 manifest[name]=dict(sha256=hashlib.sha256(output).hexdigest(),external_exit=result.returncode)
(q/'oracle.json').write_text(json.dumps(manifest,indent=2)+'\n');print(json.dumps(manifest,indent=2))
