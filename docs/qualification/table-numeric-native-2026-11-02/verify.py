from pathlib import Path
import csv,hashlib,io,json,re
from decimal import Decimal
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(x['exit_code']==0 for x in checks)
assert '3329 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'table_preview';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(x['passed'] for x in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=5,opener=0,trash=0,free_space=0)
source=(d/'source.csv').read_text()
assert source=='Name,Count\nfile10,-10\nfile2,-2\nfile2,9007199254740993\nёж,9007199254740992\n'
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
numeric_asc=sorted(range(1,len(rows)),key=lambda i:(Decimal(rows[i][1]),i))
numeric_desc=sorted(range(1,len(rows)),key=lambda i:(-Decimal(rows[i][1]),i))
text_key=lambda i:(0,int(rows[i][1][1:])) if rows[i][1].startswith('-') else (1,int(rows[i][1]))
text_asc=sorted(range(1,len(rows)),key=text_key)
text_desc=sorted(range(1,len(rows)),key=text_key,reverse=True)
expected_states=[(100,[2,3],False),(103,[2,3,1,4],False),(106,[1,2,3,4],True),(109,numeric_asc,True),(112,numeric_asc,True),(115,numeric_desc,True),(118,numeric_desc,True),(121,text_desc,False),(124,[1,2,3,4],False),(127,text_asc,False),(130,numeric_asc,True),(133,numeric_asc,True)]
for phase,expected,numeric in expected_states:
 actual=read(d/f'numeric-phase-{phase}.json')['view']
 assert actual['rows']==expected and actual['numeric_sort']==numeric,(phase,actual,expected)
 if phase>100:assert actual['query']=='' and actual['heading'] is True
for phase,expected in [(112,numeric_asc),(118,numeric_desc),(133,numeric_asc)]:
 payload=read(d/f'numeric-phase-{phase}.json')['clipboard']
 assert list(csv.reader(io.StringIO(payload)))==[rows[0]]+[rows[i] for i in expected]
(q/'verification.json').write_text(json.dumps(dict(native_checks=14,phases=133,clipboard_calls=5,numeric_ascending=numeric_asc,numeric_descending=numeric_desc,states=len(expected_states),old_filter_stages=17),indent=2)+'\n')
print('PASS: seven gates, exact Decimal oracle, 17 existing filter stages, 12 numeric observations and 3 actual CSV exports, native 14/14')
