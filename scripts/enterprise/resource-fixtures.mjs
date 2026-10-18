import {summarizeResources,resourceCoverage,phaseContracts} from './resources.mjs';
export function fixture(engine='webkit',contract='startup') {
 const native=[],browser=[],phases=[];
 for(const [i,phase] of phaseContracts[contract].entries()){
  const startMs=100+i*6000;
  for(let tick=0;tick<=4;tick++)native.push({seq:native.length+1,ns:(startMs+tick*1000)*1e6,stage:'resource',operation:'resource',id:0,resource:{kind:'sample',phase,boundary:tick===0?'start':tick===4?'end':'interval',nativeRssInclusive:{status:'measured',bytes:100},db:{status:'measured',bytes:20},walRetained:{status:'absent'},readBytes:{metadata:0,usage:0,index:0,links:0}}});
  for(let tick=0;tick<=4;tick++)for(const role of ['desktop','paired'])browser.push({seq:browser.length+1,at:startMs+1000+(tick===4?3990:tick*1000+10),role,phase,boundary:tick===0?'start':tick===4?'end':'interval',heap:engine==='chromium'?{status:'measured',bytes:10}:{status:'unknown',reason:'engine_unavailable'}});
  phases.push({phase,start:{beforeMs:startMs+995,afterMs:startMs+1005,nativeSeq:i*5+1,nativeNs:startMs*1e6},end:{beforeMs:startMs+4995,afterMs:startMs+5005,nativeSeq:i*5+5,nativeNs:(startMs+4000)*1e6}});
 }
 const evidence={schema:1,contract,samplerIntervalMs:1000,lost:false,coverage:resourceCoverage,native,browser,browserClock:{clock:'node-performance-ms',phases}};
 return {engine,resources:{evidence,summary:summarizeResources(evidence,engine)}};
}
