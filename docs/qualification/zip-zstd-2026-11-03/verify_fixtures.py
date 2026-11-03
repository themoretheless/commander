from pathlib import Path
import hashlib,json,os,struct,subprocess,zipfile,zlib
q=Path(__file__).resolve().parent/'fixtures';m=json.loads((q/'manifest.json').read_text())
for name,record in m['fixtures'].items():
 blob=(q/name).read_bytes();assert hashlib.sha256(blob).hexdigest()==record['sha256']
 with zipfile.ZipFile(q/name) as z:
  assert len(z.infolist())==len(record['members'])
  for info,expected in zip(z.infolist(),record['members']):
   assert info.filename==expected['path'] and info.compress_type==expected['method']
   start=info.header_offset+30+struct.unpack_from('<H',blob,info.header_offset+26)[0]+struct.unpack_from('<H',blob,info.header_offset+28)[0]
   packed=blob[start:start+info.compress_size];kind=record['invalid']
   if kind=='encrypted':assert info.flag_bits&1;continue
   def decode(limit):return subprocess.run(['zstd','-q','-d','-c','-M'+limit],input=packed,capture_output=True,env={**os.environ,'LC_ALL':'C'})
   result=decode('64MiB')
   if kind in ('window','truncated','trailing','check'):
    assert result.returncode!=0,(name,result.stderr)
    if kind=='window':
     assert b'too much memory' in result.stderr or b'Window size' in result.stderr,result.stderr
     allowed=decode('256MiB');assert allowed.returncode==0
     assert hashlib.sha256(allowed.stdout).hexdigest()==expected['sha256']
    continue
   assert result.returncode==0,(name,result.stderr);data=result.stdout
   assert len(data)==expected['bytes'] and hashlib.sha256(data).hexdigest()==expected['sha256']
   if kind=='crc':assert zlib.crc32(data)!=info.CRC
   elif kind=='size':assert len(data)+1==info.file_size
   else:assert len(data)==info.file_size and zlib.crc32(data)==info.CRC
 print(name,'verified')
print(subprocess.check_output(['zstd','--version'],text=True))
