from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
results={}
for key,scene,effects in [('native-launch.json','archive_metadata',0),('native-search-launch.json','search_password',0),('native-json-launch.json','search_json',5)]:
 launch=json.loads((q/key).read_text())
 assert launch['binary_sha256']==launch['core_sha256']
 assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256'], 'capture binary differs from final build'
 directory=Path(launch['out'])/scene
 m=json.loads((directory/'manifest.json').read_text())
 assert m['status']=='captured',m
 assert m['error'] is None and len(m['checks'])==(16 if scene in ('search_password','search_json') else 14) and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=effects,opener=0,trash=0,free_space=0)
 assert (directory/'frame.png').is_file()
 if scene=='archive_metadata':
  for name,method,encrypted in [('a-stored.txt','Stored','No'),('b-deflated.txt','Deflated','No'),('c-encrypted.txt','Unsupported(222)','Yes (ZipCrypto, password required)'),('d-bzip2.txt','Bzip2','No'),('e-deflate64.txt','Deflate64','No'),('f-lzma.txt','Lzma','No'),('g-xz.txt','Xz','No'),('h-zstd93.txt','Zstd','No'),('i-zstd20.txt','Zstd (legacy method 20)','No'),('j-ppmd.txt','Ppmd','No'),('k-aes.txt','Stored','Yes (WinZip AES-256 (AE-2), password required)')]:
   path,rows=json.loads((directory/f'metadata-{name}.json').read_text())
   assert path==name and len(rows)==8 and all(row[2] is True for row in rows)
   fields={label:value for label,value,_ in rows}
   assert fields['Compression method']==method and fields['Encrypted']==encrypted
   assert fields['Kind']=='Regular file'
   if name != 'k-aes.txt':
    assert fields['Kind']=='Regular file' and fields['Declared size']==('301 bytes' if name=='e-deflate64.txt' else ('30 bytes' if name=='f-lzma.txt' else ('28 bytes' if name=='g-xz.txt' else ('30 bytes' if name in ('h-zstd93.txt','i-zstd20.txt') else ('4352 bytes' if name=='j-ppmd.txt' else '24 bytes')))))
 if scene=='search_password':
  for phase in [4,15,26,30,41]:
   state=json.loads((directory/f'password-search-{phase}.json').read_text())
   success=phase in (26,41)
   assert state['phase']==phase and state['password_empty'] is True and state['password_enabled'] is False
   assert state['expression']=='path:payload.txt content:needle archives:on'
   assert state['matched']==int(success) and state['skipped']==int(not success)
   assert len(state['paths'])==int(success)
   if success:assert state['paths'][0].endswith('/left/secret.zipx!/payload.txt')
   assert 'fixture-пароль-ёж' not in json.dumps(state,ensure_ascii=False) and 'wrong-password' not in json.dumps(state)
 if scene=='search_json':
  for name,count in [('all',5),('selected',2),('visible',4),('draft',4),('empty',0)]:
   report=json.loads((directory/f'report-{name}.json').read_text())
   assert report['format']=='commander.search-report' and report['version']==1
   assert len(report['rows'])==count and report['partial'] is False
   assert report['context']['draft_differs'] is (name=='draft')
   assert 'archive_password' not in json.dumps(report)
 results[scene]=dict(checks=len(m['checks']),effects=m['native_effect_calls'],frame_sha256=hashlib.sha256((directory/'frame.png').read_bytes()).hexdigest(),capture=str(directory))
(q/'verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
