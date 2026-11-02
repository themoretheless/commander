from pathlib import Path
import datetime as dt,hashlib,json,math,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
hashfile=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3295 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in read(q/'source-sha256.json').items():assert hashfile(root/p)==h,p
scenes={}
for scene,receipt in [('gpx_preview','native-launch.json'),('kml_preview','kml_preview-launch.json')]:
 launch=read(q/receipt);assert launch['binary_sha256']==launch['core_sha256']==hashfile(root/'target/debug/commander')
 d=Path(launch['out'])/scene;m=read(d/'manifest.json');assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=1,opener=0,trash=0,free_space=0)
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
control=read(scenes['kml_preview']/'route-state.json');assert control['timing']['valid_points']==3 and control['timing']['missing_points']==3 and control['timing']['timed_legs']==2 and control['timing']['seconds']==300
assert abs(control['distance_m']-state['distance_m'])<1e-6
assert [(p['lat'],p['lon'],p['elevation'],p['start']) for p in control['points']]==[(p['lat'],p['lon'],p['elevation'],p['start']) for p in state['points']]
print('PASS: six gates, 3295 tests, exact source/binary hashes, GPX/KML native scenes and independent time/distance/speed calculation')

ns={'k':'http://www.opengis.net/kml/2.2','gx':'http://www.google.com/kml/ext/2.2'}
xml=ET.parse(scenes['kml_preview']/'source.kml');timed_distance=0.;timed_seconds=0.;timed_points=[]
for track in xml.findall('.//gx:Track',ns):
 times=track.findall('k:when',ns);coords=track.findall('gx:coord',ns);assert len(times)==len(coords)
 previous=None
 for when,coord in zip(times,coords):
  values=list(map(float,coord.text.split()));point=(values[1],values[0]);time=dt.datetime.fromisoformat(when.text.replace('Z','+00:00'))
  timed_points.append((point,time))
  if previous:
   elapsed=(time-previous[1]).total_seconds()
   if elapsed>0:timed_seconds+=elapsed;timed_distance+=haversine(previous[0],point)
  previous=(point,time)
assert timed_seconds==300 and abs(control['timing']['distance']-timed_distance)<1e-6
assert abs(control['timing']['average_kmh']-timed_distance/timed_seconds*3.6)<1e-9
assert all(p['time'] is None for p in control['points'][:3])
for captured,(point,time) in zip(control['points'][3:],timed_points):
 assert [captured['lat'],captured['lon']]==list(point) and dt.datetime.fromisoformat(captured['time'])==time
print('PASS: independently aligned KML when/coord arrays, partial coverage and timed speed')

ns={'g':'http://www.topografix.com/GPX/1/1'};xml=ET.parse(scenes['gpx_preview']/'source.gpx')
up=down=0.;covered=eligible=known=0
for segment in xml.findall('./g:trk/g:trkseg',ns):
 elevations=[float(point.findtext('g:ele',namespaces=ns)) if point.find('g:ele',ns) is not None else None for point in segment.findall('g:trkpt',ns)]
 known+=sum(value is not None for value in elevations)
 for a,b in zip(elevations,elevations[1:]):
  eligible+=1
  if a is not None and b is not None:covered+=1;up+=max(b-a,0);down+=max(a-b,0)
expected=dict(known_points=known,eligible_legs=eligible,covered_legs=covered,ascent=up,descent=down)
assert expected==dict(known_points=6,eligible_legs=4,covered_legs=4,ascent=90.,descent=60.)
for scene in scenes:assert read(scenes[scene]/'route-state.json')['elevation_stats']==expected
print('PASS: independently summed elevation gain/loss and exact gap/coverage ownership in both formats')

source_heights=[float(point.findtext('g:ele',namespaces=ns)) for segment in xml.findall('./g:trk/g:trkseg',ns) for point in segment.findall('g:trkpt',ns)]
assert [p['elevation'] for p in read(scenes['gpx_preview']/'route-state.json')['points']]==source_heights
kns={'k':'http://www.opengis.net/kml/2.2','gx':'http://www.google.com/kml/ext/2.2'}
kxml=ET.parse(scenes['kml_preview']/'source.kml');kheights=[]
for element in kxml.findall('.//k:LineString/k:coordinates',kns):kheights.extend(float(tuple.split(',')[2]) for tuple in element.text.split())
for element in kxml.findall('.//gx:Track/gx:coord',kns):kheights.append(float(element.text.split()[2]))
assert kheights==source_heights==[p['elevation'] for p in read(scenes['kml_preview']/'route-state.json')['points']]
print('PASS: recorded per-point elevations match both original XML sources')

for scene,index in [('gpx_preview',1),('kml_preview',4)]:
 d=scenes[scene];snapshot=read(d/'point-inspector.json');route=read(d/'route-state.json');point=route['points'][index]
 assert snapshot['index']==index and len(snapshot['lines'])==6
 segment=sum(p['start'] for p in route['points'][:index+1])
 expected=[f'Point {index+1} / 6 · segment {segment}',f'Latitude: {point["lat"]:.6f}°',f'Longitude: {point["lon"]:.6f}°',f'Recorded distance: {point["distance_m"]:.1f} m',f'Elevation: {point["elevation"]:.1f} m',f'Time: {point["time"]}']
 assert snapshot['lines']==expected
 assert 0<=snapshot['position'][0]<=1280 and 0<=snapshot['position'][1]<=760
print('PASS: actual native tooltip labels match the original point owner, segment, coordinates, elevation, time and distance')

for scene,d in scenes.items():
 report=read(d/'route-report.json');route=read(d/'route-state.json')
 assert report['format']=='commander.route-preview' and report['version']==1
 assert report['source_format']==('GPX' if scene=='gpx_preview' else 'KML')
 assert report['distance_model']=='spherical_haversine' and report['earth_radius_m']==6371008.8
 assert report['gap_policy']=='no_interpolation' and report['elevation_model']=='raw_adjacent_samples'
 assert report['timing']==route['timing'] and report['elevation_stats']==route['elevation_stats']
 assert report['distance_m']==route['distance_m'] and report['segments']==route['segments']
 assert len(report['points'])==len(route['points'])
 segment=0
 for index,(actual,original) in enumerate(zip(report['points'],route['points'])):
  segment+=original['start']
  assert actual==dict(index=index,segment=segment,start=original['start'],lat=original['lat'],lon=original['lon'],elevation_m=original['elevation'],time=original['time'],time_invalid=original['invalid_time'],distance_m=original['distance_m'])
print('PASS: actual clipboard route report preserves all original point owners, segment boundaries, metadata and statistics')
