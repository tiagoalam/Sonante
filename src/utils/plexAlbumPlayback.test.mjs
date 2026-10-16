import test from 'node:test';
import assert from 'node:assert/strict';
import { playPlexAlbum } from './plexAlbumPlayback.ts';
const album={rating_key:'12',title:'Album',artist:'Artist',server_id:'server',library_id:'2'};
const track={title:'Track',duration_ms:123000,media_locator:{kind:'plex',server_id:'server',rating_key:'13',part_key:'/library/parts/13/file'}};
test('album play uses the real identity, awaits submission and maps existing metadata',async()=>{
 let key,submitted;await playPlexAlbum(album,async value=>{key=value;return[track];},async(tracks,index)=>{submitted={tracks,index};},()=>true);
 assert.equal(key,'12');assert.equal(submitted.index,0);assert.equal(submitted.tracks[0].duration,123);assert.equal(submitted.tracks[0].media_locator,track.media_locator);
 await assert.rejects(playPlexAlbum(album,async()=>[track],async()=>{throw Error('submission failed');},()=>true),/submission failed/);
});
test('late album replies after context or availability changes never submit; foreign server and empty replies cannot play',async()=>{
 let resolve,current=true,plays=0;
 const pending=playPlexAlbum(album,()=>new Promise(r=>resolve=r),async()=>plays++,()=>current);
 current=false;resolve([track]);await pending;assert.equal(plays,0);
 await playPlexAlbum(album,()=>assert.fail('unavailable must not load'),async()=>plays++,()=>false);
 await assert.rejects(playPlexAlbum(album,async()=>[{...track,media_locator:{...track.media_locator,server_id:'foreign'}}],async()=>plays++,()=>true),/PLEX_DISCOVER_CONTEXT/);
 await playPlexAlbum(album,async()=>[],async()=>plays++,()=>true);assert.equal(plays,0);
});
