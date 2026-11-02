from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/diff-source-goto-2026-10-31');launch=json.loads((q/'native-launch.json').read_text());out=Path(launch['out']);src=out/'diff_navigation';m=json.loads((src/'manifest.json').read_text());s=json.loads((src/'diff-state.json').read_text())
assert m['status']=='captured' and len(m['checks'])==15 and all(c['passed'] for c in m['checks']);assert all(c==0 for c in m['native_effect_calls'].values())
assert s['current']==2 and s['goto_selected']==11 and 11 in s['visible']
assert [(h['rows'],h['left_start'],h['left_len'],h['right_start'],h['right_len']) for h in s['hunks']]==[([10,12],11,1,11,1),([91,93],91,1,91,1),([162,164],161,1,161,1)]
assert len(s['rows'])==183
assert sum(row['kind']=='Insert' for row in s['rows'])==sum(row['kind']=='Delete' for row in s['rows'])==3
assert '(right)' in s['minus_name'] and '(left)' in s['plus_name']
assert '\n'.join(row['text'] for row in s['rows'] if row['kind']!='Insert')+'\n'==s['right_text']
assert '\n'.join(row['text'] for row in s['rows'] if row['kind']!='Delete')+'\n'==s['left_text']
assert len(s['left_text'].splitlines())==len(s['right_text'].splitlines())==180
compact=json.loads((src/'compact.json').read_text());expanded=json.loads((src/'expanded.json').read_text());last=json.loads((src/'compact-last.json').read_text())
assert compact['hidden']==159 and len(compact['rows'])==28 and compact['current']==1
assert expanded['hidden']==last['hidden']==86 and len(expanded['rows'])==len(last['rows'])==100 and last['current']==2
assert any(row.get('gap')==[15,88] for row in compact['rows']) and not any(row.get('gap')==[15,88] for row in expanded['rows'])
assert all(any(row.get('line')==index for row in expanded['rows']) for index in range(15,88))
for snap in [compact,expanded,last]:
 lines=[row['line'] for row in snap['rows'] if 'line' in row];gaps=[row['gap'] for row in snap['rows'] if 'gap' in row]
 assert len(set(lines))==len(lines) and sorted(lines)==lines
 assert sum(end-start for start,end in gaps)==snap['hidden'] and len(lines)+snap['hidden']==183
 assert all(index in lines for index in [10,11,91,92,162,163])
 assert all(s['rows'][i]['kind']=='Equal' for start,end in gaps for i in range(start,end))
assert s['compact'] is True and s['context_lines']==3 and s['display_rows']==100 and s['hidden']==86
full=json.loads((src/'full.json').read_text());assert full['hidden']==0 and full['current']==2 and full['rows']==[dict(line=i) for i in range(183)]
assert s['show_numbers'] is True
minus=plus=1
expected=[]
for row in s['rows']:
 a=minus if row['kind']!='Insert' else None
 b=plus if row['kind']!='Delete' else None
 expected.append([a,b]);minus+=a is not None;plus+=b is not None
assert s['source_numbers']==expected
assert expected[162:164]==[[161,None],[None,161]]
assert s['find_query']=='context 50' and s['find_rows']==[51] and s['find_focused'] is False
for name in ['find-hidden.json','find-wrap.json','find-previous.json']:
 snap=json.loads((src/name).read_text());assert snap['query']=='context 50' and snap['rows']==[51] and snap['selected']==51 and 51 in snap['visible'] and snap['hidden']==86
assert s['goto_plus'] is True and s['goto_query']=='11' and s['goto_selected']==11
assert s['source_numbers'][11]==[None,11] and s['rows'][11]['kind']=='Insert'
goto=json.loads((src/'goto-minus.json').read_text());assert goto['query']=='51' and goto['plus'] is False and goto['selected']==51 and 51 in goto['visible'] and goto['hidden']==86
assert s['source_numbers'][51]==[51,51]
h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest();assert launch['binary_sha256']==launch['core_sha256']==h('target/debug/commander')==h('target/CommanderFolderQA.app/Contents/MacOS/CommanderFolderQA-core')
shutil.copytree(src,q/'native',dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/'native.log')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',checks=15,difference_blocks=3,current_block=3,compact_rows=100,compact_hidden=86,expanded_rows=100,expanded_hidden=86,full_view_restored=True,visible_jump_row=True,source_reconstruction=True,native_effect_calls=0,binary_matches_launch=True),indent=2)+'\n')
files=['src/diff_goto.rs','src/diff_find.rs','src/diff_context.rs','src/diff_navigation.rs','src/app/diff_dialog.rs','src/app/keys.rs','src/app/mod.rs','src/app/update.rs','src/app/ui_state.rs','src/main.rs','src/visual_qa.rs','.github/workflows/ci.yml'];(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in files},indent=2)+'\n')
print('Native block positions, source reconstruction and launch hashes verified')
