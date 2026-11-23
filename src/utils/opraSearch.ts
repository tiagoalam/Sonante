import type { OpraProductHit } from "../types/equalizer";

const tokens = (value: string) => value.normalize("NFD").replace(/\p{M}/gu, "")
  .toLocaleLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim().split(/\s+/).filter(Boolean);

/** Splits the backend-ranked result set without changing its deterministic order. */
export function classifyOpraProducts(query: string, products: OpraProductHit[]) {
  const wanted = tokens(query);
  const direct: OpraProductHit[] = [];
  const related: OpraProductHit[] = [];
  for (const product of products) {
    const searchable = new Set(tokens(`${product.vendor_name ?? ""} ${product.product_name}`));
    (wanted.length > 0 && wanted.every(token => searchable.has(token)) ? direct : related).push(product);
  }
  return { direct, related };
}
