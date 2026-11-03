from pathlib import Path
import os,struct,zipfile,zlib,hashlib,json,subprocess,tempfile
q=Path(__file__).resolve().parent;root=q.parents[2];seven=os.environ['SEVEN_ZIP'];mask=0xffffffff
# Independent fixture producer. zlib updates a complemented CRC; APPNOTE uses
# its uncomplemented register, so convert at the boundary for each key update.
def encrypt(data,password):
 keys=[0x12345678,0x23456789,0x34567890]
 def update(byte):
  keys[0]=~zlib.crc32(bytes([byte]),~keys[0]&mask)&mask
  keys[1]=((keys[1]+(keys[0]&255))*134775813+1)&mask
  keys[2]=~zlib.crc32(bytes([keys[1]>>24]),~keys[2]&mask)&mask
 for b in password:update(b)
 out=bytearray()
 for b in data:
  temp=keys[2]|2;out.append(b^(((temp*(temp^1))>>8)&255));update(b)
 return bytes(out)
manifest={}
for name,source,descriptor,invalid in [
 ('lzma','zip-lzma/normal',False,False),('lzma-descriptor','zip-lzma/normal',True,False),
 ('xz','zip-xz/normal',False,False),('zstd','zip-zstd/normal',False,False),
 ('zstd-legacy','zip-zstd/legacy',False,False),('ppmd','zip-ppmd/normal',False,False),
 ('lzma-memory','zip-lzma/memory-limit',False,'policy'),
 ('xz-memory','zip-xz/huge-dictionary',False,'policy'),
 ('ppmd-memory','zip-ppmd/large-arena',False,'policy'),
 ('ppmd-no-eos','zip-ppmd/missing-eos',False,'eos'),
 ('ppmd-bad-crc','zip-ppmd/normal',False,'crc'),
 ('ppmd-bad-size','zip-ppmd/normal',False,'size'),
 ('xz-truncated','zip-xz/normal',False,'truncated'),
 ('xz-short-header','zip-xz/normal',False,'header'),
]:
 cohort,fixture=source.split('/');path=root/'docs/qualification'/f'{cohort}-2026-11-03'/'fixtures'/f'{fixture}.zipx'
 if not path.exists():
  print('MISSING',path);continue
 data=path.read_bytes()
 with zipfile.ZipFile(path) as z:
  info=z.infolist()[0];method=info.compress_type;crc=info.CRC;size=info.file_size
  at=info.header_offset; n,e=struct.unpack_from('<HH',data,at+26);at+=30+n+e
  packed=data[at:at+info.compress_size];flags=info.flag_bits|1
 if name=='xz-memory':
  packed=bytearray(packed);length=(packed[12]+1)*4
  at=packed.index(b'\x21\x01\x28',14,12+length)+2
  packed[at]=29 # 96 MiB dictionary, above the 64 MiB policy without 4 GiB allocation
  packed[12+length-4:12+length]=zlib.crc32(packed[12:12+length-4]).to_bytes(4,'little')
  packed=bytes(packed)
 if descriptor:flags|=8
 if invalid=='crc':crc^=1
 if invalid=='size':size+=1
 if invalid=='truncated':packed=packed[:len(packed)//2]
 dos=0x6b7d;date=0x5d63
 header=bytes(range(11))+bytes([dos>>8 if descriptor else crc>>24])
 encrypted=encrypt(header+packed,b'fixture-password')
 if invalid=='header':encrypted=encrypted[:6]
 member=b'payload.txt';extra=b''
 local=struct.pack('<IHHHHHIIIHH',0x04034b50,63,flags,method,dos,date,0 if descriptor else crc,0 if descriptor else len(encrypted),0 if descriptor else size,len(member),0)+member+encrypted
 if descriptor:local+=struct.pack('<IIII',0x08074b50,crc,len(encrypted),size)
 central=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x33f,63,flags,method,dos,date,crc,len(encrypted),size,len(member),0,0,0,0,0o100644<<16,0)+member
 output=local+central+struct.pack('<IHHHHIIH',0x06054b50,0,0,1,1,len(central),len(local),0)
 target=q/'fixtures'/f'{name}.zipx';target.write_bytes(output)
 oracle_method=method
 if method==20:
  # 7-Zip 26.05 does not route legacy ZIP method 20. Verify identical encrypted
  # compressed bytes through method 93 instead; record that limitation.
  patched=bytearray(output);struct.pack_into('<H',patched,8,93);struct.pack_into('<H',patched,len(local)+10,93)
  with tempfile.TemporaryDirectory() as tmp:
   check=Path(tmp)/'method93.zipx';check.write_bytes(patched)
   result=subprocess.run([seven,'x','-so','-pfixture-password',str(check),'payload.txt'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
  oracle_method=93
 else:
  result=subprocess.run([seven,'x','-so','-pfixture-password',str(target),'payload.txt'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 if invalid in (False,'policy'):
  assert result.returncode==0,(name,result.stderr)
  assert result.stdout==(path.parent/'expected.txt').read_bytes(),name
  (q/'fixtures'/f'{name}.txt').write_bytes(result.stdout)
 else:assert result.returncode!=0,(name,'external decoder accepted invalid data')
 manifest[name]=dict(source=str(path.relative_to(root)),method=method,descriptor=descriptor,invalid=invalid,oracle_method=oracle_method,sha256=hashlib.sha256(output).hexdigest(),external_exit=result.returncode)
(q/'custom-oracle.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(manifest,indent=2))
