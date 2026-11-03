from pathlib import Path
import os,zipfile,struct,hashlib,hmac,subprocess,tempfile,json
q=Path(__file__).resolve().parent;root=q.parents[2];source=root/'docs/qualification/zip-ppmd-2026-11-03/fixtures/normal.zipx';data=source.read_bytes()
with zipfile.ZipFile(source) as z:
 info=z.infolist()[0];n,e=struct.unpack_from('<HH',data,26);packed=data[30+n+e:30+n+e+info.compress_size]
salt=bytes(range(16));derived=hashlib.pbkdf2_hmac('sha1',b'',salt,1000,66)
counters=b''.join(i.to_bytes(16,'little') for i in range(1,(len(packed)+15)//16+1))
key_stream=subprocess.run(['openssl','enc','-aes-256-ecb','-K',derived[:32].hex(),'-nopad','-nosalt'],input=counters,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True).stdout
cipher=bytes(a^b for a,b in zip(packed,key_stream));tag=hmac.new(derived[32:64],cipher,hashlib.sha1).digest()[:10];body=salt+derived[-2:]+cipher+tag
name=b'payload.txt';extra=struct.pack('<HHH2sBH',0x9901,7,2,b'AE',3,98);flags=info.flag_bits|1
local=struct.pack('<IHHHHHIIIHH',0x04034b50,63,flags,99,0x6b7d,0x5d63,0,len(body),info.file_size,len(name),len(extra))+name+extra+body
central=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x33f,63,flags,99,0x6b7d,0x5d63,0,len(body),info.file_size,len(name),len(extra),0,0,0,0o100644<<16,0)+name+extra
output=local+central+struct.pack('<IHHHHIIH',0x06054b50,0,0,1,1,len(central),len(local),0)
path=q/'fixtures/aes-empty-password.zipx';path.write_bytes(output);expected=(source.parent/'expected.txt').read_bytes();(q/'fixtures/aes-empty-password.txt').write_bytes(expected)
# -p with no argument reads the empty password interactively; use an owned
# output directory so the prompt cannot be mistaken for decoded stdout bytes.
with tempfile.TemporaryDirectory(prefix='commander-empty-password-') as tmp:
 result=subprocess.run([os.environ['SEVEN_ZIP'],'x','-y','-p','-o'+tmp,str(path)],input=b'\n',stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=5)
 assert result.returncode==0,(result.stdout,result.stderr)
 assert (Path(tmp)/'payload.txt').read_bytes()==expected
(q/'empty-password-oracle.json').write_text(json.dumps(dict(sha256=hashlib.sha256(output).hexdigest(),expected_sha256=hashlib.sha256(expected).hexdigest(),external_exit=result.returncode),indent=2)+'\n')
print('7-Zip decoded an independently produced AES/PPMd ZIP using an empty password.')
