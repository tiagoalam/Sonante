import type {PlexTrack} from '../types/plex';
export function plexTrackDurationSeconds(track:PlexTrack):number|undefined {
 const value=track.duration_ms!=null?track.duration_ms/1000:track.duration;
 return typeof value==='number'&&Number.isFinite(value)&&value>0?value:undefined;
}
export function plexTrackArtist(track:PlexTrack,fallback=''):string {return track.artist?.trim()||fallback;}
