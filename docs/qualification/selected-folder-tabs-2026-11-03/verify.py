from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3525 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,h in json.loads((q/'source-sha256.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==h,p
launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out'])/'miller_browser'
assert launch['binary_sha256']==launch['core_sha256']==hashlib.sha256((root/'target/debug/commander').read_bytes()).hexdigest()
m=json.loads((out/'manifest.json').read_text());assert m['status']=='captured' and len(m['checks'])==14 and all(c['passed'] for c in m['checks'])
assert all(v==0 for v in m['native_effect_calls'].values())
r=json.loads((out/'background-tab.json').read_text());before=json.loads((out/'background-before.json').read_text());assert r['before']==r['after']==before
owner=Path(before['path']);assert owner.name=='column-A' and before['cursor']==str(owner/'a.txt') and before['selected']==[str(owner/'a.txt')]
assert r['tabs']['active']==0 and [i['path'] for i in r['tabs']['entries']]==[str(owner),str(owner.parent/'column-B')]
assert r['right']==str(owner.parent.parent/'right')
assert all(i['view']['mode']=='Miller' for i in r['tabs']['entries'])
assert (out/'source-A.txt').read_bytes()==(out/'preview.txt').read_bytes()==b'Miller A owner\n'
assert (out/'source-B.txt').read_bytes()==b'Miller B owner\n'
bg=json.loads((out/'menu-background.json').read_text());assert bg['owner']==before and bg['tabs']['active']==0
assert [i['path'] for i in bg['tabs']['entries']]==[str(owner),str(owner.parent/'column-B'),str(owner.parent/'column-B')]
fg=json.loads((out/'menu-foreground.json').read_text());assert fg['tabs']['active']==1 and fg['path']==str(owner.parent/'column-B') and fg['cursor']==str(owner.parent/'column-B/a.txt')
assert [i['path'] for i in fg['tabs']['entries']]==[str(owner)]+[str(owner.parent/'column-B')]*3
assert fg['right']==r['right']
for mode,count in [('Miller',5),('List',6),('Icons',7)]:
 c=json.loads((out/f'current-middle-{mode}.json').read_text());saved=json.loads((out/f'current-before-{mode}.json').read_text())
 assert c['before']==c['after']==saved and saved['mode']==mode and saved['path']==str(owner.parent)
 assert c['tabs']['active']==1 and len(c['tabs']['entries'])==count and c['tabs']['entries'][2]['path']==str(owner.parent/'Design')
 assert c['right']==r['right'] and c['tabs']['entries'][0]['path']==str(owner)
g=json.loads((out/'selected-folder-tabs.json').read_text());group_before=json.loads((out/'group-before.json').read_text())
assert g['before']==g['after']==group_before and group_before['path']==str(owner.parent)
expected=[str(owner),str(owner.parent/'column-B'),str(owner.parent/'Design')]
assert group_before['selected']==expected and group_before['cursor']==expected[-1]
assert g['tabs']['active']==0 and [i['path'] for i in g['tabs']['entries']]==[str(owner.parent)]+expected and g['right']==r['right']
for phase in [58,60,62,64]:
 k=json.loads((out/f'keyboard-{phase}.json').read_text());assert k['mode']=='Miller'
(q/'verification.json').write_text(json.dumps(dict(checks=7,tests_passed=3525,tests_ignored=3,native_checks=14,background_tab_owner_preserved=True,menu_background_verified=True,menu_foreground_verified=True,current_middle_modes=3,selected_folder_group_verified=True,keyboard_checkpoints=4,source_hashes_verified=True),indent=2)+'\n')
print('Verified seven gates, hashes, palette selected-folder group, preserved owner and prior Miller/menu/view receipts')
