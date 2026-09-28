export interface LocalItem {
  item_type: "directory" | "file";
  path: string;
  name: string;
  title?: string;
  artist?: string;
  album?: string;
  duration?: number;
}
