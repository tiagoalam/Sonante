import React from "react";
import { Disc3 } from "lucide-react";

export interface AlbumArtworkPlaceholderProps {
  className?: string;
  size?: "card" | "detail";
}

export const AlbumArtworkPlaceholder: React.FC<AlbumArtworkPlaceholderProps> = ({
  className = "",
  size = "card",
}) => (
  <div
    aria-hidden="true"
    className={`relative flex h-full w-full items-center justify-center overflow-hidden bg-[#1A1A1A] ${className}`}
  >
    <div
      className="absolute inset-0"
      style={{
        background: "radial-gradient(circle at 50% 42%, rgba(229, 160, 13, 0.13), transparent 58%)",
      }}
    />
    <div
      className={`absolute inset-2 border border-white/[0.035] ${size === "detail" ? "rounded-lg" : "rounded-md"}`}
    />
    <div
      className={`relative flex flex-col items-center text-[#B78319] ${size === "detail" ? "gap-3" : "gap-2"}`}
    >
      <div
        className={`flex items-center justify-center rounded-full border border-[#E5A00D]/20 bg-black/10 ${size === "detail" ? "h-16 w-16" : "h-12 w-12"}`}
      >
        <Disc3
          className={size === "detail" ? "h-8 w-8" : "h-6 w-6"}
          strokeWidth={1.35}
        />
      </div>
      <div className="h-px w-7 bg-[#E5A00D]/25" />
      <span
        className={`${size === "detail" ? "text-[10px]" : "text-[9px]"} font-semibold tracking-[0.24em] text-[#A77A1C]`}
      >
        SONANTE
      </span>
    </div>
  </div>
);
