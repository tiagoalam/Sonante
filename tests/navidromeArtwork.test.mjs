import { sourceEnabled } from "../src/utils/musicSources.ts";
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { currentMediaKey } from '../src/utils/playerNavigation.ts';
import { queueItemArtwork } from '../src/utils/nowPlayingPresentation.ts';
const read=file=>readFileSync(new URL('../src/'+file,import.meta.url),'utf8');
function compile(file,deps) {
    const exports={};
    new Function('require','exports',ts.transpileModule(read(file),{fileName:file,compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText)(id=>{assert.ok(id in deps,id);return deps[id]},exports);
    return exports;
}
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const image='data:image/png;base64,c3ludGhldGlj';
const raw=(connection='one',song='song',queue_index=0)=>({current_media:{kind:'navidrome',connection_id:connection,song_id:song,queue_index},title:'Track',artist:'Artist',album:'Album',thumb:null,plex_image:null,state:'play',elapsed:12,duration:180,is_updating:false});
function mount(readCover=async()=>image) {
    const slots=[],effects=[],calls=[],errors=[];let cursor=0,disposed=false;
    const react={useState(initial){const n=cursor++;if(!(n in slots))slots[n]=initial;return[slots[n],value=>{assert.equal(disposed,false,'no state writes after unmount');slots[n]=value}]},useLayoutEffect(fn,deps){const n=cursor++,prior=slots[n];if(!prior||deps.some((item,index)=>!Object.is(item,prior.deps[index])))effects.push(()=>{prior?.cleanup?.();slots[n]={deps,cleanup:fn()}})}};
    const {useNavidromePlaybackArtwork}=compile('utils/useNavidromePlaybackArtwork.ts',{
        react,'./playerNavigation':{currentMediaKey},'./navidromeArtwork':{navidromeSongCover:async(...args)=>{calls.push(args.slice(0,2));return readCover(...args)}},'./frontendDiagnostics':{logError:(...args)=>errors.push(args)},
    });
    const f={status:raw(),available:true,active:true,calls,errors,render(){cursor=0;const result=useNavidromePlaybackArtwork(f.status,f.available,f.active);effects.splice(0).forEach(run=>run());return result},dispose(){slots.forEach(slot=>slot?.cleanup?.());disposed=true}};
    return f;
}
test('current Navidrome artwork enriches only presentation and never adds IPC on elapsed-only polls',async()=>{
    const f=mount();try{const before=structuredClone(f.status);assert.equal(f.render(),f.status);await flush();const status=f.render();assert.equal(status.thumb,image);assert.equal(status.elapsed,12);assert.equal(status.state,'play');assert.deepEqual(f.status,before);f.status={...f.status,elapsed:14};assert.equal(f.render().thumb,image);assert.deepEqual(f.calls,[['one','song']]);}finally{f.dispose()}
});
test('Local/Plex/radio/UPnP are unchanged and unavailable or inactive Navidrome does no artwork read',async()=>{
    const f=mount();try{for(const current_media of [{kind:'local',uri:'a.flac',queue_index:0},{kind:'plex',server_id:'s',part_key:'p',queue_index:0},{kind:'radio',station_id:'s',queue_index:0},{kind:'upnp',session_id:'s',queue_index:0},null]){f.status={...raw(),current_media};assert.equal(f.render(),f.status)}f.status=raw();f.available=false;assert.equal(f.render(),f.status);f.available=true;f.active=false;assert.equal(f.render(),f.status);assert.deepEqual(f.calls,[]);}finally{f.dispose()}
});
test('late images cannot cross song, connection or source changes, and unmount cannot publish a cover',async()=>{
    const old=deferred(),latest=deferred();const f=mount(connection=>connection==='one'?old.promise:latest.promise);
    f.render();f.status=raw('two');assert.equal(f.render().thumb,null);latest.resolve(image);await flush();assert.equal(f.render().thumb,image);old.resolve('data:image/png;base64,b2xk');await flush();assert.equal(f.render().thumb,image);
    f.status={...raw(),current_media:{kind:'local',uri:'local.flac',queue_index:0}};assert.equal(f.render(),f.status);f.dispose();
    const pending=deferred();const closed=mount(()=>pending.promise);closed.render();closed.dispose();pending.resolve(image);await flush();
});
test('missing or failed artwork remains optional and cannot change playback or retry at every status poll',async()=>{
    for(const readCover of [async()=>null,async()=>{throw 'NAVIDROME_UNAVAILABLE'}]){const f=mount(readCover);try{f.render();await flush();assert.equal(f.render(),f.status);f.status={...f.status,elapsed:99};assert.equal(f.render().elapsed,99);assert.equal(f.calls.length,1);}finally{f.dispose()}}
});
test('catalog and player share the bounded image cache, while pending song-reference reads are deduplicated',async()=>{
    const {navidromeCovers}=compile('utils/navidromeCovers.ts',{});const calls=[];const reference=deferred();let live=true;
    const {navidromeAlbumCover,navidromeSongCover}=compile('utils/navidromeArtwork.ts',{'./navidromeCovers':{navidromeCovers},'../services/navidrome':{navidromeService:{cover:async(...args)=>{calls.push(['image',...args]);return image},songCoverId:async(...args)=>{calls.push(['reference',...args]);return reference.promise}}}});
    await navidromeAlbumCover('one','art');
    const first=navidromeSongCover('one','song',()=>live),second=navidromeSongCover('one','song',()=>true);assert.equal(calls.filter(item=>item[0]==='reference').length,1);live=false;reference.resolve('art');assert.equal(await first,null);assert.equal(await second,image);assert.equal(calls.filter(item=>item[0]==='image').length,1);
    await navidromeAlbumCover('two','art');assert.equal(calls.filter(item=>item[0]==='image').length,2);
});
test('current Navidrome queue artwork requires exact connection, song and position and leaves logical rows intact',()=>{
    const status={...raw(),thumb:image};const item={title:'Track',artist:'Artist',album:'Album',media_locator:{kind:'navidrome',connection_id:'one',song_id:'song'}};const before=structuredClone(item);
    assert.equal(queueItemArtwork(item,0,status).thumb,image);assert.deepEqual(item,before);assert.equal(queueItemArtwork(item,1,status),item);assert.equal(queueItemArtwork(item,0,{...status,current_media:{...status.current_media,connection_id:'two'}}),item);assert.equal(queueItemArtwork(item,0,{...status,current_media:{...status.current_media,song_id:'other'}}),item);
});
test('actual App artwork pipeline and player hosts receive Navidrome presentation without replacing raw telemetry',()=>{
    const source=read('App.tsx'),file=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);const expressions={};
    function visit(node){if(ts.isVariableDeclaration(node)&&['sourceArtworkPlayback','presentationPlayback'].includes(node.name.getText(file)))expressions[node.name.getText(file)]=node.initializer.getText(file);ts.forEachChild(node,visit)}visit(file);
    const telemetry=raw(),decorated={...telemetry,thumb:image};let localInput;
    const bindings={sourceEnabled,useNavidromePlaybackArtwork:(status,available,active)=>{assert.equal(status,telemetry);assert.ok(available&&active);return decorated},playbackStatus:telemetry,isPlaybackAvailable:true,config:{first_run:false},showSettings:false,showAbout:false};
    const sourceArtworkPlayback=new Function(...Object.keys(bindings),`return (${expressions.sourceArtworkPlayback})`)(...Object.values(bindings));
    const localBindings={sourceEnabled,useLocalPlaybackArtwork:status=>{localInput=status;return status},sourceArtworkPlayback,localCatalog:{},isPlaybackAvailable:true,config:{first_run:false},showSettings:false,showAbout:false,ensurePlaybackArtworkCatalog(){}};
    const presentation=new Function(...Object.keys(localBindings),`return (${expressions.presentationPlayback})`)(...Object.values(localBindings));assert.equal(localInput,decorated);assert.equal(presentation.thumb,image);assert.equal(telemetry.thumb,null);
    assert.match(source,/playback: presentationPlayback/);assert.match(source,/<PlayerBar[\s\S]*?status=\{presentationPlayback\}/);assert.match(source,/<NowPlayingHost[\s\S]*?playback: presentationPlayback/);assert.match(source,/<QueueDrawer[\s\S]*?status=\{presentationPlayback\}/);
});
