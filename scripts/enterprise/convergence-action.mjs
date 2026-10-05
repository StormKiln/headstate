// Action prerequisites must settle before intentionally occupying provider HTTP.
export async function prepareActionThenHold({prepare,hold,verify}){
 const preparation=await prepare();
 await hold();
 await verify();
 return preparation;
}
