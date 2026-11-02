from pathlib import Path
import hashlib,io,json,lzma,shlex,struct,subprocess,tempfile,zipfile,zlib
q=Path(__file__).resolve().parent;d=q/'fixtures';d.mkdir(exist_ok=True)
manifest={}
def compress(data):
 c=zipfile.LZMACompressor();return c.compress(data)+c.flush()
def archive(name,rows,invalid=False):
 body=bytearray();central=bytearray();records=[]
 for path,plain,packed,eos in rows:
  namebytes=path.encode();flags=0x800|(2 if eos else 0)|(1 if invalid=='encrypted' else 0);crc=zlib.crc32(plain)^(1 if invalid=='crc' else 0);offset=len(body)
  size=len(plain)+(1 if invalid=='size' else 0)
  body+=struct.pack('<IHHHHHIIIHH',0x04034b50,63,flags,14,0,0,crc,len(packed),size,len(namebytes),0)+namebytes+packed
  central+=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x33f,63,flags,14,0,0,crc,len(packed),size,len(namebytes),0,0,0,0,(0o120777 if path.startswith('link-') else 0o100644)<<16,offset)+namebytes
  records.append(dict(path=path,bytes=len(plain),sha256=hashlib.sha256(plain).hexdigest(),method=14,eos=eos))
 body+=central+struct.pack('<IHHHHIIH',0x06054b50,0,0,len(rows),len(rows),len(central),len(body),0)
 (d/name).write_bytes(body);manifest[name]=dict(sha256=hashlib.sha256(body).hexdigest(),members=records,invalid=invalid)
payload='ZIP LZMA needle: ёж / café\n'.encode();packed=compress(payload)
(d/'expected.txt').write_bytes(payload)
archive('normal.zipx',[(name,data,compress(data),True) for name,data in [('payload.txt',payload),('nested/ёж.txt',payload*3),('empty.txt',b'')]])
# LZMA1EXT can emit a fully terminated known-size stream without EOPM.
with tempfile.TemporaryDirectory() as build:
 helper=Path(build)/'make-no-eos'
 flags=shlex.split(subprocess.check_output(['pkg-config','--cflags','--libs','liblzma'],text=True))
 subprocess.run(['cc',str(q/'make_no_eos.c'),'-o',str(helper),*flags],check=True)
 trimmed=subprocess.check_output([str(helper),str(d/'expected.txt')])
 properties=trimmed[4:9]
 assert lzma.decompress(properties+len(payload).to_bytes(8,'little')+trimmed[9:],format=lzma.FORMAT_ALONE)==payload
 decoder=lzma.LZMADecompressor(format=lzma.FORMAT_RAW,filters=[lzma._decode_filter_properties(lzma.FILTER_LZMA1,properties)])
 assert decoder.decompress(trimmed[9:])==payload and not decoder.eof
archive('no-eos.zipx',[('payload.txt',payload,trimmed,False)])
archive('missing-required-eos.zipx',[('payload.txt',payload,trimmed,True)],'eos')
archive('encrypted.zipx',[('payload.txt',payload,packed,True)],'encrypted')
archive('symlinks.zipx',[(name,data,compress(data),True) for name,data in [('target.txt',payload),('link-safe',b'target.txt'),('link-escape',b'../../escape')]])
archive('bad-crc.zipx',[('payload.txt',payload,packed,True)],'crc')
archive('bad-size.zipx',[('payload.txt',payload,packed,True)],'size')
archive('truncated.zipx',[('payload.txt',payload,packed[:10],True)],'truncated')
for name,offset,value,kind in [('huge-dictionary.zipx',5,(0xffffffff).to_bytes(4,'little'),'dictionary'),('bad-properties.zipx',2,b'\x04\x00','properties'),('memory-limit.zipx',5,(64*1024*1024).to_bytes(4,'little'),'memory')]:
 changed=bytearray(packed);changed[offset:offset+len(value)]=value
 archive(name,[('payload.txt',payload,changed,True)],kind)
archive('ratio.zipx',[('ratio.txt',b'x'*262144,compress(b'x'*262144),True)])
archive('unsafe.zipx',[('../escape.txt',payload,packed,True),('inside.txt',payload,packed,True)])
(d/'manifest.json').write_text(json.dumps(dict(producer='Python stdlib zipfile LZMACompressor / liblzma',fixtures=manifest),ensure_ascii=False,indent=2)+'\n')
print('ZIP LZMA fixtures generated; actual no-EOS stream found')
