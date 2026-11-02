from pathlib import Path
import re,json,subprocess,concurrent.futures,hashlib
q=Path('docs/qualification/duplicate-json-report-2026-10-31');log=(q/'tests.log').read_text();binary=Path(re.search(r'Running unittests .* \(([^)]+)\)',log).group(1)).resolve()
cases=['panel::tests::async_navigation_records_the_visit_when_the_listing_lands','panel::tests::leaving_a_folder_before_its_listing_lands_keeps_its_remembered_cursor']
def run(item):
 i,case=item;p=subprocess.run([str(binary),'--exact',case,'--test-threads=1'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=15)
 if p.returncode:(q/f'listing-repeat-failure-{i}.log').write_bytes(p.stdout)
 return dict(attempt=i,case=case,exit_code=p.returncode)
items=list(enumerate(cases*32));
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:results=list(pool.map(run,items))
failed=sum(r['exit_code']!=0 for r in results)
(q/'listing-repeat.json').write_text(json.dumps(dict(attempts=64,parallelism=8,failed=failed,test_binary=str(binary),test_binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),results=results),indent=2)+'\n')
print('listing tests:',len(results)-failed,'/',len(results),'passed');assert failed==0
