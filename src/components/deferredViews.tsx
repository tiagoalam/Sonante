import React, { Suspense, lazy, useState, type ComponentType, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { logError } from '../utils/frontendDiagnostics';
class ViewBoundary extends React.Component<{
    children: ReactNode;
    onRetry: () => void;
    label: string;
}, {
    failed: boolean;
}> {
    state = { failed: false };
    static getDerivedStateFromError() { return { failed: true }; }
    componentDidCatch(error: unknown) { logError('app.view-load', error); }
    render() { return this.state.failed ? <div className="flex flex-1 items-center justify-center p-8" role="alert"><button type="button" className="rounded-lg border border-[color:var(--theme-border-soft,#333)] px-4 py-2 text-sm" onClick={this.props.onRetry}>{this.props.label}</button></div> : this.props.children; }
}
// Module loading never changes playback, source ownership or navigation state.
// Each wrapper keeps its identity; a retry replaces only that failed view.
export function deferredView<P extends object>(load: () => Promise<{
    default: ComponentType<P>;
}>) {
    const initial = { View: lazy(load), attempt: 0 };
    return function Deferred(props: P) {
        const { t } = useTranslation();
        const [{ View, attempt }, setView] = useState(initial);
        return <ViewBoundary key={attempt} onRetry={() => setView(previous => ({ View: lazy(load), attempt: previous.attempt + 1 }))} label={t('app.retryView')}><Suspense fallback={<div role="status" className="flex flex-1 items-center justify-center p-8 text-sm text-[color:var(--theme-fg-muted,#999)]">{t('app.loadingView')}</div>}><View {...props}/></Suspense></ViewBoundary>;
    };
}
export const RadioView = deferredView<React.ComponentProps<typeof import('./RadioView').RadioView>>(() => import('./RadioView').then(module => ({ default: module.RadioView })));
export const HistoryView = deferredView<React.ComponentProps<typeof import('./HistoryView').HistoryView>>(() => import('./HistoryView').then(module => ({ default: module.HistoryView })));
export const EqualizerView = deferredView<React.ComponentProps<typeof import('./EqualizerView').EqualizerView>>(() => import('./EqualizerView').then(module => ({ default: module.EqualizerView })));
export const PlaylistsView = deferredView<React.ComponentProps<typeof import('./PlaylistsView').PlaylistsView>>(() => import('./PlaylistsView').then(module => ({ default: module.PlaylistsView })));
export const UnifiedSearchView = deferredView<React.ComponentProps<typeof import('./UnifiedSearchView').UnifiedSearchView>>(() => import('./UnifiedSearchView').then(module => ({ default: module.UnifiedSearchView })));
export const ExploreView = deferredView<React.ComponentProps<typeof import('./ExploreView').ExploreView>>(() => import('./ExploreView').then(module => ({ default: module.ExploreView })));
export const PlexDiscoverView = deferredView<React.ComponentProps<typeof import('./PlexDiscoverView').PlexDiscoverView>>(() => import('./PlexDiscoverView').then(module => ({ default: module.PlexDiscoverView })));
export const NavidromeView = deferredView<React.ComponentProps<typeof import('./NavidromeView').NavidromeView>>(() => import('./NavidromeView').then(module => ({ default: module.NavidromeView })));

export const NavidromeArtistView = deferredView<React.ComponentProps<typeof import('./NavidromeArtistView').NavidromeArtistView>>(() => import('./NavidromeArtistView').then(module => ({ default: module.NavidromeArtistView })));

export const NavidromeExploreView = deferredView<React.ComponentProps<typeof import('./NavidromeExploreView').NavidromeExploreView>>(() => import('./NavidromeExploreView').then(module => ({ default: module.NavidromeExploreView })));
