import {navidromeQueueTrack} from '../src/utils/navidromeTracks.ts';
import * as genreFamilies from '../src/utils/localGenreFamilies.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
const read=file=>readFileSync(new URL('../src/'+file,import.meta.url),'utf8');
function compile(file,deps={}){const exports={};new Function('require','exports',ts.transpileModule(read(file),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},exports);return exports;}
const {createLocalSourceReadQueue}=compile('utils/localSourceReadQueue.ts');
const helpers=compile('utils/smartPlaylistPreview.ts',{'./localGenreFamilies':genreFamilies});
const {defaultSmartRules}=compile('utils/smartPlaylists.ts',{'./localGenreFamilies':genreFamilies});
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
const flush=()=>new Promise(resolve=>setImmediate(resolve));
const local={kind:'local'};
const albums=[{id:'a',title:'Album A',artist:'Artist',genres:[],year:null,added_at:null},{id:'b',title:'Album B',artist:'Artist',genres:[],year:null,added_at:null}];
const group=(id,disc=1)=>({disc:{number:disc},tracks:[{item_type:'file',path:'Local/'+id+'.flac',name:id}]});
function fixture(options={}){
 const queue=createLocalSourceReadQueue(),calls=[];
 const {smartPlaylistsService:service}=compile('services/smartPlaylists.ts',{'@tauri-apps/api/core':{invoke:async(command,args)=>{calls.push([command,args]);return options.invoke?options.invoke(command,args):command==='smart_playlist_catalog'?albums:command==='smart_playlist_sources'?{local:true,plex_server_id:'server-a'}:{albums,matched_albums:2}}},'../utils/localSourceReadQueue':{localSourceReadQueue:queue},'../utils/smartPlaylistPreview':helpers,'./audio':{audioService:{getLocalAlbumTracks:async id=>{calls.push(['tracks',id]);return options.tracks?options.tracks(id):[group(id)]}}},'../utils/navidromeTracks':{navidromeQueueTrack},'./navidrome':{navidromeService:{status:async()=>{calls.push(['navStatus']);return options.navStatus?options.navStatus():{connection_id:'fixture-session',account_id:'a'.repeat(64)}},album:async(c,id)=>{calls.push(['navAlbum',c,id]);return options.navAlbum?options.navAlbum(c,id):{songs:[{id:'song-'+id,title:'Track '+id,artist:'Artist',album:'Album',duration:60}]}}}},'./plex':{plexService:{getAlbumTracks:async id=>{calls.push(['plexTracks',id]);return options.plex?options.plex(id):[]}}}});
 return{service,queue,calls};
}
test('opening smart rules waits for the active gallery read and uses the native catalog once',async()=>{
 const f=fixture(),gallery=deferred();const active=f.queue(()=>gallery.promise);await flush();const result=f.service.catalog(local);await flush();assert.deepEqual(f.calls,[]);gallery.resolve();await active;assert.deepEqual(await result,albums);assert.equal(f.calls.length,1);assert.equal(f.calls[0][0],'smart_playlist_catalog');
});
test('StrictMode replacement cancels the obsolete queued opening, without a refusal or retry',async()=>{
 const f=fixture();let firstLive=true;const first=f.service.catalog(local,()=>firstLive);const refused=assert.rejects(first,e=>e==='SMART_PLAYLIST_CANCELLED');firstLive=false;const second=f.service.catalog(local,()=>true);assert.deepEqual(await second,albums);await refused;assert.equal(f.calls.filter(c=>c[0]==='smart_playlist_catalog').length,1);
});
test('a cancelled request already in flight finishes before replacement enters the same source read gate',async()=>{
 const pending=deferred();let busy=false,first=true;const f=fixture({invoke:async()=>{assert.equal(busy,false,'native source reads never overlap');busy=true;if(first){first=false;await pending.promise;}busy=false;return albums;}});let live=true;const old=f.service.catalog(local,()=>live);await flush();live=false;const replacement=f.service.catalog(local);await flush();assert.equal(f.calls.length,1);pending.resolve();await old;assert.deepEqual(await replacement,albums);assert.equal(f.calls.length,2);
});
test('native refusals remain errors and are never automatically retried',async()=>{
 const f=fixture({invoke:async()=>{throw 'SMART_PLAYLIST_READ'}});await assert.rejects(f.service.catalog(local),e=>e==='SMART_PLAYLIST_READ');assert.equal(f.calls.length,1);
});
test('preview schedules each Local album separately so player control runs before the next batch',async()=>{
 const pending=deferred();const f=fixture({tracks:async id=>id==='a'?pending.promise:[group(id)]});const rules={...defaultSmartRules(),limit:2};const result=f.service.preview(rules);await flush();assert.deepEqual(f.calls.map(c=>c[0]),['smart_playlist_preview','tracks']);
 const control=f.queue.control(async()=>{f.calls.push(['player-control']);});pending.resolve([group('a')]);await control;const value=await result;assert.deepEqual(f.calls.map(c=>c[0]),['smart_playlist_preview','tracks','player-control','tracks']);assert.equal(value.tracks.length,2);assert.equal(value.limit_reached,true);assert.deepEqual(value.tracks.map(t=>t.media_locator.uri),['Local/a.flac','Local/b.flac']);
});
test('closing a preview stops further album queries and never changes playback',async()=>{
 const pending=deferred();const f=fixture({tracks:async()=>pending.promise});let live=true;const result=f.service.preview(defaultSmartRules(),()=>live);const failure=assert.rejects(result,e=>e==='SMART_PLAYLIST_CANCELLED');await flush();live=false;pending.resolve([group('a')]);await failure;assert.equal(f.calls.filter(c=>c[0]==='tracks').length,1);assert.equal(f.calls.some(c=>c[0]==='play_tracks'),false);
});
test('disc order, track identity and the limit survive the staged Local preview',async()=>{
 const f=fixture({tracks:async()=>[group('2',2),group('1',1),group('1',1)]});const value=await f.service.preview({...defaultSmartRules(),track_mode:'album',limit:2});assert.deepEqual(value.tracks.map(t=>t.media_locator.uri),['Local/1.flac','Local/2.flac']);assert.equal(f.calls.filter(c=>c[0]==='tracks').length,1);
});
test('Plex reads do not wait for the Local gallery, and wrong-server tracks are rejected',async()=>{
 const f=fixture({plex:async()=>[{title:'Song',media_locator:{kind:'plex',server_id:'wrong-server',part_key:'/library/parts/1/file.flac'}}]});const gallery=deferred();const active=f.queue(()=>gallery.promise);await flush();const source={kind:'plex',server_id:'server-a',library_id:'7'};
 try{assert.deepEqual(await f.service.catalog(source),albums);await assert.rejects(f.service.preview(defaultSmartRules(source)),e=>e==='SMART_PLAYLIST_SOURCE');}finally{gallery.resolve();await active;}
});
test('distinct recordings with matching tags remain distinct while repeated source identities collapse',async()=>{
 const t=uri=>({title:'Same',artist:'Same',album:'Same',media_locator:{kind:'local',uri}});const selection={albums:[albums[0]],matched_albums:1};const value=await helpers.assembleSmartPreview(selection,defaultSmartRules(),async()=>[t('Local/1.flac'),t('Local/1.flac'),t('Local/2.flac')]);assert.equal(value.tracks.length,2);
 await assert.rejects(helpers.assembleSmartPreview(selection,defaultSmartRules(),async()=>[{...t('x'),media_locator:{kind:'radio',station_id:'1'}}]),e=>e==='SMART_PLAYLIST_SOURCE');
});
test('Plex duration, album artwork fallback and final selected-server check are preserved',async()=>{
 const source={kind:'plex',server_id:'server-a',library_id:'7'},image={server_id:'server-a',path:'/library/metadata/1/thumb/1'};
 const f=fixture({invoke:async command=>command==='smart_playlist_sources'?{local:true,plex_server_id:'server-a'}:{albums:[{...albums[0],image}],matched_albums:1},plex:async()=>[{title:'Plex song',duration_ms:123456,media_locator:{kind:'plex',server_id:'server-a',part_key:'/library/parts/1/file.flac'}}]});const value=await f.service.preview(defaultSmartRules(source));assert.equal(value.tracks[0].duration,123.456);assert.deepEqual(value.tracks[0].plex_image,image);assert.equal(f.calls.at(-1)[0],'smart_playlist_sources');
});
test('a server switch after the final Plex album read invalidates the whole preview',async()=>{
 const f=fixture({invoke:async command=>command==='smart_playlist_sources'?{local:true,plex_server_id:'changed-server'}:{albums:[albums[0]],matched_albums:1},plex:async()=>[{title:'Plex song',media_locator:{kind:'plex',server_id:'server-a',part_key:'/library/parts/1/file.flac'}}]});await assert.rejects(f.service.preview(defaultSmartRules({kind:'plex',server_id:'server-a',library_id:'7'})),e=>e==='SMART_PLAYLIST_SOURCE');
});

test('smart Local families cover explicit reported aliases and compound tags, without rewriting or substring guessing',()=>{
 for(const [tag,family] of [
  ['Bossanova, Samba, MPB','brazilian'],['Brazil','brazilian'],['Brazilian','brazilian'],['Brazilian Folk','brazilian'],
  ['Brazilian Jazz | Fusion','brazilian'],['Brazilian Music','brazilian'],['Brazilian Music / Rap/Hip Hop','brazilian'],
  ['Brazilian Music;Rap/Hip Hop','brazilian'],['Brazilian Rock','brazilian'],['Brazilian, Bossa, Jazz, World','brazilian'],
  ['British Reggae','reggae'],['Candomblé Music, Afoxé','brazilian'],['Caribbean And Latin American, Mpb','brazilian'],['celtic','folkCountry'],
  ['Mu\u0301sica Brasileira','brazilian'],['LATIN JAZZ','jazz'],['Rap/Hip Hop','hipHop'],['Reggaeton','latin'],
  ['16','other'],['42','other'],['Not Rock','other'],['Jazzercise','other'],['Jazz | Unknown','other'],['Jazz||Rock','other']
 ]){const album=Object.freeze({genres:Object.freeze([tag])});assert.equal(genreFamilies.classifySmartLocalAlbumFamily(album),family,tag);assert.equal(album.genres[0],tag);}
 assert.equal(genreFamilies.classifyLocalAlbumFamily({genres:['Brazil']}),'other','existing Explore classifier retains its prior meaning');
 assert.deepEqual(genreFamilies.smartLocalGenreOptions([{genres:['Brazil']},{genres:['Samba']},{genres:['16']}]),['brazilian','other']);
});
test('Local family filtering occurs before expansion and limit, using the same metadata grouping shown in the editor',async()=>{
 const selected=[{...albums[0],id:'rock',genres:['Rock']},{...albums[0],id:'a',genres:['Brazilian Rock']},{...albums[1],id:'b',genres:['Bossanova, Samba, MPB']}];
 const before=JSON.stringify(selected);const f=fixture({invoke:async()=>({albums:selected,matched_albums:3}),tracks:async id=>[group(id+'1'),group(id+'2')]});const value=await f.service.preview({...defaultSmartRules(),genre_family:'brazilian',limit:3});
 assert.deepEqual(f.calls.filter(c=>c[0]==='tracks').map(c=>c[1]),['a','b']);assert.equal(value.matched_albums,2);assert.deepEqual(value.tracks.map(t=>t.media_locator.uri),['Local/a1.flac','Local/b1.flac','Local/a2.flac']);assert.equal(JSON.stringify(selected),before);
});
test('varied tracks give each album a turn, deduplicate identity and respect disc/track order within that album',async()=>{
 const f=fixture({invoke:async()=>({albums:[...albums,{...albums[0],id:'c'}],matched_albums:3}),tracks:async id=>[group(id+'2',2),group(id+'1',1),group(id+'1',1)]});const value=await f.service.preview({...defaultSmartRules(),limit:5});assert.deepEqual(value.tracks.map(t=>t.media_locator.uri),['Local/a1.flac','Local/b1.flac','Local/c1.flac','Local/a2.flac','Local/b2.flac']);assert.equal(f.calls.filter(c=>c[0]==='tracks').length,3);
});
test('variety reads at most the requested number of contributing albums, never every album just to fill a short selection',async()=>{
 const selected=Array.from({length:4000},(_,i)=>({...albums[0],id:String(i)}));const f=fixture({invoke:async()=>({albums:selected,matched_albums:4000})});const value=await f.service.preview({...defaultSmartRules(),limit:3});assert.equal(f.calls.filter(c=>c[0]==='tracks').length,3);assert.equal(value.tracks.length,3);assert.equal(value.matched_albums,4000);
});
test('legacy rules without a track mode remain sequential and no family filter changes the supplied exact selection',async()=>{
 const rules=defaultSmartRules();delete rules.track_mode;delete rules.genre_family;rules.limit=3;rules.genre='Jazz / Rock';const f=fixture({tracks:async id=>[group(id+'1'),group(id+'2'),group(id+'3')]});const value=await f.service.preview(rules);assert.deepEqual(value.tracks.map(t=>t.media_locator.uri),['Local/a1.flac','Local/a2.flac','Local/a3.flac']);assert.equal(f.calls.filter(c=>c[0]==='tracks').length,1);
});
test('family groups cannot leak into Plex, combine with raw tags, or accept invented groups',async()=>{
 for(const rules of [{...defaultSmartRules(),genre_family:'invented'},{...defaultSmartRules(),genre_family:'jazz',genre:'Jazz'},{...defaultSmartRules({kind:'plex',server_id:'server-a',library_id:'7'}),genre_family:'jazz'}]){
  let reads=0;await assert.rejects(helpers.assembleSmartPreview({albums,matched_albums:2},rules,async()=>{reads++;return[]}),e=>e==='SMART_PLAYLIST_INVALID');assert.equal(reads,0);
 }
});
test('Other includes unidentified metadata, and a missing family succeeds with an empty selection and no track reads',async()=>{
 const selected=[{...albums[0],genres:['16']},{...albums[1],genres:['Reggae']}];const f=fixture({invoke:async()=>({albums:selected,matched_albums:2})});const other=await f.service.preview({...defaultSmartRules(),genre_family:'other'});assert.equal(other.matched_albums,1);assert.equal(other.tracks.length,1);const previousReads=f.calls.filter(c=>c[0]==='tracks').length;const empty=await f.service.preview({...defaultSmartRules(),genre_family:'jazz'});assert.equal(empty.tracks.length,0);assert.equal(empty.matched_albums,0);assert.equal(f.calls.filter(c=>c[0]==='tracks').length,previousReads);
});

test('Navidrome smart selections vary albums, preserve durable identity, and reject changed accounts and sessions',async()=>{
 const rules={...defaultSmartRules({kind:'navidrome',account_id:'a'.repeat(64)}),limit:2};
 const f=fixture();const result=await f.service.preview(rules);assert.deepEqual(result.tracks.map(t=>t.media_locator.song_id),['song-a','song-b']);assert.ok(result.tracks.every(t=>t.media_locator.account_id==='a'.repeat(64)));assert.equal(f.calls.filter(c=>c[0]==='plexTracks'||c[0]==='tracks').length,0);
 const other=fixture({navStatus:async()=>({connection_id:'other',account_id:'b'.repeat(64)})});await assert.rejects(other.service.preview(rules),e=>e==='SMART_PLAYLIST_SOURCE');assert.equal(other.calls.filter(c=>c[0]==='navAlbum').length,0);
 let reads=0;const changed=fixture({navStatus:async()=>({connection_id:++reads===1?'old':'new',account_id:'a'.repeat(64)})});await assert.rejects(changed.service.preview(rules),e=>e==='SMART_PLAYLIST_SOURCE');
});
