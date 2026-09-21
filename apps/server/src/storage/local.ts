import {mkdir,writeFile,rename,readFile,readdir,stat} from 'node:fs/promises';
import path from 'node:path';
import {randomUUID,createHash} from 'node:crypto';
import {config} from '../config.js';
import {AppError} from '../domain/store.js';
export interface Storage {put(bytes:Buffer):Promise<{key:string;size:number;checksum:string}>;get(key:string):Promise<Buffer>;reconcile(referenced:Set<string>):Promise<{missing:string[];quarantined:string[]}>}
export class LocalStorage implements Storage {
 constructor(private base=path.join(config.dataDir,'files')){}
 private location(key:string){if(!/^[0-9a-f-]{36}$/.test(key))throw new AppError(400,'Invalid storage key');return path.join(this.base,key);}
 async put(bytes:Buffer){if(bytes.length>10*1024*1024)throw new AppError(413,'Files are limited to 10 MB.');await mkdir(this.base,{recursive:true,mode:0o700});const key=randomUUID(),temp=path.join(this.base,`.upload-${key}`);await writeFile(temp,bytes,{flag:'wx',mode:0o600});await rename(temp,this.location(key));return {key,size:bytes.length,checksum:createHash('sha256').update(bytes).digest('hex')};}
 async get(key:string){return readFile(this.location(key));}
 async reconcile(referenced:Set<string>){await mkdir(this.base,{recursive:true,mode:0o700});const keys=await readdir(this.base),missing=[...referenced].filter(k=>!keys.includes(k)),quarantined:string[]=[];const q=path.join(config.dataDir,'quarantine');await mkdir(q,{recursive:true,mode:0o700});for(const key of keys){if(referenced.has(key))continue;const info=await stat(path.join(this.base,key));if(Date.now()-info.mtimeMs<3600000)continue;await rename(path.join(this.base,key),path.join(q,`${Date.now()}-${key}`));quarantined.push(key);}return {missing,quarantined};}
}
export const storage=new LocalStorage();
