from pathlib import Path
import datetime as dt,hashlib,json,math,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
hashfile=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3287 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in read(q/'source-sha256.json').items():assert hashfile(root/p)==h,p
scenes={}
for scene,receipt in [('gpx_preview','native-launch.json'),('kml_preview','kml_preview-launch.json')]:
 launch=read(q/receipt);assert launch['binary_sha256']==launch['core_sha256']==hashfile(root/'target/debug/commander')
 d=Path(launch['out'])/scene;m=read(d/'manifest.json');assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 assert (d/'frame.png').stat().st_size>10000;scenes[scene]=d
p=scenes['gpx_preview'];state=read(p/'route-state.json');doc=ET.parse(p/'source.gpx');ns={'g':'http://www.topografix.com/GPX/1/1'}
points=[];seconds=0.;distance=0.;legs=0
haversine=lambda a,b:2*6371008.8*math.asin(math.sqrt(min(1,math.sin(math.radians(b[0]-a[0])/2)**2+math.cos(math.radians(a[0]))*math.cos(math.radians(b[0]))*math.sin(math.radians(b[1]-a[1])/2)**2)))
for segment in doc.findall('./g:trk/g:trkseg',ns):
 previous=None
 for element in segment.findall('g:trkpt',ns):
  point=(float(element.attrib['lat']),float(element.attrib['lon']))
  time=dt.datetime.fromisoformat(element.findtext('g:time',namespaces=ns).replace('Z','+00:00'))
  points.append((point,time))
  if previous is not None:
   elapsed=(time-previous[1]).total_seconds()
   if elapsed>0:seconds+=elapsed;distance+=haversine(previous[0],point);legs+=1
  previous=(point,time)
timing=state['timing'];assert timing['valid_points']==6 and timing['timed_legs']==timing['eligible_legs']==legs==4
assert timing['seconds']==seconds==480 and (points[-1][1]-points[0][1]).total_seconds()==3900
assert abs(timing['distance']-distance)<1e-6 and abs(timing['average_kmh']-distance/seconds*3.6)<1e-9
for captured,(point,time) in zip(state['points'],points):
 assert [captured['lat'],captured['lon']]==list(point)
 assert dt.datetime.fromisoformat(captured['time'])==time and not captured['invalid_time']
control=read(scenes['kml_preview']/'route-state.json');assert control['timing']['valid_points']==0 and control['timing']['average_kmh'] is None
assert abs(control['distance_m']-state['distance_m'])<1e-6
assert [(p['lat'],p['lon'],p['elevation'],p['start']) for p in control['points']]==[(p['lat'],p['lon'],p['elevation'],p['start']) for p in state['points']]
print('PASS: six gates, 3287 tests, exact source/binary hashes, GPX/KML native scenes and independent time/distance/speed calculation')
