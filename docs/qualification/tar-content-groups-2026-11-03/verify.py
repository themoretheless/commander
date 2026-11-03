from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
results={}
for key,scene,effects in [('native-launch.json','archive_tar_content_groups',2),('native-zip-launch.json','archive_content_groups',2)]:
 launch=json.loads((q/key).read_text())
 assert launch['binary_sha256']==launch['core_sha256']
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256'], 'capture binary differs from final build'
 directory=Path(launch['out'])/scene
 manifest=json.loads((directory/'manifest.json').read_text())
 assert manifest['status']=='captured',manifest
 assert manifest['error'] is None and len(manifest['checks'])==14 and all(c['passed'] for c in manifest['checks'])
 assert manifest['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 assert (directory/'frame.png').is_file()
 if scene=='archive_content_groups':
  import zipfile
  payload=(root/'docs/qualification/zip-password-2026-11-03/fixtures/aes-utf8-password.txt').read_bytes()
  expected_hash=hashlib.sha256(payload).hexdigest()
  painted=json.loads((directory/'content-group-rendered.json').read_text())
  assert painted==dict(sha256=expected_hash,paths=['plain.txt','enc-one.txt','enc-two.txt'],visible=True)
  with zipfile.ZipFile(directory/'fixture.zipx') as archive:
   assert archive.namelist()==['plain.txt','enc-one.txt','enc-two.txt','different.txt']
   assert archive.read('plain.txt')==payload and archive.read('different.txt')==b'different contents'
   assert all(archive.getinfo(name).flag_bits & 1 for name in ['enc-one.txt','enc-two.txt'])
  for phase in [7,15,19]:
   state=json.loads((directory/f'content-groups-{phase}.json').read_text())
   assert state['phase']==phase and state['password_empty'] is True and state['source_unchanged'] is True and state['no_extracted_files'] is True
   if phase==7:assert 'password' in state['error'] and state['groups'] is None
   else:
    assert state['error'] is None and len(state['groups'])==1
    group=state['groups'][0]
    assert group['sha256']==expected_hash and group['size']==len(payload)
    assert group['members']==[{'index':i,'path':name,'size':len(payload)} for i,name in enumerate(['plain.txt','enc-one.txt','enc-two.txt'])]
   listing=state['listing']
   assert [(r['index'],r['path']) for r in listing]==[(3,'different.txt'),(1,'enc-one.txt'),(2,'enc-two.txt'),(0,'plain.txt')]
   if phase==19:assert state['selected']==[r['id'] for r in listing if r['index'] in (0,1,2)]==[1,2,3]
   assert 'fixture-пароль-ёж' not in json.dumps(state,ensure_ascii=False) and 'wrong-password' not in json.dumps(state)
 if scene=='archive_content_groups':
  import datetime
  copied=json.loads((directory/'content-group-report.json').read_text())
  assert copied['schema']=='commander.archive-check/v1' and copied['status']=='complete'
  assert copied['content_comparison']=='sha256_and_size' and copied['strict_headers'] is True
  assert copied['members']==4 and copied['decoded_bytes']==3*len(payload)+len(b'different contents')
  assert copied['archive'].endswith('/left/groups.ZIPX') and copied['error'] is None
  datetime.datetime.fromisoformat(copied['recorded_at'])
  assert copied['content_groups']==[{'sha256':expected_hash,'size':len(payload),'members':[{'index':i,'path':name,'size':len(payload)} for i,name in enumerate(['plain.txt','enc-one.txt','enc-two.txt'])]}]
  text=(directory/'content-group-report.txt').read_text()
  assert 'Content comparison: SHA-256 and size\nContent groups: 1\nGroup #0\n' in text
  assert f'SHA-256: "{expected_hash}"' in text
  for i,name in enumerate(['plain.txt','enc-one.txt','enc-two.txt']):assert f'Entry #{i} ({len(payload)} bytes): "{name}"' in text
  for report in [text,json.dumps(copied,ensure_ascii=False)]:assert 'fixture-пароль-ёж' not in report and 'wrong-password' not in report
 if scene=='archive_tar_content_groups':
  import tarfile,collections,gzip
  oracle=json.loads((q/'fixture-oracle.json').read_text())
  entries=oracle['entries']
  assert hashlib.sha256((directory/'fixture.tar.gz').read_bytes()).hexdigest()==oracle['files']['groups.tar.gz']
  with tarfile.open(directory/'fixture.tar.gz') as archive:
   assert len(archive.getmembers())==11
   for member,fact in zip(archive.getmembers(),entries):
    assert member.name==fact['path'] and member.size==fact['size']
    if fact['regular']:assert hashlib.sha256(archive.extractfile(member).read()).hexdigest()==fact['sha256']
  counts=collections.Counter((m['size'],m['sha256']) for m in entries if m['regular'])
  expected=[dict(size=size,sha256=digest,members=[dict(index=m['index'],path=m['path'],size=m['size']) for m in entries if m['regular'] and (m['size'],m['sha256'])==(size,digest)]) for (size,digest),count in sorted(counts.items()) if count>1]
  assert len(expected)==2 and sum(len(g['members']) for g in expected)==7
  indexes=sorted(m['index'] for group in expected for m in group['members'])
  for phase in [3,7]:
   state=json.loads((directory/f'tar-content-groups-{phase}.json').read_text())
   assert state['kind']=='tar.gz' and state['strict_headers'] is False and state['members']==11
   assert state['decoded']==len(gzip.decompress((directory/'fixture.tar.gz').read_bytes()))
   assert state['source_unchanged'] is True and state['no_extracted_files'] is True and state['groups']==expected
   assert {r['index'] for r in state['listing']}==set(range(11))
   for row in state['listing']:
    fact=entries[row['index']]
    assert row['path'].rstrip('/')==fact['path'].rstrip('/') and row['size']==fact['size'] and row['regular']==fact['regular']
   if phase==7:assert state['selected_indexes']==indexes and state['selected']==sorted(r['id'] for r in state['listing'] if r['index'] in indexes)
  painted=json.loads((directory/'tar-group-rendered.json').read_text())
  assert painted==dict(sha256=hashlib.sha256(b'').hexdigest(),paths=['empty-one','empty-two'],visible=True)
  copied=json.loads((directory/'tar-content-report.json').read_text())
  assert copied['schema']=='commander.archive-check/v1' and copied['status']=='complete' and copied['archive_format']=='tar.gz' and copied['strict_headers'] is False
  assert copied['content_comparison']=='sha256_and_size' and copied['content_groups']==expected
  text=(directory/'tar-content-report.txt').read_text()
  assert 'Content groups: 2' in text
  for group in expected:
   assert f'SHA-256: "{group["sha256"]}"' in text
   for m in group['members']:assert f'Entry #{m["index"]} ({m["size"]} bytes): '+json.dumps(m['path'],ensure_ascii=False) in text
 results[scene]=dict(checks=len(manifest['checks']),effects=manifest['native_effect_calls'],frame_sha256=hashlib.sha256((directory/'frame.png').read_bytes()).hexdigest(),capture=str(directory))
(q/'verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
