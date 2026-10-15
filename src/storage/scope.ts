/** A view may only display records fetched for its current local identity. */
export interface StoredViewScope {
  localDeviceId: string;
  collection: string;
}

export function isCurrentScope(
  loaded: StoredViewScope | null,
  localDeviceId: string | null,
  collection: string,
): boolean {
  return localDeviceId !== null && loaded?.localDeviceId === localDeviceId && loaded.collection === collection;
}
