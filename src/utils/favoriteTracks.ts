import type {FavoriteAlbum} from '../types/favorite';
import type {LocalAlbum,LocalAlbumDiscTracks,LocalItem} from '../types/local';
import {flattenAlbumDiscs} from './localAlbumDiscs';
import {plexTrackDurationSeconds,plexTrackArtist} from './plexTrackMetadata';
import type {PlexTrack} from '../types/plex';
export function favoritePlexTracks(tracks:PlexTrack[],fav:FavoriteAlbum){
 return tracks.map(track=>({title:track.title,artist:plexTrackArtist(track,fav.artist),duration:plexTrackDurationSeconds(track),media_locator:track.media_locator}));
}
export async function favoriteLocalTracks(fav:FavoriteAlbum,service:{getLocalAlbums:()=>Promise<LocalAlbum[]>;getLocalAlbumTracks:(id:string)=>Promise<LocalAlbumDiscTracks[]>;listLocalDirectory:(path:string)=>Promise<LocalItem[]>}) {
 const albums=await service.getLocalAlbums();
 const exact=albums.find(album=>album.id===fav.id);
 const folder=albums.filter(album=>album.folder_path===fav.path_or_key);
 const named=folder.filter(album=>album.title.trim().toLowerCase()===fav.title.trim().toLowerCase());
 const candidates=exact?[exact]:named.length?named:folder;
 if(candidates.length>1)throw new Error('FAVORITE_ALBUM_AMBIGUOUS');
 // Existing catalog resolver preserves disc/track order and CUE virtual URIs.
 const files=candidates.length?flattenAlbumDiscs(await service.getLocalAlbumTracks(candidates[0].id)).tracks:
   (await service.listLocalDirectory(fav.path_or_key)).filter(item=>item.item_type==='file');
 return files.map(file=>({title:file.title||file.name,artist:file.artist||fav.artist,duration:file.duration,uri:file.path}));
}
