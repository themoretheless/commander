from pathlib import Path
import hashlib,json,os,struct,subprocess,tempfile,zipfile,zlib
q=Path(__file__).resolve().parent;d=q/'fixtures';d.mkdir(exist_ok=True);manifest={}
seven=Path(os.environ['SEVEN_ZIP']) if os.environ.get('SEVEN_ZIP') else Path(Path('/tmp/commander-ppmd-tool-path').read_text().strip())/'7zz'
def archive(name,rows,invalid=False):
 body=bytearray();central=bytearray();records=[]
 for path,plain,packed in rows:
  encoded=path.encode();flags=0x800|(1 if invalid=='encrypted' else 0);crc=zlib.crc32(plain)^(1 if invalid=='crc' else 0);offset=len(body);size=len(plain)+(1 if invalid=='size' else 0)
  mode=0o120777 if path.startswith('link-') else 0o100644
  body+=struct.pack('<IHHHHHIIIHH',0x04034b50,63,flags,98,0,0,crc,len(packed),size,len(encoded),0)+encoded+packed
  central+=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x33f,63,flags,98,0,0,crc,len(packed),size,len(encoded),0,0,0,0,mode<<16,offset)+encoded
  records.append(dict(path=path,bytes=len(plain),sha256=hashlib.sha256(plain).hexdigest(),method=98))
 body+=central+struct.pack('<IHHHHIIH',0x06054b50,0,0,len(rows),len(rows),len(central),len(body),0)
 (d/name).write_bytes(body);manifest[name]=dict(sha256=hashlib.sha256(body).hexdigest(),members=records,invalid=invalid)
with tempfile.TemporaryDirectory() as temporary:
 tmp=Path(temporary)
 (tmp/'Cargo.toml').write_text('[package]\nname="ppmd-fixture-producer"\nversion="0.0.0"\nedition="2024"\n[dependencies]\nppmd-rust="=1.5.2"\n')
 (tmp/'src').mkdir();(tmp/'src/main.rs').write_bytes((q/'make_stream.rs').read_bytes())
 subprocess.run(['cargo','build','--offline','--quiet','--manifest-path',str(tmp/'Cargo.toml')],check=True,env={**os.environ,'CARGO_TARGET_DIR':str(tmp/'target'),'CARGO_INCREMENTAL':'0'})
 helper=tmp/'target/debug/ppmd-fixture-producer'
 def raw(data,eos=True):return subprocess.check_output([str(helper)]+([] if eos else ['--no-eos']),input=data)
 def compress(data):
  source=tmp/'payload.txt';output=tmp/'produce.zipx';source.write_bytes(data)
  if output.exists():output.unlink()
  subprocess.run([str(seven),'a','-tzip','-mm=PPMd:mem=1m:o=6',str(output),str(source)],check=True,stdout=subprocess.DEVNULL)
  blob=output.read_bytes()
  with zipfile.ZipFile(output) as z:
   info=z.infolist()[0];assert info.compress_type==98
   start=info.header_offset+30+struct.unpack_from('<H',blob,info.header_offset+26)[0]+struct.unpack_from('<H',blob,info.header_offset+28)[0]
   return blob[start:start+info.compress_size]
 payload=('PPMd archive needle: ёж / café\n'*128).encode();(d/'expected.txt').write_bytes(payload)
 normal=compress(payload)
 archive('normal.zipx',[('payload.txt',payload,normal),('nested/ёж.txt',payload*3,compress(payload*3)),('empty.txt',b'',raw(b''))])
 archive('missing-eos.zipx',[('payload.txt',payload,raw(payload,False))],'eos')
 for name,arena,restore,invalid in [('limit-arena',64,0,False),('cutoff',1,1,False),('large-arena',65,0,'memory'),('bad-restore',1,15,'restore'),('freeze',1,2,'restore')]:
  parameter=int.from_bytes(normal[:2],'little');parameter=(parameter&15)|((arena-1)<<4)|(restore<<12)
  archive(name+'.zipx',[('payload.txt',payload,parameter.to_bytes(2,'little')+normal[2:])],invalid)
 bad=bytearray(normal);bad[0]&=240
 archive('bad-order.zipx',[('payload.txt',payload,bad)],'order')
 archive('bad-crc.zipx',[('payload.txt',payload,normal)],'crc')
 archive('bad-size.zipx',[('payload.txt',payload,normal)],'size')
 archive('truncated.zipx',[('payload.txt',payload,normal[:len(normal)//2])],'truncated')
 archive('short-header.zipx',[('payload.txt',payload,normal[:1])],'truncated')
 archive('encrypted.zipx',[('payload.txt',payload,normal)],'encrypted')
 archive('ratio.zipx',[('ratio.txt',b'x'*262144,compress(b'x'*262144))])
 archive('unsafe.zipx',[('../escape.txt',payload,normal),('inside.txt',payload,normal)])
 archive('symlinks.zipx',[('target.txt',payload,normal),('link-safe',b'target.txt',raw(b'target.txt')),('link-escape',b'../../escape',raw(b'../../escape'))])
(d/'manifest.json').write_text(json.dumps(dict(producer='7-Zip 26.05; Rust helper for empty/known-size/link streams',fixtures=manifest),ensure_ascii=False,indent=2)+'\n')
(q/'oracle.json').write_text(json.dumps(dict(source='https://github.com/ip7z/7zip/releases/download/26.05/7z2605-mac.tar.xz',binary=str(seven),binary_sha256=hashlib.sha256(seven.read_bytes()).hexdigest(),version=subprocess.check_output([str(seven),'i'],text=True).split('Formats:')[0]),indent=2)+'\n')
