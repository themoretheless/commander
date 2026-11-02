from pathlib import Path
import hashlib,json,zipfile
q=Path(__file__).resolve().parent/'fixtures'
m=json.loads((q/'manifest.json').read_text())
for name,record in m['fixtures'].items():
 p=q/name
 assert hashlib.sha256(p.read_bytes()).hexdigest()==record['sha256']
 with zipfile.ZipFile(p) as z:
  assert len(z.infolist())==len(record['members'])
  for info,expected in zip(z.infolist(),record['members']):
   assert info.filename==expected['path'] and info.compress_type==expected['method'] and info.file_size==expected['bytes']
   if 'invalid' in record:
    try: z.read(info)
    except zipfile.BadZipFile: pass
    else: raise AssertionError('Expected bad CRC')
   else:
    data=z.read(info)
    assert len(data)==expected['bytes'] and hashlib.sha256(data).hexdigest()==expected['sha256']
 print(name,'verified')
