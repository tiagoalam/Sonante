import React, { useEffect, useState } from "react";
import { Folder, Music, Play, ArrowLeft, Disc3, Clock, Home } from "lucide-react";
import { LocalItem } from "../types/local";
import { audioService } from "../services/audio";

export const LocalBrowserView: React.FC = () => {
  const [currentPath, setCurrentPath] = useState<string>("");
  const [items, setItems] = useState<LocalItem[]>([]);
  const [folderCover, setFolderCover] = useState<string | null>(null);
  const [loading, setLoading] = useState<boolean>(true);

  const loadDirectory = async (path: string) => {
    setLoading(true);
    try {
      const [data, cover] = await Promise.all([
        audioService.listLocalDirectory(path),
        audioService.getLocalCover(path),
      ]);
      setItems(data);
      setFolderCover(cover);
      setCurrentPath(path);
    } catch (err) {
      console.error("Erro ao listar diretório local:", err);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadDirectory("");
  }, []);

  const handleGoUp = () => {
    if (!currentPath) return;
    const parts = currentPath.split("/").filter(Boolean);
    parts.pop();
    loadDirectory(parts.join("/"));
  };

  const handleFolderClick = (dirPath: string) => {
    loadDirectory(dirPath);
  };

  const formatDuration = (secs?: number) => {
    if (!secs || isNaN(secs)) return "--:--";
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
  };

  const folders = items.filter((i) => i.item_type === "directory");
  const files = items.filter((i) => i.item_type === "file");

  const handlePlayFile = (fileIndex: number) => {
    const metaTracks = files.map((f) => ({
      title: f.title || f.name,
      artist: f.artist || "Arquivo Local",
      album: f.album || (currentPath.split("/").pop() || "Armazenamento Local"),
      thumb: folderCover || null,
      uri: f.path,
    }));

    if (metaTracks.length > 0) {
      audioService.playTracks(metaTracks, fileIndex).catch(console.error);
    }
  };

  const handlePlayAll = () => {
    if (files.length > 0) {
      handlePlayFile(0);
    }
  };

  const pathParts = currentPath ? currentPath.split("/").filter(Boolean) : [];

  return (
    <main className="flex-1 flex flex-col overflow-hidden bg-[#121212] select-none">
      {/* Cabeçalho com Suporte a Capa do Álbum Local */}
      <div className="p-8 pb-5 border-b border-[#222222] flex items-end justify-between gap-6">
        <div className="flex items-center space-x-5 min-w-0">
          {folderCover ? (
            <div className="w-24 h-24 rounded-lg bg-[#1C1C1C] overflow-hidden border border-[#2B2B2B] shadow-lg shrink-0">
              <img src={folderCover} alt="Capa" className="w-full h-full object-cover" />
            </div>
          ) : (
            <div className="w-16 h-16 rounded-lg bg-[#1A1A1A] border border-[#262626] flex items-center justify-center text-[#E5A00D] shrink-0">
              <Folder size={28} />
            </div>
          )}

          <div className="flex flex-col space-y-1.5 min-w-0">
            <div className="flex items-center space-x-2 text-xs text-[#888888]">
              <button
                onClick={() => loadDirectory("")}
                className="flex items-center space-x-1 hover:text-[#E5A00D] transition-colors cursor-pointer"
              >
                <Home size={14} />
                <span>Raiz</span>
              </button>

              {pathParts.map((part, index) => {
                const fullSubPath = pathParts.slice(0, index + 1).join("/");
                const isLast = index === pathParts.length - 1;
                return (
                  <React.Fragment key={fullSubPath}>
                    <span>/</span>
                    <button
                      onClick={() => loadDirectory(fullSubPath)}
                      className={`hover:text-[#E5A00D] transition-colors truncate max-w-[150px] cursor-pointer ${
                        isLast ? "text-white font-bold" : ""
                      }`}
                    >
                      {part}
                    </button>
                  </React.Fragment>
                );
              })}
            </div>

            <h2 className="text-2xl font-bold text-white tracking-tight truncate">
              {pathParts.length > 0 ? pathParts[pathParts.length - 1] : "Armazenamento Local"}
            </h2>

            {files.length > 0 && (
              <span className="text-xs text-[#888888]">
                {files.length} {files.length === 1 ? "faixa de áudio" : "faixas de áudio"}
              </span>
            )}
          </div>
        </div>

        {files.length > 0 && (
          <button
            onClick={handlePlayAll}
            className="flex items-center space-x-2 px-5 py-2.5 rounded-full bg-[#E5A00D] hover:bg-[#F5B01D] text-black font-bold text-xs shadow-md transition-transform active:scale-95 cursor-pointer shrink-0"
          >
            <Play size={15} fill="black" className="ml-0.5" />
            <span>Tocar Pasta</span>
          </button>
        )}
      </div>

      {/* Conteúdo */}
      <div className="flex-1 overflow-y-auto p-8">
        {loading ? (
          <div className="h-40 flex items-center justify-center text-xs text-[#666666]">
            A carregar diretório...
          </div>
        ) : items.length === 0 ? (
          <div className="h-40 flex flex-col items-center justify-center text-[#666666] space-y-2">
            <Disc3 size={36} className="opacity-40" />
            <span className="text-xs">Nenhum ficheiro de áudio ou subpasta encontrada.</span>
          </div>
        ) : (
          <div className="space-y-6">
            {currentPath && (
              <button
                onClick={handleGoUp}
                className="flex items-center space-x-2 px-3 py-2 rounded-lg bg-[#181818] hover:bg-[#222222] border border-[#2B2B2B] text-xs font-semibold text-[#CCCCCC] hover:text-white transition-colors cursor-pointer w-fit"
              >
                <ArrowLeft size={14} />
                <span>Subir um nível (..)</span>
              </button>
            )}

            {folders.length > 0 && (
              <div>
                <h3 className="text-xs font-bold text-[#888888] uppercase tracking-wider mb-3">
                  Pastas ({folders.length})
                </h3>
                <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
                  {folders.map((folder) => (
                    <div
                      key={folder.path}
                      onClick={() => handleFolderClick(folder.path)}
                      className="flex items-center space-x-3 p-3 rounded-lg bg-[#181818] border border-[#262626] hover:border-[#E5A00D]/60 hover:bg-[#1E1E1E] transition-all cursor-pointer group"
                    >
                      <div className="w-9 h-9 rounded bg-[#252525] flex items-center justify-center text-[#E5A00D] group-hover:scale-105 transition-transform shrink-0">
                        <Folder size={18} />
                      </div>
                      <span className="text-xs font-semibold text-white truncate flex-1" title={folder.name}>
                        {folder.name}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {files.length > 0 && (
              <div>
                <h3 className="text-xs font-bold text-[#888888] uppercase tracking-wider mb-3">
                  Ficheiros de Áudio ({files.length})
                </h3>

                <div className="w-full bg-[#161616] rounded-xl border border-[#222222] overflow-hidden divide-y divide-[#1F1F1F]">
                  <div className="grid grid-cols-[40px_1fr_180px_70px] px-4 py-2.5 text-[11px] font-bold text-[#666666] uppercase tracking-wider bg-[#1A1A1A]">
                    <span className="text-center">#</span>
                    <span>Título</span>
                    <span className="hidden sm:block">Artista / Álbum</span>
                    <span className="text-right flex items-center justify-end">
                      <Clock size={13} />
                    </span>
                  </div>

                  {files.map((file, idx) => (
                    <div
                      key={file.path}
                      onClick={() => handlePlayFile(idx)}
                      className="group grid grid-cols-[40px_1fr_180px_70px] items-center px-4 py-3 text-xs text-[#CCCCCC] hover:bg-[#202020] hover:text-white transition-colors cursor-pointer"
                    >
                      <div className="flex items-center justify-center">
                        <span className="group-hover:hidden text-[#666666] font-mono text-[11px]">
                          {idx + 1}
                        </span>
                        <Play size={13} className="hidden group-hover:block text-[#E5A00D]" fill="#E5A00D" />
                      </div>

                      <div className="flex items-center space-x-2.5 truncate pr-3">
                        <Music size={14} className="text-[#666666] shrink-0" />
                        <span className="truncate font-medium text-white group-hover:text-[#E5A00D] transition-colors">
                          {file.title || file.name}
                        </span>
                      </div>

                      <div className="hidden sm:block truncate text-[#777777] text-[11px] pr-2">
                        {file.artist && file.album
                          ? `${file.artist} — ${file.album}`
                          : file.artist || file.album || "--"}
                      </div>

                      <span className="text-right font-mono text-[11px] text-[#666666]">
                        {formatDuration(file.duration)}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </main>
  );
};
