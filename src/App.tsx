import React, { useEffect, useState, useRef } from "react";
import {
  Disc3,
  Folder,
  Server,
  Sparkles,
  Play,
  ArrowUpDown,
  Layers,
  Settings,
  ArrowLeft,
  Search,
  X,
  User,
  Info,
  RefreshCw,
  CheckCircle2,
  Heart,
} from "lucide-react";
import { PlayerBar } from "./components/PlayerBar";
import { AlbumView } from "./components/AlbumView";
import { ArtistView } from "./components/ArtistView";
import { QueueDrawer } from "./components/QueueDrawer";
import { LocalBrowserView } from "./components/LocalBrowserView";
import { FavoritesView } from "./components/FavoritesView";
import { SettingsModal } from "./components/SettingsModal";
import { AboutModal } from "./components/AboutModal";
import { WelcomeWizard } from "./components/WelcomeWizard";
import { plexService } from "./services/plex";
import { audioService } from "./services/audio";
import { configService } from "./services/config";
import { favoritesService } from "./services/favorites";
import {
  PlexLibrary,
  PlexAlbum,
  PlexCollection,
  SelectedArtist,
  PlexSearchResults,
} from "./types/plex";
import { PlaybackStatus, AudioDevice } from "./types/audio";
import { AppConfig } from "./types/config";
import { FavoriteAlbum } from "./types/favorite";

export function App() {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [mediaSource, setMediaSource] = useState<"plex" | "local" | "favorites">("local");

  const [libraries, setLibraries] = useState<PlexLibrary[]>([]);
  const [selectedLibrary, setSelectedLibrary] = useState<PlexLibrary | null>(null);
  const [activeTab, setActiveTab] = useState<"library" | "collections">("library");
  const [hasCollections, setHasCollections] = useState(false);
  const [sortBy, setSortBy] = useState<string>("added");
  const [showSettings, setShowSettings] = useState(false);
  const [showAbout, setShowAbout] = useState(false);

  // Favoritos Plex (IDs em cache para marcação rápida nos cards)
  const [plexFavIds, setPlexFavIds] = useState<Set<string>>(new Set());

  // Fila e status global
  const [showQueue, setShowQueue] = useState(false);
  const [playbackStatus, setPlaybackStatus] = useState<PlaybackStatus>({
    state: "stop",
    elapsed: 0.0,
    duration: 0.0,
    audio_format: "",
    current_file: "",
    title: "",
    artist: "",
    album: "",
    thumb: null,
    volume: 100,
    is_updating: false,
  });

  const statusRef = useRef(playbackStatus);
  useEffect(() => {
    statusRef.current = playbackStatus;
  }, [playbackStatus]);

  // Estados de listagem normal
  const [albums, setAlbums] = useState<PlexAlbum[]>([]);
  const [collections, setCollections] = useState<PlexCollection[]>([]);

  // Estados de navegação interna
  const [activeCollection, setActiveCollection] = useState<PlexCollection | null>(null);
  const [collectionAlbums, setCollectionAlbums] = useState<PlexAlbum[]>([]);
  const [activeAlbum, setActiveAlbum] = useState<PlexAlbum | null>(null);
  const [activeArtist, setActiveArtist] = useState<SelectedArtist | null>(null);

  // Busca Global
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<PlexSearchResults | null>(null);
  const [isSearching, setIsSearching] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const [loading, setLoading] = useState(false);

  // Carregar Configurações e Dispositivos
  useEffect(() => {
    Promise.all([configService.getConfig(), configService.getAudioDevices()])
      .then(([cfg, devs]) => {
        setConfig(cfg);
        setDevices(devs);
        if (cfg.plex_token && cfg.plex_token.trim().length > 0) {
          setMediaSource("plex");
        } else {
          setMediaSource("local");
        }
      })
      .catch(console.error);

    favoritesService
      .getFavorites()
      .then((favs) => {
        setPlexFavIds(new Set(favs.filter((f) => f.source === "plex").map((f) => f.id)));
      })
      .catch(console.error);
  }, []);

  // Telemetria global (1s)
  useEffect(() => {
    const update = async () => {
      try {
        const s = await audioService.getStatus();
        setPlaybackStatus(s);
      } catch (err) {
        console.error("Erro ao sincronizar status global:", err);
      }
    };
    update();
    const interval = setInterval(update, 1000);
    return () => clearInterval(interval);
  }, []);

  // Título Dinâmico da Janela
  useEffect(() => {
    if (playbackStatus.state === "play" && playbackStatus.title) {
      const trackLabel = playbackStatus.artist
        ? `${playbackStatus.artist} — ${playbackStatus.title}`
        : playbackStatus.title;
      audioService.setWindowTitle(`Sonante • ${trackLabel}`).catch(console.error);
    } else {
      audioService.setWindowTitle("Sonante").catch(console.error);
    }
  }, [playbackStatus.state, playbackStatus.title, playbackStatus.artist]);

  // Atalhos de Teclado Globais
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const activeEl = document.activeElement;
      const isInput =
        activeEl?.tagName === "INPUT" ||
        activeEl?.tagName === "SELECT" ||
        activeEl?.tagName === "TEXTAREA";

      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
        return;
      }

      if (e.key === "Escape") {
        if (showAbout) {
          setShowAbout(false);
        } else if (showSettings) {
          setShowSettings(false);
        } else if (showQueue) {
          setShowQueue(false);
        } else if (searchQuery) {
          setSearchQuery("");
        } else if (activeEl instanceof HTMLElement) {
          activeEl.blur();
        }
        return;
      }

      if (isInput) return;

      const current = statusRef.current;

      if (e.code === "Space") {
        e.preventDefault();
        audioService.togglePlay().catch(console.error);
      } else if (e.code === "ArrowRight") {
        e.preventDefault();
        audioService.next().catch(console.error);
      } else if (e.code === "ArrowLeft") {
        e.preventDefault();
        audioService.previous().catch(console.error);
      } else if (e.code === "ArrowUp") {
        e.preventDefault();
        const nextVol = Math.min((current.volume ?? 100) + 5, 100);
        audioService.setVolume(nextVol).catch(console.error);
      } else if (e.code === "ArrowDown") {
        e.preventDefault();
        const prevVol = Math.max((current.volume ?? 100) - 5, 0);
        audioService.setVolume(prevVol).catch(console.error);
      } else if (e.key.toLowerCase() === "m") {
        e.preventDefault();
        const currentVol = current.volume ?? 100;
        audioService.setVolume(currentVol > 0 ? 0 : 100).catch(console.error);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [showSettings, showAbout, showQueue, searchQuery]);

  // Carregar bibliotecas do Plex
  useEffect(() => {
    if (!config?.plex_token) return;
    plexService
      .getLibraries()
      .then((libs) => {
        setLibraries(libs);
        if (libs.length > 0) {
          setSelectedLibrary(libs[0]);
        }
      })
      .catch(console.error);
  }, [config?.plex_token]);

  // Atualizar coleções da biblioteca ativa
  useEffect(() => {
    if (!selectedLibrary) return;
    setActiveAlbum(null);
    setActiveCollection(null);
    setActiveArtist(null);
    setSearchQuery("");
    setSearchResults(null);

    plexService
      .getCollections(selectedLibrary.key)
      .then((cols) => {
        const available = cols.length > 0;
        setHasCollections(available);
        if (!available && activeTab === "collections") {
          setActiveTab("library");
        }
      })
      .catch(() => setHasCollections(false));
  }, [selectedLibrary]);

  // Carregar álbuns/coleções do Plex
  useEffect(() => {
    if (!selectedLibrary || searchQuery.trim().length > 0 || mediaSource !== "plex") return;

    setLoading(true);
    if (activeTab === "library") {
      plexService
        .getAlbums(selectedLibrary.key, sortBy)
        .then(setAlbums)
        .catch(console.error)
        .finally(() => setLoading(false));
    } else {
      plexService
        .getCollections(selectedLibrary.key)
        .then(setCollections)
        .catch(console.error)
        .finally(() => setLoading(false));
    }
  }, [selectedLibrary, activeTab, sortBy, searchQuery, mediaSource]);

  // Busca Plex
  useEffect(() => {
    const q = searchQuery.trim();
    if (q.length === 0 || mediaSource !== "plex") {
      setSearchResults(null);
      setIsSearching(false);
      return;
    }

    setIsSearching(true);
    const timer = setTimeout(() => {
      plexService
        .search(q, selectedLibrary?.key)
        .then((res) => setSearchResults(res))
        .catch(console.error)
        .finally(() => setIsSearching(false));
    }, 300);

    return () => clearTimeout(timer);
  }, [searchQuery, selectedLibrary, mediaSource]);

  const handleSelectCollection = async (col: PlexCollection) => {
    setActiveCollection(col);
    setLoading(true);
    try {
      const items = await plexService.getCollectionAlbums(col.rating_key);
      setCollectionAlbums(items);
    } catch (err) {
      console.error("Falha ao carregar álbuns da coleção:", err);
    } finally {
      setLoading(false);
    }
  };

  const handlePlayQuick = async (e: React.MouseEvent, album: PlexAlbum) => {
    e.stopPropagation();
    try {
      const tracks = await plexService.getAlbumTracks(album.rating_key);
      const metaTracks = tracks.map((t) => ({
        title: t.title,
        artist: album.artist,
        album: album.title,
        thumb: t.thumb || album.thumb || null,
        uri: t.play_uri,
      }));
      if (metaTracks.length > 0) {
        audioService.playTracks(metaTracks, 0);
      }
    } catch (err) {
      console.error("Falha na reprodução rápida:", err);
    }
  };
  
  const handleTogglePlexCardFav = async (e: React.MouseEvent, album: PlexAlbum) => {
    e.stopPropagation();
    const favItem: FavoriteAlbum = {
      id: album.rating_key,
      source: "plex",
      title: album.title,
      artist: album.artist,
      year: album.year != null ? String(album.year) : undefined, // <-- Conversão segura para string
      thumb: album.thumb || null,
      path_or_key: album.rating_key,
      exists: true,
    };
    try {
      const added = await favoritesService.toggleFavorite(favItem);
      setPlexFavIds((prev) => {
        const next = new Set(prev);
        if (added) next.add(album.rating_key);
        else next.delete(album.rating_key);
        return next;
      });
    } catch (err) {
      console.error("Erro ao favoritar no Plex:", err);
    }
  };

  const renderAlbumCard = (album: PlexAlbum) => {
    const isFav = plexFavIds.has(album.rating_key);
    return (
      <div
        key={album.rating_key}
        onClick={() => setActiveAlbum(album)}
        className="group flex flex-col cursor-pointer relative"
      >
        <div className="relative aspect-square w-full rounded-lg bg-[#202020] overflow-hidden mb-2.5 shadow-md">
          {album.thumb ? (
            <img
              src={album.thumb}
              alt={album.title}
              className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
              loading="lazy"
            />
          ) : (
            <div className="w-full h-full flex items-center justify-center text-[#444444]">
              <Disc3 size={40} />
            </div>
          )}
	{/* Botão de Coração sobre a Capa */}
        <button
          onClick={(e) => handleTogglePlexCardFav(e, album)}
          className={`absolute top-2 right-2 p-1.5 rounded-full backdrop-blur-xs transition-transform active:scale-90 cursor-pointer shadow z-20 ${
            isFav
              ? "bg-black/60 text-[#E5A00D]"
              : "bg-black/40 text-white/70 hover:text-white opacity-0 group-hover:opacity-100"
          }`}
          title={isFav ? "Remover dos favoritos" : "Adicionar aos favoritos"}
        >
          <Heart size={14} fill={isFav ? "#E5A00D" : "none"} />
        </button>

        <div className="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center pointer-events-none">
          <button
            onClick={(e) => handlePlayQuick(e, album)}
            className="w-12 h-12 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black flex items-center justify-center shadow-lg transition-transform active:scale-95 cursor-pointer pointer-events-auto"
            title="Tocar Álbum"
          >
            <Play size={20} className="ml-1" fill="black" />
          </button>
        </div>
        </div>

        <span className="text-sm font-semibold text-white truncate" title={album.title}>
          {album.title}
        </span>
        <button
          onClick={(e) => {
            e.stopPropagation();
            if (album.artist_rating_key) {
              setActiveArtist({ rating_key: album.artist_rating_key, name: album.artist });
            }
          }}
          className="text-xs text-[#999999] hover:text-[#E5A00D] transition-colors truncate mt-0.5 text-left cursor-pointer"
        >
          {album.artist}
        </button>
        {album.year && (
          <span className="text-[11px] text-[#666666] mt-0.5">
            {album.year}
          </span>
        )}
      </div>
    );
  };

  // Se for o primeiro acesso, renderiza o Wizard
  if (config && config.first_run) {
    return (
      <WelcomeWizard
        initialConfig={config}
        devices={devices}
        onFinish={(newConfig) => {
          setConfig(newConfig);
          if (newConfig.plex_token && newConfig.plex_token.trim().length > 0) {
            setMediaSource("plex");
          } else {
            setMediaSource("local");
          }
        }}
      />
    );
  }

  return (
    <div className="h-screen w-screen flex flex-col bg-[#121212] text-[#E0E0E0] overflow-hidden select-none">
      <div className="flex-1 flex overflow-hidden">
        {/* Barra Lateral */}
        <aside className="w-64 bg-[#181818] border-r border-[#262626] flex flex-col p-4 shrink-0">
          <div className="flex items-center space-x-2.5 px-2 py-3 mb-6">
            <Sparkles className="text-[#E5A00D]" size={22} />
            <h1 className="text-lg font-black tracking-wider text-[#E5A00D]">SONANTE</h1>
          </div>

          <div className="text-[11px] font-bold text-[#666666] tracking-wider uppercase px-2 mb-2">
            Fontes de Mídia
          </div>

          <nav className="space-y-1 mb-6">
            {/* Opção Armazenamento Local com Indicador de Sincronização */}
            <button
              onClick={() => {
                setMediaSource("local");
                setActiveAlbum(null);
                setActiveArtist(null);
              }}
              className={`w-full flex items-center justify-between px-3 py-2.5 rounded-md text-sm font-medium transition-colors cursor-pointer ${
                mediaSource === "local"
                  ? "bg-[#242424] text-white"
                  : "text-[#888888] hover:bg-[#202020] hover:text-white"
              }`}
            >
              <div className="flex items-center space-x-3">
                <Folder size={16} className={mediaSource === "local" ? "text-[#E5A00D]" : ""} />
                <span>Armazenamento Local</span>
              </div>

              {playbackStatus.is_updating ? (
                <div className="flex items-center space-x-1 text-[#E5A00D]" title="Indexando pastas locais...">
                  <RefreshCw size={13} className="animate-spin" />
                </div>
              ) : (
                <div className="flex items-center text-[#4BB543]/80" title="Biblioteca Sincronizada">
                  <CheckCircle2 size={13} />
                </div>
              )}
            </button>

            {/* Opção Servidor Plex */}
            <button
              onClick={() => {
                setMediaSource("plex");
                setActiveAlbum(null);
                setActiveArtist(null);
              }}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-md text-sm font-medium transition-colors cursor-pointer ${
                mediaSource === "plex"
                  ? "bg-[#242424] text-white"
                  : "text-[#888888] hover:bg-[#202020] hover:text-white"
              }`}
            >
              <Server size={16} className={mediaSource === "plex" ? "text-[#E5A00D]" : ""} />
              <span>Servidor Plex</span>
            </button>

            {/* Opção Favoritos */}
            <button
              onClick={() => {
                setMediaSource("favorites");
                setActiveAlbum(null);
                setActiveArtist(null);
              }}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-md text-sm font-medium transition-colors cursor-pointer ${
                mediaSource === "favorites"
                  ? "bg-[#242424] text-white"
                  : "text-[#888888] hover:bg-[#202020] hover:text-white"
              }`}
            >
              <Heart
                size={16}
                className={mediaSource === "favorites" ? "text-[#E5A00D]" : ""}
                fill={mediaSource === "favorites" ? "#E5A00D" : "none"}
              />
              <span>Favoritos</span>
            </button>
          </nav>

          {/* Bibliotecas de áudio Plex */}
          {mediaSource === "plex" && (
            <>
              <div className="text-[11px] font-bold text-[#666666] tracking-wider uppercase px-2 mb-2">
                Bibliotecas de áudio Plex
              </div>

              <div className="flex-1 overflow-y-auto space-y-1 pr-1 text-sm">
                {libraries.length === 0 ? (
                  <span className="text-xs text-[#666666] px-2 block">Nenhuma biblioteca encontrada.</span>
                ) : (
                  libraries.map((lib) => {
                    const isSelected = selectedLibrary?.key === lib.key;
                    return (
                      <button
                        key={lib.key}
                        onClick={() => {
                          setSelectedLibrary(lib);
                          setActiveAlbum(null);
                          setActiveCollection(null);
                          setActiveArtist(null);
                          setSearchQuery("");
                        }}
                        className={`w-full text-left px-3 py-2 rounded-md truncate transition-colors cursor-pointer ${
                          isSelected
                            ? "bg-[#332B15] text-[#E5A00D] font-bold"
                            : "text-[#CCCCCC] hover:bg-[#202020] hover:text-white"
                        }`}
                      >
                        {lib.title}
                      </button>
                    );
                  })
                )}
              </div>
            </>
          )}

          {mediaSource !== "plex" && <div className="flex-1" />}

          {/* Rodapé Lateral: Preferências & Sobre */}
          <div className="pt-3 border-t border-[#262626] mt-auto space-y-1">
            <button
              onClick={() => setShowSettings(true)}
              className="w-full flex items-center space-x-3 px-3 py-2 rounded-md text-xs font-semibold text-[#888888] hover:bg-[#202020] hover:text-white transition-colors cursor-pointer"
            >
              <Settings size={15} />
              <span>Preferências</span>
            </button>

            <button
              onClick={() => setShowAbout(true)}
              className="w-full flex items-center space-x-3 px-3 py-2 rounded-md text-xs font-semibold text-[#888888] hover:bg-[#202020] hover:text-[#E5A00D] transition-colors cursor-pointer"
            >
              <Info size={15} />
              <span>Sobre o Sonante</span>
            </button>
          </div>
        </aside>

        {/* Painel Central */}
        {mediaSource === "favorites" ? (
          <FavoritesView />
        ) : mediaSource === "local" ? (
          <LocalBrowserView />
        ) : activeArtist ? (
          <ArtistView
            artist={activeArtist}
            onBack={() => setActiveArtist(null)}
            onSelectAlbum={(alb) => setActiveAlbum(alb)}
          />
        ) : activeAlbum ? (
          <AlbumView
            album={activeAlbum}
            onBack={() => setActiveAlbum(null)}
            onSelectArtist={(art) => {
              setActiveAlbum(null);
              setActiveArtist(art);
            }}
          />
        ) : (
          <main className="flex-1 flex flex-col overflow-hidden bg-[#121212]">
            {/* Topo com Título e Barra de Busca */}
            <div className="flex items-center justify-between p-8 pb-4 border-b border-[#222222]">
              <div className="flex items-center space-x-3">
                {activeCollection && (
                  <button
                    onClick={() => setActiveCollection(null)}
                    className="p-1.5 rounded-lg bg-[#1E1E1E] border border-[#333333] hover:bg-[#2A2A2A] text-white transition-colors cursor-pointer mr-1"
                    title="Voltar para Coleções"
                  >
                    <ArrowLeft size={16} />
                  </button>
                )}
                <div>
                  <h2 className="text-2xl font-bold text-white tracking-tight">
                    {searchQuery.trim().length > 0
                      ? `Resultados para "${searchQuery}"`
                      : activeCollection
                      ? activeCollection.title
                      : selectedLibrary?.title || "Carregando..."}
                  </h2>
                </div>
              </div>

              {/* Barra de Pesquisa */}
              <div className="flex items-center space-x-4">
                <div className="relative flex items-center w-72">
                  <Search size={15} className="absolute left-3 text-[#666666]" />
                  <input
                    ref={searchInputRef}
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder="Pesquisar artistas, álbuns... (Ctrl+F)"
                    className="w-full bg-[#1A1A1A] border border-[#2B2B2B] rounded-lg pl-9 pr-8 py-1.5 text-xs text-white placeholder-[#666666] outline-none focus:border-[#E5A00D] transition-colors"
                  />
                  {searchQuery && (
                    <button
                      onClick={() => setSearchQuery("")}
                      className="absolute right-2.5 text-[#666666] hover:text-white cursor-pointer"
                    >
                      <X size={14} />
                    </button>
                  )}
                </div>

                {!activeCollection && searchQuery.trim().length === 0 && (
                  <>
                    {activeTab === "library" && (
                      <div className="flex items-center space-x-2 bg-[#1E1E1E] px-3 py-1.5 rounded-lg border border-[#333333] text-xs text-[#CCCCCC]">
                        <ArrowUpDown size={14} className="text-[#888888]" />
                        <select
                          value={sortBy}
                          onChange={(e) => setSortBy(e.target.value)}
                          className="bg-transparent border-none outline-none text-white cursor-pointer"
                        >
                          <option value="added">Adicionados Recentemente</option>
                          <option value="title">Título (A-Z)</option>
                          <option value="year">Ano de Lançamento</option>
                        </select>
                      </div>
                    )}

                    <div className="flex bg-[#1E1E1E] p-1 rounded-lg border border-[#333333]">
                      <button
                        onClick={() => {
                          setActiveTab("library");
                          setActiveCollection(null);
                        }}
                        className={`px-4 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                          activeTab === "library"
                            ? "bg-[#E5A00D] text-black shadow"
                            : "text-[#999999] hover:text-white"
                        }`}
                      >
                        Biblioteca
                      </button>

                      {hasCollections && (
                        <button
                          onClick={() => {
                            setActiveTab("collections");
                            setActiveCollection(null);
                          }}
                          className={`px-4 py-1.5 rounded-md text-xs font-semibold transition-all cursor-pointer ${
                            activeTab === "collections"
                              ? "bg-[#E5A00D] text-black shadow"
                            : "text-[#999999] hover:text-white"
                          }`}
                        >
                          Coleções
                        </button>
                      )}
                    </div>
                  </>
                )}
              </div>
            </div>

            {/* Grid Principal */}
            <div className="flex-1 overflow-y-auto p-8">
              {searchQuery.trim().length > 0 ? (
                isSearching ? (
                  <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                    Buscando na biblioteca...
                  </div>
                ) : searchResults &&
                  (searchResults.artists.length > 0 ||
                    searchResults.albums.length > 0 ||
                    searchResults.tracks.length > 0) ? (
                  <div className="space-y-8">
                    {searchResults.artists.length > 0 && (
                      <div>
                        <h3 className="text-sm font-bold text-[#888888] uppercase tracking-wider mb-3">
                          Artistas
                        </h3>
                        <div className="flex flex-wrap gap-3">
                          {searchResults.artists.map((art) => (
                            <button
                              key={art.rating_key}
                              onClick={() =>
                                setActiveArtist({ rating_key: art.rating_key, name: art.name })
                              }
                              className="flex items-center space-x-3 bg-[#181818] border border-[#262626] hover:border-[#E5A00D] rounded-full pl-1.5 pr-4 py-1.5 transition-colors cursor-pointer"
                            >
                              <div className="w-8 h-8 rounded-full bg-[#242424] overflow-hidden flex items-center justify-center">
                                {art.thumb ? (
                                  <img src={art.thumb} alt="" className="w-full h-full object-cover" />
                                ) : (
                                  <User size={14} className="text-[#666666]" />
                                )}
                              </div>
                              <span className="text-xs font-bold text-white">{art.name}</span>
                            </button>
                          ))}
                        </div>
                      </div>
                    )}

                    {searchResults.albums.length > 0 && (
                      <div>
                        <h3 className="text-sm font-bold text-[#888888] uppercase tracking-wider mb-3">
                          Álbuns
                        </h3>
                        <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                          {searchResults.albums.map(renderAlbumCard)}
                        </div>
                      </div>
                    )}

                    {searchResults.tracks.length > 0 && (
                      <div>
                        <h3 className="text-sm font-bold text-[#888888] uppercase tracking-wider mb-3">
                          Faixas
                        </h3>
                        <div className="divide-y divide-[#1A1A1A] bg-[#141414] rounded-xl border border-[#222222] p-2">
                          {searchResults.tracks.map((track) => (
                            <div
                              key={track.rating_key}
                              onClick={() => {
                                const meta = [
                                  {
                                    title: track.title,
                                    artist: track.album_title || "Plex Track",
                                    album: track.album_title || "",
                                    thumb: track.thumb || null,
                                    uri: track.play_uri,
                                  },
                                ];
                                audioService.playTracks(meta, 0);
                              }}
                              className="flex items-center justify-between p-2.5 rounded-lg hover:bg-[#1E1E1E] transition-colors cursor-pointer group"
                            >
                              <div className="flex items-center space-x-3 min-w-0 pr-4">
                                <div className="w-9 h-9 rounded bg-[#202020] overflow-hidden shrink-0">
                                  {track.thumb ? (
                                    <img src={track.thumb} alt="" className="w-full h-full object-cover" />
                                  ) : (
                                    <div className="w-full h-full flex items-center justify-center text-[#444444]">
                                      <Disc3 size={16} />
                                    </div>
                                  )}
                                </div>
                                <div className="flex flex-col min-w-0">
                                  <span className="text-xs font-bold text-white group-hover:text-[#E5A00D] transition-colors truncate">
                                    {track.title}
                                  </span>
                                  {track.album_title && (
                                    <span className="text-[11px] text-[#777777] truncate">
                                      {track.album_title}
                                    </span>
                                  )}
                                </div>
                              </div>
                              <Play size={14} className="text-[#888888] group-hover:text-white shrink-0 mr-2" />
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>
                ) : (
                  <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
                    Nenhum resultado encontrado para "{searchQuery}"
                  </div>
                )
              ) : loading ? (
                <div className="h-full flex items-center justify-center text-[#666666]">
                  Carregando mídias...
                </div>
              ) : activeCollection ? (
                <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                  {collectionAlbums.map(renderAlbumCard)}
                </div>
              ) : activeTab === "library" ? (
                <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                  {albums.map(renderAlbumCard)}
                </div>
              ) : (
                <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-6">
                  {collections.map((col) => (
                    <div
                      key={col.rating_key}
                      onClick={() => handleSelectCollection(col)}
                      className="flex flex-col cursor-pointer group"
                    >
                      {col.thumb ? (
                        <div className="relative aspect-square w-full rounded-lg bg-[#202020] overflow-hidden mb-2.5 shadow-md">
                          <img
                            src={col.thumb}
                            alt={col.title}
                            className="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
                            loading="lazy"
                          />
                        </div>
                      ) : (
                        <div className="relative aspect-square w-full rounded-lg bg-gradient-to-br from-[#242424] via-[#1B1B1B] to-[#121212] border border-[#2B2B2B] overflow-hidden mb-2.5 shadow-md flex flex-col items-center justify-center p-4 text-center group-hover:border-[#E5A00D] transition-colors">
                          <Layers size={36} className="text-[#E5A00D] opacity-80 mb-2" />
                          <span className="text-xs font-bold text-[#AAAAAA] line-clamp-2">
                            {col.title}
                          </span>
                        </div>
                      )}

                      <span className="text-sm font-semibold text-white truncate" title={col.title}>
                        {col.title}
                      </span>
                      <span className="text-xs text-[#E5A00D] font-bold mt-0.5">
                        {col.child_count} {col.child_count === 1 ? "álbum" : "itens"}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </main>
        )}
      </div>

      <PlayerBar
        onToggleQueue={() => setShowQueue(!showQueue)}
        isQueueOpen={showQueue}
      />

      <QueueDrawer
        isOpen={showQueue}
        onClose={() => setShowQueue(false)}
        status={playbackStatus}
      />

      {showSettings && (
        <SettingsModal
          onClose={() => setShowSettings(false)}
          onSaved={() => {
            configService.getConfig().then(setConfig).catch(console.error);
            plexService.getLibraries().then(setLibraries).catch(console.error);
          }}
        />
      )}

      {showAbout && <AboutModal onClose={() => setShowAbout(false)} />}
    </div>
  );
}

export default App;
