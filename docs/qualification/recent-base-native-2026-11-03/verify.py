from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3374 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'recent_scope';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
s=[read(d/f'scope-phase-{phase}.json') for phase in range(37)]
left=s[2]['left'];right=s[2]['right'];inside=str(Path(left)/'Design');outside=str(Path(right)/'Archive');both=[outside,inside]
for phase,base,enabled,query,visible in [(2,left,False,'',both),(5,left,True,'',[inside]),(8,left,False,'',both),(11,left,True,'',[inside]),(17,right,False,'',both),(20,right,True,'',[outside]),(25,right,True,'arch',[outside]),(28,right,False,'arch',[outside]),(33,right,False,' ',both),(36,right,True,' ',[outside])]:
 row=s[phase];assert row['state']['base']==base and row['state']['enabled']==enabled
 assert row['state']['query']==query and row['visible']==visible and row['state']['selected']==0
 if enabled:
  assert all(Path(path).is_relative_to(base) for path in visible)
assert s[13]['state'] is None and s[15]['active']=='Right'
assert all(row['paths']==both and row['stats']==s[2]['stats'] for row in s)
assert all(row['left']==left and row['right']==right for row in s)
(q/'verification.json').write_text(json.dumps(dict(native_checks=16,observations=37,native_effects=0,scope_toggle_reopen_reset_and_cross_pane_capture=True,history_and_rank_preserved=True,no_folder_navigation=True,source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 16/16, 37 observations, scope toggle, close-while-enabled/reopen reset, new pane base capture, stable history/rank and no folder navigation/effects')
