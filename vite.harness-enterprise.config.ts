import {defineConfig,mergeConfig} from 'vite';
import base from './vite.config';
export default defineConfig(env=>mergeConfig(base(env),{
 resolve:{alias:[{find:/^react-dom\/client$/,replacement:'react-dom/profiling'}, {find:/^(?:.*\/api\/transport|\.\/transport)(?:\.ts)?$/,replacement:new URL('./src/harness/enterpriseTransport.ts',import.meta.url).pathname}]},
 build:{outDir:'dist-harness-enterprise',emptyOutDir:true,minify:false,rollupOptions:{input:new URL('./harness/enterprise.html',import.meta.url).pathname}},
}));
