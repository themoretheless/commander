from pathlib import Path
import base64,hashlib,json,subprocess,urllib.request
q=Path(__file__).resolve().parent;root=q.parents[2]
scratch=root/'target/xxh3-reference';scratch.mkdir(parents=True,exist_ok=True)
url='https://raw.githubusercontent.com/Cyan4973/xxHash/v0.8.3/xxhash.h'
header=urllib.request.urlopen(url).read();(scratch/'xxhash.h').write_bytes(header)
source=r'''
#define XXH_INLINE_ALL
#include "xxhash.h"
#include <stdio.h>
#include <stdlib.h>
int main(int argc,char **argv) {
 if(argc!=2)return 2;
 FILE *f=fopen(argv[1],"rb");if(!f)return 3;
 fseek(f,0,SEEK_END);long n=ftell(f);rewind(f);
 if(n<0 || n>200000)return 4;
 unsigned char *p=malloc((size_t)n+1);if(!p)return 5;
 if(fread(p,1,(size_t)n,f)!=(size_t)n)return 6;
 fclose(f);XXH128_hash_t h=XXH3_128bits(p,(size_t)n);
 printf("%016llx %016llx%016llx\n",(unsigned long long)XXH3_64bits(p,(size_t)n),(unsigned long long)h.high64,(unsigned long long)h.low64);
 free(p);return 0;
}
'''
(scratch/'oracle.c').write_text(source)
subprocess.run(['cc','-O2',str(scratch/'oracle.c'),'-o',str(scratch/'oracle')],check=True)
vectors=[]
for size in [0,1,3,16,17,31,32,64,65,96,127,128,129,240,241,255,256,257,1024,65535,65536,65537,131123]:
 data=b'abc' if size==3 else bytes((i*131+17)%256 for i in range(size))
 (scratch/'input.bin').write_bytes(data)
 a,b=subprocess.check_output([str(scratch/'oracle'),str(scratch/'input.bin')],text=True).split()
 vectors.append(dict(size=size,sha256=hashlib.sha256(data).hexdigest(),xxh3_64=a,xxh3_128=b,base64_64=base64.b64encode(bytes.fromhex(a)).decode(),base64_128=base64.b64encode(bytes.fromhex(b)).decode()))
(q/'reference.json').write_text(json.dumps(dict(header_url=url,header_sha256=hashlib.sha256(header).hexdigest(),wrapper_sha256=hashlib.sha256(source.encode()).hexdigest(),compiler=subprocess.check_output(['cc','--version'],text=True).splitlines()[0]),indent=2)+'\n')
(q/'vectors.json').write_text(json.dumps(vectors,indent=2)+'\n')
print(json.dumps(next(v for v in vectors if v['size']==3),indent=2))
