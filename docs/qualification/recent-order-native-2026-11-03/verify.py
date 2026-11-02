from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3367 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'recent_forget';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
s=[read(d/f'recent-phase-{phase}.json') for phase in range(28)]
folder=Path(s[2]['left']).parent;forgot=str(folder/'ForgetMe');kept=str(folder/'KeepMe')
for phase,order,visible in [(2,'Frecency',[kept,forgot]),(5,'Chronological',[forgot,kept]),(8,'Frecency',[kept,forgot]),(11,'Chronological',[forgot,kept])]:
 assert s[phase]['order']==order and s[phase]['visible']==visible and s[phase]['query']==''
 assert s[phase]['selected']==0
 assert s[phase]['paths']==[forgot,kept]
 assert s[phase]['stats']['uses'][kept]==dict(count=8,last=8)
 assert s[phase]['stats']['uses'][forgot]==dict(count=2,last=10)
 if phase!=2:assert s[phase]['saved']['order']==order
assert s[16]['query']=='ForgetMe' and s[16]['visible']==[forgot,kept]
assert s[19]['query']=='ForgetMe' and s[19]['visible']==[kept]
for phase in [19,24,27]:
 assert forgot not in s[phase]['paths'] and forgot not in s[phase]['stats']['uses']
 assert forgot not in s[phase]['saved']['paths'] and forgot not in s[phase]['saved']['stats']['uses']
 assert s[phase]['stats']['uses'][kept]==s[phase]['saved']['stats']['uses'][kept]==dict(count=8,last=8)
assert s[24]['query']==s[27]['query']=='KeepMe'
assert s[24]['order']==s[24]['saved']['order']=='Chronological'
assert s[27]['order']==s[27]['saved']['order']=='Frecency'
assert s[27]['visible']==[kept] and s[27]['selected']==0
assert all(row['left']==s[2]['left'] and row['right']==s[2]['right'] for row in s)
assert (d/'keep.txt').read_bytes()==b'kept payload'
(q/'verification.json').write_text(json.dumps(dict(native_checks=16,observations=28,native_effects=0,both_order_modes_and_saved_choice=True,removal_after_order_switch=True,peer_rank_and_source_preserved=True,no_panel_navigation=True,source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 16/16, 28 observations, both orderings and saved choice, removal with peer preservation, zero navigation/effects')
