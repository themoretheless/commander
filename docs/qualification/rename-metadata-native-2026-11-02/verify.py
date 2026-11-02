from pathlib import Path
import json,hashlib
from datetime import datetime,timezone
q=Path(__file__).resolve().parent
launch=json.loads((q/'rename_presets-launch.json').read_text())
p=Path(launch['out'])/'rename_presets'
m=json.loads((p/'manifest.json').read_text())
assert m['status']=='captured' and all(x['passed'] for x in m['checks'])
assert all(value==0 for value in m['native_effect_calls'].values())
def raw(phase): return json.loads((p/f'preset-phase-{phase}.json').read_text())
def state(phase):
 d=raw(phase)
 if d['editor'] is not None:d['editor'].pop('discard_prompt',None)
 return d
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
for phase in [65,71]:
 assert raw(phase)['editor']['discard_prompt']
 assert raw(phase)['editor']['rule']['prefix']=='Edited_'
assert not raw(68)['editor']['discard_prompt']
assert raw(68)['editor']['rule']['prefix']=='Edited_'
assert raw(74)['editor'] is None
assert raw(77)['editor']['rule']==original and not raw(77)['editor']['discard_prompt']
assert raw(77)['presets']==raw(55)['presets']
before={item['name']:item for item in json.loads((p/'metadata-files-78.json').read_text())}
assert len(before)==4 and all(item['bytes'] for item in before.values())
assert before['notes.md']['bytes']==list(b'# Notes\n- deterministic\n')
assert before['README.txt']['bytes']==list(b'Commander visual QA fixture\n')
after={item['name']:item for item in json.loads((p/'metadata-files-105.json').read_text())}
restored={item['name']:item for item in json.loads((p/'metadata-files-110.json').read_text())}
preview=raw(99)['preview'];assert preview['applicable']
assert preview['rule']['date_stamp']==dict(format='%Y_',position='Prefix')
assert preview['rule']['parent_affix']==dict(depth=1,separator='--',suffix=True)
folder=Path(raw(99)['folder']).name
assert len(preview['plans'])==2
expected_after=dict(before)
for index,row in enumerate(preview['plans']):
 source=row['from'];facts=before[source]
 assert raw(99)['modified'][source]==facts['modified']
 stem,extension=source.rsplit('.',1)
 year=datetime.fromtimestamp(facts['modified'],timezone.utc).strftime('%Y_')
 expected=f'{year}Photo_{stem}--{folder}{index+1:02d}.{extension}'
 assert row['to']==expected and row['status']=='Ok'
 assert source not in after and after[expected]['bytes']==facts['bytes'] and after[expected]['modified']==facts['modified']
 expected_after.pop(source);expected_after[expected]=dict(facts,name=expected)
assert after==expected_after
assert restored==before
assert raw(105)['editor'] is None
assert raw(110)['editor'] is None
assert launch['binary_sha256']==launch['core_sha256']
result=dict(checks=len(m['checks']),phases=111,observations=101,native_effects=0,verified=['rename','saved-copy','set-launch','clear-launch','set-again','close','keyboard-reopen-load','sort-desc','move-up','move-down','sort-asc','dirty-escape-guard','keep-editing','discard-close','reopen-saved-rule','date-input','folder-suffix-input','cached-preview','filesystem-apply','undo-restores-bytes'],frame_sha256=hashlib.sha256((p/'frame.png').read_bytes()).hexdigest())
(q/'verification.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result))
