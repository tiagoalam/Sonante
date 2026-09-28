export interface TrackMetadata {
  title: string;
  artist: string;
  album: string;
  thumb?: string | null;
  uri: string;
  duration?: number;
}

export interface AudioDevice {
  id: string;
  name: string;
  is_bitperfect: boolean;
}

export interface PlaybackStatus {
  state: string;
  elapsed: number;
  duration: number;
  audio_format: string;
  current_file: string;
  title: string;
  artist: string;
  album: string;
  thumb?: string | null;
  volume: number;
  is_updating: boolean; // <-- Necessário para o ícone de indexação/sincronização
}

export interface QueueTrack {
  id: number;
  pos: number;
  file: string;
  title?: string;
  artist?: string;
  album?: string;
  duration?: number;
}
