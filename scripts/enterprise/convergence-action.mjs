// Action prerequisites must settle before intentionally occupying provider HTTP.
export async function prepareActionThenHold({prepare,hold,verify}){
 const preparation=await prepare();
 await hold();
 await verify();
 return preparation;
}
export function holdReadiness(observation,minimumRemainingMs=15000){
 const stack=observation.stacks;
 const eligible=observation.enabled===true&&stack.length===1&&stack[0].fresh===true&&stack[0].expiresAt-observation.at>=minimumRemainingMs;
 return {eligible,minimumRemainingMs,observation};
}
export async function observeHoldReadiness(observe,timeoutMs=1000){
 let timer;
 try{return await Promise.race([Promise.resolve().then(observe),new Promise(resolve=>{timer=setTimeout(()=>resolve({eligible:false,reason:'readiness observation timed out'}),timeoutMs);})]);}
 catch{return {eligible:false,reason:'readiness observation failed'};}
 finally{clearTimeout(timer);}
}
