from pathlib import Path
import hashlib,json,shutil,collections
q=Path(__file__).resolve().parent;root=q.parents[2]
for path,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/path).read_bytes()).hexdigest()==digest,path
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3167 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clipboard in [('duplicate_relative','native-launch.json',0),('duplicate_roots','duplicate_roots-launch.json',1)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clipboard and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
d=q/'duplicate_relative'
v=json.loads((d/'results-viewport.json').read_text());assert v['height']>=40 and 0<=v['min'][1]<v['max'][1]<=760 and v['max'][0]>v['min'][0]
raw={p.relative_to(d/'sources').as_posix():p.read_bytes() for p in (d/'sources').rglob('*') if p.is_file()}
assert len(raw)==7 and len({len(v) for v in raw.values()})==1 and len({hashlib.sha256(v).digest() for v in raw.values()})==2
base=json.loads((d/'relative-2.json').read_text());roots={'left':Path(base['report']['context']['root']),'right':Path(base['report']['context']['additional_root'])}
def short(path):
 p=Path(path)
 candidates=[(side,r) for side,r in roots.items() if p.is_relative_to(r)]
 assert candidates,path
 side,r=max(candidates,key=lambda pair:len(pair[1].parts))
 return side+'/'+p.relative_to(r).as_posix()
order=[short(path) for group in base['groups'] for path in group]
assert len(order)==6 and set(order)==set(raw)-{'right/Docs/unique.bin'}
assert len({hashlib.sha256(raw[p]).digest() for p in order})==1
fold=lambda key:key.translate(str.maketrans('ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz'))
def expected(component,ignore,both,cross):
 names=[p for p in order if both or p.startswith('left/')]
 buckets=collections.OrderedDict()
 for p in names:
  key=p.split('/',1)[1] if component=='RelativePath' else ''
  if ignore:key=fold(key)
  buckets.setdefault(key,[]).append(p)
 return [g for g in buckets.values() if len(g)>=2 and (not cross or len({p.split('/',1)[0] for p in g})>=2)]
# applied component/case, draft component/case, effective scope, effective cross-root flag.
transitions={
 '2':('Any',False,'Any',False,True,False),
 '3':('Any',False,'Any',False,True,False),
 '4':('Any',False,'RelativePath',False,True,False),
 '5':('RelativePath',False,'RelativePath',False,True,False),
 '6':('RelativePath',False,'RelativePath',True,True,False),
 '7':('RelativePath',True,'RelativePath',True,True,False),
 '8':('RelativePath',True,'RelativePath',True,True,True),
 '9':('Any',False,'Any',False,True,True),
 '10':('Any',False,'Any',False,False,False),
 '11':('Any',False,'RelativePath',False,False,False),
 '12':('RelativePath',False,'RelativePath',False,False,False),
 '13':('Any',False,'Any',False,False,False),
 'final':('Any',False,'Any',False,True,True),
}
for label,(component,ignore,draft,dignore,both,cross) in transitions.items():
 s=json.loads((d/f'relative-{label}.json').read_text());a=s['applied'];b=s['draft']
 assert (a['grouping'],a['grouping_ignore_ascii_case'])==(component,ignore),label
 assert (b['grouping'],b['grouping_ignore_ascii_case'])==(draft,dignore),label
 assert a['extension']==b['extension']=='Any'
 groups=[[short(p) for p in g] for g in s['groups']];assert groups==expected(component,ignore,both,cross),(label,groups,expected(component,ignore,both,cross))
 assert len(s['keep'])==len(groups)
 for g,k in zip(s['groups'],s['keep']):
  assert type(k) is int and k==min(range(len(g)),key=lambda i:len(g[i].encode('utf-8')))
 for g in groups:assert len({hashlib.sha256(raw[p]).digest() for p in g})==1
 # Existing Duplicate Finder report contract rejects empty results (duplicate_report::json).
 # This exact single-root transition must be empty and must not invent an export.
 if label=='12':
  assert groups==[] and s['keep']==[] and s['report'] is None
  continue
 r=s['report'];c=r['context'];summary=r['summary'];assert c['names']==a and Path(c['root'])==roots['left']
 assert c['scope']==('two_trees' if both else 'recursive') and c['metadata_source']=='fresh_tree_enumeration'
 if both:assert Path(c['additional_root'])==roots['right'] and c['only_cross_root_groups']==cross
 else:assert 'additional_root' not in c and 'only_cross_root_groups' not in c
 assert c['keep_policy']=='shortest_path' and c['minimum_copies']==2 and c['group_sort']=='scan'
 trash=sum(len(g)-1 for g in groups)
 assert summary['groups']==len(groups) and summary['files']==sum(map(len,groups)) and summary['proposed_trash_files']==trash and summary['proposed_trash_bytes']==trash*len(raw[order[0]])
 assert summary['scan_issues']==0 and not summary['partial']
 assert [[short(f['path']) for f in g['files']] for g in r['groups']]==groups
 for g,k in zip(r['groups'],s['keep']):assert [f['action'] for f in g['files']]==['keep' if i==k else 'trash' for i in range(len(g['files']))]
r=json.loads((q/'duplicate_roots/two-roots-report.json').read_text());c=r['context'];s=r['summary']
assert c['scope']=='two_trees' and c['metadata_source']=='fresh_tree_enumeration' and Path(c['root']).name=='left' and Path(c['additional_root']).name=='right'
assert c['names']['grouping']=='Any' and not c['names']['grouping_ignore_ascii_case']
assert (s['groups'],s['files'],s['proposed_trash_files'],s['proposed_trash_bytes'])==(4,8,4,42) and s['scan_issues']==0 and not s['partial']
print('PASS: six checks; 3167 tests; 13 relative-path/draft/scope states match independent SHA-256 and relative keys; nonempty JSON keeper actions and counts/bytes; single-root empty result and resets; two native workflows 14/14; final source/binary/tool hashes')
