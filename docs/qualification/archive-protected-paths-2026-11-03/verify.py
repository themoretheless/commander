from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
root=q.parents[2]
read=lambda p:json.loads(p.read_text())
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3483 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for path,digest in read(q/'source-sha256.json').items():assert hashlib.sha256((root/path).read_bytes()).hexdigest()==digest,path
results={}
for key,scene,effects in [('native-launch.json','archive_tar_content_groups',4),('native-zip-launch.json','archive_content_groups',2)]:
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
    assert member.name==fact['path'] and member.size==fact['size'] and member.mtime==fact['modified']
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
 if scene=='archive_tar_content_groups':
  group_indexes=[sorted(m['index'] for m in group['members']) for group in expected]
  cases={23:group_indexes[1],27:[],31:group_indexes[0],35:sorted(group_indexes[0]+group_indexes[1])}
  for phase,wanted in cases.items():
   state=json.loads((directory/f'tar-group-action-{phase}.json').read_text())
   assert state['phase']==phase and state['selected_indexes']==wanted
   assert {r['index'] for r in state['listing']}==set(range(11))
   for row in state['listing']:assert row['path'].rstrip('/')==entries[row['index']]['path'].rstrip('/')
   assert state['selected']==sorted(r['id'] for r in state['listing'] if r['index'] in wanted)
 if scene=='archive_tar_content_groups':
  scoped=json.loads((directory/'tar-single-group.json').read_text())
  assert scoped['schema']=='commander.archive-check/v1'
  assert scoped['content_group_index']==1 and scoped['status']=='complete'
  assert scoped['members']==11 and scoped['decoded_bytes']==20480
  assert scoped['content_groups']==[expected[1]]
  text=(directory/'tar-single-group.txt').read_text()
  assert 'Scope: content group #1 (counts above describe the complete scan)' in text
  assert 'Content groups: 1\nGroup #1\n' in text
  assert 'Group #0' not in text and 'empty-one' not in text and 'empty-two' not in text
  assert expected[1]['sha256'] in text
  for member in expected[1]['members']:
   assert f"Entry #{member['index']} ({member['size']} bytes): "+json.dumps(member['path'],ensure_ascii=False) in text
 if scene=='archive_tar_content_groups':
  keepers={min(member['index'] for member in group['members']) for group in expected}
  all_indexes={member['index'] for group in expected for member in group['members']}
  for phase,removed in [(47,{min(member['index'] for member in expected[1]['members'])}),(51,keepers)]:
   state=json.loads((directory/f'tar-redundant-selection-{phase}.json').read_text())
   assert state['phase']==phase
   assert state['selected_indexes']==sorted(all_indexes-removed)
   assert state['selected']==sorted(row['id'] for row in state['listing'] if row['index'] in all_indexes-removed)
 if scene=='archive_tar_content_groups':
  state=json.loads((directory/'tar-keep-last.json').read_text())
  assert state['keep_count']==1 and state['keep_last'] is True
  keepers={max(member['index'] for member in group['members']) for group in expected}
  indexes={member['index'] for group in expected for member in group['members']}-keepers
  assert state['selected_indexes']==sorted(indexes)==[0,2,4,8,9]
  assert state['selected']==sorted(row['id'] for row in state['listing'] if row['index'] in indexes)
 if scene=='archive_tar_content_groups':
  all_indexes={member['index'] for group in expected for member in group['members']}
  def keepers(newest):
   return {sorted(group['members'],key=lambda member:((-1 if newest else 1)*entries[member['index']]['modified'],member['index']))[0]['index'] for group in expected}
  selected_last=json.loads((directory/'tar-keep-last.json').read_text())['selected_indexes']
  selected_newest=sorted(all_indexes-keepers(True))
  selected_oldest=sorted(all_indexes-keepers(False))
  for phase,order,wanted in [(62,'Newest timestamps',selected_last),(66,'Newest timestamps',selected_newest),(70,'Oldest timestamps',selected_newest),(74,'Oldest timestamps',selected_oldest)]:
   state=json.loads((directory/f'tar-keep-time-{phase}.json').read_text())
   assert state['phase']==phase and state['keep_count']==1 and state['order']==order
   assert state['selected_indexes']==wanted
   assert state['selected']==sorted(row['id'] for row in state['listing'] if row['index'] in wanted)
   for row in state['listing']:
    fact=entries[row['index']]
    assert row['path'].rstrip('/')==fact['path'].rstrip('/') and row['modified']==fact['modified']
  assert selected_newest!=selected_oldest and selected_last not in [selected_newest,selected_oldest]
 if scene=='archive_tar_content_groups':
  from pathlib import PurePosixPath
  all_indexes={member['index'] for group in expected for member in group['members']}
  def apply_depth(selected,groups,deepest):
   selected=set(selected)
   for group in groups:
    ranked=sorted(group['members'],key=lambda member:((-1 if deepest else 1)*(len(PurePosixPath(member['path']).parts)-1),member['index']))
    selected.update(member['index'] for member in ranked)
    selected.difference_update(member['index'] for member in ranked[:1])
   return sorted(selected)
  old=json.loads((directory/'tar-keep-time-74.json').read_text())['selected_indexes']
  shallow_group=apply_depth(old,[expected[1]],False)
  shallow_all=apply_depth(shallow_group,expected,False)
  deep_group=apply_depth(shallow_all,[expected[1]],True)
  deep_all=apply_depth(deep_group,expected,True)
  assert shallow_group!=shallow_all and deep_all!=shallow_all
  for phase,order,wanted in [(78,'Shallowest paths',old),(82,'Shallowest paths',shallow_group),(86,'Shallowest paths',shallow_all),(90,'Deepest paths',shallow_all),(94,'Deepest paths',deep_group),(98,'Deepest paths',deep_all)]:
   state=json.loads((directory/f'tar-keep-depth-{phase}.json').read_text())
   assert state['phase']==phase and state['keep_count']==1 and state['order']==order
   assert state['selected_indexes']==wanted
   assert state['selected']==sorted(row['id'] for row in state['listing'] if row['index'] in wanted)
   for row in state['listing']:assert row['path'].rstrip('/')==entries[row['index']]['path'].rstrip('/')
 if scene=='archive_tar_content_groups':
  import re
  previous=json.loads((directory/'tar-keep-depth-98.json').read_text())['selected_indexes']
  def apply_protected(selected,groups,glob):
   # These native vectors deliberately exercise exact, one-folder and recursive
   # forms. Rust's general matcher is separately covered by its glob matrix.
   expression={'dup.txt':r'dup\.txt','*':r'[^/]*','**':r'.*'}[glob]
   selected=set(selected)
   for group in groups:
    protected={m['index'] for m in group['members'] if re.fullmatch(expression,m['path'])}
    ranked=sorted(group['members'],key=lambda m:(-(len(PurePosixPath(m['path']).parts)-1),m['index']))
    keepers=protected|{m['index'] for m in [m for m in ranked if m['index'] not in protected][:max(0,1-len(protected))]}
    selected.update(m['index'] for m in group['members']);selected.difference_update(keepers)
   return sorted(selected)
  duplicated=apply_protected(previous,expected,'dup.txt')
  roots=apply_protected(duplicated,expected,'*')
  everything=apply_protected(roots,expected,'**')
  one_group=apply_protected(everything,[expected[1]],'dup.txt')
  all_groups=apply_protected(one_group,expected,'dup.txt')
  assert roots==[10] and everything==[] and one_group!=all_groups
  cases=[(104,'dup.txt',previous),(108,'dup.txt',duplicated),(114,'*',duplicated),(118,'*',roots),(124,'**',roots),(128,'**',everything),(134,'[',everything),(140,'dup.txt',everything),(144,'dup.txt',one_group),(148,'dup.txt',all_groups)]
  for phase,glob,wanted in cases:
   state=json.loads((directory/f'tar-protected-{phase}.json').read_text())
   assert state['phase']==phase and state['glob']==glob and state['order']=='Deepest paths' and state['keep_count']==1
   assert state['global_enabled'] is (phase!=134) and state['group_enabled'] is (phase!=134)
   assert state['selected_indexes']==wanted
   assert state['selected']==sorted(row['id'] for row in state['listing'] if row['index'] in wanted)
   for row in state['listing']:assert row['path'].rstrip('/')==entries[row['index']]['path'].rstrip('/')
 results[scene]=dict(checks=len(manifest['checks']),effects=manifest['native_effect_calls'],frame_sha256=hashlib.sha256((directory/'frame.png').read_bytes()).hexdigest(),capture=str(directory))
(q/'verification.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
