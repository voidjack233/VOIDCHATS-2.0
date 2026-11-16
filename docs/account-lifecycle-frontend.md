# Account lifecycle frontend

The local cache is keyed by VOID-ID. It stores explicit Active and Deleted results. Absence of evidence means Unknown. A confirmed Deleted result is terminal. The cache is independent of Signal trust, chat history, and Requests.

## Proposed backend operations

- Query a peer by VOID-ID. An authenticated and correlated response must identify the same VOID-ID and explicitly report Active or permanently Deleted. Lookup failures, transport failures, and missing prekeys leave the cached value unchanged.
- Handle an explicit `account_deleted` result from lookup or send as a status update for the correlated peer. Provide only authenticated results to `Worker::record_verified_peer_status`. The send failure must preserve the pending event and its ciphertext.
- After reconnect, verify response correlation and connection generation before applying fresh results. Keep confirmed deletion across reconnects and restarts. The server must enforce deletion for all clients.
- Request permanent deletion for the currently authenticated account. Require separately confirmed authorization for that account and expose the capability before enabling the final UI action. The server must define confirmation, retries after lost responses, deactivation, queued-message handling, and public-prekey handling. Local cleanup requires an explicit authenticated confirmation identifying the local VOID-ID.

The current frontend leaves the final action disabled. It has no deletion transport or local cleanup path attached to the prompt.
