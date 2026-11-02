from pathlib import Path
import hashlib,base64,json
vectors=[]
for size in [0,3,65535,65536,65537,700001]:
 data=b'abc' if size==3 else bytes((i*37+11)%256 for i in range(size))
 a,b=hashlib.sha3_256(data),hashlib.sha3_512(data)
 vectors.append(dict(size=size,sha3_256=a.hexdigest(),sha3_512=b.hexdigest(),base64_256=base64.b64encode(a.digest()).decode(),base64_512=base64.b64encode(b.digest()).decode()))
expected=json.loads((Path(__file__).resolve().parent/'python-oracle.json').read_text())
assert vectors==expected
print('PASS: Python hashlib SHA3-256/SHA3-512 hex/Base64 oracle for six independent input sizes')
