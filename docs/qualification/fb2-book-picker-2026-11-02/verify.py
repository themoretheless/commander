from pathlib import Path
import hashlib,json,zipfile,xml.etree.ElementTree as ET
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==9 and all(c['exit_code']==0 for c in checks)
assert '3307 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
assert '1 passed; 0 failed' in (q/'menu-final.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
scenes={}
for scene,key in [('book_collection_preview','native-launch.json'),('zipped_fictionbook_preview','zipped_fictionbook_preview-launch.json')]:
 launch=read(q/key);assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
 d=Path(launch['out'])/scene;m=read(d/'manifest.json')
 assert m['status']=='captured' and m['error'] is None and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
 assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=1,opener=0,trash=0,free_space=0)
 assert (d/'frame.png').stat().st_size>10000;scenes[scene]=d
ns={'f':'http://www.gribuser.ru/xml/fictionbook/2.0'}
d=scenes['book_collection_preview'];selected=read(d/'selected-book.json')
with zipfile.ZipFile(d/'source.fb2.zip') as archive:
 assert archive.namelist()==['first.fb2','second.FB2']==selected['names']
 assert all(info.compress_type==zipfile.ZIP_DEFLATED for info in archive.infolist())
 first=ET.fromstring(archive.read('first.fb2').decode('cp1251'));second=ET.fromstring(archive.read('second.FB2').decode('utf-8'))
 assert first.findtext('./f:description/f:title-info/f:book-title',namespaces=ns)=='Книга'
 expected=second.findtext('./f:body/f:p',namespaces=ns)+'\n'
 assert expected=='message: Second book unique\n'
 assert selected['index']==1 and selected['selected']==expected
 assert (d/'copied-book.txt').read_bytes()==expected.encode('utf-8')
d=scenes['zipped_fictionbook_preview']
with zipfile.ZipFile(d/'source.fb2.zip') as archive:
 assert archive.namelist()==['book/BOOK.FB2','cover.png']
 xml=ET.fromstring(archive.read('book/BOOK.FB2').decode('cp1251'))
 assert xml.findtext('./f:description/f:title-info/f:book-title',namespaces=ns)=='Книга'
 expected='[Book: Книга]\nГлава первая\nmessage: café / ёж 🌍\nСтрока 1\nСтрока 2\n\nA\tB\n[Body: notes]\nNote text\n'
 assert (d/'copied-book.txt').read_bytes()==expected.encode('utf-8')
print('PASS: gates, 3307 tests, fresh menu test, source/binary hashes, two native scenes 14/14, actual selected member and exact clipboard bytes against original ZIP/XML')
