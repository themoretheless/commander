from pathlib import Path
import csv,hashlib,io,json,re
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==6 and all(x['exit_code']==0 for x in checks)
assert '3297 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'table_preview';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(x['passed'] for x in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=2,opener=0,trash=0,free_space=0)
source='Name,Count\nfile10,2\nfile2,10\nfile2,1\nёж,3\n'
assert '"'+source.replace('\n','\\n')+'"' in (root/'src/visual_qa.rs').read_text()
rows=list(csv.reader(io.StringIO(source)));natural=lambda text:[int(p) if p.isdigit() else p for p in re.split(r'(\d+)',text)]
order=sorted(range(1,len(rows)),key=lambda i:(natural(rows[i][0]),i))
state=dict(query='fl2',fuzzy=True,regex=False,whole_words=False,column=None,exclude=False)
changes=[{},dict(fuzzy=False,regex=True),dict(query=''),dict(query='^file2$'),dict(exclude=True),{},dict(column=1),dict(exclude=False),{},dict(column=None),dict(whole_words=True),dict(query=''),dict(query='['),dict(exclude=True),dict(query=''),dict(query='file2'),dict(exclude=False)]
for stage,change in enumerate(changes):
 state.update(change);actual=read(d/f'filter-stage-{stage}.json')
 assert all(actual[key]==value for key,value in state.items()),(stage,actual,state)
 assert actual['heading'] is True
 query=state['query'].strip();error=False
 try:pattern=re.compile((r'\b(?:'+query+r')\b') if state['whole_words'] else query,re.IGNORECASE) if state['regex'] and query else None
 except re.error:error=True;pattern=None
 def matches(cell):
  if pattern:return bool(pattern.search(cell))
  if state['fuzzy']:
   chars=iter(cell.casefold());return all(any(c==wanted for c in chars) for wanted in query.casefold())
  return query.casefold() in cell.casefold()
 expected=[] if error else [i for i in order if not query or any(matches(cell) for j,cell in enumerate(rows[i]) if state['column'] is None or state['column']==j)!=state['exclude']]
 assert actual['rows']==expected,(stage,actual['rows'],expected)
 assert bool(actual['error'])==error
assert read(d/'filter-stage-16.json')['rows']==[2,3]
print('PASS: six gates, source/binary hashes, native 14/14, exact modes and independent row predicate for all 17 stages')
