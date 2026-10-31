// Frozen pre-optimization sampler from 8a13a1c for behavioral comparison.
// Grouping/normalization remain shared because this lot changes only scoring.
import {groupExplorePresentation, explorePresentationArtistKey as artistKey} from '../../src/utils/explorePresentation.ts';
const EXPLORE_SUGGESTION_COUNT = 10;
export function referencePickExploreSuggestions(pool, retained = [], random = Math.random, preserveRetained = false) {
  const groups = groupExplorePresentation(pool, retained);
  // Once published, a valid card keeps its original source even if another Local copy arrives.
  const candidatesByGroup = groups.map(group => ({ ...group, candidate: preserveRetained
    ? groupExplorePresentation(group.members.filter(entry => retained.some(old => old.key === entry.key)))[0]?.candidate ?? group.candidate
    : group.candidate }));
  const representatives = new Map(candidatesByGroup.flatMap(group => group.members.map(entry => [entry.key, group.candidate])));
  const selected = [];
  for (const old of retained) {
    const current = representatives.get(old.key);
    if (current && selected.length < EXPLORE_SUGGESTION_COUNT && !selected.includes(current)) selected.push(current);
  }
  // Artist variety is chosen when creating/renewing, never by displacing valid cards.
  const candidates = candidatesByGroup.map(group => group.candidate).filter(entry => !selected.includes(entry));
  for (let i = candidates.length - 1; i > 0; i--) { const j = Math.floor(random() * (i + 1)); [candidates[i], candidates[j]] = [candidates[j], candidates[i]]; }
  while (selected.length < EXPLORE_SUGGESTION_COUNT && candidates.length) {
    const choices = candidates;
    const score = (entry) => selected.filter(item => artistKey(item) === artistKey(entry)).length;
    const best = choices.reduce((a, b) => score(b) < score(a) ? b : a);
    selected.push(best); candidates.splice(candidates.indexOf(best), 1);
  }
  return selected;
}
