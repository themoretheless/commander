from pathlib import Path
import csv,hashlib,io,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
c=read(q/'checks.json');assert len(c)==7 and all(x['exit_code']==0 for x in c)
assert '3343 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'copy_details';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(x['passed'] for x in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=2,opener=0,trash=0,free_space=0)
for stem,count in [('one',1),('many',12)]:
 expected=read(d/f'expected-{stem}.json');assert len(expected)==count
 expected=[["'"+cell if cell.startswith(('=','+','-','@','\t','\r')) else cell for cell in row] for row in expected]
 payload=(d/f'{stem}.tsv').read_bytes();text=payload.decode('utf-8')
 assert list(csv.reader(io.StringIO(text,newline=''),delimiter='\t'))==[['Name','Size','Modified']]+expected
 out=io.StringIO(newline='');csv.writer(out,delimiter='\t',lineterminator='\n',quoting=csv.QUOTE_ALL).writerows([['Name','Size','Modified']]+expected)
 assert payload==out.getvalue().encode('utf-8')
assert read(d/'expected-one.json')[0][0]=='README.txt'
names={r[0] for r in read(d/'expected-many.json')}
assert names=={'=SUM(1,2).txt','+cmd.txt','-1.txt','@sum.txt','\tformula.txt','\rformula.txt','Design','detail\tline\n"ёж".txt','notes.md','photo.png','quarterly-design-review-with-a-deliberately-long-filename-for-layout.txt','README.txt'}
(q/'verification.json').write_text(json.dumps(dict(native_checks=14,clipboard_calls=2,single_rows=1,selected_rows=12,oracle='Python prefix rule plus csv.reader/csv.writer QUOTE_ALL',source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 14/14; two actual TSV clipboard payloads match independent prefix/quoting rules, including all six formula markers and control/Unicode data')
