from pathlib import Path
import io,tarfile,gzip,bz2,lzma,hashlib,json,subprocess
q=Path(__file__).resolve().parent;f=q/'fixtures';f.mkdir(exist_ok=True)
payload='tar archive needle: ёж / café\n'.encode()
stream=io.BytesIO()
with tarfile.open(fileobj=stream,mode='w',format=tarfile.PAX_FORMAT) as archive:
 for name,kind,body in [('z-same.txt','file',payload),('folder/','dir',b''),('a-same.txt','file',payload),('different.txt','file',b'other'),('empty-one','file',b''),('empty-two','file',b''),('sym','sym',b''),('hard','hard',b''),('dup.txt','file',payload),('dup.txt','file',payload),('nested/'+'long-'*25+'ёж.txt','file',payload)]:
  info=tarfile.TarInfo(name);info.mtime=0;info.mode=0o644;info.size=len(body)
  if kind=='dir':info.type=tarfile.DIRTYPE
  elif kind=='sym':info.type=tarfile.SYMTYPE;info.linkname='z-same.txt'
  elif kind=='hard':info.type=tarfile.LNKTYPE;info.linkname='z-same.txt'
  archive.addfile(info,io.BytesIO(body))
raw=stream.getvalue();(f/'groups.tar').write_bytes(raw)
(f/'groups.tar.gz').write_bytes(gzip.compress(raw,mtime=0));(f/'groups.tar.bz2').write_bytes(bz2.compress(raw));(f/'groups.tar.xz').write_bytes(lzma.compress(raw))
subprocess.run(['/opt/homebrew/bin/zstd','-q','-f',str(f/'groups.tar'),'-o',str(f/'groups.tar.zst')],check=True)
with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
 entries=[dict(index=i,path=m.name,size=m.size,regular=m.isfile(),sha256=hashlib.sha256(archive.extractfile(m).read()).hexdigest() if m.isfile() else None) for i,m in enumerate(archive.getmembers())]
(q/'fixture-oracle.json').write_text(json.dumps(dict(entries=entries,files={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in f.iterdir()}),indent=2,ensure_ascii=False)+'\n')
