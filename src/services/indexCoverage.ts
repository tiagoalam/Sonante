import{useSyncExternalStore}from'react';import{invoke}from'@tauri-apps/api/core';import{listen}from'@tauri-apps/api/event';import{createIndexCoverageStore}from'../utils/indexCoverage';
const store=createIndexCoverageStore(()=>invoke('get_local_index_coverage'),async receive=>listen('local-library-index-coverage',event=>receive(event.payload)));
export const useIndexCoverage=()=>useSyncExternalStore(store.subscribe,store.getSnapshot,store.getSnapshot);
