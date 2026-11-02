from pathlib import Path
import hashlib,json,struct,zipfile
q=Path(__file__).resolve().parent; d=q/'fixtures';d.mkdir(exist_ok=True)
records={}
def archive(name, rows):
 p=d/name
 with zipfile.ZipFile(p,'w') as z:
  for path,data,method in rows:
   info=zipfile.ZipInfo(path,date_time=(2001,2,3,4,5,6));info.compress_type=method;info.external_attr=0o100644<<16
   z.writestr(info,data)
 records[name]={'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'members':[{'path':p,'bytes':len(b),'sha256':hashlib.sha256(b).hexdigest(),'method':m} for p,b,m in rows]}
 return p
payload='BZip2 archive needle: ёж / café\n'.encode()
archive('mixed.zipx', [('stored.txt',b'stored\n',zipfile.ZIP_STORED),('deflated.txt',b'deflated\n',zipfile.ZIP_DEFLATED),('nested/ёж.txt',payload,zipfile.ZIP_BZIP2),('empty.txt',b'',zipfile.ZIP_BZIP2)])
p=archive('bad-crc.zipx',[('payload.txt',payload,zipfile.ZIP_BZIP2)])
b=bytearray(p.read_bytes());central=b.index(b'PK\x01\x02');b[14]^=1;b[central+16]^=1;p.write_bytes(b);records[p.name]['sha256']=hashlib.sha256(b).hexdigest();records[p.name]['invalid']='Deliberately incorrect CRC in local and central headers'
archive('unsafe.zipx',[('../escape.txt',b'outside',zipfile.ZIP_BZIP2),('inside.txt',payload,zipfile.ZIP_BZIP2)])
archive('ratio.zipx',[('ratio.txt',b'x'*(256*1024),zipfile.ZIP_BZIP2)])
(d/'manifest.json').write_text(json.dumps({'producer':'Python stdlib zipfile (independent of zip-rs)','fixtures':records},ensure_ascii=False,indent=2)+'\n')
