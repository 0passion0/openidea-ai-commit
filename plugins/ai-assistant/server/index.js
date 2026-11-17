import { GoogleGenAI } from '@google/genai';
import { createServer } from './server.js';
import { readConfig } from './config.js';

const config = readConfig(process.env);
const ai = config.enabled ? new GoogleGenAI({ apiKey: config.apiKey }) : undefined;
createServer(config, ai).listen(config.port, '127.0.0.1');
