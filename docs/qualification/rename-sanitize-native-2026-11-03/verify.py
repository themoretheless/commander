from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
c=read(q/'checks.json');assert len(c)==7 and all(x['exit_code']==0 for x in c)
assert '3347 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'rename_sanitize';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==16 and all(x['passed'] for x in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
assert (d/'source.txt').read_bytes()==b'rename payload'
snapshots=[read(d/f'sanitize-phase-{phase}.json') for phase in range(35)]
for s in snapshots:
 assert s['editor']==s['preview']['rule']
for phase,replacement,target,status,applicable,error in [
 (5,None,'folder/sanitize.txt','Invalid',False,False),
 (8,'_','folder_sanitize.txt','Ok',True,False),
 (13,'-','folder-sanitize.txt','Ok',True,False),
 (18,'/','folder/sanitize.txt','Invalid',False,True),
 (23,'','foldersanitize.txt','Ok',True,False),
 (28,'_','folder_sanitize.txt','Ok',True,False),
 (31,None,'folder/sanitize.txt','Invalid',False,False),
 (34,'_','folder_sanitize.txt','Ok',True,False),
]:
 s=snapshots[phase];rule=s['editor'];preview=s['preview']
 assert rule['prefix']=='folder/' and rule.get('invalid_char_replacement')==replacement
 plan=preview['plans'];assert len(plan)==1 and plan[0]['from']=='sanitize.txt' and plan[0]['to']==target and plan[0]['status']==status
 assert preview['applicable']==applicable and bool(preview['rule_error'])==error
 if replacement is not None and not error:assert target==('folder/'+'sanitize.txt').replace('/',replacement)
(q/'verification.json').write_text(json.dumps(dict(native_checks=16,observations=35,key_states=8,native_effects=0,source_bytes_unchanged=True,oracle='Python literal string replacement and fixed refusal states',source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 16/16, 35 snapshots, eight independently verified editor/preview states, preserved source bytes and zero native-port effects')
