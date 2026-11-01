import {useEffect,useState} from 'react';
import {navidromeService} from '../services/navidrome';
/** Small local configuration read only. Never opens a proxy or contacts the server. */
export function useNavidromeConfiguration(enabled:boolean){
 const [state,setState]=useState({configured:false,revision:0});
 useEffect(()=>{
  let live=true,stop:(()=>void)|undefined,request=0;
  setState(previous=>({...previous,configured:false}));
  if(!enabled)return;
  const refresh=()=>{const own=++request;void navidromeService.configuration().then(configured=>{if(live&&own===request)setState(previous=>({...previous,configured}));}).catch(()=>{if(live&&own===request)setState(previous=>({...previous,configured:false}));});};
  void navidromeService.subscribeConfiguration(()=>{if(!live)return;setState(previous=>({...previous,revision:previous.revision+1}));refresh();}).then(unlisten=>{if(live){stop=unlisten;refresh();}else unlisten();}).catch(()=>{if(live)refresh();});
  return()=>{live=false;request++;stop?.();};
 },[enabled]);
 return enabled?state:{...state,configured:false};
}
