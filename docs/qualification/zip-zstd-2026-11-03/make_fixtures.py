from pathlib import Path
import hashlib,json,struct,subprocess,zlib
q=Path(__file__).resolve().parent;d=q/'fixtures';d.mkdir(exist_ok=True);manifest={}
def compress(data,check=True):return subprocess.check_output(['zstd','-q','-c','--no-content-size','--check' if check else '--no-check'],input=data)
def archive(name,rows,invalid=False,method=93):
 body=bytearray();central=bytearray();records=[]
 for path,plain,packed in rows:
  encoded=path.encode();flags=0x800|(1 if invalid=='encrypted' else 0);crc=zlib.crc32(plain)^(1 if invalid=='crc' else 0);offset=len(body);size=len(plain)+(1 if invalid=='size' else 0)
  mode=0o120777 if path.startswith('link-') else 0o100644
  body+=struct.pack('<IHHHHHIIIHH',0x04034b50,63,flags,method,0,0,crc,len(packed),size,len(encoded),0)+encoded+packed
  central+=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x33f,63,flags,method,0,0,crc,len(packed),size,len(encoded),0,0,0,0,mode<<16,offset)+encoded
  records.append(dict(path=path,bytes=len(plain),sha256=hashlib.sha256(plain).hexdigest(),method=method))
 body+=central+struct.pack('<IHHHHIIH',0x06054b50,0,0,len(rows),len(rows),len(central),len(body),0)
 (d/name).write_bytes(body);manifest[name]=dict(sha256=hashlib.sha256(body).hexdigest(),members=records,invalid=invalid)
def huge_window(packed,log=27):
 packed=bytearray(packed);assert packed[:4]==b'\x28\xb5\x2f\xfd' and not packed[4]&32
 packed[5]=(log-10)<<3 # 128 MiB window, larger than the 64 MiB policy
 return bytes(packed)
payload='ZIP Zstd needle: ёж / café\n'.encode();(d/'expected.txt').write_bytes(payload);normal=compress(payload)
archive('normal.zipx',[(name,data,compress(data,check)) for name,data,check in [('payload.txt',payload,True),('nested/ёж.txt',payload*3,True),('empty.txt',b'',True),('unchecked.txt',payload,False)]])
archive('limit-window.zipx',[('payload.txt',payload,huge_window(normal,26))])
archive('legacy.zipx',[('payload.txt',payload,normal)],method=20)
skip=struct.pack('<II',0x184d2a50,4)+b'meta'
archive('concatenated.zipx',[('payload.txt',payload*3,normal+skip+compress(payload*2))])
archive('skippable.zipx',[('empty.txt',b'',skip)])
archive('huge-window.zipx',[('payload.txt',payload,huge_window(normal))],'window')
archive('legacy-window.zipx',[('payload.txt',payload,huge_window(normal))],'window',method=20)
archive('later-window.zipx',[('payload.txt',payload*2,normal+huge_window(normal))],'window')
archive('bad-crc.zipx',[('payload.txt',payload,compress(payload,False))],'crc')
archive('bad-size.zipx',[('payload.txt',payload,normal)],'size')
archive('truncated.zipx',[('payload.txt',payload,normal[:-3])],'truncated')
archive('trailing.zipx',[('payload.txt',payload,normal+b'bad')],'trailing')
corrupt=bytearray(normal);corrupt[-1]^=1
archive('bad-check.zipx',[('payload.txt',payload,corrupt)],'check')
archive('encrypted.zipx',[('payload.txt',payload,normal)],'encrypted')
archive('ratio.zipx',[('ratio.txt',b'x'*262144,compress(b'x'*262144))])
archive('unsafe.zipx',[('../escape.txt',payload,normal),('inside.txt',payload,normal)])
archive('symlinks.zipx',[(name,data,compress(data)) for name,data in [('target.txt',payload),('link-safe',b'target.txt'),('link-escape',b'../../escape')]])
(d/'manifest.json').write_text(json.dumps(dict(producer='zstd CLI with Python ZIP container writer',fixtures=manifest),ensure_ascii=False,indent=2)+'\n')
