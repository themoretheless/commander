from pathlib import Path
import hashlib,json,struct,re,tomllib
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3221 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['exif_info','file_identity_info']:
 launch=json.loads((q/('native-launch.json' if scene=='exif_info' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='file_identity_info':
  shown=json.loads((d/'info-state.json').read_text());facts=json.loads((d/'identity-facts.json').read_text());fact=next(f for f in facts if f['name']=='alias.bin')
  assert shown['filesystem_id']==f"Device {fact['device_id']}, inode {fact['file_id']}"
  assert int(shown['hard_links'])==fact['hard_links']==2 and int(shown['owner_uid'])==fact['owner_uid'] and int(shown['owner_gid'])==fact['owner_gid']
  continue
 # Independent JPEG marker and TIFF IFD decoder, without Rust/exif/image libraries.
 data=(d/'source.jpg').read_bytes();assert data[:2]==b'\xff\xd8';offset=2;tiff=None
 while offset<len(data):
  assert data[offset]==255;marker=data[offset+1];offset+=2
  if marker in (0xd9,0xda):break
  size=int.from_bytes(data[offset:offset+2],'big');assert size>=2 and offset+size<=len(data)
  payload=data[offset+2:offset+size];offset+=size
  if marker==0xe1 and payload.startswith(b'Exif\0\0'):tiff=payload[6:];break
 assert tiff is not None and tiff[:4]==b'II\x2a\0'
 def uint(pos,n):assert 0<=pos<=len(tiff)-n;return int.from_bytes(tiff[pos:pos+n],'little')
 def ifd(pos):
  count=uint(pos,2);assert count<256;fields={}
  for i in range(count):
   p=pos+2+12*i;tag=uint(p,2);kind=uint(p+2,2);count=uint(p+4,4);size={2:1,3:2,4:4,5:8}[kind]*count;start=p+8 if size<=4 else uint(p+8,4)
   assert start+size<=len(tiff);raw=tiff[start:start+size]
   if kind==2:value=raw.rstrip(b'\0').decode('ascii')
   elif kind in (3,4):value=[int.from_bytes(raw[x:x+size//count],'little') for x in range(0,size,size//count)]
   else:value=[struct.unpack('<II',raw[x:x+8]) for x in range(0,size,8)]
   fields[tag]=value
  return fields
 root=ifd(uint(4,4));exif=ifd(root[34665][0]);gps=ifd(root[34853][0]);ratio=lambda value:value[0][0]/value[0][1]
 dms=lambda value:' '.join(f'{n/v:g} {unit}' for (n,v),unit in zip(value,['deg','min','sec']))
 assert root[274]==[6]
 expected={'Camera maker':json.dumps(root[271]),'Camera model':json.dumps(root[272]),'Lens':json.dumps(exif[42036]),'Taken':exif[36867].replace(':','-',2),'Exposure':f'{exif[33434][0][0]}/{exif[33434][0][1]} s','Aperture':f'f/{ratio(exif[33437]):g}','ISO':str(exif[34855][0]),'Focal length':f'{ratio(exif[37386]):g} mm','Orientation':'row 0 at right and column 0 at top','Pixel width':f'{exif[40962][0]} pixels','Pixel height':f'{exif[40963][0]} pixels','GPS latitude':dms(gps[2])+' '+gps[1],'GPS longitude':dms(gps[4])+' '+gps[3],'GPS altitude':f'{ratio(gps[6]):g} meters above sea level'}
 shown=json.loads((d/'exif-state.json').read_text());assert dict(shown['rows'])==expected
 assert Path(shown['left_path']).name=='left' and Path(shown['right_path'])==Path(shown['left_path']).parent/'right'
 visibility=json.loads((d/'exif-visibility.json').read_text());clip=visibility['clip'];row=visibility['gps_altitude']
 assert clip['min_x']<=row['min_x']<row['max_x']<=clip['max_x'] and clip['min_y']<=row['min_y']<row['max_y']<=clip['max_y']
 (q/'independent-exif-facts.json').write_text(json.dumps(expected,indent=2)+'\n')
# Audit failures remain failures: prove that removing only the added EXIF packages
# reproduces the same diagnostic set, rather than changing policy or hiding errors.
comparison=json.loads((q/'dependency-comparison.json').read_text());assert comparison['cargo_lock_sha256']==h(repo/'Cargo.lock')
def signatures(s):return {'advisories':sorted(set(re.findall(r'ID: (RUSTSEC-\d{4}-\d+)',s))),'errors':sorted(re.findall(r'^error\[[^\]]+\]:.*$',s,re.M)),'licenses':sorted(set(re.findall(r'license = "([^"]+)"',s)))}
assert signatures((q/'dependencies.log').read_text())==signatures((q/'dependencies-baseline.log').read_text())==comparison['diagnostics']
assert comparison['current_exit_code']==comparison['baseline_without_exif_exit_code']==5
packages=lambda p:{(x['name'],x['version']) for x in tomllib.loads(p.read_text())['package']}
assert packages(repo/'Cargo.lock')-packages(q/'dependency-baseline/Cargo.lock')=={('kamadak-exif','0.6.1'),('mutate_once','0.1.2')}
assert comparison['deny_policy_sha256']==h(repo/'deny.toml')==h(q/'dependency-baseline/deny.toml')
audit=json.loads((q/'audit-tool.json').read_text());assert h(Path(audit['path']))==audit['sha256']
assert not packages(q/'dependency-baseline/Cargo.lock')-packages(repo/'Cargo.lock')
assert (q/'dependency-baseline/Cargo.toml').read_text()==(repo/'Cargo.toml').read_text().replace('kamadak-exif = "=0.6.1"\n','')+'\n[workspace]\n'
print('PASS feature: six gates; 3221 tests; both native workflows 14/14; independent JPEG/TIFF facts and visible GPS; source/binary/tool hashes. Global dependency audit remains FAILED with the same diagnostics as the baseline without EXIF.')
