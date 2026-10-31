import test from 'node:test';
import assert from 'node:assert/strict';
import { queuePresentation, confirmedQueueIndex, formatQueueTime } from './queuePresentation.ts';
import { playConfirmedQueuePosition } from './nowPlayingPresentation.ts';
const local = (uri, extra={}) => ({ title:'Água', artist:'Björk', album:'Coleção', duration:100, media_locator:{kind:'local',uri}, ...extra });
const sequential = {repeat:false,random:false,single:'off'};
const status = (index=0, extra={}) => ({elapsed:25,duration:100,current_media:{kind:'local',uri:'a',queue_index:index},options:sequential,...extra});

test('queue filter matches title, artist and album accent-insensitively, preserving order and duplicate occurrences',()=>{
 const duplicate=local('a'); const items=[duplicate,local('b',{title:'Other',artist:'Else',album:'Different'}),duplicate];
 const before=structuredClone(items), view=queuePresentation(items);
 for(const text of ['agua','BJORK','colecao','agua bjork'])assert.deepEqual(view.filter(text).map(row=>row.index),[0,2]);
 assert.equal(view.filter('agua')[1].item,duplicate);assert.deepEqual(view.filter('absent'),[]);
 assert.deepEqual(view.filter('  ').map(row=>row.index),[0,1,2]);assert.deepEqual(items,before);
});

test('total/remaining times describe the whole queue and preserve repeated tracks, regardless of filtering',()=>{
 const items=[local('a'),local('b',{duration:200}),local('a')];const view=queuePresentation(items);
 assert.deepEqual(view.total,{seconds:400,unknown:0});view.filter('absent');
 assert.deepEqual(view.remaining(status(0),true),{seconds:375,unknown:0});
 assert.deepEqual(view.remaining(status(2),true),{seconds:75,unknown:0});
 assert.equal(confirmedQueueIndex(items,status(2).current_media),2);
 assert.equal(confirmedQueueIndex(items,{kind:'local',uri:'b',queue_index:2}),null);
 assert.deepEqual(view.remaining(status(2,{elapsed:150}),true),{seconds:0,unknown:0});
});

test('unknown/live/bad durations remain unknown instead of silently becoming zero',()=>{
 const items=[local('a'),local('b',{duration:0}),local('c',{duration:NaN}),local('d',{duration:-4}),local('e',{duration:Infinity})];
 const view=queuePresentation(items);assert.deepEqual(view.total,{seconds:100,unknown:4});
 assert.deepEqual(view.remaining(status(),true),{seconds:75,unknown:4});
 const live=queuePresentation([{media_locator:{kind:'upnp',session_id:'s'}}]);
 const remote=status(0,{current_media:{kind:'upnp',session_id:'s',queue_index:0},duration:0,elapsed:0});
 assert.deepEqual(live.total,{seconds:0,unknown:1});assert.deepEqual(live.remaining(remote,true),{seconds:0,unknown:1});
 assert.deepEqual(live.remaining({...remote,duration:200,elapsed:15},true),{seconds:185,unknown:0});
});

test('remaining estimate rejects unknown position, mismatched source, unavailable engine and nonsequential modes',()=>{
 const view=queuePresentation([local('a'),local('b')]);
 for(const options of [null,{...sequential,random:true},{...sequential,repeat:true},{...sequential,single:'on'},{...sequential,single:'oneshot'}])assert.equal(view.remaining(status(0,{options}),true),null);
 for(const extra of [{current_media:null},{current_media:{kind:'local',uri:'changed',queue_index:0}},{snapshot_consistent:false},{elapsed:NaN},{elapsed:-1}])assert.equal(view.remaining(status(0,extra),true),null);
 assert.equal(view.remaining(status(),false),null);
 assert.equal(confirmedQueueIndex([local('a')],{kind:'local',uri:'a',queue_index:-1}),null);
 assert.equal(confirmedQueueIndex([local('a')],{kind:'local',uri:'a',queue_index:0.5}),null);
});

test('Plex, mapped Local and UPnP identities stay distinct and remote sessions are revalidated before queue play',async()=>{
 const items=[local('mapped'),{media_locator:{kind:'plex',server_id:'s',part_key:'p'},duration:200},{media_locator:{kind:'upnp',session_id:'remote'},duration:300}];
 const view=queuePresentation(items);
 assert.deepEqual(view.remaining(status(1,{current_media:{kind:'plex',server_id:'s',part_key:'p',queue_index:1},duration:200}),true),{seconds:475,unknown:0});
 assert.equal(confirmedQueueIndex(items,{kind:'upnp',session_id:'remote',queue_index:2}),2);
 assert.equal(confirmedQueueIndex(items,{kind:'upnp',session_id:'old',queue_index:2}),null);
 const played=[];await playConfirmedQueuePosition(items,2,async()=>structuredClone(items),async index=>played.push(index),()=>true);assert.deepEqual(played,[2]);
 await assert.rejects(playConfirmedQueuePosition(items,2,async()=>[...items.slice(0,2),{media_locator:{kind:'upnp',session_id:'other'}}],async()=>assert.fail('stale session'),()=>true));
});

test('queue time supports multi-hour/day totals and never displays NaN or Infinity',()=>{
 for(const [seconds,value] of [[0,'0:00'],[59.9,'0:59'],[60,'1:00'],[3601,'1:00:01'],[90061,'25:01:01'],[NaN,'—'],[Infinity,'—'],[-1,'—']])assert.equal(formatQueueTime(seconds),value);
 const empty=queuePresentation([]);assert.deepEqual(empty.total,{seconds:0,unknown:0});assert.equal(empty.remaining(status(),true),null);
});
