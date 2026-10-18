import React, { useEffect, useId, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";

export interface ToolbarSelectOption {
  value: string;
  label: string;
}

interface ToolbarSelectProps {
  value: string;
  options: readonly ToolbarSelectOption[];
  onChange: (value: string) => void;
  label: string;
  disabled?: boolean;
  className?: string;
}

export const ToolbarSelect: React.FC<ToolbarSelectProps> = ({
  value,
  options,
  onChange,
  label,
  disabled = false,
  className = "",
}) => {
  const rootRef = useRef<HTMLDivElement>(null);
  const listboxId = useId();
  const [open, setOpen] = useState(false);
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const selectedIndex = options.findIndex((option) => option.value === value);
  const selectedOption = selectedIndex >= 0 ? options[selectedIndex] : options[0];

  const openMenu = () => {
    if (disabled || options.length === 0) return;
    setHighlightedIndex(selectedIndex >= 0 ? selectedIndex : 0);
    setOpen(true);
  };

  const closeMenu = () => setOpen(false);

  useEffect(() => {
    if (!open) return;
    const currentIndex = options.findIndex((option) => option.value === value);
    setHighlightedIndex(currentIndex >= 0 ? currentIndex : 0);
  }, [open, options, value]);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) closeMenu();
    };
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [open]);

  useEffect(() => {
    if (disabled) closeMenu();
  }, [disabled]);

  const selectIndex = (index: number) => {
    const option = options[index];
    if (!option) return;
    if (option.value !== value) onChange(option.value);
    closeMenu();
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (disabled || options.length === 0) return;
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "].includes(event.key)) {
        event.preventDefault();
        openMenu();
      }
      return;
    }

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        setHighlightedIndex((current) => Math.min(current + 1, options.length - 1));
        break;
      case "ArrowUp":
        event.preventDefault();
        setHighlightedIndex((current) => Math.max(current - 1, 0));
        break;
      case "Home":
        event.preventDefault();
        setHighlightedIndex(0);
        break;
      case "End":
        event.preventDefault();
        setHighlightedIndex(options.length - 1);
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        selectIndex(highlightedIndex);
        break;
      case "Escape":
        event.preventDefault();
        closeMenu();
        break;
    }
  };

  return (
    <div ref={rootRef} className={`relative min-w-[180px] max-w-[280px] ${className}`}>
      <button
        type="button"
        aria-label={label}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        onClick={() => open ? closeMenu() : openMenu()}
        onKeyDown={handleKeyDown}
        disabled={disabled}
        className="flex w-full items-center justify-between gap-3 rounded-lg border border-[color:var(--theme-border-soft,#2B2B2B)] bg-[var(--theme-surface-panel,#1A1A1A)] px-3 py-1.5 text-left text-xs text-[color:var(--theme-fg-primary,#ffffff)] outline-none transition-colors hover:border-[color:var(--theme-border-soft,#444444)] focus-visible:border-[color:var(--theme-border-accent,#E5A00D)] focus-visible:ring-1 focus-visible:ring-[color:var(--theme-border-accent,#E5A00D)]/40 disabled:cursor-not-allowed disabled:text-[color:var(--theme-fg-muted,#666666)]"
        title={selectedOption?.label}
      >
        <span className="truncate">{selectedOption?.label ?? ""}</span>
        <ChevronDown
          size={14}
          aria-hidden="true"
          className={`shrink-0 text-[color:var(--theme-fg-muted,#888888)] transition-transform ${open ? "rotate-180 text-[color:var(--theme-fg-accent,#E5A00D)]" : ""}`}
        />
      </button>

      {open && (
        <div
          id={listboxId}
          role="listbox"
          aria-label={label}
          aria-activedescendant={`${listboxId}-option-${highlightedIndex}`}
          className="absolute left-0 top-full z-50 mt-1 max-h-64 w-full min-w-full overflow-y-auto rounded-lg border border-[color:var(--theme-border-soft,#333333)] bg-[var(--theme-surface-panel,#1A1A1A)] p-1 shadow-2xl"
        >
          {options.map((option, index) => {
            const selected = option.value === value;
            const highlighted = index === highlightedIndex;
            return (
              <button
                key={option.value}
                id={`${listboxId}-option-${index}`}
                type="button"
                role="option"
                aria-selected={selected}
                tabIndex={-1}
                title={option.label}
                onMouseEnter={() => setHighlightedIndex(index)}
                onClick={() => selectIndex(index)}
                className={`flex w-full items-center justify-between gap-3 rounded-md px-3 py-2 text-left text-xs transition-colors ${
                  highlighted ? "bg-[var(--theme-surface-hover,#282828)] text-[color:var(--theme-fg-primary,#ffffff)]" : "text-[color:var(--theme-fg-secondary,#CCCCCC)] hover:bg-[var(--theme-surface-raised,#242424)] hover:text-[color:var(--theme-fg-primary,#ffffff)]"
                } ${selected ? "text-[color:var(--theme-fg-accent,#E5A00D)]" : ""}`}
              >
                <span className="truncate">{option.label}</span>
                {selected && <Check size={13} aria-hidden="true" className="shrink-0 text-[color:var(--theme-fg-accent,#E5A00D)]" />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
};
