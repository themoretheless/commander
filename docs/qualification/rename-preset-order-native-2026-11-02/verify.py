from pathlib import Path
import json,hashlib
q=Path(__file__).resolve().parent
launch=json.loads((q/'rename_presets-launch.json').read_text())
p=Path(launch['out'])/'rename_presets'
m=json.loads((p/'manifest.json').read_text())
assert m['status']=='captured' and all(x['passed'] for x in m['checks'])
assert all(value==0 for value in m['native_effect_calls'].values())
def state(phase): return json.loads((p/f'preset-phase-{phase}.json').read_text())
original=state(10)['presets'][0]['rule']
assert state(20)['presets']==[dict(name='Album',rule=original)]
assert state(29)['presets']==[dict(name='Album',rule=original),dict(name='Copy',rule=original)]
for phase,active in [(32,True),(35,False),(38,True)]:
 rows=state(phase)['presets'];assert rows[0].get('at_launch',False)==active
 assert rows[1]==dict(name='Copy',rule=original)
for phase,names in [(46,['Copy','Album']),(49,['Album','Copy']),(52,['Copy','Album']),(55,['Album','Copy'])]:
 rows=state(phase)['presets'];assert [row['name'] for row in rows]==names
 album=next(row for row in rows if row['name']=='Album');assert album['at_launch'] and album['rule']==original
 assert state(phase)['editor']==dict(name='Album',selected='Album',rule=original)
assert state(40)['editor'] is None
assert state(42)['editor']==dict(name='Album',selected='Album',rule=original)
assert launch['binary_sha256']==launch['core_sha256']
result=dict(checks=len(m['checks']),phases=56,observations=46,native_effects=0,verified=['rename','saved-copy','set-launch','clear-launch','set-again','close','keyboard-reopen-load','sort-desc','move-up','move-down','sort-asc'],frame_sha256=hashlib.sha256((p/'frame.png').read_bytes()).hexdigest())
(q/'verification.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result))
