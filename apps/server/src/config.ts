import {readFileSync,existsSync} from 'node:fs';
import path from 'node:path';
export const root=process.env.WORKSPACE_ROOT||process.cwd();
const envPath=path.join(root,'.local/config.json');
const local=existsSync(envPath)?JSON.parse(readFileSync(envPath,'utf8')):{};
export const config={root,port:Number(process.env.PORT||local.port||4310),databaseUrl:process.env.DATABASE_URL||local.databaseUrl||'',dataDir:process.env.DATA_DIR||local.dataDir||path.join(root,'.local/data'),webhookTokenHash:local.webhookTokenHash||'',ownerTokenHash:process.env.OWNER_TOKEN_HASH||local.ownerTokenHash||'',codexPath:process.env.CODEX_PATH||local.codexPath||'codex',test:process.env.NODE_ENV==='test'};
export const origin=`http://127.0.0.1:${config.port}`;
if(!config.databaseUrl) throw new Error('Run pnpm setup first (dedicated PostgreSQL required).');
