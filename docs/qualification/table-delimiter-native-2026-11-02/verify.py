from pathlib import Path
import csv,hashlib,io,json
from decimal import Decimal
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text())
h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
def rows(text,delimiter):return list(csv.reader(io.StringIO(text,newline=''),delimiter=delimiter))
def payload(records,delimiter):
 stream=io.StringIO(newline='');csv.writer(stream,delimiter=delimiter,lineterminator='\n').writerows(records);return stream.getvalue()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
for path,digest in read(q/'source-sha256.json').items():assert h(root/path)==digest,path
reports=[]
for scene,end in [('table_dialect',30),('table_ambiguous',33)]:
 launch=read(q/(scene+'-launch.json'));assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
 d=Path(launch['out'])/scene;m=read(d/'manifest.json')
 assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=2,opener=0,trash=0,free_space=0)
 source=(d/'source.csv').read_text()
 snapshots={phase:read(d/f'dialect-phase-{phase}.json') for phase in range(end+1)}
 if scene=='table_dialect':
  assert source=='Name;Count;Note\nbeta;2;"one;two|three"\nalpha;-10;"line\nbreak"\n'
  expected=rows(source,';')
  assert snapshots[3]['table']['delimiter']==59 and snapshots[3]['table']['selected']==59 and not snapshots[3]['table']['detection_error']
  assert snapshots[3]['table']['rows']==expected
  order=sorted(range(1,len(expected)),key=lambda i:(Decimal(expected[i][1]),i))
  assert snapshots[12]['table']['view']['rows']==order and snapshots[12]['table']['view']['numeric_sort']
  assert snapshots[15]['clipboard']==payload([expected[0]]+[expected[i] for i in order],';')
  reset=snapshots[21]['table'];assert reset['delimiter']==44 and reset['selected'] is None and not reset['view']['heading'] and not reset['view']['numeric_sort'] and reset['view']['sort'] is None
  assert snapshots[27]['table']['rows']==expected and snapshots[27]['table']['delimiter']==59
  assert snapshots[30]['clipboard']==payload(expected,';')
 else:
  assert source=='A,B;C\n1,2;3\n4,5;6\n'
  before=snapshots[9]['table'];failed=snapshots[12]['table']
  assert before['rows']==rows(source,',') and before['view']['heading'] and before['view']['numeric_sort']
  assert failed['detection_error'] and failed['selected'] is None and failed['delimiter']==44
  assert failed['rows']==before['rows'] and failed['view']==before['view']
  assert snapshots[15]['clipboard']==payload(rows(source,','),',')
  chosen=snapshots[21]['table'];assert chosen['delimiter']==59 and chosen['selected']==59 and chosen['rows']==rows(source,';') and not chosen['detection_error']
  assert not chosen['view']['heading'] and not chosen['view']['numeric_sort'] and chosen['view']['sort'] is None
  assert snapshots[24]['table']['detection_error'] and snapshots[24]['table']['rows']==chosen['rows'] and snapshots[24]['table']['view']==chosen['view']
  assert not snapshots[30]['table']['detection_error'] and snapshots[30]['table']['selected']==59
  assert snapshots[33]['clipboard']==payload(rows(source,';'),';')
 reports.append(dict(scene=scene,native_checks=14,phases=end,observations=len(snapshots),clipboard_calls=2))
(q/'verification.json').write_text(json.dumps(reports,indent=2)+'\n')
print('PASS: two native 14/14 captures; independent CSV cells, ordering, export bytes, detection failure preservation and same-value manual recovery')
