from pathlib import Path
import io,tarfile,gzip,json,hashlib
q=Path(__file__).resolve().parent;root=q.parents[2]
source=root/'docs/qualification/tar-content-groups-2026-11-03/fixtures/groups.tar.gz'
stamps=[300,0,100,0,500,200,0,0,400,50,250]
stream=io.BytesIO()
with tarfile.open(source) as old,tarfile.open(fileobj=stream,mode='w',format=tarfile.PAX_FORMAT) as new:
 for index,member in enumerate(old.getmembers()):
  data=old.extractfile(member).read() if member.isfile() else b''
  member.mtime=stamps[index]
  member.pax_headers.pop('mtime',None)
  new.addfile(member,io.BytesIO(data))
raw=gzip.compress(stream.getvalue(),mtime=0)
(q/'fixtures').mkdir(exist_ok=True);(q/'fixtures/groups.tar.gz').write_bytes(raw)
with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
 entries=[dict(index=i,path=m.name,size=m.size,modified=m.mtime,regular=m.isfile(),sha256=hashlib.sha256(archive.extractfile(m).read()).hexdigest() if m.isfile() else None) for i,m in enumerate(archive.getmembers())]
(q/'fixture-oracle.json').write_text(json.dumps(dict(entries=entries,files={'groups.tar.gz':hashlib.sha256(raw).hexdigest()}),indent=2,ensure_ascii=False)+'\n')
