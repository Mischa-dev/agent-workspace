import pg from 'pg';
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {createHash} from 'node:crypto';
import path from 'node:path';
const cfg=JSON.parse(readFileSync('.local/config.json','utf8')),name=`workspace_browser_${Date.now()}`,url=new URL(cfg.databaseUrl);url.pathname='/postgres';const admin=new pg.Pool({connectionString:url.toString()});await admin.query(`CREATE DATABASE ${name}`);url.pathname=`/${name}`;
const dataDir=path.resolve('.local/qualification',name);mkdirSync(dataDir,{recursive:true,mode:0o700});writeFileSync('.local/browser-config.json',JSON.stringify({...cfg,databaseUrl:url.toString(),dataDir,ownerTokenHash:createHash('sha256').update('browser-qualification-only').digest('hex')},null,2),{mode:0o600});await admin.end();console.log(`Dedicated browser test database created: ${name}`);
