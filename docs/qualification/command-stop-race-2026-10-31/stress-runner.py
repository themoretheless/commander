from pathlib import Path
from concurrent.futures import ThreadPoolExecutor,as_completed
import subprocess,json,re,time,os,signal,hashlib
q=Path('docs/qualification/command-stop-race-2026-10-31');log=(q/'process-tests.log').read_text();binary=re.search(r'Running unittests .* \(([^)]+)\)',log).group(1);case='app::run_command_dialog::per_item_process_tests::stopping_live_per_item_run_kills_current_group_and_leaves_later_paths_untouched'
start=time.monotonic()
def run(i):
 p=subprocess.run([binary,case,'--exact','--nocapture'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=12)
 if p.returncode:
  (q/f'stress-failure-{i}.log').write_text(p.stdout)
  match=re.search(r'stopped shell (\d+) did not report',p.stdout)
  if match:
   group=int(match.group(1));ps=subprocess.run(['ps','-Ao','pid,ppid,pgid,stat,command'],capture_output=True,text=True).stdout
   members=[line for line in ps.splitlines() if len(line.split())>=5 and line.split()[2]==str(group)]
   (q/f'stress-failure-{i}-owned-group.log').write_text('\n'.join(members)+'\n')
   if members and all('sleep 30' in line or 'printf done >' in line for line in members):
    try:os.killpg(group,signal.SIGKILL)
    except ProcessLookupError:pass
 return dict(attempt=i,exit_code=p.returncode)
results=[]
with ThreadPoolExecutor(max_workers=8) as pool:
 for f in as_completed([pool.submit(run,i) for i in range(256)]):
  results.append(f.result())
(q/'stress-after.json').write_text(json.dumps(dict(attempts=256,parallel=8,test_binary_sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest(),case=case,elapsed=time.monotonic()-start,failed=sum(r['exit_code']!=0 for r in results),results=results),indent=2)+'\n');print('attempts',len(results),'failed',sum(r['exit_code']!=0 for r in results),flush=True)
