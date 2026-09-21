import {migrate} from 'drizzle-orm/node-postgres/migrator';
import {db,pool} from './db.js';
import {config} from './config.js';
import path from 'node:path';
await migrate(db,{migrationsFolder:path.join(config.root,'apps/server/drizzle')});
await pool.end();
console.log('Database migrations applied.');
