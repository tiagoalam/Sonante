export interface RadioStation {
  id: string; name: string; country_code: string; tags: string[]; codec: string; bitrate: number;
  favicon: string | null; stream_url: string; manual: boolean;
}
export interface RadioQuery { name: string; country: string; genre: string; offset: number }
export interface RadioPage { stations: RadioStation[]; has_more: boolean }
export interface RadioChoice { name: string; stationcount: number }
export interface RadioFilters { countries: RadioChoice[]; genres: RadioChoice[] }
