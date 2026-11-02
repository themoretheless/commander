from pathlib import Path
import json,hashlib,shutil
q=Path('docs/qualification/broken-symlink-search-2026-10-31');load=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest();binary=h('target/debug/commander')
for key,dest,scene,count in [('native-launch.json','native','broken_link_search',16),('symlink_info-launch.json','native-info','symlink_info',14)]:
 launch=load(q/key);assert launch['binary_sha256']==launch['core_sha256']==binary
 out=Path(launch['out']);src=out/scene;m=load(src/'manifest.json')
 assert m['status']=='captured' and len(m['checks'])==count and all(c['passed'] for c in m['checks'])
 assert all(v==0 for v in m['native_effect_calls'].values())
 shutil.copytree(src,q/dest,dirs_exist_ok=True);shutil.copy2(str(out)+'.log',q/(dest+'.log'))
base=load(q/'native/links-4.json');root=Path(base['root'])
for name,phase,expression,want,skipped in [
 ('links-4.json',4,'type:broken-link archives:off',{'broken.lnk','chain.lnk','notdir.lnk'},1),
 ('links-7.json',7,'types:broken-link,symlink archives:off',{'broken.lnk','chain.lnk','notdir.lnk','valid.lnk','directory.lnk','loop.lnk'},0),
 ('links-final.json',10,'type:broken-link archives:off',{'notdir.lnk'},1),
]:
 s=load(q/'native'/name)
 assert s['phase']==phase and s['root']==str(root) and s['expression']==expression and s['skipped']==skipped
 assert s['matched']==len(want) and not s['truncated'] and s['source']=='Live (archives off)'
 assert set(s['paths'])=={str(root/p) for p in want}
 assert all(details==[expression.split()[0],'archives:off'] for details in s['explanations'])
assert all(f'Broken-link search QA phase {phase}\n' in (q/'native.log').read_text() for phase in range(10))
info=load(q/'native-info/info-state.json');assert info['kind']=='Symlink to TXT file' and info['link_status']=='Target exists' and info['path'].endswith('/left/link.txt -> ../right/portable.txt') and info['size']=='16 B'
checks=load(q/'checks.json');assert len(checks)==6 and all(c['exit_code']==0 for c in checks)
assert '3077 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
(q/'source-sha256.json').write_text(json.dumps({p:h(p) for p in ['src/query.rs','src/search.rs','src/content_index.rs','src/fs_util.rs','src/panel/link_info.rs','src/app/find_dialog.rs','src/visual_qa.rs','.github/workflows/ci.yml']},indent=2)+'\n')
(q/'native-assertions.json').write_text(json.dumps(dict(status='passed',search_native_checks=16,link_info_regression_checks=14,input_phases=10,exact_path_sets=True,live_refresh_after_target_repair=True,loop_unknown_in_partial_count=True,known_or_branch_includes_loop=True,canonical_explanations=True,all_effect_calls=0,binary_matches_launch=True,full_tests_passed=3077,ignored=3,checks=6),indent=2)+'\n')
print('16 search + 14 Info native checks; exact result sets, partial status, repair refresh and six checks verified')
