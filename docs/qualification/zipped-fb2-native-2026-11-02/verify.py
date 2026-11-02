from pathlib import Path
import hashlib,json,zipfile,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(x['exit_code']==0 for x in checks)
assert '3302 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for path,digest in read(q/'source-sha256.json').items():assert h(root/path)==digest,path
scenes={}
for name,receipt in [('zipped_fictionbook_preview','native-launch.json'),('fictionbook_preview','fictionbook_preview-launch.json')]:
 launch=read(q/receipt);assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
 d=Path(launch['out'])/name;m=read(d/'manifest.json')
 assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=1,opener=0,trash=0,free_space=0)
 assert (d/'frame.png').stat().st_size>10000;scenes[name]=d
z=scenes['zipped_fictionbook_preview']
with zipfile.ZipFile(z/'source.fb2.zip') as archive:
 assert archive.namelist()==['book/BOOK.FB2','cover.png']
 assert archive.getinfo('book/BOOK.FB2').compress_type==zipfile.ZIP_DEFLATED
 raw=archive.read('book/BOOK.FB2');assert b'encoding="windows-1251"' in raw
 xml=ET.fromstring(raw.decode('cp1251'));ns={'f':'http://www.gribuser.ru/xml/fictionbook/2.0'}
 assert xml.tag=='{'+ns['f']+'}FictionBook'
 title=xml.findtext('./f:description/f:title-info/f:book-title',namespaces=ns)
 bodies=xml.findall('./f:body',ns);assert len(bodies)==2 and bodies[1].attrib['name']=='notes'
 section=bodies[0].find('./f:section',ns)
 chapter=section.findtext('./f:title/f:p',namespaces=ns)
 message=section.findtext('./f:p',namespaces=ns)
 verses=[e.text for e in section.findall('./f:poem/f:stanza/f:v',ns)]
 cells=[e.text for e in section.findall('./f:table/f:tr/f:td',ns)]
 note=bodies[1].findtext('./f:section/f:p',namespaces=ns)
 expected=f'[Book: {title}]\n{chapter}\n{message}\n'+''.join(v+'\n' for v in verses)+'\n'+'\t'.join(cells)+'\n[Body: notes]\n'+note+'\n'
 assert expected=='[Book: Книга]\nГлава первая\nmessage: café / ёж 🌍\nСтрока 1\nСтрока 2\n\nA\tB\n[Body: notes]\nNote text\n'
 assert (z/'copied-book.txt').read_bytes()==expected.encode('utf-8')
 assert 'binary secret' not in expected and 'metadata secret' not in expected
print('PASS: seven gates, 3302 tests, source/binary hashes, ZIP/raw native 14/14, independently decoded original ZIP and exact actual clipboard text')
