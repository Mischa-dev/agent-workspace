import {readFileSync} from 'node:fs';
import {randomUUID} from 'node:crypto';
import path from 'node:path';
const root=path.resolve(import.meta.dirname,'..'),scopeId=process.argv[2],id=process.argv[3]||randomUUID();
if(!scopeId)throw new Error('Usage: pnpm trigger <approved-scope-id> [stable-trigger-id]');
const token=readFileSync(path.join(root,'.local/work-hook-token'),'utf8').trim();
const cfg=JSON.parse(readFileSync(path.join(root,'.local/config.json'),'utf8'));
const response=await fetch(`http://127.0.0.1:${cfg.port}/api/hooks/work`,{method:'POST',headers:{'Content-Type':'application/json',Authorization:`Bearer ${token}`,'Idempotency-Key':id},body:JSON.stringify({scopeId})});const result=await response.json();if(!response.ok)throw new Error(result.error);console.log(JSON.stringify({triggerId:id,sessionId:result.value.id,status:result.value.data.status},null,2));
