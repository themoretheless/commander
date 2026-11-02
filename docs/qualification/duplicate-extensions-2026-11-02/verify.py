from pathlib import Path
import hashlib,json,shutil,collections
q=Path(__file__).resolve().parent;root=q.parents[2]
for relative,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest,relative
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3156 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key in [('duplicate_extensions','native-launch.json'),('duplicate_names','duplicate_names-launch.json')]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
d=q/'duplicate_extensions';raw={p.name:p.read_bytes() for p in (d/'sources').iterdir() if p.is_file()};assert len(raw)==8
assert len({hashlib.sha256(data).digest() for data in raw.values()})==2 and len({len(data) for data in raw.values()})==1
baseline=json.loads((d/'extensions-draft.json').read_text());order=[Path(path).name for group in baseline['groups'] for path in group];assert len(order)==7 and set(order)==set(raw)-{'unique.txt'}
def expected(mode):
 buckets=collections.OrderedDict()
 for name in order:
  ext=Path(name).suffix[1:] if mode!='Any' else ''
  if mode=='IgnoreAsciiCase':ext=ext.translate(str.maketrans('ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
  buckets.setdefault(ext,[]).append(name)
 return [names for names in buckets.values() if len(names)>=2]
for label,mode,draft,trash in [('draft','Any','CaseSensitive',6),('sensitive','CaseSensitive','CaseSensitive',3),('draft-ignore','CaseSensitive','IgnoreAsciiCase',3),('insensitive','IgnoreAsciiCase','IgnoreAsciiCase',4),('final','Any','Any',6)]:
 s=json.loads((d/f'extensions-{label}.json').read_text());assert s['applied']['extension']==mode and s['draft']['extension']==draft
 groups=[[Path(path).name for path in group] for group in s['groups']];assert groups==expected(mode),(label,groups,expected(mode))
 assert len(s['keep'])==len(groups) and all(type(k) is int and 0<=k<len(g) for k,g in zip(s['keep'],groups))
 for group in groups:assert len({hashlib.sha256(raw[name]).digest() for name in group})==1
 r=s['report'];assert r['context']['names']['extension']==mode and r['summary']['groups']==len(groups) and r['summary']['proposed_trash_files']==trash and r['summary']['proposed_trash_bytes']==trash*len(raw['one.txt']) and r['summary']['scan_issues']==0 and not r['summary']['partial']
 assert [[Path(f['path']).name for f in group['files']] for group in r['groups']]==groups
s=json.loads((q/'duplicate_names/names-final.json').read_text());assert s['issues']==0 and len(s['files'])==6 and not s['applied']['include'] and not s['applied']['exclude'] and s['applied']['extension']=='Any'
print('PASS: six checks; 3156 tests; sensitive/ASCII-insensitive/reset extension groups and applied JSON agree with preserved fixture bytes and independent SHA-256; draft isolation and valid keep plans; two native scenarios 14/14 each')
