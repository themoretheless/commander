from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent;root=q.parents[2]
read=lambda p:json.loads(p.read_text());h=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
checks=read(q/'checks.json');assert len(checks)==7 and all(c['exit_code']==0 for c in checks)
assert '3471 passed; 0 failed; 3 ignored' in (q/'tests.log').read_text()
for p,digest in read(q/'source-sha256.json').items():assert h(root/p)==digest,p
launch=read(q/'native-launch.json');assert launch['binary_sha256']==launch['core_sha256']==h(root/'target/debug/commander')
d=Path(launch['out'])/'recent_forget';m=read(d/'manifest.json')
assert m['status']=='captured' and m['error'] is None and len(m['checks'])==16 and all(c['passed'] for c in m['checks'])
assert m['native_effect_calls']==dict(context_menu=0,deferred_menu=0,clipboard=0,opener=0,trash=0,free_space=0)
s=[read(d/f'recent-phase-{phase}.json') for phase in range(117)]
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
for phase,mode,query,visible in [
 (27,'Fuzzy','KeepMe',[kept]),
 (30,'OrderedTerms','KeepMe',[kept]),
 (35,'OrderedTerms','recent_forget / kee',[kept]),
 (40,'OrderedTerms','kee recent_forget',[]),
 (45,'OrderedTerms','recent_forget',[]),
 (48,'Fuzzy','recent_forget',[kept]),
 (51,'OrderedTerms','recent_forget',[]),
 (61,'OrderedTerms','RECENT_FORGET / KEE',[kept]),
]:
 assert s[phase]['match_mode']==mode and s[phase]['saved']['match_mode']==mode
 assert s[phase]['query']==query and s[phase]['visible']==visible
 assert s[phase]['match_error'] is None
 assert s[phase]['stats']['uses'][kept]==dict(count=8,last=8)
 assert forgot not in s[phase]['paths'] and forgot not in s[phase]['stats']['uses']
assert s[56]['match_mode']=='OrderedTerms' and s[56]['query']=='x'*1025
assert s[56]['visible']==[] and '1024' in s[56]['match_error']
assert s[61]['selected']==0 and s[61]['order']=='Frecency'
# Independent ordered-fragment oracle for the native valid/refusal queries.
def matches(query,path):
 terms=query.lower().split();tail=path.lower()
 for term in terms:
  i=tail.find(term)
  if i<0:return False
  tail=tail[i+len(term):]
 return not terms or terms[-1].split('/')[-1] in path.lower().rstrip('/').split('/')[-1]
for phase in [35,40,45,51,61]:
 assert s[phase]['visible']==[path for path in s[phase]['paths'] if matches(s[phase]['query'],path)]
assert s[66]['query']=='kee !KeepMe' and s[66]['visible']==[] and s[66]['match_error'] is None
assert s[71]['query']=='kee !' and s[71]['visible']==[] and s[71]['match_error'] is not None
assert s[76]['query']=='RECENT_FORGET / KEE !ForgetMe' and s[76]['visible']==[kept] and s[76]['match_error'] is None
assert s[76]['selected']==0 and s[76]['match_mode']==s[76]['saved']['match_mode']=='OrderedTerms'
for phase in [66,76]:
 query=s[phase]['query'].lower().split()
 positive=[term for term in query if not term.startswith('!')]
 negative=[term[1:] for term in query if term.startswith('!')]
 expected=[path for path in s[phase]['paths'] if matches(' '.join(positive),path) and all(term not in path.lower() for term in negative)]
 assert s[phase]['visible']==expected
 assert s[phase]['paths']==s[61]['paths'] and s[phase]['stats']==s[61]['stats']
assert s[81]['query']=='KeepMe$' and s[81]['visible']==[kept]
assert s[86]['query']=='Keep$' and s[86]['visible']==[]
assert s[91]['query']=='^/missing kee' and s[91]['visible']==[]
assert s[96]['query']==f'^{folder} KEEPMe$ !ForgetMe' and s[96]['visible']==[kept]
# Independent regex oracle for edge anchors plus ordered positive fragments.
import re
def anchored_match(query,path):
 positive=[]
 for token in query.lower().split():
  inverse=token.startswith('!');token=token[1:] if inverse else token
  start=token.startswith('^');end=token.endswith('$')
  text=token[int(start):len(token)-int(end) if end else len(token)]
  pattern=('^' if start else '')+re.escape(text)+('$' if end else '')
  if inverse:
   if re.search(pattern,path.lower().rstrip('/')):return False
  else:positive.append((pattern,text))
 offset=0;last_end=0
 for pattern,text in positive:
  choices=[match for match in re.finditer(pattern,path.lower().rstrip('/')) if match.start()>=offset]
  if not choices:return False
  found=choices[0];offset=last_end=found.end()
 return not positive or '/' not in path.rstrip('/')[last_end:]
for phase in [81,86,91,96]:
 assert s[phase]['match_error'] is None
 assert s[phase]['visible']==[path for path in s[phase]['paths'] if anchored_match(s[phase]['query'],path)]
 assert s[phase]['stats']==s[76]['stats'] and s[phase]['paths']==s[76]['paths']
assert folder.name.endswith('recent_forget path')
assert s[101]['query']==r'recent_forget\ path / kee' and s[101]['visible']==[kept]
assert s[106]['query']==r'recent_forget\ \ path / kee' and s[106]['visible']==[]
assert s[111]['query']==r'kee !recent_forget\ path' and s[111]['visible']==[]
assert s[116]['query']==r'recent_forget\ path / KeepMe$ !unwanted\ path' and s[116]['visible']==[kept]
def space_match(query,path):
 tokens=[re.sub(r'\\(\s)',r'\1',token) for token in re.findall(r'(?:\\\s|[^\s])+',query.lower())]
 positive=[]
 for token in tokens:
  inverse=token.startswith('!');token=token[1:] if inverse else token
  start=token.startswith('^');end=token.endswith('$')
  text=token[int(start):len(token)-int(end) if end else len(token)]
  pattern=('^' if start else '')+re.escape(text)+('$' if end else '')
  if inverse:
   if re.search(pattern,path.lower().rstrip('/')):return False
  else:positive.append(pattern)
 offset=0
 for pattern in positive:
  choices=[match for match in re.finditer(pattern,path.lower().rstrip('/')) if match.start()>=offset]
  if not choices:return False
  offset=choices[0].end()
 return not positive or '/' not in path.rstrip('/')[offset:]
for phase in [101,106,111,116]:
 assert s[phase]['match_error'] is None
 assert s[phase]['visible']==[path for path in s[phase]['paths'] if space_match(s[phase]['query'],path)]
 assert s[phase]['stats']==s[96]['stats'] and s[phase]['paths']==s[96]['paths']
(q/'verification.json').write_text(json.dumps(dict(native_checks=16,observations=117,native_effects=0,escaped_space_terms_and_prior_anchors_exclusions=True,independent_oracle='Python escaped-whitespace tokenizer and ordered regex predicates',peer_rank_and_source_preserved=True,no_panel_navigation=True,source_and_binary_hashes_match=True),indent=2)+'\n')
print('PASS: native 16/16, 117 observations; literal spaces, mismatch, inverse phrase and anchored recovery; preserved source/history/navigation/effects')
