import type {Person} from './types';
import type {MessagingClient} from './client';

/** Public material only, carried by the authenticated backend connection.
 * Android and future iOS use the same coordinator; native stores own private keys. */
export interface PreKeyPublication {id: string; payloadBase64: string}
export interface PreKeyRequest {id: string; peer: Person}
export interface PreKeyDirectory {
  publish(publication: PreKeyPublication): Promise<void>;
  claim(request: PreKeyRequest): Promise<string>;
}

/** Uses the authenticated connection and the exact native-persisted retry IDs. */
export function createPreKeyDirectory(client: Pick<MessagingClient, 'publishPreKeys' | 'claimPreKeys'>): PreKeyDirectory {
  return {
    publish: async publication => { await client.publishPreKeys(publication.id, publication.payloadBase64); },
    claim: async request => (await client.claimPreKeys(request.id, request.peer.void_id)).payload_base64,
  };
}
