from pathlib import Path
import hashlib,json,shutil
q=Path(__file__).resolve().parent;root=q.parents[2]
for path,digest in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/path).read_bytes()).hexdigest()==digest,path
checks=json.loads((q/'checks.json').read_text());assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3164 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for tool in json.loads((q/'validators.json').read_text()):assert hashlib.sha256(Path(tool['path']).read_bytes()).hexdigest()==tool['sha256']
for scene,key,clipboard in [('duplicate_ranking','native-launch.json',0),('duplicate_policy','duplicate_policy-launch.json',3)]:
 launch=json.loads((q/key).read_text());assert hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()==launch['binary_sha256']==launch['core_sha256']
 source=Path(launch['out'])/scene;target=q/scene
 if source.exists():shutil.copytree(source,target,dirs_exist_ok=True)
 m=json.loads((target/'manifest.json').read_text());assert m['status']=='captured' and m['error'] is None
 assert len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']['clipboard']==clipboard and all(v==0 for k,v in m['native_effect_calls'].items() if k!='clipboard')
d=q/'duplicate_ranking';raw={p.relative_to(d/'sources').as_posix():p.read_bytes() for p in (d/'sources').rglob('*') if p.is_file()}
assert len(raw)==7 and len({len(v) for v in raw.values()})==1 and len({hashlib.sha256(v).digest() for v in raw.values()})==2
base=json.loads((d/'ranking-1.json').read_text());fixture=Path(base['report']['context']['root'])
short=lambda path:Path(path).relative_to(fixture).as_posix()
order=[short(path) for group in base['groups'] for path in group];assert len(order)==6 and set(order)==set(raw)-{'unique-same-size.bin'}
assert len({hashlib.sha256(raw[name]).digest() for name in order})==1
name=lambda path:Path(path).name.encode('utf-8')
length=lambda path:len(name(path))
depth=lambda path:len(Path(path).parent.parts)
# Full native prefix is common, so it adds a constant to byte lengths/depths.
winners={
 'shortest_path':min(order,key=lambda p:len(str(fixture/Path(p)).encode('utf-8'))),
 'first_name':min(order,key=name),'last_name':max(order,key=name),
 'shortest_name':min(order,key=length),'longest_name':max(order,key=length),
 'shallowest_path':min(order,key=depth),'deepest_path':max(order,key=depth),
}
assert len({winners[k] for k in winners if k!='shortest_path'})==6,winners
transitions={
 '1':('shortest_path',False,'scan'),'2':('first_name',False,'scan'),
 '3':('last_name',False,'scan'),'4':('shortest_name',False,'scan'),
 '5':('longest_name',False,'scan'),'6':('shallowest_path',False,'scan'),
 '7':('deepest_path',False,'scan'),'8':('deepest_path',True,'scan'),
 '9':('deepest_path',False,'scan'),'10':('deepest_path',False,'savings'),
 'final':('shortest_path',False,'savings'),
}
for label,(policy,skip,sort) in transitions.items():
 s=json.loads((d/f'ranking-{label}.json').read_text());groups=[[short(path) for path in g] for g in s['groups']];assert groups==[order],label
 assert len(s['keep'])==1
 kept=s['keep'][0]
 if skip:assert kept is None
 else:assert type(kept) is int and 0<=kept<6 and order[kept]==winners[policy],(label,kept,winners[policy])
 r=s['report'];c=r['context'];summary=r['summary'];assert c['keep_policy']==policy and c['group_sort']==sort
 assert c['scope']=='recursive' and c['minimum_copies']==2
 assert summary['groups']==1 and summary['files']==6 and summary['scan_issues']==0 and not summary['partial']
 trash=0 if skip else 5
 assert summary['proposed_trash_files']==trash and summary['proposed_trash_bytes']==trash*len(raw[order[0]])
 rows=r['groups'][0]['files'];assert [short(f['path']) for f in rows]==order
 actions=[f['action'] for f in rows];expected=['skip' if skip else 'keep' if i==kept else 'trash' for i in range(6)]
 assert actions==expected,(label,actions,expected)
print('PASS: six checks; 3164 tests; six independent keeper winners, skip/reapply and sorted plans agree with preserved SHA-256 fixture bytes; two native workflows 14/14, legacy CSV writes 3; final source/binary/tool hashes')
