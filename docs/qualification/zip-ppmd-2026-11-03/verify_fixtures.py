from pathlib import Path
import hashlib,json,os,struct,subprocess,zipfile
q=Path(__file__).resolve().parent;d=q/'fixtures';m=json.loads((d/'manifest.json').read_text())
seven=Path(os.environ.get('SEVEN_ZIP') or json.loads((q/'oracle.json').read_text())['binary'])
for name,record in m['fixtures'].items():
 blob=(d/name).read_bytes();assert hashlib.sha256(blob).hexdigest()==record['sha256']
 with zipfile.ZipFile(d/name) as z:
  assert len(z.infolist())==len(record['members'])
  for info,expected in zip(z.infolist(),record['members']):
   assert info.filename==expected['path'] and info.compress_type==98
   start=info.header_offset+30+struct.unpack_from('<H',blob,info.header_offset+26)[0]+struct.unpack_from('<H',blob,info.header_offset+28)[0]
   packed=blob[start:start+info.compress_size];kind=record['invalid']
   if kind=='encrypted':assert info.flag_bits&1;continue
   if kind=='restore':assert int.from_bytes(packed[:2],'little')>>12>1;continue
   if kind=='order':assert int.from_bytes(packed[:2],'little')&15==0;continue
   result=subprocess.run([str(seven),'x','-so','-bd','-bsp0',str(d/name),info.filename],capture_output=True,env={**os.environ,'LC_ALL':'C'})
   if kind in ('crc','size','truncated','eos'):assert result.returncode!=0,(name,result.stderr);continue
   assert result.returncode==0,(name,result.stderr)
   assert len(result.stdout)==expected['bytes'] and hashlib.sha256(result.stdout).hexdigest()==expected['sha256']
   if kind=='memory':assert ((int.from_bytes(packed[:2],'little')>>4)&255)+1==65
 print(name,'verified')
print(subprocess.check_output([str(seven),'i'],text=True).split('Formats:')[0])
