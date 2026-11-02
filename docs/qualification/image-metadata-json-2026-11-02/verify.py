from pathlib import Path
import hashlib,json,struct,re,tomllib
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3228 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
import zlib

def tiff_fields(tiff,pos):
 def uint(p,n):assert 0<=p<=len(tiff)-n;return int.from_bytes(tiff[p:p+n],'little')
 count=uint(pos,2);assert count<256;fields={}
 for i in range(count):
  p=pos+2+12*i;tag=uint(p,2);kind=uint(p+2,2);count=uint(p+4,4);width={2:1,3:2,4:4,5:8}[kind];size=width*count;start=p+8 if size<=4 else uint(p+8,4)
  assert start+size<=len(tiff);raw=tiff[start:start+size]
  if kind==2:value=raw.rstrip(b'\0').decode('ascii')
  elif kind in (3,4):value=[int.from_bytes(raw[x:x+width],'little') for x in range(0,size,width)]
  else:value=[struct.unpack('<II',raw[x:x+8]) for x in range(0,size,8)]
  fields[tag]=value
 return fields

def contents(data,format):
 tiff=None;dims=None
 if format=='jpeg':
  assert data[:2]==b'\xff\xd8';p=2
  while p<len(data):
   assert data[p]==255;marker=data[p+1];p+=2
   if marker in (0xd9,0xda):break
   size=int.from_bytes(data[p:p+2],'big');assert size>=2 and p+size<=len(data);payload=data[p+2:p+size];p+=size
   if marker==0xe1 and payload.startswith(b'Exif\0\0'):tiff=payload[6:]
   if marker in (0xc0,0xc2):dims=struct.unpack('>HH',payload[1:5])[::-1]
 elif format=='png':
  assert data[:8]==b'\x89PNG\r\n\x1a\n';p=8;ended=False
  while p<len(data):
   size=int.from_bytes(data[p:p+4],'big');tag=data[p+4:p+8];payload=data[p+8:p+8+size];assert p+12+size<=len(data)
   assert zlib.crc32(tag+payload)&0xffffffff==int.from_bytes(data[p+8+size:p+12+size],'big');p+=12+size
   if tag==b'IHDR':dims=struct.unpack('>II',payload[:8])
   if tag==b'eXIf':tiff=payload
   if tag==b'IEND':ended=True;break
  assert ended and p==len(data)
 elif format=='tiff':
  tiff=data;assert tiff[:4]==b'II\x2a\0';root=tiff_fields(tiff,int.from_bytes(tiff[4:8],'little'));dims=(root[256][0],root[257][0]);assert root[279]==[36]
 elif format=='webp':
  assert data[:4]==b'RIFF' and data[8:12]==b'WEBP' and int.from_bytes(data[4:8],'little')+8==len(data);p=12
  while p<len(data):
   tag=data[p:p+4];size=int.from_bytes(data[p+4:p+8],'little');payload=data[p+8:p+8+size];assert p+8+size<=len(data);p+=8+size+(size&1)
   if tag==b'VP8X':dims=(int.from_bytes(payload[4:7],'little')+1,int.from_bytes(payload[7:10],'little')+1)
   if tag==b'VP8L' and dims is None:
    assert payload[0]==0x2f;bits=int.from_bytes(payload[1:5],'little');dims=((bits&0x3fff)+1,((bits>>14)&0x3fff)+1)
   if tag==b'EXIF':tiff=payload[6:] if payload.startswith(b'Exif\0\0') else payload
  assert p==len(data)
 else:raise AssertionError(format)
 assert dims==(4,3)
 return dims,tiff

facts=[];payload_hashes=[]
for scene,format,has_exif in [('image_metadata_json','jpeg',True),('image_header_info','png',False)]:
 launch=json.loads((q/('native-launch.json' if scene=='image_metadata_json' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks']) and m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=int(scene=='image_metadata_json'),opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 data=(d/('source.jpg' if format=='jpeg' else 'source.'+format)).read_bytes();dims,tiff=contents(data,format)
 expected={'Format':{'jpeg':'JPEG','png':'PNG','tiff':'TIFF','webp':'WebP'}[format],'Header dimensions':f'{dims[0]} × {dims[1]} pixels'}
 if has_exif:
  assert tiff[:4]==b'II\x2a\0';root=tiff_fields(tiff,int.from_bytes(tiff[4:8],'little'));exif=tiff_fields(tiff,root[34665][0]);gps=tiff_fields(tiff,root[34853][0]);assert root[274]==[6]
  ratio=lambda value:value[0][0]/value[0][1]
  dms=lambda value:' '.join(f'{n/v:g} {unit}' for (n,v),unit in zip(value,['deg','min','sec']))
  expected.update({'Camera maker':json.dumps(root[271]),'Camera model':json.dumps(root[272]),'Lens':json.dumps(exif[42036]),'Taken':exif[36867].replace(':','-',2),'Exposure':f'{exif[33434][0][0]}/{exif[33434][0][1]} s','Aperture':f'f/{ratio(exif[33437]):g}','ISO':str(exif[34855][0]),'Focal length':f'{ratio(exif[37386]):g} mm','Orientation':'row 0 at right and column 0 at top','Pixel width':f'{exif[40962][0]} pixels','Pixel height':f'{exif[40963][0]} pixels','GPS latitude':dms(gps[2])+' '+gps[1],'GPS longitude':dms(gps[4])+' '+gps[3],'GPS altitude':f'{ratio(gps[6]):g} meters above sea level'})
  assert exif[34850]==[3] and exif[37383]==[5]
  expected.update({'Artist':json.dumps(root[315]),'Copyright':json.dumps(root[33432]),'Software':json.dumps(root[305]),'Taken UTC offset':json.dumps(exif[36881]),'Taken subsecond digits':json.dumps(exif[37521]),'Exposure program':'aperture priority','Metering mode':'pattern'})
  assert exif[36881]=='+05:30' and exif[37521]=='250' and expected['Taken']=='2026-11-02 12:34:56'
  payload_hashes.append(hashlib.sha256(tiff).hexdigest())
 else:assert tiff is None
 shown=json.loads((d/'exif-state.json').read_text());assert shown['container']==format and shown['has_exif']==has_exif and dict(shown['rows'])==expected
 assert shown['note'] is None if has_exif else shown['note']=='No EXIF metadata found'
 assert Path(shown['left_path']).name=='left' and Path(shown['right_path'])==Path(shown['left_path']).parent/'right'
 if scene=='image_metadata_json':
  raw=(d/'metadata.json').read_bytes();assert len(raw)<=65536
  report=json.loads(raw);assert report['format']=='commander.image-metadata' and report['version']==1
  assert report['snapshot']=='retained inspector display values' and report['read_limit_bytes']==2097152
  assert {p['name']:p['display_value'] for p in report['properties']}==expected and len(report['properties'])==23 and report['note']==shown['note']
  source=report['source'];assert source['path']=={'encoding':'utf8','text':str(Path(shown['left_path'])/'photo-exif.jpg')}
  kernel=json.loads((d/'source-facts.json').read_text());assert all(source[k]==v for k,v in kernel.items()) and source['size_bytes']==len(data)
 else:
  visibility=json.loads((d/'exif-visibility.json').read_text());assert visibility['target']=='Header dimensions';clip=visibility['clip'];row=visibility['row']
  assert clip['min_x']<=row['min_x']<row['max_x']<=clip['max_x'] and clip['min_y']<=row['min_y']<row['max_y']<=clip['max_y']
 facts.append({'scenario':scene,'source_sha256':hashlib.sha256(data).hexdigest(),'properties':expected})
assert len(payload_hashes)==1 and len(facts)==2
(q/'independent-container-facts.json').write_text(json.dumps(facts,indent=2)+'\n')
# Dependencies were not changed. Preserve, rather than waive, the global audit failures.
previous=repo/'docs/qualification/image-exif-info-2026-11-02'
comparison=json.loads((previous/'dependency-comparison.json').read_text());assert comparison['cargo_lock_sha256']==h(repo/'Cargo.lock') and comparison['deny_policy_sha256']==h(repo/'deny.toml')
def signatures(s):return {'advisories':sorted(set(re.findall(r'ID: (RUSTSEC-\d{4}-\d+)',s))),'errors':sorted(re.findall(r'^error\[[^\]]+\]:.*$',s,re.M)),'licenses':sorted(set(re.findall(r'license = "([^"]+)"',s)))}
assert signatures((q/'dependencies.log').read_text())==comparison['diagnostics']
print('PASS: six gates; 3228 tests; JSON copy and header native scenes each 14/14; actual clipboard snapshot matches independent TIFF properties and kernel identity; source/binary/tool hashes. Existing dependency audit remains failing.')
