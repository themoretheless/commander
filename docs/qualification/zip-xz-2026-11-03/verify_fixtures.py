from pathlib import Path
import hashlib,json,lzma,os,struct,subprocess,zipfile,zlib
q=Path(__file__).resolve().parent/'fixtures';m=json.loads((q/'manifest.json').read_text())
for name,record in m['fixtures'].items():
 blob=(q/name).read_bytes();assert hashlib.sha256(blob).hexdigest()==record['sha256']
 with zipfile.ZipFile(q/name) as z:
  assert len(z.infolist())==len(record['members'])
  for info,expected in zip(z.infolist(),record['members']):
   assert info.filename==expected['path'] and info.compress_type==95
   start=info.header_offset+30+struct.unpack_from('<H',blob,info.header_offset+26)[0]+struct.unpack_from('<H',blob,info.header_offset+28)[0]
   packed=blob[start:start+info.compress_size];kind=record['invalid']
   if kind=='encrypted':assert info.flag_bits&1;continue
   result=subprocess.run(['xz','--decompress','--stdout','--memlimit-decompress=64MiB'],input=packed,capture_output=True,env={**os.environ,"LC_ALL":"C"})
   if kind in ('memory','truncated','trailing','check'):
    assert result.returncode!=0,(name,result.stderr)
    if kind=='memory':assert b'Memory usage limit' in result.stderr,result.stderr
    continue
   assert result.returncode==0,(name,result.stderr)
   data=result.stdout
   assert len(data)==expected['bytes'] and hashlib.sha256(data).hexdigest()==expected['sha256']
   remaining=packed;chunks=[]
   while remaining:
    decoder=lzma.LZMADecompressor(format=lzma.FORMAT_XZ,memlimit=64*1024*1024)
    chunks.append(decoder.decompress(remaining));assert decoder.eof
    remaining=decoder.unused_data
    padding=len(remaining)-len(remaining.lstrip(b'\0'));assert padding%4==0
    remaining=remaining[padding:]
   assert b''.join(chunks)==data
   if kind=='crc':assert zlib.crc32(data)!=info.CRC
   elif kind=='size':assert len(data)+1==info.file_size
   else:assert len(data)==info.file_size and zlib.crc32(data)==info.CRC
 print(name,'verified')
print(subprocess.check_output(['xz','--version'],text=True))
