from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
results={}
for key,scene,effects in [('native-launch.json','archive_metadata',0),('native-export-launch.json','archive_export',5)]:
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
  for name,method,encrypted in [('a-stored.txt','Stored','No'),('b-deflated.txt','Deflated','No'),('c-encrypted.txt','Unsupported(222)','Yes (ZipCrypto, password required)'),('d-bzip2.txt','Bzip2','No'),('e-deflate64.txt','Deflate64','No'),('f-lzma.txt','Lzma','No'),('g-xz.txt','Xz','No'),('h-zstd93.txt','Zstd','No'),('i-zstd20.txt','Zstd (legacy method 20)','No'),('j-ppmd.txt','Ppmd','No'),('k-aes.txt','Stored','Yes (WinZip AES-256 (AE-2), password required)')]:
   path,rows=json.loads((directory/f'metadata-{name}.json').read_text())
   assert path==name and len(rows)==9 and all(row[2] is True for row in rows)
   fields={label:value for label,value,_ in rows}
   assert fields['Compression method']==method and fields['Encrypted']==encrypted
   # Independent payload CRC from original fixtures; encrypted AE-2 has no CRC.
   import zlib
   if name=='k-aes.txt':expected=None
   elif name in ('a-stored.txt','b-deflated.txt','c-encrypted.txt','d-bzip2.txt'):expected=zlib.crc32(b'Native metadata fixture\n')
   else:
    fixtures={'e-deflate64.txt':('zip-deflate64','extended'),'f-lzma.txt':('zip-lzma','normal'),'g-xz.txt':('zip-xz','normal'),'h-zstd93.txt':('zip-zstd','normal'),'i-zstd20.txt':('zip-zstd','legacy'),'j-ppmd.txt':('zip-ppmd','normal')}
    folder,stem=fixtures[name]
    import zipfile
    with zipfile.ZipFile(root/f'docs/qualification/{folder}-2026-11-03/fixtures/{stem}.zipx') as archive:expected=archive.infolist()[0].CRC
   assert fields['Declared CRC-32']==('Unavailable (not recorded per member)' if expected is None else f'{expected:08X}')
   assert fields['Kind']=='Regular file'
   if name != 'k-aes.txt':
    assert fields['Kind']=='Regular file' and fields['Declared size']==('301 bytes' if name=='e-deflate64.txt' else ('30 bytes' if name=='f-lzma.txt' else ('28 bytes' if name=='g-xz.txt' else ('30 bytes' if name in ('h-zstd93.txt','i-zstd20.txt') else ('4352 bytes' if name=='j-ppmd.txt' else '24 bytes')))))
 if scene=='archive_metadata':
  baseline=json.loads((directory/'crc-sort-baseline.json').read_text())
  for phase in [39,43]:
   state=json.loads((directory/f'crc-sort-{phase}.json').read_text())
   assert state['descending'] is (phase==43) and state['selected']==baseline
   rows=state['rows']
   assert len(rows)==11 and {r['id'] for r in rows}==set(range(11))
   assert [r['position'] for r in rows]==list(range(11))
   for row in rows:
    _,details=json.loads((directory/f"metadata-{row['path']}.json").read_text())
    crc={k:v for k,v,_ in details}['Declared CRC-32']
    assert row['crc32']==(None if crc.startswith('Unavailable') else int(crc,16))
   expected=sorted(rows,key=lambda r:(r['crc32'] is None,(-r['crc32'] if state['descending'] else r['crc32']) if r['crc32'] is not None else 0,r['path'].lower(),r['id']))
   assert [r['id'] for r in rows]==[r['id'] for r in expected]
 if scene=='archive_metadata':
  from collections import Counter
  state=json.loads((directory/'crc-repeat-selection.json').read_text())
  rows=state['rows']
  assert len(rows)==11 and {r['id'] for r in rows}==set(range(11))
  keys={}
  for row in rows:
   _,details=json.loads((directory/f"metadata-{row['path']}.json").read_text())
   details={k:v for k,v,_ in details}
   assert row['size']==int(details['Declared size'].split()[0])
   crc=details['Declared CRC-32']
   expected_crc=None if crc.startswith('Unavailable') else int(crc,16)
   assert row['crc32']==expected_crc
   if row['regular'] and not row['directory'] and not row['symlink'] and expected_crc is not None:keys[row['id']]=(row['size'],expected_crc)
  counts=Counter(keys.values())
  expected=sorted(id for id,key in keys.items() if counts[key]>1)
  assert expected and len(expected)<len(rows) and state['selected']==expected
  assert all(r['id'] not in state['selected'] for r in rows if r['crc32'] is None)
 results[scene]=dict(checks=len(m['checks']),effects=m['native_effect_calls'],frame_sha256=hashlib.sha256((directory/'frame.png').read_bytes()).hexdigest(),capture=str(directory))
(q/'verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
