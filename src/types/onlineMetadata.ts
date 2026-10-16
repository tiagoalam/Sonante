export type OnlineMetadataError = "invalid_query" | "service_unavailable" | "rate_limited" | "timeout"
  | "http_error" | "invalid_response" | "response_too_large" | "invalid_mbid" | "release_not_found" | "identity_mismatch" | "unknown";
export interface OnlineMetadataQuery { title: string; artist: string }
export interface OnlineMetadataCandidate {
  id: string;
  title: string | null;
  artist_credit: string | null;
  date: string | null;
  country: string | null;
  formats: (string | null)[];
  labels: { name: string | null; catalog_number: string | null }[];
  score: number | null;
}
export interface OnlineMetadataResult {
  candidates: OnlineMetadataCandidate[];
  total: number;
  limited: boolean;
}

export interface OnlineMetadataDetails {
  id: string;
  title: string | null;
  artist_credit: string | null;
  edition_date: string | null;
  release_group_id: string | null;
  first_release_date: string | null;
}
