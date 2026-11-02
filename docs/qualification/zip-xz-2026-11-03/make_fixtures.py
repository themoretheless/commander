from pathlib import Path
import hashlib,json,lzma,struct,subprocess,zlib
q=Path(__file__).resolve().parent;d=q/'fixtures';d.mkdir(exist_ok=True);manifest={}
def archive(name,rows,invalid=False):
 body=bytearray();central=bytearray();records=[]
 for path,plain,packed in rows:
  encoded=path.encode();flags=0x800|(1 if invalid=='encrypted' else 0);crc=zlib.crc32(plain)^(1 if invalid=='crc' else 0);offset=len(body);size=len(plain)+(1 if invalid=='size' else 0)
  mode=0o120777 if path.startswith('link-') else 0o100644
  body+=struct.pack('<IHHHHHIIIHH',0x04034b50,63,flags,95,0,0,crc,len(packed),size,len(encoded),0)+encoded+packed
  central+=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x33f,63,flags,95,0,0,crc,len(packed),size,len(encoded),0,0,0,0,mode<<16,offset)+encoded
  records.append(dict(path=path,bytes=len(plain),sha256=hashlib.sha256(plain).hexdigest(),method=95))
 body+=central+struct.pack('<IHHHHIIH',0x06054b50,0,0,len(rows),len(rows),len(central),len(body),0)
 (d/name).write_bytes(body);manifest[name]=dict(sha256=hashlib.sha256(body).hexdigest(),members=records,invalid=invalid)
def vli(blob,offset):
 value=0;shift=0
 while True:
  byte=blob[offset];offset+=1;value|=(byte&127)<<shift
  if byte<128:return value,offset
  shift+=7
  assert shift<63
# Read the independent producer's index to locate each block, not a magic offset.
def blocks(packed):
 index_size=(int.from_bytes(packed[-8:-4],'little')+1)*4;index=len(packed)-12-index_size
 assert packed[index]==0
 count,pos=vli(packed,index+1);offset=12;result=[]
 for _ in range(count):
  unpadded,pos=vli(packed,pos);size,pos=vli(packed,pos);result.append((offset,unpadded));offset+=(unpadded+3)&~3
 return result
def large_dictionary(packed,block):
 packed=bytearray(packed);offset,unused=blocks(packed)[block];length=(packed[offset]+1)*4
 flags=packed[offset+1];pos=offset+2
 if flags&64:_,pos=vli(packed,pos)
 if flags&128:_,pos=vli(packed,pos)
 found=False
 for _ in range((flags&3)+1):
  kind,pos=vli(packed,pos);size,pos=vli(packed,pos)
  if kind==33 and size==1:packed[pos]=40;found=True # LZMA2 4 GiB dictionary
  pos+=size
 assert found
 packed[offset+length-4:offset+length]=zlib.crc32(packed[offset:offset+length-4]).to_bytes(4,'little')
 return bytes(packed)
def xz(data,check=lzma.CHECK_CRC64):return lzma.compress(data,format=lzma.FORMAT_XZ,check=check)
payload='ZIP XZ needle: ёж / café\n'.encode();(d/'expected.txt').write_bytes(payload)
normal=xz(payload)
archive('normal.zipx',[(name,data,xz(data,check)) for name,data,check in [('payload.txt',payload,lzma.CHECK_CRC32),('nested/ёж.txt',payload*3,lzma.CHECK_CRC64),('empty.txt',b'',lzma.CHECK_SHA256),('unchecked.txt',payload,lzma.CHECK_NONE)]])
joined=xz(payload)+b'\0'*4+xz(payload*2)+b'\0'*8
archive('concatenated.zipx',[('payload.txt',payload*3,joined)])
archive('huge-dictionary.zipx',[('payload.txt',payload,large_dictionary(normal,0))],'memory')
multi=subprocess.check_output(['xz','--compress','--stdout','--check=crc32','--block-size=16'],input=payload*3)
assert len(blocks(multi))>1
archive('later-block-memory.zipx',[('payload.txt',payload*3,large_dictionary(multi,1))],'memory')
archive('later-stream-memory.zipx',[('payload.txt',payload*2,normal+large_dictionary(normal,0))],'memory')
archive('bad-crc.zipx',[('payload.txt',payload,xz(payload,lzma.CHECK_NONE))],'crc')
archive('bad-size.zipx',[('payload.txt',payload,normal)],'size')
archive('truncated.zipx',[('payload.txt',payload,normal[:-4])],'truncated')
archive('trailing.zipx',[('payload.txt',payload,normal+b'bad')],'trailing')
corrupt=bytearray(normal);offset,unpadded=blocks(normal)[0];corrupt[offset+unpadded-8]^=1
archive('bad-check.zipx',[('payload.txt',payload,corrupt)],'check')
archive('encrypted.zipx',[('payload.txt',payload,normal)],'encrypted')
archive('ratio.zipx',[('ratio.txt',b'x'*262144,xz(b'x'*262144))])
archive('unsafe.zipx',[('../escape.txt',payload,normal),('inside.txt',payload,normal)])
archive('symlinks.zipx',[(name,data,xz(data)) for name,data in [('target.txt',payload),('link-safe',b'target.txt'),('link-escape',b'../../escape')]])
(d/'manifest.json').write_text(json.dumps(dict(producer='Python lzma FORMAT_XZ and xz --block-size encoder',fixtures=manifest),ensure_ascii=False,indent=2)+'\n')
print('XZ ZIP method-95 fixtures generated')
