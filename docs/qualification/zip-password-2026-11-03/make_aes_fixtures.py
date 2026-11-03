from pathlib import Path
import os,struct,zipfile,zlib,hashlib,hmac,json,subprocess,tempfile
q=Path(__file__).resolve().parent;root=q.parents[2];seven=os.environ['SEVEN_ZIP'];manifest={};password=b'fixture-password'
def packed_source(cohort,fixture='normal',index=0,content=None):
 path=root/'docs/qualification'/f'{cohort}-2026-11-03'/'fixtures'/f'{fixture}.zipx';data=path.read_bytes()
 with zipfile.ZipFile(path) as z:
  info=z.infolist()[index];at=info.header_offset;n,e=struct.unpack_from('<HH',data,at+26);at+=30+n+e
  packed=data[at:at+info.compress_size]
 expected=content if content is not None else (b'' if info.file_size==0 else (path.parent/'expected.txt').read_bytes())
 assert zlib.crc32(expected)==info.CRC and len(expected)==info.file_size
 return info.compress_type,info.flag_bits,packed,expected

def make(name,source,strength=3,version=2,invalid=False,tail=b'',descriptor=False,override_crc=None,pwd=password):
 method,flags,packed,expected=source;flags|=1
 if descriptor:flags|=8
 key_len={1:16,2:24,3:32}[strength];salt=bytes(range(key_len//2))
 keys=hashlib.pbkdf2_hmac('sha1',pwd,salt,1000,2*key_len+2)
 plain=packed+tail
 counters=b''.join(i.to_bytes(16,'little') for i in range(1,(len(plain)+15)//16+1))
 keystream=subprocess.run(['openssl','enc',f'-aes-{key_len*8}-ecb','-K',keys[:key_len].hex(),'-nopad','-nosalt'],input=counters,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True).stdout
 encrypted=bytes(a^b for a,b in zip(plain,keystream))
 tag=hmac.new(keys[key_len:key_len*2],encrypted,hashlib.sha1).digest()[:10]
 if invalid=='tag':tag=tag[:-1]+bytes([tag[-1]^1])
 body=salt+keys[-2:]+encrypted+tag
 if invalid=='short':body=body[:key_len//2+1]
 crc=zlib.crc32(expected) if version==1 else 0
 if override_crc is not None:crc=override_crc
 if invalid=='crc':crc^=1
 size=len(expected)+(1 if invalid=='size' else 0)
 extra=struct.pack('<HHH2sBH',0x9901,7,version,b'AE',strength,method)
 member=b'payload.txt';dos=0x6b7d;date=0x5d63
 local=struct.pack('<IHHHHHIIIHH',0x04034b50,63,flags,99,dos,date,0 if descriptor else crc,0 if descriptor else len(body),0 if descriptor else size,len(member),len(extra))+member+extra+body
 if descriptor:local+=struct.pack('<IIII',0x08074b50,crc,len(body),size)
 central=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x33f,63,flags,99,dos,date,crc,len(body),size,len(member),len(extra),0,0,0,0o100644<<16,0)+member+extra
 output=local+central+struct.pack('<IHHHHIIH',0x06054b50,0,0,1,1,len(central),len(local),0)
 target=q/'fixtures'/f'aes-{name}.zipx';target.write_bytes(output)
 oracle_method=method;check=target
 with tempfile.TemporaryDirectory() as tmp:
  if method==20:
   # 7-Zip 26.05 does not route legacy method 20. Only relabel the inner
   # compression method to 93; salt/verifier/ciphertext/MAC are identical.
   patch=bytearray(output);struct.pack_into('<H',patch,30+len(member)+9,93);struct.pack_into('<H',patch,len(local)+46+len(member)+9,93)
   check=Path(tmp)/'method93.zipx';check.write_bytes(patch);oracle_method=93
  result=subprocess.run([seven,'x','-so','-p'+pwd.decode(),str(check),'payload.txt'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 decomposed_oracle=False
 if invalid is False:
  if result.returncode!=0 and (not expected or tail):
   # Some 7-Zip AES/custom-codec empty-entry routes report data-after-EOS.
   # Verify the exact encrypted compressed bytes through AES+Stored instead.
   # The original empty codec stream is retained from its qualified fixture.
   patch=bytearray(output)
   struct.pack_into('<H',patch,30+len(member)+9,0)
   struct.pack_into('<H',patch,len(local)+46+len(member)+9,0)
   struct.pack_into('<I',patch,22,len(plain))
   struct.pack_into('<I',patch,len(local)+24,len(plain))
   with tempfile.TemporaryDirectory() as tmp:
    check=Path(tmp)/'stored-envelope.zip';check.write_bytes(patch)
    raw=subprocess.run([seven,'x','-so','-p'+pwd.decode(),str(check),'payload.txt'],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
   assert raw.stdout==plain,name
   decomposed_oracle=True
  else:
   assert result.returncode==0,(name,result.stderr)
   assert result.stdout==expected,name
 elif invalid in ('tag','crc','size','short'):
  assert result.returncode!=0,(name,'independent decoder accepted invalid entry')
 (q/'fixtures'/f'aes-{name}.txt').write_bytes(expected)
 manifest[name]=dict(method=method,oracle_method=oracle_method,bits=key_len*8,version=version,invalid=invalid,descriptor=descriptor,tail_bytes=len(tail),sha256=hashlib.sha256(output).hexdigest(),expected_sha256=hashlib.sha256(expected).hexdigest(),external_exit=result.returncode,decomposed_oracle=decomposed_oracle)

sources={name:packed_source(cohort,fixture) for name,cohort,fixture in [('lzma','zip-lzma','normal'),('xz','zip-xz','normal'),('zstd','zip-zstd','normal'),('legacy','zip-zstd','legacy'),('ppmd','zip-ppmd','normal')]}
for name,source in sources.items():
 for strength in [1,2,3]:
  for version in [1,2]:make(f'{name}-{strength}-v{version}',source,strength,version)
 make(f'{name}-bad-tag',source,invalid='tag')
for name in ['lzma','xz','ppmd']:
 make(f'{name}-empty',packed_source('zip-'+name,index=2))
make('descriptor',sources['lzma'],descriptor=True)
make('utf8-password',sources['ppmd'],pwd='fixture-пароль-ёж'.encode())
make('ae1-bad-crc',sources['ppmd'],version=1,invalid='crc')
make('ae2-nonzero-crc',sources['ppmd'],override_crc=0x12345678)
make('bad-size',sources['ppmd'],invalid='size')
make('short-header',sources['xz'],invalid='short')
# Deliberately keep ciphertext after PPMd EOS. Authentication must include it,
# even when a codec has already yielded the exact declared plaintext and CRC.
make('ppmd-tail',sources['ppmd'],tail=b'\x5a'*131072)
make('ppmd-tail-bad-tag',sources['ppmd'],tail=b'\x5a'*131072,invalid='tag')
# Valid encrypted directories must authenticate before filesystem creation.
# Check the envelope through a regular file first: some external archivers
# skip directory payloads/tags. Path/mode metadata is not covered by the MAC.
for name,invalid in [('directory',False),('directory-bad-tag','tag')]:
 make(name,(0,0x800,b'',b''),invalid=invalid)
 target=q/'fixtures'/f'aes-{name}.zipx';data=bytearray(target.read_bytes())
 with zipfile.ZipFile(target) as z: info=z.infolist()[0];central=z.start_dir
 data[30:41]=b'empty-dir1/'
 data[central+46:central+57]=b'empty-dir1/'
 struct.pack_into('<I',data,central+38,(0o40755<<16)|0x10)
 target.write_bytes(data);manifest[name]['sha256']=hashlib.sha256(data).hexdigest();manifest[name]['directory']=True
for name,index,content,invalid in [('symlink-safe',1,b'target.txt',False),('symlink-escape',2,b'../../escape',False),('symlink-bad-tag',1,b'target.txt','tag')]:
 make(name,packed_source('zip-ppmd','symlinks',index,content),invalid=invalid)
 target=q/'fixtures'/f'aes-{name}.zipx';data=bytearray(target.read_bytes())
 with zipfile.ZipFile(target) as z: central=z.start_dir
 struct.pack_into('<I',data,central+38,0o120777<<16)
 target.write_bytes(data);manifest[name]['sha256']=hashlib.sha256(data).hexdigest();manifest[name]['symlink']=True
# The decoder budgets still apply after AES decryption, before allocation.
for name,cohort,fixture in [('lzma-memory','zip-lzma','memory-limit'),('xz-memory','zip-xz','huge-dictionary'),('ppmd-memory','zip-ppmd','large-arena'),('zstd-memory','zip-zstd','huge-window')]:
 source=packed_source(cohort,fixture)
 if name=='xz-memory':
  method,flags,packed,expected=source;packed=bytearray(packed);length=(packed[12]+1)*4
  at=packed.index(b'\x21\x01\x28',14,12+length)+2;packed[at]=29
  packed[12+length-4:12+length]=zlib.crc32(packed[12:12+length-4]).to_bytes(4,'little')
  source=(method,flags,bytes(packed),expected)
 make(name,source)
# Also keep genuine externally created standard-codec AES archives, not only
# independent OpenSSL envelopes. Their random salts make regeneration differ.
with tempfile.TemporaryDirectory(prefix='commander-aes-fixtures-') as tmp:
 base=Path(tmp);expected=(q/'fixtures'/'expected.txt').read_bytes();(base/'payload.txt').write_bytes(expected)
 for name,method in [('stored','Copy'),('deflate','Deflate'),('bzip2','BZip2'),('deflate64','Deflate64')]:
  for bits in [128,192,256]:
   target=q/'fixtures'/f'aes-{name}-{bits}.zip';target.unlink(missing_ok=True)
   subprocess.run([seven,'a','-tzip',f'-mem=AES{bits}','-pfixture-password',f'-mm={method}',str(target),'payload.txt'],cwd=base,check=True,stdout=subprocess.DEVNULL)
   result=subprocess.run([seven,'x','-so','-pfixture-password',str(target),'payload.txt'],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
   assert result.stdout==expected
   manifest[f'{name}-{bits}']=dict(producer='7-Zip 26.05',bits=bits,sha256=hashlib.sha256(target.read_bytes()).hexdigest(),external_exit=0)
(q/'aes-oracle.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps(manifest,indent=2))
