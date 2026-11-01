export interface NavidromeStatus {
    connection_id: string | null;
    account_id?: string | null;
    server: string | null;
    username: string | null;
}
export interface NavidromeAlbum {
    id: string;
    title: string;
    artist: string;
    year: number | null;
    cover_id: string | null;
    song_count: number | null;
}
export interface NavidromeSong {
    id: string;
    title: string;
    artist: string;
    album: string;
    duration: number | null;
    disc: number;
    track: number;
}
export interface NavidromeDetail {
    album: NavidromeAlbum;
    songs: NavidromeSong[];
    artist_id?: string | null;
}
export interface NavidromePlayerNavigation {
    connection_id: string; song_id: string; account_id: string; album_id: string; artist_id: string | null;
}
export interface NavidromePage {
    albums: NavidromeAlbum[];
    offset: number;
    more: boolean;
    total?: number | null;
}
export type NavidromeSort = 'recent' | 'name';

export interface NavidromeArtist {id:string;name:string;album_count:number|null}
export interface NavidromeSearchSong extends NavidromeSong {album_id:string|null}
export interface NavidromeSearchResults {connection_id:string;account_id:string;artists:NavidromeArtist[];albums:NavidromeAlbum[];songs:NavidromeSearchSong[]}
export interface NavidromeArtistDetail {artist:NavidromeArtist;albums:NavidromeAlbum[]}
export interface NavidromeExplore {suggested:NavidromeAlbum[];added:NavidromeAlbum[]}
