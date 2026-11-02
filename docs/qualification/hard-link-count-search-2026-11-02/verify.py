from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for path,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/path).read_bytes()).hexdigest()==digest,path
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3169 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clipboard in [('links_search','native-launch.json',1),('identity_find','identity_find-launch.json',0)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clipboard and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
d=q/'links_search';raw=json.loads((d/'links-facts.json').read_text());fixture=Path(raw['root']);facts={f['path']:f for f in raw['facts']};assert len(facts)==7
short=lambda path:Path(path).relative_to(fixture).as_posix()
linked={p for p,f in facts.items() if f['regular'] and f['nlink']>1 and Path(p).name.startswith('link-')}
singles={p for p,f in facts.items() if f['regular'] and f['nlink']==1 and Path(p).name.startswith('link-')}
assert len(linked)==3 and singles=={'link-copy.txt'}
assert len({(facts[p]['device'],facts[p]['inode']) for p in linked})==1
assert (facts['link-copy.txt']['device'],facts['link-copy.txt']['inode']) not in {(facts[p]['device'],facts[p]['inode']) for p in linked}
reference=(d/'links-find-reference.bin').read_bytes();assert reference.endswith(b'\0')
assert {short(p.decode()) for p in reference.split(b'\0') if p}==linked
for label,expression,expected,skipped in [
 ('4','links:>1 name:link-',linked,1),('7','links:1 name:link-',singles,1),('10','links:>3 name:link-',set(),1),('13','links:3 archives:off name:link-',linked,0),('final','links:3 archives:off name:link-',linked,0),
]:
 s=json.loads((d/f'links-{label}.json').read_text());assert s['expression']==expression and Path(s['root'])==fixture
 assert s['matched']==len(expected) and len(s['rows'])==len(expected) and s['skipped']==skipped
 assert s['source']==('Live + archives' if skipped else 'Live (archives off)')
 assert {short(r['path']) for r in s['rows']}==expected
 for r in s['rows']:
  fact=facts[short(r['path'])];assert fact['regular'] and not fact['symlink'] and not fact['directory']
  assert r['hard_links']==fact['nlink'] and any(c.startswith('links:') and f"observed {fact['nlink']} hard links" in c for c in r['explanations'])
r=json.loads((d/'links-report.json').read_text());assert r['format']=='commander.search-report' and r['version']==1 and not r['partial'] and r['exported_rows']==3 and r['scope']=='visible_results'
assert r['root']=={'encoding':'utf8','text':str(fixture)}
c=r['context'];assert c['completed'] and not c['running'] and not c['pending_rerun'] and not c['draft_differs'] and not c['truncated'] and c['skipped']==0 and c['matched']==3 and c['visible_rows']==3 and not c['selected_only']
assert c['source']=='Live (archives off)' and c['expression']=='links:>=3 links:<=3 archives:off link-'
assert {short(row['location']['path']['text']) for row in r['rows']}==linked
for row in r['rows']:
 assert row['location']['kind']=='native' and row['location']['path']['encoding']=='utf8' and not row['is_directory']
 f=facts[short(row['location']['path']['text'])];assert row['hard_links']==f['nlink']==3 and row['native_id']=={'device_id':f['device'],'file_id':f['inode']}
 assert any(c['detail'].startswith('links:') and 'observed 3 hard links' in c['detail'] for c in row['explanation'])
s=json.loads((q/'identity_find/identity-final.json').read_text());raw=json.loads((q/'identity_find/identity-facts.json').read_text());facts={f['path']:f for f in raw}
assert s['matched']==2 and s['skipped']==1 and len(s['rows'])==2
assert {Path(row['path']).name for row in s['rows']}=={'identity-original.txt','identity-alias.txt'}
assert (facts['identity-original.txt']['device'],facts['identity-original.txt']['inode'])==(facts['nested/identity-alias.txt']['device'],facts['nested/identity-alias.txt']['inode'])
print('PASS: six checks; 3169 tests; four link-count queries and real JSON agree with raw nlink/device/inode and independent find -links; archive unknowns, singleton/empty/off states; two native workflows 16/16; final source/binary/tool hashes')
