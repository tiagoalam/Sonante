import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import React from 'react';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
const read=file=>readFileSync(new URL('../src/'+file,import.meta.url),'utf8');
function compile(file,deps){const exports={};new Function('require','exports',ts.transpileModule(read(file),{fileName:file,compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},exports);return exports;}
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no});return{promise,resolve,reject}};
const flush=async()=>{for(let i=0;i<40;i++)await Promise.resolve()};
const connection={connection_id:'fixture-session',account_id:'a'.repeat(64),server:'http://fixture.invalid',username:'fixture'};
const album={id:'album-one',title:'One',artist:'Artist',year:2020,cover_id:null,song_count:2};
function nodes(tree,predicate){return Array.isArray(tree)?tree.flatMap(n=>nodes(n,predicate)):!tree||typeof tree!=='object'?[]:[...(predicate(tree)?[tree]:[]),...nodes(tree.props?.children,predicate)];}
function hooks(){let cursor=0,disposed=false;const slots=[],effects=[];
 const same=(a,b)=>a?.length===b?.length&&a.every((v,i)=>Object.is(v,b[i]));
 const react={...React,useState(initial){const n=cursor++;if(!(n in slots))slots[n]=typeof initial==='function'?initial():initial;return[slots[n],value=>{assert.equal(disposed,false,'no state after unmount');slots[n]=typeof value==='function'?value(slots[n]):value}]},useRef(initial){return slots[cursor++]??={current:initial}},useMemo(fn,deps){const n=cursor++;if(!same(slots[n]?.deps,deps))slots[n]={deps,value:fn()};return slots[n].value;},useEffect(fn,deps){const n=cursor++,old=slots[n];if(!same(old?.deps,deps))effects.push(()=>{old?.cleanup?.();slots[n]={deps,cleanup:fn()}})}};
 return{react,render(fn){cursor=0;const result=fn();effects.splice(0).forEach(fn=>fn());return result;},close(){slots.forEach(slot=>slot?.cleanup?.());disposed=true;}};
}
function fixture(kind='Explore',options={}){
 const h=hooks(),calls=[];let props={available:true,active:true,revision:0,onConfigure(){},artist:{account_id:connection.account_id,id:'artist-one'},onBack(){},...options.props};
 const service=Object.fromEntries(['status','explore','favorites','artist','album','play'].map(name=>[name,async(...args)=>{calls.push([name,...args]);if(options[name])return options[name](...args);return{name:undefined,status:connection,explore:{suggested:[album],added:[{...album,id:'new',title:'New'}]},favorites:[],artist:{artist:{id:'artist-one',name:'Artist',album_count:1},albums:[album]},album:{album,songs:[]}}[name];}]));
 const Card=function Card(){},View=function View(){},Grid=function Grid(){};
 let token=0;
 const shared={'./deferredViews':{NavidromeArtistView:function NavidromeArtistView(){}},react:h.react,'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:key=>key})},'../services/navidrome':{navidromeService:service,navidromeError:e=>e==='NAVIDROME_CONTEXT_CHANGED'?'changed':'unavailable'},'./NavidromeView':{NavidromeView:View},'./NavidromeDiscoveryCard':{NavidromeDiscoveryCard:Card},'./VirtualAlbumGrid':{VirtualAlbumGrid:Grid},'./PlaybackRequestNotice':{PlaybackRequestNotice:()=>null},'../utils/frontendDiagnostics':{logError(){}},'./frontendDiagnostics':{logError(){}},'./playbackIntent':{beginPlaybackIntent(){const own=++token;return{current:()=>own===token,confirmCurrent:async()=>own===token,cancel(){if(own===token)token++;}}},isPlaybackSuperseded:()=>false,playbackFailure:()=> 'failed'}};
 shared['../utils/usePlaybackAttempt']=compile('utils/usePlaybackAttempt.ts',shared);
 const name=`Navidrome${kind}View`,Component=compile(`components/${name}.tsx`,shared)[name];
 const f={calls,Card,View,Grid,tree:null,paint(next=props){props=next;return f.tree=h.render(()=>Component(props));},props:()=>props,async settle(){for(let i=0;i<4;i++){f.paint();await flush();}return f.tree;},close:h.close};
 f.paint();return f;
}
test('Navidrome Explore requests only bounded shelves and favorites, preserves scroll on Back and reuses loaded data when hidden',async()=>{
 const f=fixture();try{let tree=await f.settle();assert.deepEqual(f.calls.map(c=>c[0]),['status','explore','favorites']);const scroll=nodes(tree,n=>n.props?.onScroll)[0];const container={scrollTop:0};scroll.props.ref(container);f.paint();scroll.props.onScroll({currentTarget:{scrollTop:345}});
  nodes(f.tree,n=>n.type===f.Card)[0].props.onOpen();tree=f.paint();assert.equal(tree.type,f.View);assert.deepEqual(tree.props.initialAlbum,{account_id:connection.account_id,id:album.id});assert.equal(f.calls.some(c=>c[0]==='play'),false);tree.props.onAlbumBack();f.paint();assert.equal(container.scrollTop,345);
  f.paint({...f.props(),active:false});await flush();f.paint({...f.props(),active:true});await f.settle();assert.equal(f.calls.length,3);
 }finally{f.close()}
});
test('a failed Navidrome Explore query preserves Sonante favorites and never displays private error text',async()=>{
 const f=fixture('Explore',{explore:async()=>{throw Error('PRIVATE')},favorites:async()=>[album]});try{const tree=await f.settle();assert.equal(nodes(tree,n=>n.type===f.Card).length,1);assert.equal(nodes(tree,n=>n.props?.role==='alert').length,1);assert.doesNotMatch(JSON.stringify(tree),/PRIVATE/);}finally{f.close()}
});
test('inactive Explore performs no reads; a changed account or leaving fences late shelf responses and pending playback',async()=>{
 const pending=deferred(),prepared=deferred();let count=0;const f=fixture('Explore',{props:{active:false},explore:async()=>++count===1?pending.promise:{suggested:[{...album,id:'current'}],added:[]},album:()=>prepared.promise});
 try{await f.settle();assert.deepEqual(f.calls,[]);f.paint({...f.props(),active:true});await flush();f.paint({...f.props(),revision:1});await f.settle();assert.equal(nodes(f.tree,n=>n.type===f.Card)[0].props.album.id,'current');pending.resolve({suggested:[album],added:[]});await f.settle();assert.equal(nodes(f.tree,n=>n.type===f.Card)[0].props.album.id,'current');
  nodes(f.tree,n=>n.type===f.Card)[0].props.onPlay();await flush();f.paint({...f.props(),active:false});prepared.resolve({album,songs:[]});await flush();assert.equal(f.calls.some(c=>c[0]==='play'),false);
 }finally{f.close()}
});
test('Navidrome artist requires the searched account and virtualizes only that artist; no full catalog or autoplay',async()=>{
 const f=fixture('Artist');try{const tree=await f.settle();assert.deepEqual(f.calls,[['status'],['artist',connection.connection_id,'artist-one']]);const grid=nodes(tree,n=>n.type===f.Grid)[0];assert.deepEqual(grid.props.items,[album]);grid.props.renderItem(album).props.onOpen();const detail=f.paint();assert.equal(detail.type,f.View);assert.equal(detail.props.initialAlbum.id,album.id);assert.equal(f.calls.some(c=>c[0]==='play'),false);}finally{f.close()}
 const changed=fixture('Artist',{status:async()=>({...connection,account_id:'b'.repeat(64)})});try{await changed.settle();assert.deepEqual(changed.calls,[['status']]);assert.equal(nodes(changed.tree,n=>n.props?.role==='alert').length,1);}finally{changed.close()}
});
test('unmounted artist ignores a late reply and hiding cancels explicit album playback preparation',async()=>{
 const pending=deferred();const f=fixture('Artist',{artist:()=>pending.promise});await flush();f.close();pending.resolve({artist:{id:'artist-one',name:'Artist'},albums:[album]});await flush();
 const prepare=deferred();const g=fixture('Artist',{album:()=>prepare.promise});try{await g.settle();nodes(g.tree,n=>n.type===g.Grid)[0].props.renderItem(album).props.onPlay();await flush();g.paint({...g.props(),active:false});prepare.resolve({album,songs:[]});await flush();assert.equal(g.calls.some(c=>c[0]==='play'),false);}finally{g.close()}
});
function configFixture(options={}){const h=hooks(),calls=[];let enabled=false,changed;const {useNavidromeConfiguration}=compile('utils/useNavidromeConfiguration.ts',{react:h.react,'../services/navidrome':{navidromeService:{configuration:async()=>{calls.push('configuration');return options.configuration?options.configuration():true},subscribeConfiguration:async fn=>{calls.push('subscribe');changed=fn;return options.subscribe?options.subscribe():()=>calls.push('unsubscribe');}}}});return{calls,paint(value=enabled){enabled=value;return h.render(()=>useNavidromeConfiguration(enabled))},changed:()=>changed?.(),close:h.close};}
test('configuration discovery is local-only and disabled sources do not subscribe or read; configuration events invalidate the account identity',async()=>{
 const f=configFixture();try{assert.equal(f.paint(false).configured,false);await flush();assert.deepEqual(f.calls,[]);f.paint(true);await flush();assert.equal(f.paint().configured,true);assert.deepEqual(f.calls,['subscribe','configuration']);f.changed();await flush();assert.deepEqual(f.paint(),{configured:true,revision:1});f.paint(false);assert.equal(f.paint().configured,false);assert.equal(f.calls.at(-1),'unsubscribe');}finally{f.close()}
});
test('late configuration reads cannot enable a disabled source and late subscription disposes after unmount',async()=>{
 const pending=deferred();const f=configFixture({configuration:()=>pending.promise});f.paint(true);await flush();f.paint(false);pending.resolve(true);await flush();assert.equal(f.paint().configured,false);f.close();
 const subscription=deferred();let disposed=0;const g=configFixture({subscribe:()=>subscription.promise});g.paint(true);g.close();subscription.resolve(()=>disposed++);await flush();assert.equal(disposed,1);assert.deepEqual(g.calls,['subscribe']);
});
