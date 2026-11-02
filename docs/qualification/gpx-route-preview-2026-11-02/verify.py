from pathlib import Path
import hashlib,json,math,struct,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;repo=q.parents[2]
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for f,digest in json.loads((q/'source-sha256.json').read_text()).items():assert h(repo/f)==digest,f
checks=json.loads((q/'checks.json').read_text());assert [c['name'] for c in checks]==['build','tests','clippy','check','fmt','diff'] and all(c['exit_code']==0 for c in checks)
assert '3232 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert h(Path(tool['path']))==tool['sha256']
binary=h(repo/'target/debug/commander');assert h(repo/'target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')==binary
facts=[]
for scene in ['gpx_preview','image_header_info']:
 launch=json.loads((q/('native-launch.json' if scene=='gpx_preview' else scene+'-launch.json')).read_text());assert launch['scenario']==scene and launch['binary_sha256']==launch['core_sha256']==binary
 d=Path(launch['out'])/scene;m=json.loads((d/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
 frame=(d/'frame.png').read_bytes();assert frame[:8]==b'\x89PNG\r\n\x1a\n' and list(struct.unpack('>II',frame[16:24]))==m['image_size']
 if scene=='gpx_preview':
  source=(d/'source.gpx').read_bytes();root=ET.fromstring(source);ns={'g':'http://www.topografix.com/GPX/1/1'}
  segments=root.findall('g:trk/g:trkseg',ns);assert len(segments)==2
  expected=[];total=0.
  for segment in segments:
   previous=None
   for node in segment.findall('g:trkpt',ns):
    lat=float(node.attrib['lat']);lon=float(node.attrib['lon']);ele=float(node.find('g:ele',ns).text)
    if previous:
     a,b=previous;u=math.sin(math.radians(lat-a)/2)**2+math.cos(math.radians(a))*math.cos(math.radians(lat))*math.sin(math.radians(lon-b)/2)**2
     total+=2*6371008.8*math.atan2(math.sqrt(u),math.sqrt(1-u))
    expected.append(dict(lat=lat,lon=lon,elevation=ele,start=previous is None,distance_m=total));previous=lat,lon
  shown=json.loads((d/'route-state.json').read_text());assert shown['segments']==2 and len(shown['points'])==len(expected)==6
  for actual,point in zip(shown['points'],expected):
   for k,v in point.items():assert actual[k]==v if isinstance(v,bool) else math.isclose(actual[k],v,rel_tol=0,abs_tol=1e-6),(k,actual,point)
  assert math.isclose(shown['distance_m'],total,abs_tol=1e-6) and round(total/1000,2)==5.55
  assert min(p['elevation'] for p in expected)==420 and max(p['elevation'] for p in expected)==510
  facts.append(dict(scenario=scene,source_sha256=hashlib.sha256(source).hexdigest(),points=expected,distance_m=total))
 else:
  shown=json.loads((d/'exif-state.json').read_text());assert dict(shown['rows'])=={'Format':'PNG','Header dimensions':'4 × 3 pixels'} and shown['note']=='No EXIF metadata found'
  data=(d/'source.png').read_bytes();assert data[:8]==b'\x89PNG\r\n\x1a\n' and struct.unpack('>II',data[16:24])==(4,3)
  facts.append(dict(scenario=scene,source_sha256=hashlib.sha256(data).hexdigest()))
# No dependency changes: refer to the preceding run rather than claim a new audit.
old=repo/'docs/qualification/image-metadata-json-2026-11-02';oldhash=json.loads((old/'source-sha256.json').read_text())
assert oldhash['Cargo.toml']==h(repo/'Cargo.toml') and oldhash['Cargo.lock']==h(repo/'Cargo.lock')
comparison=json.loads((repo/'docs/qualification/image-exif-info-2026-11-02/dependency-comparison.json').read_text());assert comparison['deny_policy_sha256']==h(repo/'deny.toml')
(q/'independent-route-facts.json').write_text(json.dumps(facts,indent=2)+'\n')
print('PASS: six gates; 3232 tests; GPX and header control each 14/14; independently parsed GPX coordinates, elevation, segment boundaries and spherical distances; source/binary/tool hashes. Dependency audit remains open with unchanged graph/policy.')
