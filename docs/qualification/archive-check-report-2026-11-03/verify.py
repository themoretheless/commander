from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
results={}
for key,scene,effects in [('native-launch.json','archive_metadata',0),('native-password-launch.json','archive_password',4)]:
 launch=json.loads((q/key).read_text())
 assert launch['binary_sha256']==launch['core_sha256']
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256'], 'capture binary differs from final build'
 directory=Path(launch['out'])/scene
 m=json.loads((directory/'manifest.json').read_text())
 assert m['status']=='captured',m
 assert m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 assert (directory/'frame.png').is_file()
 if scene=='archive_metadata':
  for name,method,encrypted in [('a-stored.txt','Stored','No'),('b-deflated.txt','Deflated','No'),('c-encrypted.txt','Unsupported(222)','Yes (password required)'),('d-bzip2.txt','Bzip2','No'),('e-deflate64.txt','Deflate64','No'),('f-lzma.txt','Lzma','No'),('g-xz.txt','Xz','No'),('h-zstd93.txt','Zstd','No'),('i-zstd20.txt','Zstd (legacy method 20)','No'),('j-ppmd.txt','Ppmd','No')]:
   path,rows=json.loads((directory/f'metadata-{name}.json').read_text())
   assert path==name and len(rows)==8 and all(row[2] is True for row in rows)
   fields={label:value for label,value,_ in rows}
   assert fields['Compression method']==method and fields['Encrypted']==encrypted
   assert fields['Kind']=='Regular file' and fields['Declared size']==('301 bytes' if name=='e-deflate64.txt' else ('30 bytes' if name=='f-lzma.txt' else ('28 bytes' if name=='g-xz.txt' else ('30 bytes' if name in ('h-zstd93.txt','i-zstd20.txt') else ('4352 bytes' if name=='j-ppmd.txt' else '24 bytes')))))
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
 results[scene]=dict(checks=len(m['checks']),effects=m['native_effect_calls'],frame_sha256=hashlib.sha256((directory/'frame.png').read_bytes()).hexdigest(),capture=str(directory))
(q/'verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
