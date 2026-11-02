from pathlib import Path
import json,hashlib,shutil,datetime
q=Path('docs/qualification/file-timestamps-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest();binary=h('target/debug/commander')
for key,dest,scene in [('native-launch.json','native','timestamps'),('permissions-launch.json','native-permissions','permissions')]:
 launch=load(q/key);assert launch['binary_sha256']==launch['core_sha256']==binary
 out=Path(launch['out']);src=out/scene;m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
first=load(q/'native-first-launch.json');assert first['binary_sha256']==first['core_sha256']==binary
m=load(q/'native-first-pass/manifest.json');assert m['status']=='captured' and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
for folder in ['native','native-first-pass']:
 expected_modified='2026-10-31T06:30:00Z';expected_accessed='2025-01-02T00:00:00Z';initial={'times-a.txt':'2020-01-01T00:00:00Z','times-b.txt':'2021-01-01T00:00:00Z','times-untouched.txt':'1999-01-01T00:00:00Z'}
 for phase in [4,15,18,21,24,27]:
  s=load(q/folder/('times-final.json' if phase==27 else f'times-{phase}.json'));v=s['view']
  assert s['phase']==phase and v['timestamps'] and not v['busy'] and v['error'] is None
  files={Path(f['path']).name:f for f in s['files']};assert set(files)==set(initial)
  for name,f in files.items():
   applied=phase>=18 and name!='times-untouched.txt'
   assert f['size']==18 and f['modified']==(expected_modified if applied else initial[name]) and f['accessed']==(expected_accessed if applied else '2022-01-01T00:00:00Z')
  assert len(v['rows'])==2 and all(r['error'] is None and r['modified'] is not None and r['accessed'] is not None for r in v['rows'])
  for row in v['rows']:
   name=Path(row['path']).name;assert name in ['times-a.txt','times-b.txt']
   reloaded=phase>=21
   assert row['modified']==(expected_modified if reloaded else initial[name]) and row['accessed']==(expected_accessed if reloaded else '2022-01-01T00:00:00Z')
  if phase==4:assert v['modified']=='not a date' and v['accessed']==''
  if phase in [15,18]:assert v['modified']=='2026-10-31T10:30:00+04:00' and v['accessed']=='2025-01-02'
  if phase in [21,27]:assert v['modified']==v['accessed']==''
  if phase==24:
   assert v['modified']==v['accessed'] and v['modified'];datetime.datetime.fromisoformat(v['modified'].replace('Z','+00:00'))
  if phase>=18:assert v['report']==dict(cancelled=False,changed=2,errors=[],unchanged=0)
  assert v['needs_reload']==(phase==18)
 assert all(f'Timestamp QA phase {phase}\n' in (q/(folder+'.log')).read_text() for phase in range(27))
 for press,release,kind in [(6,7,'reset'),(17,18,'apply'),(26,27,'reset')]:
  p=load(q/folder/f'pointer-{press}.json')[kind];r=load(q/folder/f'pointer-{release}.json')[kind]
  assert p['id']==r['id'] and p['down'] and not p['clicked'] and r['clicked'] and not r['down'] and p['hovered'] and r['hovered'] and p['enabled'] and r['enabled']
 assert not load(q/folder/'pointer-6.json')['apply']['enabled']
 assert load(q/folder/'times-end-observation.json')['ready'] is True
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3087 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/timestamps.rs','src/permissions.rs','src/app/permissions_dialog.rs','src/app/update.rs','src/command.rs','src/ui_request.rs','src/workspace.rs','src/workspace/tests.rs','src/visual_qa.rs','.github/workflows/ci.yml','src/main.rs']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',timestamps_native_checks=16,timestamps_same_binary_passes=2,permissions_regression_checks=16,input_phases=27,invalid_input_cannot_apply=True,offset_converted_to_utc=True,exact_committed_file_times=True,unselected_file_unchanged=True,reload_has_usable_rebound_rows=True,now_draft_not_applied=True,all_recording_effect_calls=0,real_metadata_writes_in_fixture=True,binary_matches_launch=True,full_tests_passed=3087,ignored=3,checks=6),indent=2)+'\n')
print('2 x 16 timestamps + 16 permissions native checks; exact filesystem times, usable Reload, unselected file and six checks verified')
