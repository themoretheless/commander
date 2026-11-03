from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
checks=json.loads((q/'checks.json').read_text());assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3553 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
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
drag_before=json.loads((out/'drag-before.json').read_text());assert {k:v for k,v in drag_before.items() if k!='preview'}==g['after'] and drag_before['preview'] is False
for name,order,active in [('drag-inactive.json',[0,3,1,2],0),('drag-active.json',[3,1,2,0],3)]:
 d=json.loads((out/name).read_text());assert d['before']==d['after']==drag_before
 assert d['tabs']['active']==active and d['tabs']['entries']==[g['tabs']['entries'][i] for i in order]
 assert d['tabs'].get('closed',[])==g['tabs'].get('closed',[]) and d['right']==g['right']
for phase in [58,60,62,64]:
 k=json.loads((out/f'keyboard-{phase}.json').read_text());assert k['mode']=='Miller'
cross_before=json.loads((out/'cross-before.json').read_text())
inactive=json.loads((out/'cross-inactive.json').read_text())
active=json.loads((out/'cross-active.json').read_text())
restored=json.loads((out/'cross-restored.json').read_text())
ordered=json.loads((out/'drag-active.json').read_text())['tabs']['entries']
original_right={'path':g['right'],'view':{'mode':'List'}}
assert cross_before['left']==drag_before
assert inactive['left']==cross_before['left'] and inactive['right']==cross_before['right']
assert inactive['left_tabs']['entries']==ordered[1:] and inactive['left_tabs']['active']==2
assert inactive['right_tabs']['entries'][0]==ordered[0] and inactive['right_tabs']['active']==1
assert active['left_tabs']['entries']==ordered[1:3] and active['left_tabs']['active']==1
assert active['right_tabs']['entries']==inactive['right_tabs']['entries']+[ordered[3]] and active['right_tabs']['active']==1
assert active['right']==cross_before['right'] and active['focus']=='Left'
assert active['left']['path']==str(owner.parent/'column-B') and active['left']['cursor']==str(owner.parent/'column-B/a.txt')
assert inactive['focus']=='Left'
assert restored['left_tabs']==active['left_tabs'] and restored['right_tabs']['entries']==active['right_tabs']['entries']
assert restored['right_tabs']['active']==2 and restored['focus']=='Right'
assert restored['left']==active['left']
for key in ['path','cursor','selected','preview']:assert restored['right'][key]==cross_before['left'][key],key
assert len(restored['right']['selected'])==3
assert all(x['left_tabs'].get('closed',[])==g['tabs'].get('closed',[]) for x in [inactive,active,restored])
assert all(x['right_tabs'].get('closed',[])==[] for x in [inactive,active,restored])
retries=out/'initial-mode-retries.json'
if retries.exists():assert 1<=json.loads(retries.read_text())<=2
pin_before=json.loads((out/'pin-before.json').read_text())
pin_away=json.loads((out/'pin-away.json').read_text())
pin_restored=json.loads((out/'pin-restored.json').read_text())
home=str(owner.parent)
for r in [pin_before,pin_away,pin_restored]:
 assert r['pin']==home and r['tabs']['active']==2 and r['focus']=='Right'
 assert r['tabs']['entries'][2]['pinned_path']=={'platform':'unix','path':list(home.encode())}
 assert r['tabs']['entries'][:2]==restored['right_tabs']['entries'][:2]
for r in [pin_before,pin_restored]:
 for key in ['path','cursor','selected']:assert r[key]==restored['right'][key],key
assert pin_restored==pin_before
assert pin_away['path']==str(owner.parent/'Design')
assert pin_away['tabs']['entries'][2]['path']==pin_away['path']
assert pin_away['selected']==[]
expected=restored['right_tabs']['entries'][2].copy();expected['pinned_path']=pin_before['tabs']['entries'][2]['pinned_path']
assert pin_before['tabs']['entries'][2]==expected
creation_before=json.loads((out/'opposite-before.json').read_text())
created=json.loads((out/'opposite-created.json').read_text())
renamed=json.loads((out/'opposite-renamed.json').read_text())
base=owner.parent/'column-B'
assert creation_before['source']['path']==home and creation_before['source']['selected']==pin_restored['selected']
assert created['source']==creation_before['source']
assert created['created'] is True and created['renamed'] is False and created['rename']==str(base/'New Folder')
assert renamed['created'] is False and renamed['renamed'] is True and renamed['rename'] is None
for r in [creation_before,created,renamed]:
 assert r['focus']=='Right' and not r['source_created'] and not r['source_renamed']
 assert r['destination']['path']==str(base)
 for key in ['path','cursor','selected']:assert r['source'][key]==creation_before['source'][key],key
invalid=json.loads((out/'quote-invalid.json').read_text())
double=json.loads((out/'quote-double.json').read_text())
single=json.loads((out/'quote-single-relative.json').read_text())
assert invalid['status']=='Error(UnclosedQuote)' and invalid['path']==home
assert invalid['input'].startswith('"') and not invalid['input'].endswith('"')
assert invalid['probe_input'].startswith('"')
assert double['path']==double['canonical_target'] and Path(double['path']).name==' space dir '
assert Path(double['path']).parent.name=='Design'
assert double['submitted']=='"'+str(owner.parent/'Design/ space dir ')+'"'
assert single['path']==single['canonical_relative'] and single['submitted']=="'../../column-A'"
assert single['path']==str(Path(double['path']).parent.parent/'column-A')
for r in [invalid,double,single]:assert r['left']==str(base) and r['focus']=='Right' and r['target_exists']
assert double['input'] is None and single['input'] is None
source=Path(single['path']);parent=source.parent
previous=None
for mode,count in [('List',4),('Icons',5),('Miller',6)]:
 r=json.loads((out/f'parent-middle-{mode}.json').read_text());before=json.loads((out/f'parent-before-{mode}.json').read_text())
 assert r['before']==r['after']==before and before['path']==str(source) and before['mode']==mode
 assert before['cursor']==str(source/'a.txt') and before['selected']==[str(source/'a.txt')]
 assert r['tabs']['active']==2 and len(r['tabs']['entries'])==count and r['tabs']['entries'][3]['path']==str(parent)
 assert r['tabs']['entries'][3]['view']['mode']==mode and r['focus']=='Right'
 if previous:
  assert r['tabs']['entries'][3:]==[r['tabs']['entries'][3]]+previous['tabs']['entries'][3:]
  assert r['left']==previous['left']
 previous=r
foreground=json.loads((out/'parent-menu-foreground.json').read_text())
background=json.loads((out/'parent-menu-background.json').read_text())
assert foreground['path']==str(parent) and foreground['tabs']['active']==3 and len(foreground['tabs']['entries'])==7
assert foreground['tabs']['entries'][:3]==previous['tabs']['entries'][:3]
assert foreground['tabs']['entries'][4:]==previous['tabs']['entries'][3:]
assert foreground['tabs']['entries'][3]['path']==str(parent) and foreground['tabs']['entries'][3]['view']['mode']=='Miller'
assert background['path']==str(parent) and background['tabs']['active']==3 and len(background['tabs']['entries'])==8
assert background['tabs']['entries'][4]['path']==str(parent.parent)
assert background['tabs']['entries'][:4]==foreground['tabs']['entries'][:4]
assert background['tabs']['entries'][5:]==foreground['tabs']['entries'][4:]
assert foreground['left']==background['left']==previous['left'] and foreground['focus']==background['focus']=='Right'
assert background['tabs'].get('closed',[])==foreground['tabs'].get('closed',[])==previous['tabs'].get('closed',[])
before=json.loads((out/'tabs-picker-before.json').read_text())
all_tabs=json.loads((out/'tabs-picker-all.json').read_text())
cancel=json.loads((out/'tabs-picker-cancel.json').read_text())
query=json.loads((out/'tabs-picker-query.json').read_text())
activated=json.loads((out/'tabs-picker-activated.json').read_text())
assert before==cancel and before['focus']=='Right'
assert all_tabs['input']=='#' and all_tabs['selected']==5
assert len(all_tabs['entries'])==len(all_tabs['matches'])==10
assert all_tabs['entries']==all_tabs['matches']
assert [(x['side'],x['index']) for x in all_tabs['entries']]==[('Left',i) for i in range(2)]+[('Right',i) for i in range(8)]
assert sum(x['focused'] for x in all_tabs['entries'])==1 and sum(x['current'] for x in all_tabs['entries'])==2
assert query['input']=='#Workspace Alpha' and len(query['matches'])==1 and query['selected']==0
chosen=query['matches'][0]
assert chosen['side']=='Left' and chosen['index']==0 and chosen['name']=='Workspace Alpha'
assert chosen['path']==str(owner) and chosen==all_tabs['entries'][0]
assert activated['focus']=='Left' and activated['left']==chosen['path'] and activated['left_tabs']['active']==0
assert activated['right']==before['right'] and activated['right_tabs']==before['right_tabs']
assert activated['left_tabs']['entries']==before['left_tabs']['entries']
right=json.loads((out/'tabs-picker-right.json').read_text())
left=json.loads((out/'tabs-picker-left.json').read_text())
assert all_tabs['scope']==query['scope']=='Both panes'
for record,side,count in [(right,'Right',8),(left,'Left',2)]:
 assert record['input']=='#' and record['scope']==side+' pane' and record['selected']==0
 assert record['entries']==all_tabs['entries'] and len(record['matches'])==count
 assert record['matches']==[e for e in all_tabs['entries'] if e['side']==side]
creation_before=json.loads((out/'empty-file-before.json').read_text())
created=json.loads((out/'empty-file-created.json').read_text())
renamed=json.loads((out/'empty-file-renamed.json').read_text())
assert created['focus']==renamed['focus']==creation_before['focus']=='Left'
assert created['right']==renamed['right']==creation_before['right']
assert created['right_tabs']==renamed['right_tabs']==creation_before['right_tabs']
assert created['left_tabs']==renamed['left_tabs']==creation_before['left_tabs']
assert created['rename_path']==str(owner/'New File 1.txt') and created['bytes']==0
assert renamed['rename_path'] is None and renamed['path']==str(owner/'Draft.txt') and renamed['bytes']==0
assert created['collision']==renamed['collision']=='existing document'
assert not renamed['old_exists'] and renamed['cursor']==renamed['path']
retries=out/'initial-entry-retries.json'
if retries.exists():assert 1<=json.loads(retries.read_text())<=2
(q/'verification.json').write_text(json.dumps(dict(checks=7,tests_passed=3553,tests_ignored=3,native_checks=14,new_empty_file_created=True,new_empty_file_renamed=True,collision_preserved=True,tab_choices=10,pane_scopes=["Both panes","Right pane","Left pane"],scope_reset_on_reopen=True,cancel_preserves_state=True,title_search=True,cross_pane_activation=True,source_hashes_verified=True,prior_receipts_verified=True),indent=2)+'\n')
print('Verified seven gates, empty-file create/rename, collision preservation and every preceding receipt')
