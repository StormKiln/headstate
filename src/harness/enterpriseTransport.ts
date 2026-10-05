import { assertRemoteReply, remoteEventError } from '../api/wireContract';
export type UnlistenFn=()=>void;
export interface Transport {call<T>(name:string,args?:Record<string,unknown>):Promise<T>;listen<T>(event:string,cb:(e:{payload:T})=>void):Promise<UnlistenFn>}
const role=new URL(location.href).searchParams.get('role')==='paired'?'paired':'desktop';
const source=new EventSource(`/bridge/events/${role}`);
export const telemetry:{kind:string;at:number;name:string;duration?:number;ok?:boolean;callId?:number;demandOp?:string;refusal?:string}[]=[];
export const measurement={lost:false};
export function boundedPush<T>(items:T[],value:T){if(items.length>=100000){measurement.lost=true;return;}items.push(value);}
export const call:Transport['call']=async<T,>(name:string,args?:Record<string,unknown>):Promise<T>=>{
 const at=performance.now();let callId:number|undefined;let refusal:string|undefined;const demandOp=name==='stats_demand'?(args?.request as {op?:string}|undefined)?.op:undefined;
 try{
  const response=await fetch(`/bridge/call/${role}/${name}`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(args??{})});
  const body=await response.json();callId=body.callId;if(!response.ok&&body.wire?.message==='stats lease expired or released')refusal='expired-lease';if(!response.ok)throw new Error(body.error??body.wire?.message??'Native request refused');
  const value=role==='paired'?body.wire:body.value;
  if(role==='paired')assertRemoteReply(name,value);
  boundedPush(telemetry,{kind:'call',at,name,duration:performance.now()-at,callId,demandOp,ok:true});return value as T;
 }catch(error){boundedPush(telemetry,{kind:'call',at,name,duration:performance.now()-at,callId,demandOp,refusal,ok:false});throw error;}
};
export const listen:Transport['listen']=async<T,>(name:string,cb:(e:{payload:T})=>void)=>{
 const handler=(event:MessageEvent<string>)=>{const payload=JSON.parse(event.data) as T;const error=remoteEventError(name,payload);if(error)throw new Error(error);boundedPush(telemetry,{kind:'event',at:performance.now(),name});cb({payload});};
 source.addEventListener(name,handler as EventListener);return()=>source.removeEventListener(name,handler as EventListener);
};
