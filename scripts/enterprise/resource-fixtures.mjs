import {summarizeResources,resourceCoverage} from './resources.mjs';
export function fixture(engine='webkit') {
 const sample=(seq,boundary)=>({seq,ns:seq*100,stage:'resource',operation:'resource',id:0,resource:{kind:'sample',phase:'startup',boundary,nativeRssInclusive:{status:'measured',bytes:100},db:{status:'measured',bytes:20},walRetained:{status:'absent'},readBytes:{metadata:0,usage:0,index:0,links:0}}});
 const evidence={schema:1,contract:'startup',samplerIntervalMs:1000,lost:false,coverage:resourceCoverage,native:[sample(1,'start'),sample(3,'end')],browser:[{seq:1,role:'desktop',phase:'startup',heap:engine==='chromium'?{status:'measured',bytes:10}:{status:'unknown',reason:'engine_unavailable'}},{seq:2,role:'paired',phase:'startup',heap:engine==='chromium'?{status:'measured',bytes:10}:{status:'unknown',reason:'engine_unavailable'}}]};
 return {engine,resources:{evidence,summary:summarizeResources(evidence,engine)}};
}
