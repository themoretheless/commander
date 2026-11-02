from pathlib import Path
import hashlib,json,struct,subprocess,zlib
q=Path(__file__).resolve().parent;d=q/'fixtures';d.mkdir(exist_ok=True)
class Bits:
 def __init__(self): self.bits=[]
 def integer(self,value,width): self.bits.extend((value>>i)&1 for i in range(width))
 def huffman(self,value,width): self.bits.extend((value>>i)&1 for i in reversed(range(width)))
 def symbol(self,value):
  if value<144: self.huffman(48+value,8)
  elif value<256: self.huffman(400+value-144,9)
  elif value<280: self.huffman(value-256,7)
  else: self.huffman(192+value-280,8)
 def finish(self):
  self.symbol(256)
  return bytes(sum(bit<<j for j,bit in enumerate(self.bits[i:i+8])) for i in range(0,len(self.bits),8))
def stream(prefix,length=0,distance=0):
 b=Bits();b.integer(1,1);b.integer(1,2) # final fixed-Huffman block
 for byte in prefix:b.symbol(byte)
 if length:
  b.symbol(285);b.integer(length-3,16) # actual Deflate64 length extension
  if distance==1:b.huffman(0,5)
  elif distance==65536:b.huffman(31,5);b.integer(16383,14)
  else:raise ValueError(distance)
 return b.finish()
manifest={}
def archive(name,rows,invalid=False):
 body=bytearray();central=bytearray();records=[]
 for path,packed,plain in rows:
  encoded=path.encode();crc=zlib.crc32(plain)^(1 if invalid=='crc' else 0);offset=len(body)
  body+=struct.pack('<IHHHHHIIIHH',0x04034b50,21,0x800,9,0,0,crc,len(packed),len(plain),len(encoded),0)+encoded+packed
  central+=struct.pack('<IHHHHHHIIIHHHHHII',0x02014b50,0x315,21,0x800,9,0,0,crc,len(packed),len(plain),len(encoded),0,0,0,0,0o100644<<16,offset)+encoded
  records.append(dict(path=path,bytes=len(plain),sha256=hashlib.sha256(plain).hexdigest(),method=9))
 body+=central+struct.pack('<IHHHHIIH',0x06054b50,0,0,len(rows),len(rows),len(central),len(body),0)
 p=d/name;p.write_bytes(body);manifest[name]=dict(sha256=hashlib.sha256(body).hexdigest(),members=records,invalid=invalid)
 # Independent, already-installed decoder. Never extract to the filesystem.
 for path,packed,plain in rows:
  result=subprocess.run(['/usr/bin/unzip','-p',str(p),path],capture_output=True)
  if invalid:assert result.returncode!=0,(name,result.stderr)
  else:assert result.returncode==0 and result.stdout==plain,(name,result.returncode,result.stderr)
small=b'A'*301
prefix=(b'Deflate64 needle: '+b''.join(hashlib.sha256(str(i).encode()).hexdigest().encode() for i in range(1024)))[:65536]
long=prefix+prefix[:3]
(d/'expected-extended.txt').write_bytes(small);(d/'expected-distance.txt').write_bytes(long)
archive('extended.zipx',[('extended.txt',stream(b'A',300,1),small),('nested/ёж.txt',stream(prefix,3,65536),long),('empty.txt',stream(b''),b'')])
archive('bad-crc.zipx',[('bad.txt',stream(b'A',300,1),small)],'crc')
archive('truncated.zipx',[('bad.txt',stream(b'A',300,1)[:2],small)],'truncated')
archive('ratio.zipx',[('ratio.txt',stream(b'A',65538,1),b'A'*65539)])
(d/'manifest.json').write_text(json.dumps(dict(producer='Independent fixed-Huffman Deflate64 fixture encoder',oracle='/usr/bin/unzip -p (Info-ZIP USE_DEFLATE64)',fixtures=manifest),ensure_ascii=False,indent=2)+'\n')
print('All fixture bytes independently verified with Info-ZIP')
