import type { RadioPage, RadioQuery, RadioStation } from "../types/radio";
export const RADIO_PAGE_SIZE = 40;
export const radioQueryKey = ({ name, country, genre }: RadioQuery) => JSON.stringify([name.trim(), country, genre.trim().toLowerCase()]);
export const radioStationMatches = (station: RadioStation, query: RadioQuery) => {
  const normalized = (value: string) => value.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLocaleLowerCase();
  return (!query.name || normalized(station.name).includes(normalized(query.name))) && (!query.country || station.country_code === query.country)
    && (!query.genre || station.tags.some(tag => normalized(tag).includes(normalized(query.genre))));
};
export interface RadioDirectorySnapshot { query: RadioQuery; stations: RadioStation[]; phase: "idle" | "loading" | "ready" | "error"; hasMore: boolean }
// A single in-flight directory request. A newer filter invalidates old results
// and queues only the latest query. Never races requests against the music IPCs.
export class RadioDirectorySession {
  private listeners = new Set<() => void>();
  private state: RadioDirectorySnapshot = { query: { name: "", country: "", genre: "", offset: 0 }, stations: [], phase: "idle", hasMore: false };
  private active = false;
  private epoch = 0;
  private running = false;
  private load: (query: RadioQuery) => Promise<RadioPage>;
  constructor(load: (query: RadioQuery) => Promise<RadioPage>) { this.load = load; }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  getSnapshot = () => this.state;
  private publish(state: RadioDirectorySnapshot) { this.state = state; this.listeners.forEach(listener => listener()); }
  set(query: RadioQuery, active: boolean) {
    const same = radioQueryKey(query) === radioQueryKey(this.state.query) && query.offset === this.state.query.offset;
    if (same && active === this.active && this.state.phase !== "idle") return;
    this.epoch++; this.active = active;
    if (!active) return;
    const append = query.offset > 0 && radioQueryKey(query) === radioQueryKey(this.state.query);
    this.publish({ query, stations: append ? this.state.stations : [], phase: "loading", hasMore: append && this.state.hasMore });
    void this.read();
  }
  retry() { if (!this.active || this.running) return; this.epoch++; this.publish({ ...this.state, phase: "loading" }); void this.read(); }
  close() { this.active = false; this.epoch++; }
  private async read() {
    if (this.running || !this.active) return;
    const epoch = this.epoch, query = this.state.query;
    this.running = true;
    try {
      const page = await this.load(query);
      if (!this.active || epoch !== this.epoch) return;
      const stations: RadioStation[] = [];
      const ids = new Set(stations.map(station => station.id));
      for (const station of page.stations) if (!ids.has(station.id)) { ids.add(station.id); stations.push(station); }
      this.publish({ query, stations, phase: "ready", hasMore: page.has_more });
    } catch { if (this.active && epoch === this.epoch) this.publish({ ...this.state, phase: "error" }); }
    finally { this.running = false; if (this.active && epoch !== this.epoch) void this.read(); }
  }
}
