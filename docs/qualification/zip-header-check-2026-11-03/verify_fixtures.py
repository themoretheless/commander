from pathlib import Path
import hashlib,json
q=Path(__file__).resolve().parent
records=json.loads((q/'oracle.json').read_text())
for name,record in records.items():assert hashlib.sha256((q/'fixtures'/f'{name}.zip').read_bytes()).hexdigest()==record['sha256'],name
print(f'{len(records)} fixture archive hashes verified.')
