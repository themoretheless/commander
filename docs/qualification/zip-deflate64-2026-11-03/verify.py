from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
results={}
for key,scene,effects in [('native-launch.json','archive_metadata',0)]:
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
  for name,method,encrypted in [('a-stored.txt','Stored','No'),('b-deflated.txt','Deflated','No'),('c-encrypted.txt','Unsupported(222)','Yes (password extraction unavailable)'),('d-bzip2.txt','Bzip2','No'),('e-deflate64.txt','Deflate64','No')]:
   path,rows=json.loads((directory/f'metadata-{name}.json').read_text())
   assert path==name and len(rows)==8 and all(row[2] is True for row in rows)
   fields={label:value for label,value,_ in rows}
   assert fields['Compression method']==method and fields['Encrypted']==encrypted
   assert fields['Kind']=='Regular file' and fields['Declared size']==('301 bytes' if name=='e-deflate64.txt' else '24 bytes')
 results[scene]=dict(checks=len(m['checks']),effects=m['native_effect_calls'],frame_sha256=hashlib.sha256((directory/'frame.png').read_bytes()).hexdigest(),capture=str(directory))
(q/'verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
