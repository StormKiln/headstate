import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ActivityChart } from '../components/stats/ActivityChart';
import { useCountdown } from '../lib/countdown';
import '../index.css';
const points=Array.from({length:30},(_,i)=>({date:`2026-09-${String(i+1).padStart(2,'0')}`,opened:10+i%4,merged:7+i%5}));
const changed=points.map((p,i)=>({...p,merged:i===10?40:p.merged}));
const deadline=Date.now()+120000;
export function ActivityChartFixture(){
 const [updated,setUpdated]=useState(false);const [days,setDays]=useState(30);
 const remaining=useCountdown(deadline);
 return <main style={{width:800}}><output data-testid="countdown">{remaining}</output><button onClick={()=>setUpdated(true)}>Change measurements</button><ActivityChart points={updated?changed:points} days={days} onDaysChange={setDays}/></main>;
}
createRoot(document.getElementById('root')!).render(<ActivityChartFixture/>);
