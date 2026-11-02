from pathlib import Path
import hashlib,json,lzma,struct,zipfile
q=Path(__file__).resolve().parent/'fixtures'
m=json.loads((q/'manifest.json').read_text())
for name,record in m['fixtures'].items():
 p=q/name;blob=p.read_bytes();assert hashlib.sha256(blob).hexdigest()==record['sha256']
 with zipfile.ZipFile(p) as z:
  assert len(z.infolist())==len(record['members'])
  for info,expected in zip(z.infolist(),record['members']):
   assert info.filename==expected['path'] and info.compress_type==14
   start=info.header_offset+30+struct.unpack_from('<H',blob,info.header_offset+26)[0]+struct.unpack_from('<H',blob,info.header_offset+28)[0]
   packed=blob[start:start+info.compress_size];kind=record['invalid']
   if kind in ('dictionary','memory'):
    dictionary=int.from_bytes(packed[5:9],'little');assert dictionary==(0xffffffff if kind=='dictionary' else 64*1024*1024)
    continue # never ask an independent decoder to allocate the forged dictionary
   if kind=='properties':assert packed[2:4]==b'\x04\x00';continue
   if kind=='encrypted':assert info.flag_bits&1;continue
   if kind=='size':assert info.file_size==expected['bytes']+1;continue
   if kind=='crc':
    try:z.read(info)
    except zipfile.BadZipFile:pass
    else:raise AssertionError('bad CRC accepted')
    continue
   alone=packed[4:9]+((2**64-1) if info.flag_bits&2 else info.file_size).to_bytes(8,'little')+packed[9:]
   if kind in ('eos','truncated'):
    try:lzma.decompress(alone,format=lzma.FORMAT_ALONE)
    except lzma.LZMAError:pass
    else:raise AssertionError('invalid stream accepted')
    continue
   data=z.read(info);assert len(data)==expected['bytes'] and hashlib.sha256(data).hexdigest()==expected['sha256']
   assert lzma.decompress(alone,format=lzma.FORMAT_ALONE)==data
   if not expected['eos']:
    raw=lzma.LZMADecompressor(format=lzma.FORMAT_RAW,filters=[lzma._decode_filter_properties(lzma.FILTER_LZMA1,packed[4:9])])
    assert raw.decompress(packed[9:])==data and not raw.eof
 print(name,'verified')
