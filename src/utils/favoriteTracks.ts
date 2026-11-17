import type {FavoriteAlbum} from '../types/favorite';
import type {LocalAlbum,LocalAlbumDiscTracks,LocalItem} from '../types/local';
import {flattenAlbumDiscs} from './localAlbumDiscs.ts';
import {plexTrackDurationSeconds,plexTrackArtist} from './plexTrackMetadata.ts';
import type {PlexTrack} from '../types/plex';
export function favoritePlexTracks(tracks:PlexTrack[],fav:FavoriteAlbum){
 return tracks.map(track=>({title:track.title,artist:plexTrackArtist(track,fav.artist),duration:plexTrackDurationSeconds(track),media_locator:track.media_locator}));
}
export async function favoriteLocalTracks(fav:FavoriteAlbum,service:{getLocalAlbums:()=>Promise<LocalAlbum[]>;getLocalAlbumTracks:(id:string)=>Promise<LocalAlbumDiscTracks[]>;listLocalDirectory:(path:string)=>Promise<LocalItem[]>},sqliteBacked=false) {
 const albums=await service.getLocalAlbums();
 // The opaque virtual path is the durable format marker. IDs alone are not:
 // a legacy Local directory may legitimately have a similar string.
 const sqliteReference=fav.source==='local'&&fav.path_or_key.startsWith('sqlite-album://');
 let candidates:LocalAlbum[];
 if(sqliteReference){
  const exact=albums.find(album=>album.id===fav.id&&album.folder_path===fav.path_or_key);
  if(!exact)throw new Error('SQLITE_FAVORITE_REFERENCE_UNAVAILABLE');
  candidates=[exact];
 }else{
  const exact=sqliteBacked?undefined:albums.find(album=>album.id===fav.id);
  const folder=albums.filter(album=>album.folder_path===fav.path_or_key);
  const named=folder.filter(album=>album.title.trim().toLowerCase()===fav.title.trim().toLowerCase());
  candidates=exact?[exact]:named.length?named:folder;
 }
 if(candidates.length>1)throw new Error('FAVORITE_ALBUM_AMBIGUOUS');
 if(sqliteBacked&&!candidates.length)throw new Error('SQLITE_LEGACY_FAVORITE_REQUIRES_RESOLUTION');
 // Existing catalog resolver preserves disc/track order and CUE virtual URIs.
 const files=candidates.length?flattenAlbumDiscs(await service.getLocalAlbumTracks(candidates[0].id)).tracks:
   (await service.listLocalDirectory(fav.path_or_key)).filter(item=>item.item_type==='file');
 return files.map(file=>({title:file.title||file.name,artist:file.artist||fav.artist,duration:file.duration,uri:file.path}));
}
