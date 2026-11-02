from pathlib import Path
import hashlib,json,math,struct,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3240 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
for scene in ['custom_shortcuts','gpx_preview']:
 launch=json.loads((q/('native-launch.json' if scene=='custom_shortcuts' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='custom_shortcuts':
  shown=json.loads((d/'shortcut-state.json').read_text());assert shown['phase']==17 and shown['left_hidden'] and not shown['right_hidden'] and shown['left_sort']=='descending'
  expected=[{'command':'Toggle hidden files','keys':'Alt+G H'},{'command':'Reverse sort order','keys':'Alt+G S'}];assert shown['bindings']==expected
  names=['README.txt','quarterly-design-review-with-a-deliberately-long-filename-for-layout.txt','photo.png','notes.md','.shortcut-hidden.txt'];assert shown['names']==['Design']+sorted(names,key=str.casefold,reverse=True)
  assert Path(shown['left_path']).name=='left' and Path(shown['right_path'])==Path(shown['left_path']).parent/'right'
 else:
  root=ET.fromstring((d/'source.gpx').read_bytes());ns={'g':'http://www.topografix.com/GPX/1/1'};expected=[];total=0.
  for segment in root.findall('g:trk/g:trkseg',ns):
   previous=None
   for node in segment.findall('g:trkpt',ns):
    lat=float(node.attrib['lat']);lon=float(node.attrib['lon']);ele=float(node.find('g:ele',ns).text)
    if previous:
     a,b=previous;u=math.sin(math.radians(lat-a)/2)**2+math.cos(math.radians(a))*math.cos(math.radians(lat))*math.sin(math.radians(lon-b)/2)**2
     total+=2*6371008.8*math.atan2(math.sqrt(u),math.sqrt(1-u))
    expected.append(dict(lat=lat,lon=lon,elevation=ele,start=previous is None,distance_m=total));previous=lat,lon
  shown=json.loads((d/'route-state.json').read_text());assert shown['segments']==2 and len(shown['points'])==len(expected)==6
  for actual,point in zip(shown['points'],expected):
   for k,v in point.items():assert actual[k]==v if isinstance(v,bool) else math.isclose(actual[k],v,rel_tol=0,abs_tol=1e-6)
  assert math.isclose(shown['distance_m'],total,abs_tol=1e-6)
old=repo/'docs/qualification/image-metadata-json-2026-11-02';oldhash=json.loads((old/'source-sha256.json').read_text())
assert oldhash['Cargo.toml']==h(repo/'Cargo.toml') and oldhash['Cargo.lock']==h(repo/'Cargo.lock')
comparison=json.loads((repo/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(repo/'deny.toml')
print('PASS: six gates; 3240 tests; native custom-shortcut Apply/modal-isolation/execution and GPX control each 14/14; independent applied binding, pane visibility and natural-order checks; source/binary/tool hashes. Dependency audit remains open with unchanged graph/policy.')
