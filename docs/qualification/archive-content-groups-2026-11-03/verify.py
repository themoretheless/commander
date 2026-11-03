from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
results={}
for key,scene,effects in [('native-launch.json','archive_content_groups',0),('native-password-launch.json','archive_password',4)]:
 launch=json.loads((q/key).read_text())
 assert launch['binary_sha256']==launch['core_sha256']
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256'], 'capture binary differs from final build'
 directory=Path(launch['out'])/scene
 m=json.loads((directory/'manifest.json').read_text())
 assert m['status']=='captured',m
 assert m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 assert (directory/'frame.png').is_file()
 if scene=='archive_password':
  for phase in [4,12,20,28,45]:
   state=json.loads((directory/f'password-input-{phase}.json').read_text())
   expected='wrong-password' if phase in (4,28) else 'fixture-пароль-ёж'
   assert state['characters']==len(expected) and state['bytes']==len(expected.encode())
  for phase in [7,15,23]:
   state=json.loads((directory/f'password-result-{phase}.json').read_text())
   assert state['password_empty'] is True
   if phase==7:
    assert state['output_exists'] is False and state['output_matches'] is False and 'password' in state['error']
   else:
    assert state['output_exists'] is True and state['output_matches'] is True and state['error'] is None
    assert ('Extracted 1' if phase==15 else 'skipped existing 1') in state['status']
 if scene=='archive_password':
  for phase in [31,48]:
   state=json.loads((directory/f'password-check-{phase}.json').read_text())
   assert state['password_empty'] is True and state['output_unchanged'] is True
   if phase==31:
    assert 'password' in state['result']['Err']
   else:
    report=state['result']['Ok']
    assert report['members']==1 and report['decoded']==4352
    assert 'AES authentication when present' in report['summary']
    assert report['strict_headers'] is True and 'strict ZIP headers' in report['summary']
 if scene=='archive_password':
  import datetime
  for index in [1,2,3,4]:
   copied=(directory/f'check-report-{index}.txt').read_text()
   assert 'fixture-пароль-ёж' not in copied and 'wrong-password' not in copied
   if index in [2,4]:
    report=json.loads(copied)
    assert report['schema']=='commander.archive-check/v1'
    assert report['status']==('incomplete' if index==2 else 'complete')
    assert report['strict_headers'] is (index==4)
    assert report['archive'].endswith('/left/password.ZIPX')
    datetime.datetime.fromisoformat(report['recorded_at'])
    if index==2:assert report['members'] is None and report['decoded_bytes'] is None and 'password' in report['error']
    else:assert report['members']==1 and report['decoded_bytes']==4352 and report['error'] is None
   else:
    assert ('Status: incomplete\nStrict ZIP headers: false' if index==1 else 'Status: complete\nStrict ZIP headers: true') in copied
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
 results[scene]=dict(checks=len(m['checks']),effects=m['native_effect_calls'],frame_sha256=hashlib.sha256((directory/'frame.png').read_bytes()).hexdigest(),capture=str(directory))
(q/'verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
