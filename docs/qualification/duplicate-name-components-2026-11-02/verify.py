from pathlib import Path
import hashlib,json,shutil,collections
q=Path(__file__).resolve().parent;root=q.parents[2]
for path,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/path).read_bytes()).hexdigest()==digest,path
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3161 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key in [('duplicate_components','native-launch.json'),('duplicate_extensions','duplicate_extensions-launch.json'),('duplicate_names','duplicate_names-launch.json')]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks']);assert all(v==0 for v in m['native_effect_calls'].values())
d=q/'duplicate_components';raw={p.relative_to(d/'sources').as_posix():p.read_bytes() for p in (d/'sources').rglob('*') if p.is_file()}
assert len(raw)==6 and len({len(v) for v in raw.values()})==1
assert len({hashlib.sha256(v).digest() for v in raw.values()})==2
short=lambda path:'/'.join(Path(path).parts[-3:])
base=json.loads((d/'components-1.json').read_text());order=[short(path) for group in base['groups'] for path in group]
assert len(order)==5 and set(order)==set(raw)-{'F/third/unique.txt'}
ascii_fold=lambda value:value.translate(str.maketrans('ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
def partition(groups,key):
 output=[]
 for group in groups:
  buckets=collections.OrderedDict()
  for name in group:buckets.setdefault(key(name),[]).append(name)
  output.extend(g for g in buckets.values() if len(g)>=2)
 return output
def expected(component,fold,extension):
 groups=[order]
 if extension!='Any':groups=partition(groups,lambda name:Path(name).suffix[1:])
 if component!='Any':
  def key(name):
   p=Path(name);value=p.name if component=='Basename' else p.stem if component=='Stem' else p.parent.name
   return ascii_fold(value) if fold else value
  groups=partition(groups,key)
 return groups
# Explicit transition expectations prevent a UI script from validating itself against its output.
transitions={
 '1':('Any',False,'Any','Any',False,'Any'),
 '2':('Any',False,'Any','Any',False,'Any'),
 '3':('Any',False,'Any','Basename',False,'Any'),
 '4':('Basename',False,'Any','Basename',False,'Any'),
 '5':('Basename',False,'Any','Basename',True,'Any'),
 '6':('Basename',True,'Any','Basename',True,'Any'),
 '7':('Basename',True,'Any','Stem',True,'Any'),
 '8':('Stem',True,'Any','Stem',True,'Any'),
 '9':('Stem',True,'Any','Stem',True,'CaseSensitive'),
 '10':('Stem',True,'CaseSensitive','Stem',True,'CaseSensitive'),
 '11':('Any',False,'Any','Any',False,'Any'),
 '12':('Any',False,'Any','ParentName',False,'Any'),
 '13':('ParentName',False,'Any','ParentName',False,'Any'),
 '14':('ParentName',False,'Any','ParentName',True,'Any'),
 '15':('ParentName',True,'Any','ParentName',True,'Any'),
 'final':('Any',False,'Any','Any',False,'Any'),
}
for label,(component,fold,extension,draft,dfold,dext) in transitions.items():
 s=json.loads((d/f'components-{label}.json').read_text())
 a=s['applied'];b=s['draft'];assert (a['grouping'],a['grouping_ignore_ascii_case'],a['extension'])==(component,fold,extension),label
 assert (b['grouping'],b['grouping_ignore_ascii_case'],b['extension'])==(draft,dfold,dext),label
 groups=[[short(path) for path in group] for group in s['groups']];assert groups==expected(component,fold,extension),(label,groups,expected(component,fold,extension))
 assert len(s['keep'])==len(groups) and all(type(k) is int and 0<=k<len(g) for k,g in zip(s['keep'],groups))
 for group in groups:assert len({hashlib.sha256(raw[name]).digest() for name in group})==1
 r=s['report'];summary=r['summary'];trash=sum(len(g)-1 for g in groups)
 assert r['context']['names']==a and summary['groups']==len(groups) and summary['proposed_trash_files']==trash and summary['proposed_trash_bytes']==trash*len(raw[order[0]])
 assert summary['scan_issues']==0 and not summary['partial']
 assert [[short(f['path']) for f in g['files']] for g in r['groups']]==groups
# Independently repeat the neighboring extension partitions from their preserved bytes.
d=q/'duplicate_extensions';raw={p.name:p.read_bytes() for p in (d/'sources').iterdir() if p.is_file()};assert len(raw)==8
assert len({hashlib.sha256(v).digest() for v in raw.values()})==2
base=json.loads((d/'extensions-draft.json').read_text());order=[Path(p).name for g in base['groups'] for p in g];assert len(order)==7 and set(order)==set(raw)-{'unique.txt'}
for label,mode,draft in [('draft','Any','CaseSensitive'),('sensitive','CaseSensitive','CaseSensitive'),('draft-ignore','CaseSensitive','IgnoreAsciiCase'),('insensitive','IgnoreAsciiCase','IgnoreAsciiCase'),('final','Any','Any')]:
 s=json.loads((d/f'extensions-{label}.json').read_text());assert s['applied']['extension']==mode and s['draft']['extension']==draft
 assert s['applied']['grouping']=='Any' and not s['applied']['grouping_ignore_ascii_case']
 groups=[order] if mode=='Any' else partition([order],lambda name:ascii_fold(Path(name).suffix) if mode=='IgnoreAsciiCase' else Path(name).suffix)
 assert [[Path(p).name for p in g] for g in s['groups']]==groups
 r=s['report'];assert r['context']['names']==s['applied'] and r['summary']['proposed_trash_files']==sum(len(g)-1 for g in groups)
s=json.loads((q/'duplicate_names/names-final.json').read_text());assert s['issues']==0 and len(s['files'])==6
assert s['applied']==dict(include='',exclude='',case=False,extension='Any',grouping='Any',grouping_ignore_ascii_case=False)
print('PASS: six checks; 3161 tests; 16 native component states agree with independent name keys and fixture SHA-256; draft/applied JSON and proposals; three native workflows 14/14 each; final source/binary/tool hashes')
