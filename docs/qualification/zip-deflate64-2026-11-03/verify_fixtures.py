from pathlib import Path
import hashlib,json,struct,subprocess,zipfile
q=Path(__file__).resolve().parent/'fixtures'
m=json.loads((q/'manifest.json').read_text())
for name,record in m['fixtures'].items():
 p=q/name
 assert hashlib.sha256(p.read_bytes()).hexdigest()==record['sha256']
 with zipfile.ZipFile(p) as z:
  entries=z.infolist();assert len(entries)==len(record['members'])
  for info,expected in zip(entries,record['members']):
   assert info.filename==expected['path'] and info.compress_type==9 and info.file_size==expected['bytes']
   result=subprocess.run(['/usr/bin/unzip','-p',str(p),info.filename],capture_output=True)
   if record['invalid']:assert result.returncode!=0
   else:
    assert result.returncode==0
    assert len(result.stdout)==expected['bytes'] and hashlib.sha256(result.stdout).hexdigest()==expected['sha256']
 print(name,'verified by Info-ZIP')
# The ordinary Deflate decoder must not be the oracle for these extensions.
assert (q/'expected-extended.txt').read_bytes()==b'A'*301
assert len((q/'expected-distance.txt').read_bytes())==65539
print(subprocess.check_output(['/usr/bin/unzip','-v'],text=True).split('UnZip and ZipInfo environment options:')[0])
