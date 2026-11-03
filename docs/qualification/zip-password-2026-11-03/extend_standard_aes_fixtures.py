from pathlib import Path
import os,zipfile,struct,zlib,hashlib,json,subprocess
q=Path(__file__).resolve().parent;seven=os.environ['SEVEN_ZIP'];manifest=json.loads((q/'aes-oracle.json').read_text());expected=(q/'fixtures'/'expected.txt').read_bytes()
def field_offset(data,start,length):
 end=start+length
 while start<end:
  tag,size=struct.unpack_from('<HH',data,start)
  if tag==0x9901:return start
  start+=4+size
 raise AssertionError('missing AES extra field')
for method in ['stored','deflate','bzip2','deflate64']:
 for bits in [128,192,256]:
  path=q/'fixtures'/f'aes-{method}-{bits}.zip';original=path.read_bytes()
  with zipfile.ZipFile(path) as z:info=z.infolist()[0];central=z.start_dir
  n,e=struct.unpack_from('<HH',original,26);local_extra=30+n;offset=local_extra+e
  cn,ce=struct.unpack_from('<HH',original,central+28);central_extra=central+46+cn
  local_aes=field_offset(original,local_extra,e);central_aes=field_offset(original,central_extra,ce)
  data=bytearray(original)
  for at in [local_aes+4,central_aes+4]:struct.pack_into('<H',data,at,1)
  for at in [14,central+16]:struct.pack_into('<I',data,at,zlib.crc32(expected))
  name=f'{method}-{bits}-v1';target=q/'fixtures'/f'aes-{name}.zip';target.write_bytes(data)
  result=subprocess.run([seven,'x','-so','-pfixture-password',str(target),'payload.txt'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
  assert result.returncode==0 and result.stdout==expected,(name,result.stderr)
  manifest[name]=dict(producer='7-Zip AES packet; AE-1 metadata/CRC conversion',sha256=hashlib.sha256(data).hexdigest(),external_exit=result.returncode)
  if bits==256:
   for problem in ['bad-tag','bad-size']:
    data=bytearray(original)
    if problem=='bad-tag':data[offset+info.compress_size-1]^=1
    else:
     for at in [22,central+24]:struct.pack_into('<I',data,at,len(expected)+1)
    name=f'{method}-{problem}';target=q/'fixtures'/f'aes-{name}.zip';target.write_bytes(data)
    result=subprocess.run([seven,'x','-so','-pfixture-password',str(target),'payload.txt'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    if problem=='bad-tag':assert result.returncode!=0,name
    manifest[name]=dict(producer='7-Zip AES packet; negative metadata/tag mutation',sha256=hashlib.sha256(data).hexdigest(),external_exit=result.returncode,declared_size=len(expected)+int(problem=='bad-size'),actual_size=len(expected))
(q/'aes-oracle.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('Verified AE-1 standard-codec matrix; retained bad-tag and forged-size cases with exact external exit codes.')
